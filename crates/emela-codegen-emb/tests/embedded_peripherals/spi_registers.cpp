/* Actual BSP source against a deterministic controller and GPIO model. */
#include <cassert>
#include <cstdint>
#include <cstddef>
#include <map>
#include <vector>
#include "emela_spi.h"
#define __asm
#define volatile(...) ((void)0)
#define BIT(n) (UINT32_C(1) << (n))
static uint64_t now_us, physical_end;
static bool spi_pins_owned, uart_pins_owned, i2c_pins_owned, in_flight, stalled;
static unsigned expected_bits = 8;
static std::vector<uint16_t> wire;
static std::map<uintptr_t, uint32_t> values;
static uint32_t read_register(uintptr_t address);
static void write_register(uintptr_t address, uint32_t value);
struct Register {
    uintptr_t address;
    operator uint32_t() const { return read_register(address); }
    Register &operator=(uint32_t value) { write_register(address, value); return *this; }
    Register &operator|=(uint32_t value) { return *this = uint32_t(*this) | value; }
    Register &operator&=(uint32_t value) { return *this = uint32_t(*this) & value; }
};
static Register &reg(uintptr_t address) {
    static std::map<uintptr_t, Register> registers;
    return registers.try_emplace(address, Register{address}).first->second;
}
#define REG(address) reg(address)
static void main_context(void) {}
static void require_main(void) {}
static int selected = -1;
static unsigned selects, deselects;
static void cs_change(unsigned pin, bool level) {
    if (level) {
        if (selected == int(pin)) {
            assert(!in_flight && now_us >= physical_end + 2); /* final clock + hold */
            selected = -1; deselects++;
        }
    } else { assert(selected < 0); selected = int(pin); selects++; }
}
#ifdef TEST_STM32
static int fault;
#define EMELA_GPIO_PORT_COUNT 8
#define EMELA_GPIO_PIN_MASKS {0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,0xffff,3}
#define EMELA_CONSOLE_TX_PIN 2
#define EMELA_CONSOLE_RX_PIN 3
#define BOARD_HAS_GPIO 1
static uint32_t configured_pins[8], spi_cs_owned[8];
static uint64_t elapsed_ms;
static uint32_t interrupt_state(void) { now_us++; elapsed_ms = now_us / 1000u; return 0; }
#define SYSTICK_VALUE REG(0xe000e018)
#define RCC_AHB1ENR REG(0x40023830)
#define RCC_APB1ENR REG(0x40023840)
#define GPIO_REG(port, offset) REG(0x40020000u + (port) * 0x400u + (offset))
#define GPIO_BSRR(port) GPIO_REG(port, 0x18)
#define GPIO_OTYPER(port) GPIO_REG(port, 4)
#define GPIO_PUPDR(port) GPIO_REG(port, 0x0c)
#define GPIO_AFR(port, pin) GPIO_REG(port, 0x20 + ((pin) / 8) * 4)
#define GPIO_MODER(port) GPIO_REG(port, 0)
#define SPI_DATA8 REG(0x4000380c)
#define SPI_DATA16 REG(0x4000380c)
static constexpr uintptr_t base = 0x40003800;
static uint16_t received;
static bool rx_ready;
static uint32_t read_register(uintptr_t address) {
    if (address == 0xe000e018) return now_us % 1000u ? 16000u - uint32_t(now_us % 1000u) * 16u : 0;
    if (address == base + 8) {
        if (in_flight && !stalled && now_us >= physical_end) in_flight = false;
        return 2u | (rx_ready && !stalled ? 1u : 0u) | (in_flight ? BIT(7) : 0) | (fault == 16 ? BIT(5) : fault == 12 ? BIT(6) : 0);
    }
    if (address == base + 12) { assert(rx_ready); rx_ready = false; return received; }
    return values[address];
}
static void write_register(uintptr_t address, uint32_t value) {
    values[address] = value;
    if (address == base + 12) {
        assert(!rx_ready); /* transmit must drain its unused received words */
        assert(((values[base] & BIT(11)) != 0) == (expected_bits == 16));
        assert(expected_bits == 16 || value <= 255);
        wire.push_back(uint16_t(value)); received = uint16_t(value ^ (expected_bits == 8 ? 255u : 65535u));
        rx_ready = true; in_flight = true; physical_end = now_us + 3;
    }
    if (address == 0x40023820 && (value & BIT(14))) { in_flight = rx_ready = false; fault = 0; }
    for (unsigned port = 0; port < 8; port++) if (address == 0x40020018u + port * 0x400u) {
        for (unsigned pin = 0; pin < 16; pin++) if (spi_cs_owned[port] & BIT(pin)) {
            if (value & BIT(pin)) cs_change(port * 16u + pin, true);
            if (value & BIT(pin + 16)) cs_change(port * 16u + pin, false);
        }
    }
}
static constexpr int cs1 = 16, cs2 = 17;
#else
#define BOARD_GPIO 1
#define TICKS_PER_MS 16000u
static uint32_t configured_inputs, spi_cs_owned;
static uint32_t irq_disable(void) { return 0; }
static void irq_restore(uint32_t) {}
static uint64_t timer_ticks(void) { return (++now_us * 16u) & ((UINT64_C(1) << 52) - 1u); }
#define GPIO_OUT_SET REG(0x60004008)
#define GPIO_OUT_CLEAR REG(0x6000400c)
#define GPIO_ENABLE_SET REG(0x60004024)
#define GPIO_ENABLE_CLEAR REG(0x60004028)
#define GPIO_PIN(pin) REG(0x60004074u + (pin) * 4u)
#define GPIO_OUT_SELECT(pin) REG(0x60004554u + (pin) * 4u)
#define IO_MUX(pin) REG(0x60009004u + (pin) * 4u)
static constexpr uintptr_t base = 0x60024000;
static void progress(void) {
    if (in_flight && !stalled && now_us >= physical_end) {
        in_flight = false; values[base] &= ~BIT(24); values[base + 0x3c] |= BIT(12);
    }
}
static uint32_t read_register(uintptr_t address) { progress(); return values[address]; }
static void write_register(uintptr_t address, uint32_t value) {
    values[address] = value;
    if (address == base) {
        values[address] &= ~BIT(23);
        if (value & BIT(24)) {
            assert(!in_flight);
            unsigned bits = values[base + 0x1c] + 1u;
            assert(bits % expected_bits == 0 && bits <= 512);
            unsigned n = bits / expected_bits, bytes = expected_bits / 8u;
            bool lsb = (values[base + 8] & BIT(26)) != 0;
            std::vector<uint8_t> input(bits / 8u), output(bits / 8u);
            for (unsigned i = 0; i < input.size(); i++) input[i] = uint8_t(values[base + 0x98 + (i / 4u) * 4u] >> ((i % 4u) * 8u));
            for (unsigned i = 0; i < n; i++) {
                uint16_t word = input[i * bytes];
                if (bytes == 2) word = lsb ? uint16_t(word | uint16_t(input[i*bytes+1]) << 8) : uint16_t(uint16_t(word) << 8 | input[i*bytes+1]);
                wire.push_back(word); word ^= expected_bits == 8 ? 255u : 65535u;
                if (bytes == 1) output[i] = uint8_t(word);
                else { output[i*bytes] = uint8_t(lsb ? word : word >> 8); output[i*bytes+1] = uint8_t(lsb ? word >> 8 : word); }
            }
            for (unsigned i = 0; i < output.size(); i += 4) {
                uint32_t packed = 0;
                for (unsigned j = 0; j < 4 && i+j < output.size(); j++) packed |= uint32_t(output[i+j]) << (j*8u);
                values[base + 0x98 + i] = packed;
            }
            in_flight = true; physical_end = now_us + 3;
        }
    }
    if (address == base + 0x38) values[base + 0x3c] &= ~value;
    if (address == 0x600c0018 && (value & BIT(6))) { in_flight = false; values[base] = 0; values[base + 0x3c] = 0; }
    if (address == 0x60004008 || address == 0x6000400c) {
        for (unsigned pin = 0; pin < 11; pin++) if ((spi_cs_owned & BIT(pin)) && (value & BIT(pin))) cs_change(pin, address == 0x60004008);
    }
}
static constexpr int cs1 = 2, cs2 = 3;
#endif
/* @BSP@ */
int main(void) {
    EmelaSpiConfig config = {0,0,0,0,0,0,0};
    uint16_t tx[65], out[65], length;
    for (unsigned i = 0; i < 65; i++) tx[i] = uint16_t((i * 17u) & 255u);
    assert(emela_spi_transmit(1, tx, 1, 100) == EMELA_PERIPHERAL_NOT_CONFIGURED);
    assert(emela_spi_configure(0, config) == EMELA_PERIPHERAL_UNSUPPORTED);
    config.mode = 1; assert(emela_spi_configure(1, config) == EMELA_PERIPHERAL_UNSUPPORTED); config.mode = 0;
    for (unsigned bits : {8u,16u}) for (int order = 0; order < 2; order++) for (int mode = 0; mode < 4; mode++) {
        config.data_size = bits == 16; config.bit_order = order; config.polarity = mode >> 1; config.phase = mode & 1;
        expected_bits = bits;
        assert(emela_spi_configure(1, config) == 0 && spi_pins_owned);
#ifdef TEST_STM32
        assert((values[base] & 3u) == unsigned(mode == 1 ? 1 : mode == 2 ? 2 : mode == 3 ? 3 : 0));
#else
        assert(bool(values[base+0x20] & BIT(29)) == bool(mode >> 1));
        assert(bool(values[base+0x10] & BIT(9)) == bool((mode >> 1) != (mode & 1)));
#endif
        for (unsigned i = 0; i < 65; i++) tx[i] = bits == 16 ? uint16_t(0x1234u + i * 31u) : uint16_t(i);
        tx[0] = 0; tx[1] = bits == 16 ? 65535u : 255u;
        for (unsigned n : {1u,2u,3u,31u,32u,33u,64u,65u}) {
            wire.clear();
            assert(emela_spi_transfer(1, tx, uint16_t(n), 100, out, 65, &length) == 0 && length == n);
            assert(wire == std::vector<uint16_t>(tx, tx+n));
            for (unsigned i = 0; i < n; i++) assert(out[i] == uint16_t(tx[i] ^ (bits == 8 ? 255u : 65535u)));
        }
        wire.clear(); assert(emela_spi_receive(1, 3, 100, out, 65, &length) == 0 && (wire == std::vector<uint16_t>{0,0,0}));
        assert(emela_spi_transmit(1, tx, 3, 100) == 0);
        assert(emela_spi_receive(1, 1, 100, out, 65, &length) == 0 && out[0] == (bits == 8 ? 255u : 65535u));
    }
    config.data_size = 0; config.bit_order = 0; expected_bits = 8;
    assert(emela_spi_configure(1, config) == 0);
    wire.clear(); uint16_t invalid[] = {0,256};
    assert(emela_spi_transmit(1, invalid, 2, 100) == EMELA_PERIPHERAL_INVALID_ARGUMENT && wire.empty());
    tx[0] = 1;
    assert(emela_spi_transmit(1, tx, 1, 0) == EMELA_PERIPHERAL_TIMEOUT && wire.empty());
    assert(emela_spi_transmit(1, nullptr, 0, 0) == 0 && wire.empty());
    assert(emela_spi_receive(1, 0, 0, nullptr, 0, &length) == 0);
    for (int p = 0; p < 8; p++) {
        config.prescaler = p; assert(emela_spi_configure(1, config) == 0);
#ifdef TEST_STM32
        assert(((values[base] >> 3) & 7u) == unsigned(p));
#else
        uint32_t clock = values[base + 0x0c];
        assert((((clock >> 18) & 15u) + 1u) * (((clock >> 12) & 63u) + 1u) == (2u << p));
        assert((values[base + 0xe8] & BIT(2)) == 0);
#endif
    }
    EmelaSpiDeviceConfig device = {1, config, {cs1}, 0, 2, 2};
    EmelaSpiDevice one, two; EmelaSpiTransactionStatus result;
    emela_spi_create_device(device, &one, &result); assert(!result.source && !result.cleanup_status);
    device.cs.id = cs2; emela_spi_create_device(device, &two, &result); assert(!result.source);
    assert(spi_hw_cs_validate(cs1) == EMELA_PERIPHERAL_RESOURCE_CONFLICT);
    const EmelaSpiOperation ops[] = {{1,1,0,0}, {2,2,1,0}, {3,1,1,2}};
    uint16_t payload[] = {0x80,0x23};
    for (auto handle : {one,two}) {
        wire.clear(); emela_spi_transaction(handle, ops, 3, payload, 2, 100, out, 65, &result);
        assert(!result.source && !result.cleanup_status && selected < 0);
        assert((wire == std::vector<uint16_t>{0x80,0,0,0x23}));
    }
    assert(selects == 2 && deselects == 2);
    stalled = true; assert(emela_spi_transmit(1, payload, 1, 1) == EMELA_PERIPHERAL_TIMEOUT);
    stalled = false; emela_spi_recover(1, &result); assert(!result.source && !result.cleanup_status);
    assert(emela_spi_configure(1, config) == 0 && emela_spi_transmit(1, payload, 1, 100) == 0);
#ifdef TEST_STM32
    fault = 16; assert(emela_spi_transmit(1, payload, 1, 100) == EMELA_PERIPHERAL_MODE_FAULT);
    assert(emela_spi_configure(1, config) == 0);
    fault = 12; assert(emela_spi_transmit(1, payload, 1, 100) == EMELA_PERIPHERAL_OVERRUN);
#endif
}
