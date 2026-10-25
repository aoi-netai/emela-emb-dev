/* I2C1, PB6 SCL / PB7 SDA, AF4, external pull-ups. APB1 = 16MHz.
 * Transfer phases are polled in main; IRQ31 is software-pended only after
 * STOP/abort and publishes a single final scalar result in interrupt context.
 */
#define APP_I2C(offset) REG(UINT32_C(0x40005400) + (offset))
#define I2C_REQUIRE_MAIN() main_context()
static uint32_t i2c_hw_lock(void) {
    uint32_t previous = interrupt_state();
    __asm volatile("cpsid i" ::: "memory"); return previous;
}
static void i2c_hw_unlock(uint32_t previous) { __asm volatile("msr primask, %0" :: "r"(previous) : "memory"); }
static uint64_t i2c_hw_now(void) { return peripheral_now_ms(); }
static bool i2c_expired(uint64_t start, int64_t timeout);
static uint32_t i2c_ccr(EmelaI2cConfig config) {
    uint32_t factor = config.clock_speed_hz <= 100000 ? 2u : config.duty_cycle ? 25u : 3u;
    uint32_t denominator = (uint32_t)config.clock_speed_hz * factor;
    return (UINT32_C(16000000) + denominator - 1u) / denominator;
}
static int32_t i2c_hw_validate(EmelaI2cConfig config) {
#if BOARD_HAS_SPI
    if (spi_cs_owned[1] & ((1u << 6) | (1u << 7))) return EMELA_PERIPHERAL_RESOURCE_CONFLICT;
#endif
#if BOARD_HAS_GPIO
    if (configured_pins[1] & ((1u << 6) | (1u << 7))) return EMELA_PERIPHERAL_RESOURCE_CONFLICT;
#endif
    uint32_t ccr = i2c_ccr(config);
    if (ccr > 4095u || ccr < (config.clock_speed_hz <= 100000 ? 4u : 1u)) return EMELA_PERIPHERAL_UNSUPPORTED;
    return 0;
}
static int32_t i2c_hw_configure(EmelaI2cConfig config) {
    RCC_AHB1ENR |= 2u; (void)RCC_AHB1ENR;
    RCC_APB1ENR |= 1u << 21; (void)RCC_APB1ENR;
    REG(0x40023820) |= 1u << 21; REG(0x40023820) &= ~(1u << 21);
    for (uint32_t pin = 6; pin <= 7; pin++) {
        GPIO_BSRR(1) = 1u << pin;
        GPIO_OTYPER(1) |= 1u << pin;
        GPIO_PUPDR(1) &= ~(3u << (pin * 2));
        GPIO_REG(1, 8) |= 3u << (pin * 2);
        GPIO_AFR(1, pin) = (GPIO_AFR(1, pin) & ~(15u << (pin * 4))) | (4u << (pin * 4));
        GPIO_MODER(1) = (GPIO_MODER(1) & ~(3u << (pin * 2))) | (2u << (pin * 2));
    }
    i2c_pins_owned = true;
    APP_I2C(0) = 0; APP_I2C(4) = 16; APP_I2C(8) = 1u << 14;
    APP_I2C(0x1c) = i2c_ccr(config) | (config.clock_speed_hz > 100000 ? 1u << 15 : 0)
        | (config.duty_cycle ? 1u << 14 : 0);
    APP_I2C(0x20) = config.clock_speed_hz <= 100000 ? 17u : 5u;
    APP_I2C(0) = 1;
    return 0;
}
static void i2c_hw_clear_completion(void) { REG(0xe000e280) = 1u << 31; __asm volatile("dsb\nisb" ::: "memory"); }
static void i2c_hw_notification(bool enabled, EmelaInterruptPriority priority) {
    *(volatile uint8_t *)(uintptr_t)(UINT32_C(0xe000e400) + 31u) = (uint8_t)(0xc0u - (uint32_t)priority * 0x40u);
    if (enabled) REG(0xe000e100) = 1u << 31;
    else REG(0xe000e180) = 1u << 31;
    __asm volatile("dsb\nisb" ::: "memory");
}
static void i2c_hw_pend_completion(void) { REG(0xe000e200) = 1u << 31; __asm volatile("dsb\nisb" ::: "memory"); }
static void i2c_hw_wait_notification(void) { __asm volatile("nop" ::: "memory"); }

