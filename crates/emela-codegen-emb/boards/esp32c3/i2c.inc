/* I2C0 on GPIO6 SCL / GPIO7 SDA, 40MHz XTAL, external pull-ups.
 * Command batches end with END (bus held), with one final STOP per attempt.
 * CPU software source 50 / CPU IRQ4 delivers the final scalar completion.
 * Register layout and command codes: ESP-IDF v5.4.1 ESP32-C3 SoC/LL headers.
 */
#define APP_I2C(offset) REG(UINT32_C(0x60013000) + (offset))
#define I2C_CPU_IRQ 4u
#define I2C_REQUIRE_MAIN() require_main()
static uint64_t i2c_clock_ticks, i2c_clock_ms;
static uint32_t i2c_clock_fraction;
static bool i2c_c3_active;
static uint64_t i2c_hw_now(void) {
    /* Extend the 52-bit system timer across wrap. Masking also prevents a
     * UART ISR from replacing the latched snapshot between its hi/lo reads.
     */
    uint32_t previous = irq_disable();
    uint64_t current = timer_ticks();
    uint64_t delta = ((current - i2c_clock_ticks) & ((UINT64_C(1) << 52) - 1u)) + i2c_clock_fraction;
    i2c_clock_ticks = current;
    i2c_clock_ms += delta / TICKS_PER_MS;
    i2c_clock_fraction = (uint32_t)(delta % TICKS_PER_MS);
    uint64_t now = i2c_clock_ms;
    irq_restore(previous);
    return now;
}
static bool i2c_expired(uint64_t start, int64_t timeout);
static void i2c_c3_timing(EmelaI2cConfig config, uint32_t *divider, uint32_t *low, uint32_t *high) {
    uint32_t hz = (uint32_t)config.clock_speed_hz;
    *divider = (40000000u + hz * 600u - 1u) / (hz * 600u);
    uint32_t low_factor = config.clock_speed_hz <= 100000 ? 1u : config.duty_cycle ? 16u : 2u;
    uint32_t high_factor = config.clock_speed_hz <= 100000 ? 1u : config.duty_cycle ? 9u : 1u;
    uint64_t denominator = (uint64_t)*divider * hz * (low_factor + high_factor);
    uint32_t unit = (uint32_t)((40000000u + denominator - 1u) / denominator);
    *low = unit * low_factor; *high = unit * high_factor;
}
static int32_t i2c_hw_validate(EmelaI2cConfig config) {
#ifdef BOARD_GPIO
    if (configured_inputs & (BIT(6) | BIT(7))) return EMELA_PERIPHERAL_RESOURCE_CONFLICT;
#endif
    uint32_t divider, low, high;
    i2c_c3_timing(config, &divider, &low, &high);
    if (divider > 256u || !low || low > 512u || high < 8u || high > 511u) return EMELA_PERIPHERAL_UNSUPPORTED;
    return 0;
}
static int32_t i2c_c3_sync(uint64_t start, int64_t timeout) {
    APP_I2C(4) |= BIT(11);
    while (APP_I2C(4) & BIT(11)) if (i2c_expired(start, timeout)) return EMELA_PERIPHERAL_TIMEOUT;
    return 0;
}
static void i2c_c3_fifo_reset(void) {
    APP_I2C(0x18) = BIT(12) | BIT(13);
    APP_I2C(0x18) = 0;
}
static int32_t i2c_hw_configure(EmelaI2cConfig config) {
    REG(0x600c0010) |= BIT(7);
    REG(0x600c0018) |= BIT(7); REG(0x600c0018) &= ~BIT(7);
    for (uint32_t pin = 6; pin <= 7; pin++) {
        IO_MUX(pin) = (1u << 10) | BIT(9); /* GPIO matrix, input, no internal pull. */
        GPIO_OUT_SET = BIT(pin);
        GPIO_PIN(pin) = BIT(2); /* Open drain. */
        GPIO_OUT_SELECT(pin) = pin == 6 ? 53u : 54u;
        REG(0x60004154 + (pin == 6 ? 53u : 54u) * 4u) = BIT(7) | pin;
        GPIO_ENABLE_SET = BIT(pin);
    }
    i2c_pins_owned = true;
    uint32_t divider, low, high;
    i2c_c3_timing(config, &divider, &low, &high);
    uint32_t wait_high = high / 4u;
    APP_I2C(0x54) = BIT(21) | (divider - 1u);
    APP_I2C(4) = BIT(0) | BIT(1) | BIT(4) | BIT(8) | BIT(9); /* Master, clock, arbitration. */
    APP_I2C(0) = low - 1u;
    APP_I2C(0x38) = (high - wait_high) | (wait_high << 9);
    APP_I2C(0x30) = high / 4u - 1u;
    APP_I2C(0x34) = high / 2u - 1u;
    APP_I2C(0x40) = high - 1u; APP_I2C(0x44) = high - 1u;
    APP_I2C(0x48) = high - 1u; APP_I2C(0x4c) = high - 1u;
    APP_I2C(0x50) = BIT(8) | BIT(9) | 7u | (7u << 4);
    APP_I2C(0x0c) = 0; /* The software deadline also bounds clock stretching. */
    APP_I2C(0x28) = 0; APP_I2C(0x24) = UINT32_MAX;
    i2c_c3_fifo_reset();
    return i2c_c3_sync(i2c_hw_now(), 2);
}
static void i2c_hw_clear_completion(void) { REG(0x600c0028) = 0; __asm volatile("fence iorw, iorw" ::: "memory"); }
static void i2c_hw_notification(bool enabled, EmelaInterruptPriority priority) {
    MATRIX(0x0c8u) = enabled ? I2C_CPU_IRQ : 0; /* ETS_FROM_CPU_INTR0_SOURCE = 50. */
    MATRIX(0x124u) = (uint32_t)priority + 1u;
    MATRIX(0x108u) &= ~BIT(I2C_CPU_IRQ);
    if (enabled) MATRIX(0x104u) |= BIT(I2C_CPU_IRQ);
    else MATRIX(0x104u) &= ~BIT(I2C_CPU_IRQ);
    __asm volatile("fence iorw, iorw" ::: "memory");
}
static void i2c_hw_pend_completion(void) { REG(0x600c0028) = 1; __asm volatile("fence iorw, iorw" ::: "memory"); }
static void i2c_hw_wait_notification(void) { __asm volatile("nop" ::: "memory"); }

