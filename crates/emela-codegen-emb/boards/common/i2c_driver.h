/* Shared synchronous I2C contract. Hardware hooks never retain borrowed buffers.
 * One application controller and one scalar completion record, no allocation.
 * i2c_hw_exchange includes STOP/abort cleanup and reports whether reuse is safe.
 */
static bool i2c_configured, i2c_bound, i2c_busy, i2c_recovery;
static volatile bool i2c_pending, i2c_notify;
static volatile int32_t i2c_result, i2c_operation, i2c_ready;

static int32_t i2c_id_status(int32_t id) {
    if (id < 0 || id > 2) return EMELA_PERIPHERAL_INVALID_ARGUMENT;
    return id == 0 ? 0 : EMELA_PERIPHERAL_UNSUPPORTED;
}
static int32_t i2c_state(int32_t id) {
    int32_t status = i2c_id_status(id);
    if (status) return status;
    if (!i2c_configured) return EMELA_PERIPHERAL_NOT_CONFIGURED;
    if (i2c_busy) return EMELA_PERIPHERAL_BUSY;
    if (i2c_recovery) return EMELA_PERIPHERAL_INVALID_STATE;
    return 0;
}
static bool i2c_expired(uint64_t start, int64_t timeout) {
    return i2c_hw_now() - start >= (uint64_t)timeout;
}

/* Called only by the BSP's software IRQ, after the synchronous result and
 * hardware cleanup are final. Clear pending after the callback has returned.
 */
static void i2c_completion_irq(void) {
    if (!i2c_pending) return;
    if (i2c_notify) {
        if (i2c_result) emela_i2c_transfer_error(0, i2c_operation, i2c_result);
        else if (i2c_operation == EMELA_I2C_OPERATION_IS_DEVICE_READY) emela_i2c_device_ready(0, i2c_ready);
        else emela_i2c_transfer_complete(0, i2c_operation);
    }
    i2c_pending = false;
}

int32_t emela_i2c_configure(EmelaI2cId id, EmelaI2cConfig config) {
    I2C_REQUIRE_MAIN();
    int32_t status = i2c_id_status(id);
    if (status) return status;
    if (i2c_busy) return EMELA_PERIPHERAL_BUSY;
    if (i2c_notify) return EMELA_PERIPHERAL_INVALID_STATE;
    if (config.clock_speed_hz <= 0 || config.clock_speed_hz > 400000
        || config.duty_cycle < 0 || config.duty_cycle > 1
        || (config.clock_speed_hz <= 100000 && config.duty_cycle != 0))
        return EMELA_PERIPHERAL_INVALID_ARGUMENT;
    status = i2c_hw_validate(config);
    if (status) return status; /* Preserve the previous configuration. */
    i2c_configured = false;
    status = i2c_hw_configure(config);
    if (!status) { i2c_configured = true; i2c_recovery = false; }
    return status;
}
int32_t emela_i2c_bind_interrupt(int32_t id) {
    I2C_REQUIRE_MAIN();
    int32_t status = i2c_state(id);
    if (status) return status;
    if (i2c_notify) return EMELA_PERIPHERAL_INVALID_STATE;
    if (!emela_i2c_has_binding(id)) return EMELA_PERIPHERAL_INVALID_ARGUMENT;
    i2c_bound = true;
    return 0;
}
int32_t emela_i2c_enable_interrupt(EmelaI2cId id, EmelaInterruptPriority priority) {
    I2C_REQUIRE_MAIN();
    int32_t status = i2c_state(id);
    if (status) return status;
    if (priority < 0 || priority > 2) return EMELA_PERIPHERAL_INVALID_ARGUMENT;
    if (!i2c_bound || i2c_notify) return EMELA_PERIPHERAL_INVALID_STATE;
    i2c_hw_clear_completion();
    i2c_pending = false;
    i2c_hw_notification(true, priority);
    i2c_notify = true;
    return 0;
}
int32_t emela_i2c_disable_interrupt(EmelaI2cId id) {
    I2C_REQUIRE_MAIN();
    int32_t status = i2c_id_status(id);
    if (status) return status;
    if (!i2c_configured) return EMELA_PERIPHERAL_NOT_CONFIGURED;
    if (i2c_busy) return EMELA_PERIPHERAL_BUSY;
    i2c_hw_notification(false, 0);
    i2c_hw_clear_completion();
    i2c_pending = false;
    i2c_notify = false;
    return 0;
}

