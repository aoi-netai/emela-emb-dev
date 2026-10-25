/* Execute the unchanged register-level BSP sources against deterministic I2C
 * register/FIFO models. Only CPU assembly, time and IRQ entry are substituted.
 * Trace elements: START=256, STOP=257, received byte=512+byte.
 */
#include <cassert>
#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <deque>
#include <map>
#include <vector>
#include "emela_i2c.h"
#define BIT(n) (UINT32_C(1) << (n))
#define __asm
#define volatile(...) ((void)0)
static uint64_t ticks;
static unsigned tick_stride = 1;
static bool i2c_pins_owned, foreign_busy, stuck_stop, lose_arbitration;
static unsigned nack_addresses, nack_data_at;
static unsigned received, expected_receive, data_writes, final_nacks;
static bool master, reading, address_next, stop_pending;
static std::vector<unsigned> trace;
static std::map<uintptr_t, uint32_t> values;
static uint32_t model_read(uintptr_t address);
static void model_write(uintptr_t address, uint32_t value);
struct Register {
    uintptr_t address;
    operator uint32_t() const { return model_read(address); }
    Register &operator=(uint32_t v) { model_write(address, v); return *this; }
    Register &operator|=(uint32_t v) { return *this = uint32_t(*this) | v; }
    Register &operator&=(uint32_t v) { return *this = uint32_t(*this) & v; }
};
static Register &reg(uintptr_t address) {
    static std::map<uintptr_t, Register> registers;
    return registers.try_emplace(address, Register{address}).first->second;
}
#define REG(address) reg(address)
static void main_context(void) {}
static void require_main(void) {}
static uint32_t interrupt_state(void);
static void emela_validation_stack_record_sp(void) {}