#define I2C_C3_RESTART (6u << 11)
#define I2C_C3_WRITE(n) ((1u << 11) | BIT(8) | (n))
#define I2C_C3_READ(n) ((3u << 11) | (n))
#define I2C_C3_STOP (2u << 11)
#define I2C_C3_END (4u << 11)
static int32_t i2c_c3_run(const uint32_t *commands, unsigned count, uint64_t start, int64_t timeout) {
    for (unsigned i = 0; i < 8; i++) APP_I2C(0x58u + i * 4u) = i < count ? commands[i] : I2C_C3_END;
    APP_I2C(0x24) = UINT32_MAX;
    int32_t status = i2c_c3_sync(start, timeout);
    if (status) return status;
    if (i2c_expired(start, timeout)) return EMELA_PERIPHERAL_TIMEOUT;
    if (commands[0] == I2C_C3_RESTART) i2c_c3_active = true;
    APP_I2C(4) |= BIT(5);
    for (;;) {
        uint32_t flags = APP_I2C(0x20);
        if (flags & BIT(5)) { i2c_c3_active = false; return EMELA_PERIPHERAL_ARBITRATION_LOST; }
        if (flags & (BIT(2) | BIT(6) | BIT(11) | BIT(12))) return EMELA_PERIPHERAL_BUS_ERROR;
        if (flags & BIT(10)) return EMELA_PERIPHERAL_NACK;
        if (flags & (BIT(8) | BIT(13) | BIT(14))) return EMELA_PERIPHERAL_TIMEOUT;
        if (i2c_expired(start, timeout)) return EMELA_PERIPHERAL_TIMEOUT;
        if (APP_I2C(0x58u + (count - 1u) * 4u) & BIT(31)) {
            if (commands[count - 1u] == I2C_C3_STOP) i2c_c3_active = false;
            return 0;
        }
    }
}
static int32_t i2c_c3_address(uint8_t address, bool read, uint64_t start, int64_t timeout) {
    APP_I2C(0x1c) = ((uint32_t)address << 1) | (read ? 1u : 0);
    const uint32_t commands[] = {I2C_C3_RESTART, I2C_C3_WRITE(1), I2C_C3_END};
    return i2c_c3_run(commands, 3, start, timeout);
}
static int32_t i2c_hw_exchange(uint8_t address, bool read, const uint8_t *prefix,
        unsigned prefix_size, const uint16_t *data, uint16_t size, uint16_t *out,
        uint64_t start, int64_t timeout, bool *usable) {
    *usable = true;
    while (APP_I2C(8) & BIT(4)) if (i2c_expired(start, timeout)) return EMELA_PERIPHERAL_TIMEOUT;
    i2c_c3_active = false;
    i2c_c3_fifo_reset();
    int32_t status = i2c_c3_address(address, read && !prefix_size, start, timeout);
    uint32_t remaining = prefix_size + (read ? 0u : size), index = 0;
    while (remaining && !status) {
        uint32_t count = remaining > 32u ? 32u : remaining;
        for (uint32_t i = 0; i < count; i++, index++) APP_I2C(0x1c) = index < prefix_size ? prefix[index] : data[index - prefix_size];
        const uint32_t commands[] = {I2C_C3_WRITE(count), I2C_C3_END};
        status = i2c_c3_run(commands, 2, start, timeout);
        remaining -= count;
    }
    if (!status && read && prefix_size) status = i2c_c3_address(address, true, start, timeout);
    remaining = read ? size : 0;
    while (remaining && !status) {
        uint32_t count = remaining > 32u ? 32u : remaining;
        uint32_t commands[3]; unsigned n = 0;
        if (remaining == count) {
            if (count > 1) commands[n++] = I2C_C3_READ(count - 1u);
            commands[n++] = I2C_C3_READ(1) | BIT(10); /* Normal final NACK. */
        } else commands[n++] = I2C_C3_READ(count);
        commands[n++] = I2C_C3_END;
        status = i2c_c3_run(commands, n, start, timeout);
        if (!status) {
            if (((APP_I2C(8) >> 8) & 63u) != count) status = EMELA_PERIPHERAL_BUS_ERROR;
            else for (uint32_t i = 0; i < count; i++) *out++ = (uint16_t)(APP_I2C(0x1c) & 255u);
        }
        remaining -= count;
    }
    if (i2c_c3_active && status != EMELA_PERIPHERAL_ARBITRATION_LOST) {
        /* STOP and reset are bounded independently of an expired user deadline. */
        const uint32_t commands[] = {I2C_C3_STOP};
        uint64_t cleanup = i2c_hw_now();
        int32_t stopped = i2c_c3_run(commands, 1, cleanup, 2);
        while (!stopped && (APP_I2C(8) & BIT(4)))
            if (i2c_hw_now() - cleanup >= 2u) stopped = EMELA_PERIPHERAL_TIMEOUT;
        if (stopped) {
            if (!status) status = stopped;
            *usable = false;
            /* Disconnect outputs before resetting a stuck controller. */
            GPIO_OUT_SELECT(6) = 128u | BIT(9);
            GPIO_OUT_SELECT(7) = 128u | BIT(9);
            GPIO_ENABLE_CLEAR = BIT(6) | BIT(7);
            APP_I2C(4) |= BIT(10);
            i2c_c3_active = false;
        }
    } else if (status == EMELA_PERIPHERAL_ARBITRATION_LOST) APP_I2C(4) |= BIT(10); /* No STOP on another master's bus. */
    APP_I2C(0x24) = UINT32_MAX;
    i2c_c3_fifo_reset();
    if (!status && i2c_expired(start, timeout)) status = EMELA_PERIPHERAL_TIMEOUT;
    return status;
}
#if __has_include("i2c_driver.h")
#include "i2c_driver.h"
#else
#include "../common/i2c_driver.h"
#endif
static void i2c_interrupt(void) {
    i2c_hw_clear_completion();
    i2c_completion_irq();
}
