/* One synchronous application bus, four fixed devices, no heap or retained buffers.
 * Hardware exchange returns only after the final shift, or leaves abort to this layer.
 */
typedef struct { bool reserved, ready; EmelaSpiDeviceConfig config; } SpiDeviceState;
static SpiDeviceState spi_devices[EMELA_SPI_DEVICE_CAPACITY];
static bool spi_configured, spi_busy, spi_recovery;
static EmelaSpiConfig spi_config;
static uint32_t spi_prescaler_power(EmelaSpiPrescaler p) {
    switch (p) {
        case EMELA_SPI_PRESCALER_DIV2: return 0;
        case EMELA_SPI_PRESCALER_DIV4: return 1;
        case EMELA_SPI_PRESCALER_DIV8: return 2;
        case EMELA_SPI_PRESCALER_DIV16: return 3;
        case EMELA_SPI_PRESCALER_DIV32: return 4;
        case EMELA_SPI_PRESCALER_DIV64: return 5;
        case EMELA_SPI_PRESCALER_DIV128: return 6;
        case EMELA_SPI_PRESCALER_DIV256: return 7;
        default: return 0; /* All callers first validate the configuration. */
    }
}

static int32_t spi_id_status(int32_t id) {
    if (id < 0 || id > 5) return EMELA_PERIPHERAL_INVALID_ARGUMENT;
    return id == EMELA_SPI_ID_SPI2 ? 0 : EMELA_PERIPHERAL_UNSUPPORTED;
}
static int32_t spi_validate_config(EmelaSpiConfig c) {
    if (c.mode < 0 || c.mode > 1 || c.direction < 0 || c.direction > 2 || c.data_size < 0 || c.data_size > 1
        || c.polarity < 0 || c.polarity > 1 || c.phase < 0 || c.phase > 1 || c.prescaler < 0 || c.prescaler > 7
        || c.bit_order < 0 || c.bit_order > 1) return EMELA_PERIPHERAL_INVALID_ARGUMENT;
    return spi_hw_validate(c);
}
static bool spi_expired(uint64_t start, int64_t timeout) {
    return (spi_hw_now_us() - start) / 1000u >= (uint64_t)timeout;
}
static int32_t spi_delay(uint64_t duration, uint64_t start, int64_t timeout) {
    uint64_t begin = spi_hw_now_us();
    for (;;) {
        uint64_t now = spi_hw_now_us();
        if ((now - start) / 1000u >= (uint64_t)timeout) return EMELA_PERIPHERAL_TIMEOUT;
        if (!duration || now - begin > duration) return 0;
    }
}
static int32_t spi_state(int32_t id, bool configured) {
    int32_t status = spi_id_status(id);
    if (status) return status;
    if (spi_busy) return EMELA_PERIPHERAL_BUSY;
    if (spi_recovery) return EMELA_PERIPHERAL_INVALID_STATE;
    if (configured && !spi_configured) return EMELA_PERIPHERAL_NOT_CONFIGURED;
    return 0;
}
static int32_t spi_apply_config(EmelaSpiConfig config, uint64_t start, int64_t timeout) {
    spi_configured = false;
    int32_t status = spi_hw_configure(config, start, timeout);
    if (!status) { spi_config = config; spi_configured = true; }
    return status;
}
int32_t emela_spi_configure(EmelaSpiId id, EmelaSpiConfig config) {
    SPI_REQUIRE_MAIN();
    int32_t status = spi_state(id, false);
    if (!status) status = spi_validate_config(config);
    if (status) return status;
    spi_busy = true;
    status = spi_apply_config(config, spi_hw_now_us(), 2);
    spi_busy = false;
    return status;
}
static int32_t spi_validate_words(EmelaSpiConfig config, const uint16_t *data, uint16_t size) {
    if (size && !data) return EMELA_PERIPHERAL_INVALID_ARGUMENT;
    if (config.data_size == EMELA_SPI_DATA_SIZE_BITS8)
        for (uint32_t i = 0; i < size; i++) if (data[i] > 255) return EMELA_PERIPHERAL_INVALID_ARGUMENT;
    return 0;
}
static void spi_abort(void) {
    if (spi_hw_abort()) spi_recovery = true;
    spi_configured = false;
}
static int32_t spi_transfer(int32_t id, const uint16_t *tx, int32_t size, bool read_only,
        int64_t timeout, uint16_t *out, uint16_t capacity, uint16_t *length) {
    SPI_REQUIRE_MAIN();
    int32_t status = spi_state(id, true);
    if (status) return status;
    if (timeout < 0 || timeout > INT64_MAX / 1000 || size < 0 || (length && size && !out)) return EMELA_PERIPHERAL_INVALID_ARGUMENT;
    if (length && (uint32_t)size > capacity) return EMELA_PERIPHERAL_CAPACITY_EXCEEDED;
    if (!read_only) { status = spi_validate_words(spi_config, tx, (uint16_t)size); if (status) return status; }
    if (length) *length = 0;
    if (!size) return 0;
    if (!timeout) return EMELA_PERIPHERAL_TIMEOUT;
    spi_busy = true;
    uint64_t start = spi_hw_now_us();
    status = spi_hw_exchange(spi_config, read_only ? NULL : tx, (uint16_t)size, out, start, timeout);
    if (!status && spi_expired(start, timeout)) status = EMELA_PERIPHERAL_TIMEOUT;
    if (status) spi_abort();
    spi_busy = false;
    if (!status && length) *length = (uint16_t)size;
    return status;
}
int32_t emela_spi_transmit(EmelaSpiId id, const uint16_t *data, uint16_t length, int64_t timeout) {
    return spi_transfer(id, data, length, false, timeout, NULL, 0, NULL);
}
int32_t emela_spi_receive(EmelaSpiId id, int32_t size, int64_t timeout, uint16_t *out, uint16_t capacity, uint16_t *length) {
    if (!length) return EMELA_PERIPHERAL_INVALID_ARGUMENT;
    return spi_transfer(id, NULL, size, true, timeout, out, capacity, length);
}
int32_t emela_spi_transfer(EmelaSpiId id, const uint16_t *data, uint16_t length, int64_t timeout, uint16_t *out, uint16_t capacity, uint16_t *out_length) {
    if (!out_length) return EMELA_PERIPHERAL_INVALID_ARGUMENT;
    return spi_transfer(id, data, length, false, timeout, out, capacity, out_length);
}
static void spi_result(EmelaSpiTransactionStatus *out, int32_t source, int32_t status) {
    out->source = status ? source : EMELA_SPI_FAILURE_NONE; out->status = status; out->cleanup_status = 0;
}
void emela_spi_create_device(EmelaSpiDeviceConfig config, EmelaSpiDevice *out, EmelaSpiTransactionStatus *result) {
    SPI_REQUIRE_MAIN();
    if (!result) return;
    int32_t status = spi_state(config.bus, false);
    if (!status && (!out || config.setup_us < 0 || config.hold_us < 0 || config.active < 0 || config.active > 1)) status = EMELA_PERIPHERAL_INVALID_ARGUMENT;
    if (!status) status = spi_validate_config(config.config);
    spi_result(result, EMELA_SPI_FAILURE_SPI, status);
    if (status) return;
    int slot = -1;
    for (unsigned i = 0; i < EMELA_SPI_DEVICE_CAPACITY; i++) {
        if (spi_devices[i].reserved && spi_devices[i].config.cs.id == config.cs.id) {
            spi_result(result, EMELA_SPI_FAILURE_GPIO, EMELA_PERIPHERAL_RESOURCE_CONFLICT); return;
        }
        if (!spi_devices[i].reserved && slot < 0) slot = (int)i;
    }
    if (slot < 0) { spi_result(result, EMELA_SPI_FAILURE_SPI, EMELA_PERIPHERAL_CAPACITY_EXCEEDED); return; }
    status = spi_hw_cs_validate(config.cs.id);
    if (status) { spi_result(result, EMELA_SPI_FAILURE_GPIO, status); return; }
    spi_busy = true;
    status = spi_apply_config(config.config, spi_hw_now_us(), 2);
    if (status) { spi_result(result, EMELA_SPI_FAILURE_SPI, status); spi_busy = false; return; }
    SpiDeviceState *device = &spi_devices[slot];
    device->reserved = true; device->config = config;
    status = spi_hw_cs_init(config.cs.id, !config.active);
    if (status) { spi_result(result, EMELA_SPI_FAILURE_GPIO, status); spi_recovery = true; }
    else { device->ready = true; out->slot = slot; }
    spi_busy = false;
}
void emela_spi_transaction(EmelaSpiDevice handle, const EmelaSpiOperation *operations, uint16_t count,
        const uint16_t *tx, uint16_t tx_length, int64_t timeout, uint16_t *out, uint16_t capacity, EmelaSpiTransactionStatus *result) {
    SPI_REQUIRE_MAIN();
    if (!result) return;
    spi_result(result, 0, 0);
    if (handle.slot < 0 || handle.slot >= EMELA_SPI_DEVICE_CAPACITY || !spi_devices[handle.slot].ready) {
        spi_result(result, EMELA_SPI_FAILURE_SPI, EMELA_PERIPHERAL_INVALID_ARGUMENT); return;
    }
    const EmelaSpiDeviceConfig *device = &spi_devices[handle.slot].config;
    int32_t status = spi_state(device->bus, false);
    if (!status && (timeout < 0 || timeout > INT64_MAX / 1000 || (count && !operations))) status = EMELA_PERIPHERAL_INVALID_ARGUMENT;
#if EMELA_SPI_TRANSACTION_OPERATION_CAPACITY < UINT16_MAX
    if (!status && count > EMELA_SPI_TRANSACTION_OPERATION_CAPACITY) status = EMELA_PERIPHERAL_CAPACITY_EXCEEDED;
#endif
#if EMELA_SPI_TRANSACTION_WORD_CAPACITY < UINT16_MAX
    if (!status && tx_length > EMELA_SPI_TRANSACTION_WORD_CAPACITY) status = EMELA_PERIPHERAL_CAPACITY_EXCEEDED;
#endif
    if (!status) status = spi_validate_words(device->config, tx, tx_length);
    uint32_t tx_used = 0, rx_used = 0; bool nonempty = false;
    for (uint32_t i = 0; i < count && !status; i++) {
        const EmelaSpiOperation *op = &operations[i];
        if (op->kind < EMELA_SPI_OPERATION_WRITE || op->kind > EMELA_SPI_OPERATION_TRANSFER || op->tx_offset != tx_used || op->rx_offset != rx_used) { status = EMELA_PERIPHERAL_INVALID_ARGUMENT; break; }
        if (op->kind != EMELA_SPI_OPERATION_READ) tx_used += op->length;
        if (op->kind != EMELA_SPI_OPERATION_WRITE) rx_used += op->length;
        nonempty |= op->length != 0;
        if (tx_used > tx_length || rx_used > capacity || rx_used > EMELA_SPI_TRANSACTION_WORD_CAPACITY) status = EMELA_PERIPHERAL_CAPACITY_EXCEEDED;
    }
    if (!status && (tx_used != tx_length || (rx_used && !out))) status = EMELA_PERIPHERAL_INVALID_ARGUMENT;
    if (!status && nonempty && !timeout) status = EMELA_PERIPHERAL_TIMEOUT;
    if (status) { spi_result(result, EMELA_SPI_FAILURE_SPI, status); return; }
    if (!nonempty) return;
    spi_busy = true;
    uint64_t start = spi_hw_now_us();
    status = spi_apply_config(device->config, start, timeout);
    if (!status && spi_expired(start, timeout)) status = EMELA_PERIPHERAL_TIMEOUT;
    bool selected = false;
    if (status) spi_result(result, EMELA_SPI_FAILURE_SPI, status);
    else {
        selected = true; /* Even a failed selection may have changed the pin. */
        status = spi_hw_cs_write(device->cs.id, device->active);
        spi_result(result, EMELA_SPI_FAILURE_GPIO, status);
        if (!status) status = spi_delay((uint64_t)device->setup_us, start, timeout);
        for (uint32_t i = 0; i < count && !status; i++) {
            const EmelaSpiOperation *op = &operations[i];
            if (!op->length) continue;
            if (spi_expired(start, timeout)) { status = EMELA_PERIPHERAL_TIMEOUT; break; }
            status = spi_hw_exchange(device->config, op->kind == EMELA_SPI_OPERATION_READ ? NULL : tx + op->tx_offset,
                op->length, op->kind == EMELA_SPI_OPERATION_WRITE ? NULL : out + op->rx_offset, start, timeout);
        }
        if (!status) status = spi_delay((uint64_t)device->hold_us, start, timeout);
        if (status && !result->source) spi_result(result, EMELA_SPI_FAILURE_SPI, status);
    }
    if (status) spi_abort();
    if (selected) {
        result->cleanup_status = spi_hw_cs_write(device->cs.id, !device->active);
        if (result->cleanup_status) spi_recovery = true;
    }
    if (!status && spi_expired(start, timeout)) { result->source = EMELA_SPI_FAILURE_SPI; result->status = EMELA_PERIPHERAL_TIMEOUT; }
    spi_busy = false;
}
void emela_spi_recover(EmelaSpiId id, EmelaSpiTransactionStatus *result) {
    SPI_REQUIRE_MAIN();
    if (!result) return;
    int32_t status = spi_id_status(id);
    if (!status && spi_busy) status = EMELA_PERIPHERAL_BUSY;
    spi_result(result, EMELA_SPI_FAILURE_SPI, status);
    if (status) return;
    spi_busy = true;
    spi_result(result, EMELA_SPI_FAILURE_SPI, spi_hw_abort());
    spi_configured = false;
    for (unsigned i = 0; i < EMELA_SPI_DEVICE_CAPACITY; i++) if (spi_devices[i].reserved) {
        int32_t cleanup = spi_hw_cs_write(spi_devices[i].config.cs.id, !spi_devices[i].config.active);
        if (cleanup && !result->cleanup_status) result->cleanup_status = cleanup;
    }
    spi_recovery = result->source != 0 || result->cleanup_status != 0;
    if (!spi_recovery) for (unsigned i = 0; i < EMELA_SPI_DEVICE_CAPACITY; i++) if (spi_devices[i].reserved && !spi_devices[i].ready) {
        spi_hw_cs_release(spi_devices[i].config.cs.id); spi_devices[i].reserved = false;
    }
    spi_busy = false;
}
