/* Exercise the exact shared BSP state machine with deterministic hardware/time. */
#include <assert.h>
#include <stdbool.h>
#include <stdint.h>
#include "emela_uart.h"
#define UART_REQUIRE_MAIN() ((void)0)
#define UART_CONSOLE_ID 0
static bool locked, tx_ready = true, tx_idle = true, tc_irq, fire_tc = true;
static bool inject_rx, rx_irq;
static unsigned configured, clear_rx, emitted, words_sent;
static uint64_t now;
static void uart_tx_complete_irq(void);
static void uart_rx_word(uint32_t, bool, uint16_t);
static uint32_t uart_hw_lock(void) { uint32_t previous = locked; locked = true; return previous; }
static void uart_hw_unlock(uint32_t previous) { locked = previous != 0; }
static uint64_t uart_hw_now(void) {
    if (!locked && tc_irq && fire_tc) uart_tx_complete_irq();
    if (!locked && rx_irq && inject_rx) { inject_rx = false; uart_rx_word(0, true, 42); }
    return now++;
}
static int32_t uart_hw_validate(EmelaUartConfig c) { return c.flow_control ? EMELA_PERIPHERAL_UNSUPPORTED : 0; }
static void uart_hw_configure(EmelaUartConfig c) { (void)c; configured++; }
static void uart_hw_clear_rx(void) { clear_rx++; }
static void uart_hw_rx_interrupt(bool enabled) { rx_irq = enabled; }
static void uart_hw_tc_interrupt(bool enabled) { tc_irq = enabled; }
static void uart_hw_priority(EmelaInterruptPriority priority) { assert(priority >= 0 && priority <= 2); }
static bool uart_hw_tx_ready(void) { return tx_ready; }
static bool uart_hw_tx_idle(void) { return tx_idle; }
static void uart_hw_write(uint16_t word) { assert(word <= 511); words_sent++; }
int32_t emela_uart_has_binding(int32_t id) { return id == 1; }
void emela_uart_received(int32_t id, int32_t word) { assert(id == 1 && word <= 511); emitted++; }
void emela_uart_receive_error(int32_t id, int32_t p, int32_t f, int32_t o, int32_t other) { assert(id == 1 && (p || f || o || other)); emitted++; }
void emela_uart_transmit_complete(int32_t id) { assert(id == 1); emitted++; }
/* @DRIVER@ */
int main(void) {
    EmelaUartConfig c = {115200, EMELA_UART_DATA_BITS_BITS8, EMELA_UART_STOP_BITS_ONE,
        EMELA_UART_PARITY_NO_PARITY, EMELA_UART_DIRECTION_BOTH, EMELA_UART_FLOW_CONTROL_NO_FLOW_CONTROL, EMELA_UART_OVERSAMPLING_BY16};
    uint16_t out[64], length = 0, data[] = {0,10,255}, bad[] = {0,256};
    int32_t discarded;
    assert(emela_uart_transmit(1, data, 3, 100) == EMELA_PERIPHERAL_NOT_CONFIGURED);
    assert(emela_uart_configure(0, c) == EMELA_PERIPHERAL_RESOURCE_CONFLICT);
    assert(emela_uart_configure(8, c) == EMELA_PERIPHERAL_INVALID_ARGUMENT);
    assert(emela_uart_configure(2, c) == EMELA_PERIPHERAL_UNSUPPORTED);
    assert(emela_uart_configure(1, c) == 0 && configured == 1 && rx_irq);
    assert(emela_uart_transmit(1, bad, 2, 100) == EMELA_PERIPHERAL_INVALID_ARGUMENT && words_sent == 0);
    assert(emela_uart_transmit(1, data, 0, -1) == EMELA_PERIPHERAL_INVALID_ARGUMENT);
    assert(emela_uart_transmit(1, data, 0, 0) == 0);
    assert(emela_uart_transmit(1, data, 3, 0) == EMELA_PERIPHERAL_TIMEOUT && words_sent == 0);
    assert(emela_uart_transmit(1, data, 3, 100) == 0 && words_sent == 3);
    assert(emela_uart_receive(1, -1, 0, out, 64, &length) == EMELA_PERIPHERAL_INVALID_ARGUMENT);
    assert(emela_uart_receive(1, 65, 0, out, 64, &length) == EMELA_PERIPHERAL_CAPACITY_EXCEEDED);
    assert(emela_uart_receive(1, 0, 0, out, 64, &length) == 0 && length == 0);
    uart_rx_word(0, true, 0); uart_rx_word(0, true, 255);
    assert(emela_uart_receive(1, 3, 0, out, 64, &length) == EMELA_PERIPHERAL_TIMEOUT && uart_tail == 0);
    assert(emela_uart_receive(1, 2, 0, out, 64, &length) == 0 && length == 2 && out[0] == 0 && out[1] == 255);
    inject_rx = true;
    assert(emela_uart_receive_some(1, 16, 100, out, 64, &length) == 0 && length == 1 && out[0] == 42);
    assert(emela_uart_receive_some(1, 16, 0, out, 64, &length) == EMELA_PERIPHERAL_TIMEOUT);
    uart_rx_word(3, true, 99); /* parity/framing word must not enter the ring */
    assert(emela_uart_receive(1, 1, 0, out, 64, &length) == EMELA_PERIPHERAL_PARITY);
    assert(uart_head == uart_tail && uart_errors == 0);
    uart_rx_word(2, false, 0);
    assert(emela_uart_receive(1, 1, 0, out, 64, &length) == EMELA_PERIPHERAL_FRAMING);
    for (unsigned i = 0; i < 33; i++) uart_rx_word(0, true, (uint16_t)i);
    assert(emela_uart_receive(1, 1, 0, out, 64, &length) == EMELA_PERIPHERAL_OVERRUN);
    assert(emela_uart_discard_input(1, 31, &discarded) == 0 && discarded == 31);
    assert(emela_uart_receive_some(1, 16, 0, out, 64, &length) == 0 && length == 1 && out[0] == 31);
    /* Fixed-length positive timeout consumes a prefix, but never succeeds short. */
    uart_rx_word(0, true, 7);
    assert(emela_uart_receive(1, 2, 5, out, 64, &length) == EMELA_PERIPHERAL_TIMEOUT && uart_head == uart_tail);
    assert(emela_uart_enable_interrupt(1, 1, 1) == EMELA_PERIPHERAL_INVALID_STATE);
    assert(emela_uart_bind_interrupt(1) == 0);
    assert(emela_uart_enable_interrupt(1, 1, 1) == 0); /* polling + TX notifications */
    assert(emela_uart_transmit(1, data, 3, 100) == 0 && emitted == 1 && !uart_tx_wait);
    assert(emela_uart_transmit(1, data, 0, 0) == 0 && emitted == 1);
    assert(emela_uart_transmit(1, data, 3, 4) == EMELA_PERIPHERAL_TIMEOUT && emitted == 1); /* TC serviced after deadline */
    fire_tc = false;
    assert(emela_uart_transmit(1, data, 3, 5) == EMELA_PERIPHERAL_TIMEOUT && emitted == 1 && !tc_irq);
    uart_tx_complete_irq(); assert(emitted == 1); /* stale TC cannot become a success */
    fire_tc = true;
    assert(emela_uart_transmit(1, data, 3, 100) == 0 && emitted == 2);
    assert(emela_uart_configure(1, c) == EMELA_PERIPHERAL_INVALID_STATE);
    assert(emela_uart_disable_interrupt(1) == 0 && emela_uart_disable_interrupt(1) == 0);
    c.flow_control = 1;
    assert(emela_uart_configure(1, c) == EMELA_PERIPHERAL_UNSUPPORTED && configured == 1);
    c.flow_control = 0;
    tx_ready = false;
    assert(emela_uart_transmit(1, data, 3, 5) == EMELA_PERIPHERAL_TIMEOUT);
    tx_ready = true; tx_idle = false;
    assert(emela_uart_transmit(1, data, 3, 10) == EMELA_PERIPHERAL_TIMEOUT);
    tx_idle = true;
    /* A new application boot for the one-time receive-mode selection. */
    uart_bound = false; uart_mode_selected = false;
    assert(emela_uart_set_receive_mode(1, 1) == 0);
    assert(emela_uart_set_receive_mode(1, 0) == EMELA_PERIPHERAL_INVALID_STATE);
    assert(emela_uart_bind_interrupt(1) == 0 && emela_uart_enable_interrupt(1, 2, 0) == 0);
    assert(emela_uart_receive(1, 0, 0, out, 64, &length) == EMELA_PERIPHERAL_INVALID_STATE);
    assert(emela_uart_receive_some(1, 0, 0, out, 64, &length) == EMELA_PERIPHERAL_INVALID_STATE);
    assert(emela_uart_discard_input(1, 0, &discarded) == EMELA_PERIPHERAL_INVALID_STATE);
    uart_rx_word(3, true, 99); assert(emitted == 3);
    uart_rx_word(4, true, 7); assert(emitted == 5); /* overrun + intact word */
    assert(emela_uart_disable_interrupt(1) == 0);
    uart_rx_word(0, true, 1); assert(emitted == 5);
    assert(emela_uart_configure(1, c) == 0 && uart_mode == 1 && !rx_irq);
    assert(clear_rx > 0 && !locked);
}
