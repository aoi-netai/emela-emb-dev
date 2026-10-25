/* SPI2: PB13 SCK, PB14 MISO, PB15 MOSI, AF5. APB1 = 16MHz.
 * Master/TwoLine only, 8/16 bits, CPOL/CPHA and both bit orders.
 */
#define APP_SPI(offset) REG(UINT32_C(0x40003800) + (offset))
#define SPI_REQUIRE_MAIN() main_context()
#ifndef SPI_DATA8
#define SPI_DATA8 (*(volatile uint8_t *)(uintptr_t)UINT32_C(0x4000380c))
#define SPI_DATA16 (*(volatile uint16_t *)(uintptr_t)UINT32_C(0x4000380c))
#endif
static uint32_t spi_hw_lock(void) { uint32_t p = interrupt_state(); __asm volatile("cpsid i" ::: "memory"); return p; }
static void spi_hw_unlock(uint32_t p) { __asm volatile("msr primask, %0" :: "r"(p) : "memory"); }
static uint64_t spi_hw_now_us(void) { return peripheral_now_us(); }
static bool spi_expired(uint64_t start, int64_t timeout);
static uint32_t spi_prescaler_power(EmelaSpiPrescaler p);
static const uint16_t spi_pin_masks[EMELA_GPIO_PORT_COUNT] = EMELA_GPIO_PIN_MASKS;
static int32_t spi_hw_validate(EmelaSpiConfig c) {
    if (c.mode != EMELA_SPI_MODE_MASTER || c.direction != EMELA_SPI_DIRECTION_TWO_LINE) return EMELA_PERIPHERAL_UNSUPPORTED;
#if BOARD_HAS_GPIO
    if (configured_pins[1] & UINT32_C(0xe000)) return EMELA_PERIPHERAL_RESOURCE_CONFLICT;
#endif
    return 0;
}
static int32_t spi_hw_configure(EmelaSpiConfig c, uint64_t start, int64_t timeout) {
    (void)start; (void)timeout;
    RCC_AHB1ENR |= 2u; (void)RCC_AHB1ENR;
    RCC_APB1ENR |= 1u << 14; (void)RCC_APB1ENR;
    REG(0x40023820) |= 1u << 14; REG(0x40023820) &= ~(1u << 14);
    for (uint32_t pin = 13; pin <= 15; pin++) {
        GPIO_BSRR(1) = pin == 13 && c.polarity ? 1u << pin : 1u << (pin + 16);
        GPIO_OTYPER(1) &= ~(1u << pin);
        GPIO_PUPDR(1) &= ~(3u << (pin * 2));
        GPIO_REG(1, 8) |= 3u << (pin * 2);
        uint32_t shift = (pin % 8u) * 4u;
        GPIO_AFR(1, pin) = (GPIO_AFR(1, pin) & ~(15u << shift)) | (5u << shift);
        GPIO_MODER(1) = (GPIO_MODER(1) & ~(3u << (pin * 2))) | (2u << (pin * 2));
    }
    spi_pins_owned = true;
    APP_SPI(4) = 0;
    APP_SPI(0) = (1u << 2) | (1u << 6) | (1u << 8) | (1u << 9)
        | (c.phase == EMELA_SPI_CLOCK_PHASE_SECOND_EDGE ? 1u : 0)
        | (c.polarity == EMELA_SPI_CLOCK_POLARITY_IDLE_HIGH ? 1u << 1 : 0)
        | (spi_prescaler_power(c.prescaler) << 3)
        | (c.bit_order == EMELA_SPI_BIT_ORDER_LEAST_SIGNIFICANT_FIRST ? 1u << 7 : 0)
        | (c.data_size == EMELA_SPI_DATA_SIZE_BITS16 ? 1u << 11 : 0);
    return 0;
}
static int32_t spi_st_error(void) {
    uint32_t sr = APP_SPI(8);
    if (sr & (1u << 5)) return EMELA_PERIPHERAL_MODE_FAULT;
    if (sr & (1u << 6)) return EMELA_PERIPHERAL_OVERRUN;
    return sr & (1u << 8) ? EMELA_PERIPHERAL_OTHER : 0;
}
static int32_t spi_st_wait(uint32_t mask, bool set, uint64_t start, int64_t timeout) {
    for (;;) {
        int32_t status = spi_st_error();
        if (status) return status;
        if (spi_expired(start, timeout)) return EMELA_PERIPHERAL_TIMEOUT;
        if (((APP_SPI(8) & mask) != 0) == set) return 0;
    }
}
static int32_t spi_hw_exchange(EmelaSpiConfig c, const uint16_t *tx, uint16_t size, uint16_t *out, uint64_t start, int64_t timeout) {
    for (uint32_t i = 0; i < size; i++) {
        int32_t status = spi_st_wait(2, true, start, timeout);
        if (status) return status;
        uint16_t word = tx ? tx[i] : 0;
        if (c.data_size == EMELA_SPI_DATA_SIZE_BITS8) SPI_DATA8 = (uint8_t)word;
        else SPI_DATA16 = word;
        status = spi_st_wait(1, true, start, timeout);
        if (status) return status;
        uint16_t received = c.data_size == EMELA_SPI_DATA_SIZE_BITS8 ? (uint16_t)SPI_DATA8 : (uint16_t)SPI_DATA16;
        if (out) out[i] = received;
    }
    int32_t status = spi_st_wait(2, true, start, timeout);
    if (!status) status = spi_st_wait(1u << 7, false, start, timeout);
    return status;
}
static int32_t spi_hw_abort(void) {
    uint64_t start = spi_hw_now_us(); int32_t status = 0;
    while (APP_SPI(8) & (1u << 7)) if (spi_hw_now_us() - start >= 2000u) { status = EMELA_PERIPHERAL_TIMEOUT; break; }
    APP_SPI(0) = 0;
    REG(0x40023820) |= 1u << 14; REG(0x40023820) &= ~(1u << 14);
    return status;
}
static int32_t spi_hw_cs_validate(int32_t pin) {
    if (pin < 0 || (uint32_t)pin >= EMELA_GPIO_PORT_COUNT * 16u || !(spi_pin_masks[(uint32_t)pin / 16u] & (1u << ((uint32_t)pin % 16u)))) return EMELA_PERIPHERAL_INVALID_ARGUMENT;
    uint32_t port = (uint32_t)pin / 16u, bit = 1u << ((uint32_t)pin % 16u);
    if (pin == EMELA_CONSOLE_TX_PIN || pin == EMELA_CONSOLE_RX_PIN || (pin >= 29 && pin <= 31)
        || (uart_pins_owned && (pin == 9 || pin == 10)) || (i2c_pins_owned && (pin == 22 || pin == 23)) || (spi_cs_owned[port] & bit)) return EMELA_PERIPHERAL_RESOURCE_CONFLICT;
#if BOARD_HAS_GPIO
    if (configured_pins[port] & bit) return EMELA_PERIPHERAL_RESOURCE_CONFLICT;
#endif
    return 0;
}
static int32_t spi_hw_cs_init(int32_t pin, int32_t inactive) {
    uint32_t port = (uint32_t)pin / 16u, number = (uint32_t)pin % 16u, bit = 1u << number;
    uint32_t previous = spi_hw_lock();
    spi_cs_owned[port] |= bit;
    RCC_AHB1ENR |= 1u << port; (void)RCC_AHB1ENR;
    GPIO_MODER(port) &= ~(3u << (number * 2));
    GPIO_BSRR(port) = inactive ? bit : bit << 16;
    GPIO_OTYPER(port) &= ~bit; GPIO_PUPDR(port) &= ~(3u << (number * 2));
    GPIO_MODER(port) |= 1u << (number * 2);
    spi_hw_unlock(previous); return 0;
}
static int32_t spi_hw_cs_write(int32_t pin, int32_t state) {
    uint32_t port = (uint32_t)pin / 16u, bit = 1u << ((uint32_t)pin % 16u);
    if (!(spi_cs_owned[port] & bit)) return EMELA_PERIPHERAL_NOT_CONFIGURED;
    GPIO_BSRR(port) = state ? bit : bit << 16; return 0;
}
static void spi_hw_cs_release(int32_t pin) { spi_cs_owned[(uint32_t)pin / 16u] &= ~(1u << ((uint32_t)pin % 16u)); }
#if __has_include("spi_driver.h")
#include "spi_driver.h"
#else
#include "../../common/spi_driver.h"
#endif
