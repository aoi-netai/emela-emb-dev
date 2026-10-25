/* USART1 on PA9 (TX) / PA10 (RX), HSI/APB2 = 16MHz. */
#define APP_UART(offset) REG(UINT32_C(0x40011000) + (offset))
#define UART_CONSOLE_ID EMELA_CONSOLE_UART_ID
#define UART_REQUIRE_MAIN() main_context()
static uint32_t uart_hw_lock(void) {
    uint32_t previous = interrupt_state();
    __asm volatile("cpsid i" ::: "memory"); return previous;
}
static void uart_hw_unlock(uint32_t previous) { __asm volatile("msr primask, %0" :: "r"(previous) : "memory"); }
static uint64_t uart_hw_now(void) { return peripheral_now_ms(); }
static int32_t uart_hw_validate(EmelaUartConfig config) {
#if BOARD_HAS_SPI
    if (spi_cs_owned[0] & ((1u << 9) | (1u << 10))) return EMELA_PERIPHERAL_RESOURCE_CONFLICT;
#endif
#if BOARD_HAS_GPIO
    if (configured_pins[0] & ((1u << 9) | (1u << 10))) return EMELA_PERIPHERAL_RESOURCE_CONFLICT;
#endif
    if (config.flow_control != EMELA_UART_FLOW_CONTROL_NO_FLOW_CONTROL
        || (config.stop_bits != EMELA_UART_STOP_BITS_ONE && config.stop_bits != EMELA_UART_STOP_BITS_TWO)
        || (config.data_bits == EMELA_UART_DATA_BITS_BITS7 && config.parity == EMELA_UART_PARITY_NO_PARITY)
        || (config.data_bits == EMELA_UART_DATA_BITS_BITS9 && config.parity != EMELA_UART_PARITY_NO_PARITY))
        return EMELA_PERIPHERAL_UNSUPPORTED;
    uint64_t divider = (UINT64_C(16000000) + (uint32_t)config.baud_rate / 2u) / (uint32_t)config.baud_rate;
    if (divider < (config.oversampling == EMELA_UART_OVERSAMPLING_BY8 ? 8u : 16u) || divider > (config.oversampling == EMELA_UART_OVERSAMPLING_BY8 ? 32767u : 65535u)) return EMELA_PERIPHERAL_UNSUPPORTED;
    uint64_t target = divider * (uint32_t)config.baud_rate;
    uint64_t error = target > 16000000u ? target - 16000000u : 16000000u - target;
    if (error * 100u > target * 2u) return EMELA_PERIPHERAL_UNSUPPORTED;
    return 0;
}
static void uart_hw_priority(EmelaInterruptPriority priority) {
    *(volatile uint8_t *)(uintptr_t)(UINT32_C(0xe000e400) + 37u) = (uint8_t)(UINT32_C(0xc0) - (uint32_t)priority * UINT32_C(0x40));
}
static void uart_hw_rx_interrupt(bool enabled) {
    APP_UART(0x0c) = enabled ? APP_UART(0x0c) | (1u << 5) | (1u << 8) : APP_UART(0x0c) & ~((1u << 5) | (1u << 8));
    APP_UART(0x14) = enabled ? APP_UART(0x14) | 1u : APP_UART(0x14) & ~1u;
}
static void uart_hw_tc_interrupt(bool enabled) { APP_UART(0x0c) = enabled ? APP_UART(0x0c) | (1u << 6) : APP_UART(0x0c) & ~(1u << 6); }
static void uart_hw_clear_rx(void) { (void)APP_UART(0); (void)APP_UART(4); }
static void uart_hw_configure(EmelaUartConfig config) {
    RCC_APB2ENR |= 1u << 4; (void)RCC_APB2ENR;
    init_uart_pin(9); init_uart_pin(10); /* All application USART1 pins use AF7. */
    GPIO_AFR(0, 9) = (GPIO_AFR(0, 9) & ~(UINT32_C(0xff) << 4)) | (UINT32_C(0x77) << 4);
    uart_pins_owned = true;
    APP_UART(0x0c) = 0; APP_UART(0x10) = config.stop_bits == EMELA_UART_STOP_BITS_TWO ? 2u << 12 : 0;
    APP_UART(0x14) = 0;
    uint32_t divider = (16000000u + (uint32_t)config.baud_rate / 2u) / (uint32_t)config.baud_rate;
    APP_UART(8) = config.oversampling == EMELA_UART_OVERSAMPLING_BY8 ? ((divider / 8u) << 4) | (divider % 8u) : divider;
    uint32_t cr1 = 1u << 13;
    if (config.data_bits == EMELA_UART_DATA_BITS_BITS9 || (config.data_bits == EMELA_UART_DATA_BITS_BITS8 && config.parity != EMELA_UART_PARITY_NO_PARITY)) cr1 |= 1u << 12;
    if (config.parity != EMELA_UART_PARITY_NO_PARITY) cr1 |= (1u << 10) | (config.parity == EMELA_UART_PARITY_ODD ? 1u << 9 : 0);
    if (config.oversampling == EMELA_UART_OVERSAMPLING_BY8) cr1 |= 1u << 15;
    if (config.direction != EMELA_UART_DIRECTION_TRANSMIT) cr1 |= 1u << 2;
    if (config.direction != EMELA_UART_DIRECTION_RECEIVE) cr1 |= 1u << 3;
    APP_UART(0x0c) = cr1;
    uart_hw_clear_rx();
    uart_hw_priority(EMELA_INTERRUPT_PRIORITY_LOW);
    REG(0xe000e284) = 1u << 5; REG(0xe000e104) = 1u << 5;
}
static bool uart_hw_tx_ready(void) { return (APP_UART(0) & (1u << 7)) != 0; }
static bool uart_hw_tx_idle(void) { return (APP_UART(0) & (1u << 6)) != 0; }
static void uart_hw_write(uint16_t word) { APP_UART(4) = word; }
#if __has_include("uart_driver.h")
#include "uart_driver.h"
#else
#include "../../common/uart_driver.h"
#endif
void USART1_IRQHandler(void) {
    emela_validation_stack_record_sp();
    uint32_t sr = APP_UART(0), cr1 = APP_UART(0x0c);
    if ((cr1 & (1u << 5)) && (sr & 0x2fu)) {
        uint16_t word = (uint16_t)APP_UART(4); /* SR -> DR clears RX/error flags. */
        uint32_t errors = (sr & 1u ? 1u : 0) | (sr & 2u ? 2u : 0) | (sr & 8u ? 4u : 0) | (sr & 4u ? 8u : 0);
        word &= (uint16_t)((1u << (7u + (uint32_t)uart_config.data_bits)) - 1u);
        uart_rx_word(errors, (sr & (1u << 5)) != 0, word);
    }
    if ((cr1 & (1u << 6)) && (sr & (1u << 6))) uart_tx_complete_irq();
}