static int32_t i2c_st_error(void) {
    uint32_t flags = APP_I2C(0x14);
    if (flags & (1u << 9)) return EMELA_PERIPHERAL_ARBITRATION_LOST;
    if (flags & ((1u << 8) | (1u << 11))) return EMELA_PERIPHERAL_BUS_ERROR;
    if (flags & (1u << 10)) return EMELA_PERIPHERAL_NACK;
    if (flags & (1u << 14)) return EMELA_PERIPHERAL_TIMEOUT;
    return 0;
}
static int32_t i2c_st_wait(uint32_t flag, uint64_t start, int64_t timeout) {
    for (;;) {
        int32_t status = i2c_st_error();
        if (status) return status;
        if (i2c_expired(start, timeout)) return EMELA_PERIPHERAL_TIMEOUT;
        if (APP_I2C(0x14) & flag) return 0;
    }
}
static void i2c_st_clear_address(void) { (void)APP_I2C(0x14); (void)APP_I2C(0x18); }
static int32_t i2c_st_address(uint8_t address, bool read, uint64_t start, int64_t timeout) {
    APP_I2C(0) |= 1u << 8;
    int32_t status = i2c_st_wait(1, start, timeout);
    if (status) return status;
    APP_I2C(0x10) = ((uint32_t)address << 1) | (read ? 1u : 0);
    return i2c_st_wait(2, start, timeout);
}
static int32_t i2c_st_write(uint8_t value, uint64_t start, int64_t timeout) {
    int32_t status = i2c_st_wait(1u << 7, start, timeout);
    if (!status) APP_I2C(0x10) = value;
    return status;
}
static int32_t i2c_st_read(uint16_t *out, uint16_t size, uint64_t start, int64_t timeout) {
    uint32_t previous = i2c_hw_lock();
    if (size <= 2) APP_I2C(0) &= ~(1u << 10);
    if (size == 2) APP_I2C(0) |= 1u << 11;
    i2c_st_clear_address();
    if (size == 1) APP_I2C(0) |= 1u << 9;
    i2c_hw_unlock(previous);
    while (size) {
        int32_t status = i2c_st_wait(size == 2 || size == 3 ? 4u : 1u << 6, start, timeout);
        if (status) return status;
        if (size == 3) {
            previous = i2c_hw_lock();
            APP_I2C(0) &= ~(1u << 10);
            *out++ = (uint16_t)(APP_I2C(0x10) & 255u);
            i2c_hw_unlock(previous);
            size--;
        } else if (size == 2) {
            previous = i2c_hw_lock();
            APP_I2C(0) |= 1u << 9;
            *out++ = (uint16_t)(APP_I2C(0x10) & 255u);
            *out++ = (uint16_t)(APP_I2C(0x10) & 255u);
            i2c_hw_unlock(previous);
            size = 0;
        } else { *out++ = (uint16_t)(APP_I2C(0x10) & 255u); size--; }
    }
    return 0;
}
static int32_t i2c_hw_exchange(uint8_t address, bool read, const uint8_t *prefix,
        unsigned prefix_size, const uint16_t *data, uint16_t size, uint16_t *out,
        uint64_t start, int64_t timeout, bool *usable) {
    *usable = true;
    while (APP_I2C(0x18) & 2u) if (i2c_expired(start, timeout)) return EMELA_PERIPHERAL_TIMEOUT;
    APP_I2C(0x14) = 0;
    APP_I2C(0) = 1u | (1u << 10); /* PE, ACK, POS off. */
    int32_t status = i2c_st_address(address, read && !prefix_size, start, timeout);
    if (!status && (!read || prefix_size)) {
        i2c_st_clear_address();
        for (unsigned i = 0; i < prefix_size && !status; i++) status = i2c_st_write(prefix[i], start, timeout);
        if (!read)
            for (uint16_t i = 0; i < size && !status; i++) status = i2c_st_write((uint8_t)data[i], start, timeout);
        /* Probe has no payload; ADDR acknowledgement itself is sufficient. */
        if (!status && (prefix_size || size)) status = i2c_st_wait(4, start, timeout);
        if (!status && read) status = i2c_st_address(address, true, start, timeout);
    }
    if (!status && read) status = i2c_st_read(out, size, start, timeout);
    /* Never send STOP after arbitration loss or when another master owns BUSY. */
    if (status != EMELA_PERIPHERAL_ARBITRATION_LOST && (APP_I2C(0x18) & 1u)) {
        APP_I2C(0) |= 1u << 9;
        uint64_t cleanup = i2c_hw_now();
        while ((APP_I2C(0) & (1u << 9)) || (APP_I2C(0x18) & 1u)) {
            if (i2c_hw_now() - cleanup >= 2u) {
                if (!status) status = EMELA_PERIPHERAL_TIMEOUT;
                APP_I2C(0) = 0; /* End every buffer reference and release pins. */
                *usable = false;
                break;
            }
        }
    }
    int32_t cleanup_error = i2c_st_error();
    if (!status) status = cleanup_error;
    APP_I2C(0x14) = 0;
    /* Cancel a START which timed out before mastership was obtained as well.
     * The next operation must never inherit an old in-flight address phase.
     */
    if (status) APP_I2C(0) = 0;
    APP_I2C(0) = *usable ? 1u : 0u;
    if (!status && i2c_expired(start, timeout)) status = EMELA_PERIPHERAL_TIMEOUT;
    return status;
}
#if __has_include("i2c_driver.h")
#include "i2c_driver.h"
#else
#include "../../common/i2c_driver.h"
#endif
void I2C1_EV_IRQHandler(void) {
    emela_validation_stack_record_sp();
    i2c_completion_irq();
    __asm volatile("dsb" ::: "memory");
}
