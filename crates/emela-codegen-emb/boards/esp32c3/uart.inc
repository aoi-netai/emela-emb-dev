/* UART1 via GPIO matrix: TX GPIO4, RX GPIO5; 40MHz XTAL. */
#define APP_UART(offset) REG(UINT32_C(0x60010000) + (offset))
#define UART_CONSOLE_ID EMELA_UART_ID_UART0
#define UART_REQUIRE_MAIN() require_main()
#define UART_CPU_IRQ 3u
static uint32_t uart_hw_lock(void) { return irq_disable(); }
static void uart_hw_unlock(uint32_t previous) { irq_restore(previous); }
static uint64_t uart_hw_now(void) { return timer_ticks() / TICKS_PER_MS; }
static int32_t uart_hw_validate(EmelaUartConfig config) {
#ifdef BOARD_GPIO
    if (configured_inputs & (BIT(4) | BIT(5))) return EMELA_PERIPHERAL_RESOURCE_CONFLICT;
#endif
    if (config.flow_control != EMELA_UART_FLOW_CONTROL_NO_FLOW_CONTROL || config.oversampling != EMELA_UART_OVERSAMPLING_NATIVE
        || config.data_bits == EMELA_UART_DATA_BITS_BITS9
        || (config.stop_bits != EMELA_UART_STOP_BITS_ONE && config.stop_bits != EMELA_UART_STOP_BITS_TWO)) return EMELA_PERIPHERAL_UNSUPPORTED;
    /* XTAL with an integer prescaler fitting the 12-bit UART divider. */
    if (config.baud_rate < 9600 || config.baud_rate > 1000000) return EMELA_PERIPHERAL_UNSUPPORTED;
    return 0;
}
static void uart_hw_priority(EmelaInterruptPriority priority) { MATRIX(0x120u) = (uint32_t)priority + 1u; }
static void uart_hw_rx_interrupt(bool enabled) { APP_UART(0x0c) = enabled ? APP_UART(0x0c) | 0x1du : APP_UART(0x0c) & ~0x1du; }
static void uart_hw_tc_interrupt(bool enabled) { APP_UART(0x0c) = enabled ? APP_UART(0x0c) | BIT(14) : APP_UART(0x0c) & ~BIT(14); }
static void uart_hw_sync(void) { APP_UART(0x80) |= BIT(31); while (APP_UART(0x80) & BIT(31)) {} }
static void uart_hw_clear_rx(void) {
    APP_UART(0x20) |= BIT(17); uart_hw_sync(); APP_UART(0x20) &= ~BIT(17); uart_hw_sync(); APP_UART(0x10) = 0x1du;
}
static void uart_hw_configure(EmelaUartConfig config) {
    REG(0x600c0010) |= BIT(5) | BIT(24);
    APP_UART(0x78) |= BIT(23);
    REG(0x600c0018) |= BIT(5); REG(0x600c0018) &= ~BIT(5);
    uint32_t baud = (uint32_t)config.baud_rate;
    uint32_t source_divider = (40000000u + 4095u * baud - 1u) / (4095u * baud);
    APP_UART(0x78) = BIT(25) | BIT(24) | BIT(22) | (3u << 20) | ((source_divider - 1u) << 12);
    uint32_t divider = (640000000u + source_divider * baud / 2u) / (source_divider * baud);
    APP_UART(0x14) = (divider >> 4) | ((divider & 15u) << 20);
    APP_UART(0x0c) = 0; APP_UART(0x10) = UINT32_MAX;
    APP_UART(0x20) = ((config.data_bits == EMELA_UART_DATA_BITS_BITS7 ? 2u : 3u) << 2)
        | ((config.stop_bits == EMELA_UART_STOP_BITS_TWO ? 3u : 1u) << 4)
        | (config.parity == EMELA_UART_PARITY_NO_PARITY ? 0 : BIT(1) | (config.parity == EMELA_UART_PARITY_ODD ? 1u : 0));
    APP_UART(0x24) = 1; /* Receive IRQ threshold: one word. */
    APP_UART(0x80) &= ~BIT(30); uart_hw_sync();
    IO_MUX(4) = 1u << 10; IO_MUX(5) = (1u << 10) | BIT(9) | BIT(8);
    GPIO_OUT_SELECT(4) = 9u; /* U1TXD_OUT_IDX */
    REG(0x60004154 + 9u * 4u) = BIT(7) | 5u; /* U1RXD_IN_IDX */
    GPIO_ENABLE_SET = BIT(4); GPIO_ENABLE_CLEAR = BIT(5);
    uart_pins_owned = true;
    uart_hw_clear_rx();
    MATRIX(0x058u) = UART_CPU_IRQ; /* ETS_UART1_INTR_SOURCE = 22. */
    MATRIX(0x108u) &= ~BIT(UART_CPU_IRQ);
    uart_hw_priority(EMELA_INTERRUPT_PRIORITY_LOW);
    MATRIX(0x104u) |= BIT(UART_CPU_IRQ);
}
static bool uart_hw_tx_ready(void) { return ((APP_UART(0x1c) >> 16) & 0x3ffu) < 127u; }
static bool uart_hw_tx_idle(void) { return !(APP_UART(0x1c) & (0x3ffu << 16)) && !(APP_UART(0x6c) & 0xf0u); }
static void uart_hw_write(uint16_t word) { APP_UART(0x10) = BIT(14); APP_UART(0) = word; }
#if __has_include("uart_driver.h")
#include "uart_driver.h"
#else
#include "../common/uart_driver.h"
#endif
static void uart_interrupt(void) {
    uint32_t status = APP_UART(8), errors = (status & BIT(2) ? 1u : 0) | (status & BIT(3) ? 2u : 0) | (status & BIT(4) ? 4u : 0);
    APP_UART(0x10) = status & ~BIT(14);
    if (errors & 3u) { uart_hw_clear_rx(); uart_rx_word(errors, false, 0); }
    else {
        if (errors) uart_rx_word(errors, false, 0);
        /* Bound each IRQ to the hardware FIFO capacity; new arrivals pend again. */
        for (unsigned i = 0; i < 128u && (APP_UART(0x1c) & 0x3ffu); i++) {
            uint16_t word = (uint16_t)APP_UART(0);
            word &= (uint16_t)((1u << (7u + (uint32_t)uart_config.data_bits)) - 1u);
            uart_rx_word(0, true, word);
        }
    }
    if (status & BIT(14)) { APP_UART(0x10) = BIT(14); uart_tx_complete_irq(); }
}
