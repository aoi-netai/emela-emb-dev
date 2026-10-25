/* SPI2 (GPSPI): GPIO0 SCK, GPIO1 MOSI, GPIO10 MISO; 40MHz XTAL.
 * Master/TwoLine, software CS only. Explicit word packing, independent of host endian.
 */
#define APP_SPI(offset) REG(UINT32_C(0x60024000) + (offset))
#define SPI_REQUIRE_MAIN() require_main()
static uint64_t spi_hw_now_us(void) { return peripheral_now_us(); }
static bool spi_expired(uint64_t start, int64_t timeout);
static uint32_t spi_prescaler_power(EmelaSpiPrescaler p);
static int32_t spi_hw_validate(EmelaSpiConfig c) {
    if (c.mode != EMELA_SPI_MODE_MASTER || c.direction != EMELA_SPI_DIRECTION_TWO_LINE) return EMELA_PERIPHERAL_UNSUPPORTED;
#ifdef BOARD_GPIO
    if (configured_inputs & (BIT(0) | BIT(1) | BIT(10))) return EMELA_PERIPHERAL_RESOURCE_CONFLICT;
#endif
    return 0;
}
static int32_t spi_c3_update(uint64_t start, int64_t timeout) {
    if (spi_expired(start, timeout)) return EMELA_PERIPHERAL_TIMEOUT;
    APP_SPI(0) |= BIT(23);
    while (APP_SPI(0) & BIT(23)) if (spi_expired(start, timeout)) return EMELA_PERIPHERAL_TIMEOUT;
    return 0;
}
static int32_t spi_hw_configure(EmelaSpiConfig c, uint64_t start, int64_t timeout) {
    REG(0x600c0010) |= BIT(6);
    REG(0x600c0018) |= BIT(6); REG(0x600c0018) &= ~BIT(6);
    IO_MUX(0) = 1u << 10; IO_MUX(1) = 1u << 10; IO_MUX(10) = (1u << 10) | BIT(9);
    GPIO_PIN(0) = 0; GPIO_PIN(1) = 0;
    GPIO_OUT_SELECT(0) = 63; GPIO_OUT_SELECT(1) = 65;
    REG(0x60004154 + 64u * 4u) = BIT(7) | 10u;
    GPIO_ENABLE_SET = BIT(0) | BIT(1); GPIO_ENABLE_CLEAR = BIT(10);
    spi_pins_owned = true;
    APP_SPI(0xe8) = BIT(0) | BIT(1); /* XTAL source. */
    APP_SPI(0xe0) = 0; APP_SPI(0x30) = 0; APP_SPI(0x34) = 0; APP_SPI(0x38) = UINT32_MAX;
    APP_SPI(8) = c.bit_order == EMELA_SPI_BIT_ORDER_LEAST_SIGNIFICANT_FIRST ? BIT(25) | BIT(26) : 0;
    uint32_t divisor = 2u << spi_prescaler_power(c.prescaler), n = divisor > 64u ? 64u : divisor, pre = divisor / n;
    APP_SPI(0x0c) = ((pre - 1u) << 18) | ((n - 1u) << 12) | ((n / 2u - 1u) << 6) | (n - 1u);
    APP_SPI(0x10) = BIT(0) | BIT(27) | BIT(28) | (c.polarity != c.phase ? BIT(9) : 0);
    APP_SPI(0x14) = 0; APP_SPI(0x18) = 0;
    APP_SPI(0x20) = 0x3fu | (c.polarity ? BIT(29) : 0); /* Disable every hardware CS. */
    return spi_c3_update(start, timeout);
}
static uint16_t spi_c3_word(EmelaSpiConfig c, uint16_t word) {
    return c.data_size == EMELA_SPI_DATA_SIZE_BITS16 && !c.bit_order ? (uint16_t)((word >> 8) | (word << 8)) : word;
}
static int32_t spi_hw_exchange(EmelaSpiConfig c, const uint16_t *tx, uint16_t size, uint16_t *out, uint64_t start, int64_t timeout) {
    uint32_t bits = c.data_size == EMELA_SPI_DATA_SIZE_BITS8 ? 8u : 16u, per_register = 32u / bits;
    uint32_t index = 0;
    while (index < size) {
        uint32_t count = (uint32_t)size - index, maximum = 512u / bits;
        if (count > maximum) count = maximum;
        for (uint32_t word = 0; word < count; word += per_register) {
            uint32_t packed = 0;
            for (uint32_t j = 0; j < per_register && word + j < count; j++)
                packed |= (uint32_t)spi_c3_word(c, tx ? tx[index + word + j] : 0) << (j * bits);
            APP_SPI(0x98u + (word / per_register) * 4u) = packed;
        }
        APP_SPI(0x1c) = count * bits - 1u;
        APP_SPI(0x38) = BIT(12);
        int32_t status = spi_c3_update(start, timeout);
        if (status) return status;
        if (spi_expired(start, timeout)) return EMELA_PERIPHERAL_TIMEOUT;
        APP_SPI(0) |= BIT(24);
        for (;;) {
            if (spi_expired(start, timeout)) return EMELA_PERIPHERAL_TIMEOUT;
            if (!(APP_SPI(0) & BIT(24)) && (APP_SPI(0x3c) & BIT(12))) break;
        }
        if (out) for (uint32_t word = 0; word < count; word++) {
            uint32_t packed = APP_SPI(0x98u + (word / per_register) * 4u);
            uint16_t received = (uint16_t)(packed >> ((word % per_register) * bits));
            if (bits == 8) received &= 255u;
            out[index + word] = spi_c3_word(c, received);
        }
        index += count;
    }
    return 0;
}
static int32_t spi_hw_abort(void) {
    REG(0x600c0018) |= BIT(6); REG(0x600c0018) &= ~BIT(6);
    uint64_t start = spi_hw_now_us();
    while (APP_SPI(0) & (BIT(24) | BIT(23))) if (spi_hw_now_us() - start >= 2000u) return EMELA_PERIPHERAL_TIMEOUT;
    return 0;
}
static int32_t spi_hw_cs_validate(int32_t pin) {
    if (pin < 0 || pin > 21) return EMELA_PERIPHERAL_INVALID_ARGUMENT;
    if (pin >= 11 || pin == 0 || pin == 1 || pin == 10 || (spi_cs_owned & BIT((uint32_t)pin))
        || (uart_pins_owned && (pin == 4 || pin == 5)) || (i2c_pins_owned && (pin == 6 || pin == 7))) return EMELA_PERIPHERAL_RESOURCE_CONFLICT;
#ifdef BOARD_GPIO
    if (configured_inputs & BIT((uint32_t)pin)) return EMELA_PERIPHERAL_RESOURCE_CONFLICT;
#endif
    return 0;
}
static int32_t spi_hw_cs_init(int32_t pin, int32_t inactive) {
    uint32_t bit = BIT((uint32_t)pin), previous = irq_disable(); spi_cs_owned |= bit;
    GPIO_ENABLE_CLEAR = bit;
    if (inactive) GPIO_OUT_SET = bit; else GPIO_OUT_CLEAR = bit;
    GPIO_PIN((uint32_t)pin) = 0;
    GPIO_OUT_SELECT((uint32_t)pin) = 128u | BIT(9);
    IO_MUX((uint32_t)pin) = 1u << 10;
    GPIO_ENABLE_SET = bit; irq_restore(previous); return 0;
}
static int32_t spi_hw_cs_write(int32_t pin, int32_t state) {
    uint32_t bit = BIT((uint32_t)pin);
    if (!(spi_cs_owned & bit)) return EMELA_PERIPHERAL_NOT_CONFIGURED;
    if (state) GPIO_OUT_SET = bit; else GPIO_OUT_CLEAR = bit; return 0;
}
static void spi_hw_cs_release(int32_t pin) { spi_cs_owned &= ~BIT((uint32_t)pin); }
#if __has_include("spi_driver.h")
#include "spi_driver.h"
#else
#include "../common/spi_driver.h"
#endif