static int32_t i2c_request(EmelaI2cId id, int32_t operation, int32_t address,
        int32_t memory, int32_t address_size, const uint16_t *data, int32_t size,
        int32_t trials, int64_t timeout, uint16_t *out, uint16_t capacity,
        uint16_t *length, int32_t *ready) {
    I2C_REQUIRE_MAIN();
    int32_t status = i2c_state(id);
    if (status) return status;
    bool read = operation == EMELA_I2C_OPERATION_MASTER_RECEIVE || operation == EMELA_I2C_OPERATION_MEMORY_READ;
    bool mem = operation == EMELA_I2C_OPERATION_MEMORY_WRITE || operation == EMELA_I2C_OPERATION_MEMORY_READ;
    bool probe = operation == EMELA_I2C_OPERATION_IS_DEVICE_READY;
    if (address < 8 || address > 0x77 || timeout < 0 || size < 0
        || (probe && (trials <= 0 || !ready))
        || (mem && (address_size < 0 || address_size > 1 || memory < 0 || memory > (address_size ? 65535 : 255)))
        || (read && (!length || (size && !out))) || (!read && !probe && size && !data))
        return EMELA_PERIPHERAL_INVALID_ARGUMENT;
    if (read && (uint32_t)size > capacity) return EMELA_PERIPHERAL_CAPACITY_EXCEEDED;
    if (!read && !probe)
        for (int32_t i = 0; i < size; i++) if (data[i] > 255) return EMELA_PERIPHERAL_INVALID_ARGUMENT;
    if (read) *length = 0;
    if (!probe && !size) return 0;
    if (!timeout) return EMELA_PERIPHERAL_TIMEOUT;
    uint8_t prefix[2];
    unsigned prefix_size = 0;
    if (mem) {
        if (address_size) prefix[prefix_size++] = (uint8_t)((uint32_t)memory >> 8);
        prefix[prefix_size++] = (uint8_t)memory;
    }
    i2c_busy = true;
    uint64_t start = i2c_hw_now();
    bool usable = true;
    int32_t device_ready = 0;
    for (int32_t attempt = 0; attempt < (probe ? trials : 1); attempt++) {
        if (i2c_expired(start, timeout)) { status = EMELA_PERIPHERAL_TIMEOUT; break; }
        status = i2c_hw_exchange((uint8_t)address, read, prefix, prefix_size, data,
            (uint16_t)size, out, start, timeout, &usable);
        if ((!status || (probe && status == EMELA_PERIPHERAL_NACK && usable)) && i2c_expired(start, timeout)) status = EMELA_PERIPHERAL_TIMEOUT;
        if (!status) { device_ready = 1; break; }
        if (!(probe && status == EMELA_PERIPHERAL_NACK && usable)) break;
        if (attempt == trials - 1) { status = 0; break; }
    }
    i2c_recovery = !usable;
    /* Publish a fixed scalar record only after the driver has released buffers. */
    if (i2c_notify) {
        i2c_result = status; i2c_operation = operation; i2c_ready = device_ready;
        i2c_pending = true;
        i2c_hw_pend_completion();
        while (i2c_pending) i2c_hw_wait_notification();
    }
    i2c_busy = false;
    if (!status) {
        if (read) *length = (uint16_t)size;
        if (probe) *ready = device_ready;
    }
    return status;
}
int32_t emela_i2c_master_transmit(EmelaI2cId id, int32_t address, const uint16_t *data, uint16_t length, int64_t timeout) {
    return i2c_request(id, EMELA_I2C_OPERATION_MASTER_TRANSMIT, address, 0, 0, data, length, 0, timeout, NULL, 0, NULL, NULL);
}
int32_t emela_i2c_master_receive(EmelaI2cId id, int32_t address, int32_t size, int64_t timeout, uint16_t *out, uint16_t capacity, uint16_t *length) {
    return i2c_request(id, EMELA_I2C_OPERATION_MASTER_RECEIVE, address, 0, 0, NULL, size, 0, timeout, out, capacity, length, NULL);
}
int32_t emela_i2c_memory_write(EmelaI2cId id, int32_t address, int32_t memory, EmelaI2cMemoryAddressSize address_size, const uint16_t *data, uint16_t length, int64_t timeout) {
    return i2c_request(id, EMELA_I2C_OPERATION_MEMORY_WRITE, address, memory, address_size, data, length, 0, timeout, NULL, 0, NULL, NULL);
}
int32_t emela_i2c_memory_read(EmelaI2cId id, int32_t address, int32_t memory, EmelaI2cMemoryAddressSize address_size, int32_t size, int64_t timeout, uint16_t *out, uint16_t capacity, uint16_t *length) {
    return i2c_request(id, EMELA_I2C_OPERATION_MEMORY_READ, address, memory, address_size, NULL, size, 0, timeout, out, capacity, length, NULL);
}
int32_t emela_i2c_is_device_ready(EmelaI2cId id, int32_t address, int32_t trials, int64_t timeout, int32_t *out) {
    return i2c_request(id, EMELA_I2C_OPERATION_IS_DEVICE_READY, address, 0, 0, NULL, 0, trials, timeout, NULL, 0, NULL, out);
}
