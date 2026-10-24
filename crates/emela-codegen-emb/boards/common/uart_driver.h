/* Shared synchronous UART state machine. Included after the BSP's uart_hw_* helpers.
 * One application UART, RX IRQ producer / main consumer, no retained transfer arrays.
 * IRQ masks protect all ring/error/result transitions; callbacks execute only in ISR.
 */
#define UART_RX_CAPACITY 32u
static bool uart_configured, uart_bound, uart_mode_selected;
static volatile bool uart_irq_enabled, uart_tx_notify, uart_tx_wait, uart_tx_done, uart_tx_expired;
static volatile uint64_t uart_tx_start, uart_tx_timeout;
static EmelaUartConfig uart_config;
static EmelaUartReceiveMode uart_mode;
static volatile uint16_t uart_rx[UART_RX_CAPACITY];
static volatile uint32_t uart_head, uart_tail, uart_errors;
volatile uint32_t board_uart_events;

static int32_t uart_id_status(EmelaUartId id) {
    if (id < 0 || id > 7) return EMELA_PERIPHERAL_INVALID_ARGUMENT;
    if (id == UART_CONSOLE_ID) return EMELA_PERIPHERAL_RESOURCE_CONFLICT;
    if (id != EMELA_UART_ID_UART1) return EMELA_PERIPHERAL_UNSUPPORTED;
    return 0;
}
static int32_t uart_ready_status(EmelaUartId id) {
    int32_t status = uart_id_status(id);
    return status ? status : uart_configured ? 0 : EMELA_PERIPHERAL_NOT_CONFIGURED;
}
static bool uart_can_rx(void) { return uart_config.direction != EMELA_UART_DIRECTION_TRANSMIT; }
static bool uart_can_tx(void) { return uart_config.direction != EMELA_UART_DIRECTION_RECEIVE; }
static int32_t uart_rx_status(EmelaUartId id) {
    int32_t status = uart_ready_status(id);
    if (status) return status;
    if (!uart_can_rx() || uart_mode != EMELA_UART_RECEIVE_MODE_POLLING) return EMELA_PERIPHERAL_INVALID_STATE;
    return 0;
}
/* bits 0..3 are parity/framing/overrun/other, preserved together for events. */
static int32_t uart_take_error(void) {
    uint32_t errors = uart_errors;
    uart_errors = 0;
    return errors & 1u ? EMELA_PERIPHERAL_PARITY : errors & 2u ? EMELA_PERIPHERAL_FRAMING
        : errors & 4u ? EMELA_PERIPHERAL_OVERRUN : errors ? EMELA_PERIPHERAL_OTHER : 0;
}
static void uart_rx_word(uint32_t errors, bool has_word, uint16_t word) {
    if (!uart_configured || !uart_can_rx()) return;
    if (uart_mode == EMELA_UART_RECEIVE_MODE_INTERRUPT_RECEIVE) {
        if (!uart_irq_enabled) return;
        if (errors) {
            board_uart_events++;
            emela_uart_receive_error(EMELA_UART_ID_UART1, errors & 1u, errors & 2u, errors & 4u, errors & 8u);
        }
        if (has_word && !(errors & 3u)) {
            board_uart_events++;
            emela_uart_received(EMELA_UART_ID_UART1, word);
        }
    } else {
        uart_errors |= errors;
        if (has_word && !(errors & 3u)) {
            if (uart_head - uart_tail == UART_RX_CAPACITY) uart_errors |= 4u;
            else { uart_rx[uart_head % UART_RX_CAPACITY] = word; uart_head++; }
        }
    }
}
static void uart_tx_complete_irq(void) {
    uart_hw_tc_interrupt(false);
    if (uart_tx_wait) {
        uart_tx_wait = false;
        uart_tx_expired = uart_hw_now() - uart_tx_start >= uart_tx_timeout;
        if (!uart_tx_expired && uart_irq_enabled && uart_tx_notify) {
            board_uart_events++;
            emela_uart_transmit_complete(EMELA_UART_ID_UART1);
        }
        uart_tx_done = true;
    }
}
int32_t emela_uart_configure(EmelaUartId id, EmelaUartConfig config) {
    UART_REQUIRE_MAIN();
    int32_t status = uart_id_status(id);
    if (status) return status;
    if (config.baud_rate <= 0 || config.data_bits < 0 || config.data_bits > 2 || config.stop_bits < 0 || config.stop_bits > 3
        || config.parity < 0 || config.parity > 2 || config.direction < 0 || config.direction > 2
        || config.flow_control < 0 || config.flow_control > 3 || config.oversampling < 0 || config.oversampling > 2)
        return EMELA_PERIPHERAL_INVALID_ARGUMENT;
    if (uart_irq_enabled) return EMELA_PERIPHERAL_INVALID_STATE;
    status = uart_hw_validate(config);
    if (status) return status;
    uint32_t previous = uart_hw_lock();
    uart_configured = false;
    uart_hw_configure(config);
    uart_config = config;
    uart_head = uart_tail = uart_errors = 0;
    uart_tx_wait = uart_tx_done = false;
    uart_configured = true;
    uart_hw_rx_interrupt(uart_can_rx() && uart_mode == EMELA_UART_RECEIVE_MODE_POLLING);
    uart_hw_unlock(previous);
    return 0;
}
int32_t emela_uart_set_receive_mode(EmelaUartId id, EmelaUartReceiveMode mode) {
    UART_REQUIRE_MAIN();
    int32_t status = uart_ready_status(id);
    if (status) return status;
    if (mode < 0 || mode > 1) return EMELA_PERIPHERAL_INVALID_ARGUMENT;
    if (uart_mode_selected || uart_bound || uart_irq_enabled) return EMELA_PERIPHERAL_INVALID_STATE;
    if (!uart_can_rx() && mode == EMELA_UART_RECEIVE_MODE_INTERRUPT_RECEIVE) return EMELA_PERIPHERAL_INVALID_STATE;
    uint32_t previous = uart_hw_lock();
    uart_mode = mode; uart_mode_selected = true;
    uart_head = uart_tail = uart_errors = 0;
    uart_hw_clear_rx();
    uart_hw_rx_interrupt(uart_can_rx() && mode == EMELA_UART_RECEIVE_MODE_POLLING);
    uart_hw_unlock(previous);
    return 0;
}
int32_t emela_uart_bind_interrupt(EmelaUartId id) {
    UART_REQUIRE_MAIN();
    int32_t status = uart_ready_status(id);
    if (status) return status;
    if (uart_irq_enabled) return EMELA_PERIPHERAL_INVALID_STATE;
    if (!emela_uart_has_binding(id)) return EMELA_PERIPHERAL_INVALID_ARGUMENT;
    uart_bound = true;
    return 0;
}
int32_t emela_uart_enable_interrupt(EmelaUartId id, EmelaInterruptPriority priority, int32_t notify_transmit) {
    UART_REQUIRE_MAIN();
    int32_t status = uart_ready_status(id);
    if (status) return status;
    if (priority < 0 || priority > 2 || notify_transmit < 0 || notify_transmit > 1) return EMELA_PERIPHERAL_INVALID_ARGUMENT;
    if (!uart_bound || uart_irq_enabled) return EMELA_PERIPHERAL_INVALID_STATE;
    if ((uart_mode == EMELA_UART_RECEIVE_MODE_POLLING && !notify_transmit) || (notify_transmit && !uart_can_tx())) return EMELA_PERIPHERAL_INVALID_STATE;
    uint32_t previous = uart_hw_lock();
    uart_hw_tc_interrupt(false);
    if (uart_mode == EMELA_UART_RECEIVE_MODE_INTERRUPT_RECEIVE) uart_hw_clear_rx();
    uart_hw_priority(priority);
    uart_tx_notify = notify_transmit != 0;
    uart_irq_enabled = true;
    uart_hw_rx_interrupt(uart_can_rx());
    uart_hw_unlock(previous);
    return 0;
}
int32_t emela_uart_disable_interrupt(EmelaUartId id) {
    UART_REQUIRE_MAIN();
    int32_t status = uart_ready_status(id);
    if (status) return status;
    uint32_t previous = uart_hw_lock();
    uart_irq_enabled = uart_tx_notify = uart_tx_wait = false;
    uart_hw_tc_interrupt(false);
    if (uart_mode == EMELA_UART_RECEIVE_MODE_INTERRUPT_RECEIVE) { uart_hw_rx_interrupt(false); uart_hw_clear_rx(); }
    uart_hw_unlock(previous);
    return 0;
}
int32_t emela_uart_transmit(EmelaUartId id, const uint16_t *data, uint16_t length, int64_t timeout) {
    UART_REQUIRE_MAIN();
    int32_t status = uart_ready_status(id);
    if (status) return status;
    if (timeout < 0 || (length && !data)) return EMELA_PERIPHERAL_INVALID_ARGUMENT;
    if (!uart_can_tx()) return EMELA_PERIPHERAL_INVALID_STATE;
    uint32_t limit = (1u << (7u + (uint32_t)uart_config.data_bits)) - 1u;
    for (uint16_t i = 0; i < length; i++) if (data[i] > limit) return EMELA_PERIPHERAL_INVALID_ARGUMENT;
    if (!length) return 0;
    if (!timeout) return EMELA_PERIPHERAL_TIMEOUT;
    uint64_t start = uart_hw_now();
    for (uint16_t i = 0; i < length; i++) {
        while (!uart_hw_tx_ready()) if (uart_hw_now() - start >= (uint64_t)timeout) goto expired;
        if (uart_hw_now() - start >= (uint64_t)timeout) goto expired;
        uart_hw_write(data[i]);
    }
    if (uart_irq_enabled && uart_tx_notify) {
        uint32_t previous = uart_hw_lock();
        uart_tx_done = uart_tx_expired = false;
        uart_tx_start = start; uart_tx_timeout = (uint64_t)timeout;
        uart_tx_wait = true;
        uart_hw_tc_interrupt(true);
        uart_hw_unlock(previous);
        while (!uart_tx_done) {
            if (uart_hw_now() - start >= (uint64_t)timeout) {
                previous = uart_hw_lock();
                if (uart_tx_done) { uart_hw_unlock(previous); return uart_tx_expired ? EMELA_PERIPHERAL_TIMEOUT : 0; }
                uart_tx_wait = false; uart_hw_tc_interrupt(false);
                uart_hw_unlock(previous);
                goto expired;
            }
        }
        if (uart_tx_expired) goto expired;
    } else {
        for (;;) {
            if (uart_hw_now() - start >= (uint64_t)timeout) goto expired;
            if (uart_hw_tx_idle()) break;
        }
    }
    return 0;
expired:
    /* Already queued words may finish physically; no array reference is retained. */
    return EMELA_PERIPHERAL_TIMEOUT;
}
static int32_t uart_receive(EmelaUartId id, int32_t size, int64_t timeout, uint16_t *out, uint16_t capacity, uint16_t *length, bool some) {
    UART_REQUIRE_MAIN();
    int32_t status = uart_rx_status(id);
    if (status) return status;
    if (size < 0 || timeout < 0 || !length || (size && !out)) return EMELA_PERIPHERAL_INVALID_ARGUMENT;
    *length = 0;
    if ((uint32_t)size > capacity) return EMELA_PERIPHERAL_CAPACITY_EXCEEDED;
    uart_mode_selected = true;
    if (!size) return 0;
    uint64_t start = uart_hw_now();
    uint32_t previous = uart_hw_lock();
    status = uart_take_error();
    if (!status && !timeout && uart_head - uart_tail < (some ? 1u : (uint32_t)size)) status = EMELA_PERIPHERAL_TIMEOUT;
    uart_hw_unlock(previous);
    if (status) return status;
    while (*length < (uint32_t)size) {
        previous = uart_hw_lock();
        status = uart_take_error();
        if (status) { uart_hw_unlock(previous); return status; }
        if (uart_head != uart_tail) { out[(*length)++] = uart_rx[uart_tail % UART_RX_CAPACITY]; uart_tail++; }
        else if (some && *length) { uart_hw_unlock(previous); return 0; }
        else if (uart_hw_now() - start >= (uint64_t)timeout) { uart_hw_unlock(previous); return EMELA_PERIPHERAL_TIMEOUT; }
        uart_hw_unlock(previous);
    }
    return 0;
}
int32_t emela_uart_receive(EmelaUartId id, int32_t size, int64_t timeout, uint16_t *out, uint16_t capacity, uint16_t *length) {
    return uart_receive(id, size, timeout, out, capacity, length, false);
}
int32_t emela_uart_receive_some(EmelaUartId id, int32_t size, int64_t timeout, uint16_t *out, uint16_t capacity, uint16_t *length) {
    return uart_receive(id, size, timeout, out, capacity, length, true);
}
int32_t emela_uart_discard_input(EmelaUartId id, int32_t max_size, int32_t *count) {
    UART_REQUIRE_MAIN();
    int32_t status = uart_rx_status(id);
    if (status) return status;
    if (max_size < 0 || !count) return EMELA_PERIPHERAL_INVALID_ARGUMENT;
    uart_mode_selected = true;
    uint32_t previous = uart_hw_lock();
    uint32_t available = uart_head - uart_tail;
    *count = (uint32_t)max_size < available ? max_size : (int32_t)available;
    uart_tail += (uint32_t)*count;
    /* Discard explicitly acknowledges observed RX errors, including overrun. */
    if (max_size) uart_errors = 0;
    uart_hw_unlock(previous);
    return 0;
}