#ifdef TEST_STM32
static constexpr uintptr_t base = 0x40005400;
static volatile uint64_t elapsed_ms;
static uint32_t interrupt_state(void) { elapsed_ms = ++ticks / 16u; return 0; }
#define SYSTICK_VALUE REG(0xe000e018)
#define RCC_AHB1ENR REG(0x40023830)
#define RCC_APB1ENR REG(0x40023840)
#define GPIO_REG(port, offset) REG(0x40020000u + (port) * 0x400u + (offset))
#define GPIO_BSRR(port) GPIO_REG(port, 0x18)
#define GPIO_OTYPER(port) GPIO_REG(port, 4)
#define GPIO_PUPDR(port) GPIO_REG(port, 0x0c)
#define GPIO_AFR(port, pin) GPIO_REG(port, 0x20 + ((pin) / 8) * 4)
#define GPIO_MODER(port) GPIO_REG(port, 0)
static uint32_t sr1, cr1;
static bool receive_pos;
static void finish_stop(void) {
    if (stuck_stop) return;
    trace.push_back(257); master = false; stop_pending = false; cr1 &= ~BIT(9);
}
static uint32_t model_read(uintptr_t address) {
    if (address == base) return cr1;
    if (address == base + 0x14) return sr1;
    if (address == base + 0x18) {
        if (sr1 & BIT(1)) {
            sr1 &= ~BIT(1);
            receive_pos = (cr1 & BIT(11)) != 0;
            if (reading) sr1 = BIT(6) | (expected_receive >= 2 ? BIT(2) : 0);
        }
        return (master ? 3u : 0u) | (foreign_busy ? 2u : 0u);
    }
    if (address == base + 0x10 && reading) {
        unsigned remaining = expected_receive - received;
        assert(remaining > 0);
        if (remaining <= 3) assert(!(cr1 & BIT(10)));
        if (expected_receive == 2) assert(receive_pos);
        if (remaining <= 2) assert(stop_pending);
        unsigned value = received++ & 255u;
        trace.push_back(512 + value);
        remaining--;
        sr1 = (remaining ? BIT(6) : 0) | (remaining >= 2 ? BIT(2) : 0);
        if (!remaining) { final_nacks++; if (stop_pending) finish_stop(); }
        return value;
    }
    return values[address];
}
static void model_write(uintptr_t address, uint32_t value) {
    values[address] = value;
    if (address == base) {
        cr1 = value;
        if (value & BIT(8)) {
            cr1 &= ~BIT(8); trace.push_back(256); master = true; address_next = true; sr1 = BIT(0);
            if (lose_arbitration) { master = false; foreign_busy = true; sr1 = BIT(9); }
        }
        if (value & BIT(9)) {
            stop_pending = true;
            if (!reading || received == expected_receive) finish_stop();
        }
    } else if (address == base + 0x14) sr1 &= value;
    else if (address == base + 0x10) {
        assert(master);
        trace.push_back(value);
        if (address_next) {
            address_next = false; reading = (value & 1) != 0;
            if (nack_addresses) { nack_addresses--; sr1 = BIT(10); }
            else sr1 = BIT(1) | BIT(7);
        } else {
            data_writes++;
            sr1 = nack_data_at == data_writes ? BIT(10) : BIT(2) | BIT(7);
        }
    }
}
#else
static constexpr uintptr_t base = 0x60013000;
static std::deque<uint8_t> tx_fifo, rx_fifo;
#define TICKS_PER_MS 16000u
static uint64_t timer_ticks(void) { ticks += tick_stride; return (ticks * (TICKS_PER_MS / 16u)) & ((UINT64_C(1) << 52) - 1u); }
static uint32_t irq_disable(void) { return 0; }
static void irq_restore(uint32_t) {}
#define IO_MUX(pin) REG(0x60009004u + (pin) * 4u)
#define GPIO_OUT_SET REG(0x60004008)
#define GPIO_PIN(pin) REG(0x60004074u + (pin) * 4u)
#define GPIO_OUT_SELECT(pin) REG(0x60004554u + (pin) * 4u)
#define GPIO_ENABLE_SET REG(0x60004024)
#define GPIO_ENABLE_CLEAR REG(0x60004028)
#define MATRIX(offset) REG(0x600c2000u + (offset))
static void run_commands(void) {
    values[base + 0x20] = 0;
    for (unsigned i = 0; i < 8; i++) {
        uint32_t command = values[base + 0x58 + i * 4];
        unsigned opcode = (command >> 11) & 7u, count = command & 255u;
        if (opcode == 6) {
            trace.push_back(256); master = true; address_next = true;
            if (lose_arbitration) { master = false; foreign_busy = true; values[base + 0x20] = BIT(5); return; }
        } else if (opcode == 1) {
            assert(master && tx_fifo.size() >= count);
            for (unsigned j = 0; j < count; j++) {
                unsigned value = tx_fifo.front(); tx_fifo.pop_front(); trace.push_back(value);
                if (address_next) {
                    address_next = false; reading = (value & 1) != 0;
                    if (nack_addresses) { nack_addresses--; values[base + 0x20] = BIT(10); return; }
                } else {
                    data_writes++;
                    if (nack_data_at == data_writes) { values[base + 0x20] = BIT(10); return; }
                }
            }
        } else if (opcode == 3) {
            assert(master && reading && rx_fifo.size() + count <= 32);
            for (unsigned j = 0; j < count; j++) {
                unsigned value = received++ & 255u;
                assert(received <= expected_receive);
                bool nack = (command & BIT(10)) != 0;
                assert(nack == (received == expected_receive));
                if (nack) final_nacks++;
                rx_fifo.push_back(value); trace.push_back(512 + value);
            }
        } else if (opcode == 2) {
            if (stuck_stop) return;
            assert(master); master = false; trace.push_back(257);
        } else assert(opcode == 4);
        values[base + 0x58 + i * 4] |= BIT(31);
        if (opcode == 2 || opcode == 4) return;
    }
    assert(false);
}
static uint32_t model_read(uintptr_t address) {
    if (address == base + 8) return ((master || foreign_busy) ? BIT(4) : 0) | (uint32_t(rx_fifo.size()) << 8);
    if (address == base + 0x1c) { assert(!rx_fifo.empty()); uint8_t v = rx_fifo.front(); rx_fifo.pop_front(); return v; }
    return values[address];
}
static void model_write(uintptr_t address, uint32_t value) {
    values[address] = value;
    if (address == base + 4) {
        values[address] &= ~(BIT(5) | BIT(10) | BIT(11));
        if (value & BIT(5)) run_commands();
        if (value & BIT(10)) master = false;
    } else if (address == base + 0x1c) { assert(tx_fifo.size() < 32); tx_fifo.push_back(uint8_t(value)); }
    else if (address == base + 0x24) values[base + 0x20] &= ~value;
    else if (address == base + 0x18) {
        if (value & BIT(12)) rx_fifo.clear();
        if (value & BIT(13)) tx_fifo.clear();
    }
}
#endif
int32_t emela_i2c_has_binding(int32_t id) { return id == 0; }
void emela_i2c_transfer_complete(int32_t, int32_t) { assert(false); }
void emela_i2c_transfer_error(int32_t, int32_t, int32_t) { assert(false); }
void emela_i2c_device_ready(int32_t, int32_t) { assert(false); }
/* @BSP@ */
static void next(unsigned n = 0) {
    trace.clear(); expected_receive = n; received = 0; data_writes = 0; final_nacks = 0;
    tick_stride = 1;
    nack_addresses = nack_data_at = 0; foreign_busy = stuck_stop = lose_arbitration = false;
    master = reading = address_next = stop_pending = false;
}
int main(void) {
    EmelaI2cConfig config = {100000, 0};
    uint16_t data[] = {0, 255}, out[64], length;
    int32_t ready;
    assert(emela_i2c_configure(0, config) == 0 && i2c_pins_owned);
#ifdef TEST_STM32
    assert(values[base + 0x1c] == 80);
#endif
    next();
    assert(emela_i2c_memory_write(0, 64, 255, 0, nullptr, 0, 0) == 0 && trace.empty());
    assert(emela_i2c_master_receive(0, 64, 0, 0, nullptr, 0, &length) == 0 && length == 0 && trace.empty());
    next();
    int32_t first_status = emela_i2c_master_transmit(0, 64, data, 2, 1000);
    if (first_status) {
        std::fprintf(stderr, "transmit status %d trace:", first_status);
        for (auto item : trace) std::fprintf(stderr, " %u", item);
        std::fprintf(stderr, "\n");
    }
    assert(first_status == 0);
    assert((trace == std::vector<unsigned>{256,128,0,255,257}));
    next();
    assert(emela_i2c_memory_write(0, 64, 0x1234, 1, data, 2, 1000) == 0);
    assert((trace == std::vector<unsigned>{256,128,0x12,0x34,0,255,257}));
    for (unsigned n : {1u,2u,3u,4u,32u,33u,64u}) {
        next(n);
        int32_t status = emela_i2c_memory_read(0, 64, 0x1234, 1, int32_t(n), 1000, out, 64, &length);
        if (status) {
            std::fprintf(stderr, "read size %u status %d trace:", n, status);
            for (auto item : trace) std::fprintf(stderr, " %u", item);
            std::fprintf(stderr, "\n");
        }
        assert(status == 0);
        assert(length == n && out[n-1] == n-1 && final_nacks == 1);
        std::vector<unsigned> expected = {256,128,0x12,0x34,256,129};
        for (unsigned i = 0; i < n; i++) expected.push_back(512 + i);
        expected.push_back(257);
        assert(trace == expected);
        next(n);
        assert(emela_i2c_master_receive(0, 64, int32_t(n), 1000, out, 64, &length) == 0 && final_nacks == 1);
    }
    next(2);
    assert(emela_i2c_memory_read(0, 64, 255, 0, 2, 1000, out, 64, &length) == 0);
    assert((trace == std::vector<unsigned>{256,128,255,256,129,512,513,257}));
    next(); nack_addresses = 2;
    assert(emela_i2c_is_device_ready(0, 64, 3, 1000, &ready) == 0 && ready == 1);
    assert((trace == std::vector<unsigned>{256,128,257,256,128,257,256,128,257}));
    next(); nack_addresses = 3;
    assert(emela_i2c_is_device_ready(0, 64, 3, 1000, &ready) == 0 && ready == 0);
    next(); nack_addresses = 1;
    assert(emela_i2c_master_transmit(0, 64, data, 2, 1000) == EMELA_PERIPHERAL_NACK);
    assert((trace == std::vector<unsigned>{256,128,257}));
    next(); nack_data_at = 1;
    assert(emela_i2c_master_transmit(0, 64, data, 2, 1000) == EMELA_PERIPHERAL_NACK);
    assert((trace == std::vector<unsigned>{256,128,0,257}));
    next(); foreign_busy = true;
    assert(emela_i2c_master_transmit(0, 64, data, 2, 10) == EMELA_PERIPHERAL_TIMEOUT && trace.empty());
    next(); lose_arbitration = true;
    assert(emela_i2c_master_transmit(0, 64, data, 2, 1000) == EMELA_PERIPHERAL_ARBITRATION_LOST);
    assert((trace == std::vector<unsigned>{256})); /* No STOP on the winner's bus. */
    next();
    assert(emela_i2c_master_transmit(0, 64, data, 2, 1000) == 0);
    next(); stuck_stop = true;
    assert(emela_i2c_master_transmit(0, 64, data, 2, 1000) == EMELA_PERIPHERAL_TIMEOUT);
    assert(emela_i2c_master_transmit(0, 64, data, 2, 1000) == EMELA_PERIPHERAL_INVALID_STATE);
    next(); config.clock_speed_hz = 400000;
    assert(emela_i2c_configure(0, config) == 0);
    next(); nack_addresses = 1; stuck_stop = true;
    assert(emela_i2c_master_transmit(0, 64, data, 2, 1000) == EMELA_PERIPHERAL_NACK);
    assert(i2c_recovery); /* Preserve the first error even if STOP also fails. */
    next();
    config.duty_cycle = 1;
    assert(emela_i2c_configure(0, config) == 0);
#ifdef TEST_STM32
    assert(values[base + 0x1c] == (BIT(15) | BIT(14) | 2u));
#else
    uint32_t low = values[base] + 1u;
    uint32_t high = (values[base + 0x38] & 511u) + ((values[base + 0x38] >> 9) & 127u);
    assert(low * 9u == high * 16u);
#endif
    assert(emela_i2c_master_transmit(0, 64, data, 2, 1000) == 0);
#ifndef TEST_STM32
    next(); tick_stride = 16;
    assert(emela_i2c_master_transmit(0, 64, data, 2, 2) == EMELA_PERIPHERAL_TIMEOUT && trace.empty());
    next();
    assert(emela_i2c_master_transmit(0, 64, data, 2, 1000) == 0);
    ticks = (UINT64_C(1) << 52) / (TICKS_PER_MS / 16u) - 10;
    uint64_t before_wrap = i2c_hw_now();
    ticks += 32;
    assert(i2c_hw_now() >= before_wrap + 1);
#endif
}
