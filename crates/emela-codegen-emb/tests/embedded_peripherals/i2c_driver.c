/* The exact BSP contract with deterministic time, transfer and software IRQ. */
#include <assert.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include "emela_i2c.h"
#define I2C_REQUIRE_MAIN() assert(!in_isr)
static bool in_isr, software_irq, enabled, reusable = true;
static uint64_t now, cost;
static unsigned transfers, notifications, configurations;
static int32_t hardware_status, validation_status, configuration_status;
static int32_t results[8], last_status, last_operation, last_ready;
static uint64_t request_start;
static uint8_t expected_prefix[2];
static unsigned expected_prefix_size;
static void i2c_completion_irq(void);
static uint64_t i2c_hw_now(void) { return now++; }
static int32_t i2c_hw_validate(EmelaI2cConfig c) { (void)c; return validation_status; }
static int32_t i2c_hw_configure(EmelaI2cConfig c) { (void)c; configurations++; return configuration_status; }
static void i2c_hw_clear_completion(void) { software_irq = false; }
static void i2c_hw_notification(bool enable, EmelaInterruptPriority p) { assert(p >= 0 && p <= 2); enabled = enable; }
static void i2c_hw_pend_completion(void) { assert(enabled && !software_irq); software_irq = true; }
static void i2c_hw_wait_notification(void) {
    assert(software_irq && !in_isr); software_irq = false;
    in_isr = true; i2c_completion_irq(); in_isr = false;
}
static int32_t i2c_hw_exchange(uint8_t address, bool read, const uint8_t *prefix, unsigned n,
        const uint16_t *data, uint16_t size, uint16_t *out, uint64_t start, int64_t timeout, bool *usable) {
    assert(address >= 8 && address <= 0x77 && timeout > 0);
    assert(n == expected_prefix_size);
    for (unsigned i = 0; i < n; i++) assert(prefix[i] == expected_prefix[i]);
    if (transfers == 0) request_start = start;
    else assert(start == request_start); /* All probe trials share one deadline. */
    *usable = reusable;
    now += cost;
    if (read) for (unsigned i = 0; i < size; i++) out[i] = i & 255u;
    else for (unsigned i = 0; i < size; i++) assert(data[i] <= 255);
    int32_t result = hardware_status ? hardware_status : results[transfers < 8 ? transfers : 7];
    transfers++;
    return result;
}
int32_t emela_i2c_has_binding(int32_t id) { return id == 0; }
static void record(int32_t op, int32_t status, int32_t ready) {
    assert(in_isr); notifications++; last_operation = op; last_status = status; last_ready = ready;
}
void emela_i2c_transfer_complete(int32_t id, int32_t op) { assert(id == 0); record(op, 0, -1); }
void emela_i2c_transfer_error(int32_t id, int32_t op, int32_t status) { assert(id == 0 && status); record(op, status, -1); }
void emela_i2c_device_ready(int32_t id, int32_t ready) { assert(id == 0); record(EMELA_I2C_OPERATION_IS_DEVICE_READY, 0, ready); }
/* @DRIVER@ */
static void reset_transfer(void) { transfers = 0; hardware_status = 0; cost = 0; reusable = true; expected_prefix_size = 0; for (unsigned i = 0; i < 8; i++) results[i] = 0; }
int main(void) {
    EmelaI2cConfig c = {100000, 0};
    uint16_t data[] = {0, 255}, bad[] = {0, 256}, out[64], length = 99;
    int32_t ready = -1;
    assert(emela_i2c_master_transmit(0, 64, data, 2, 100) == EMELA_PERIPHERAL_NOT_CONFIGURED);
    assert(emela_i2c_configure(-1, c) == EMELA_PERIPHERAL_INVALID_ARGUMENT);
    assert(emela_i2c_configure(1, c) == EMELA_PERIPHERAL_UNSUPPORTED);
    assert(emela_i2c_configure(0, c) == 0 && configurations == 1);
    for (unsigned i = 0; i < 4; i++) {
        EmelaI2cConfig invalid[] = {{0,0}, {400001,0}, {100000,1}, {100000,2}};
        assert(emela_i2c_configure(0, invalid[i]) == EMELA_PERIPHERAL_INVALID_ARGUMENT);
    }
    validation_status = EMELA_PERIPHERAL_RESOURCE_CONFLICT;
    assert(emela_i2c_configure(0, c) == validation_status && i2c_configured && configurations == 1);
    validation_status = 0;
    for (int address = -1; address < 130; address++) if (address < 8 || address > 0x77)
        assert(emela_i2c_master_transmit(0, address, data, 0, 0) == EMELA_PERIPHERAL_INVALID_ARGUMENT);
    assert(emela_i2c_master_transmit(0, 64, bad, 2, 100) == EMELA_PERIPHERAL_INVALID_ARGUMENT && transfers == 0);
    bad[1] = 65535;
    assert(emela_i2c_master_transmit(0, 64, bad, 2, 100) == EMELA_PERIPHERAL_INVALID_ARGUMENT && transfers == 0);
    assert(emela_i2c_master_transmit(0, 64, data, 0, -1) == EMELA_PERIPHERAL_INVALID_ARGUMENT);
    assert(emela_i2c_master_transmit(0, 64, NULL, 1, 1) == EMELA_PERIPHERAL_INVALID_ARGUMENT);
    assert(emela_i2c_master_receive(0, 64, -1, 0, out, 64, &length) == EMELA_PERIPHERAL_INVALID_ARGUMENT);
    assert(emela_i2c_master_receive(0, 64, 65, 0, out, 64, &length) == EMELA_PERIPHERAL_CAPACITY_EXCEEDED);
    assert(emela_i2c_master_transmit(0, 64, NULL, 0, 0) == 0);
    assert(emela_i2c_master_receive(0, 64, 0, 0, NULL, 0, &length) == 0 && length == 0);
    assert(emela_i2c_memory_write(0, 64, 256, 0, NULL, 0, 0) == EMELA_PERIPHERAL_INVALID_ARGUMENT);
    assert(emela_i2c_memory_read(0, 64, 65536, 1, 0, 0, NULL, 0, &length) == EMELA_PERIPHERAL_INVALID_ARGUMENT);
    assert(emela_i2c_memory_write(0, 64, 255, 0, NULL, 0, 0) == 0);
    assert(emela_i2c_memory_read(0, 64, 65535, 1, 0, 0, NULL, 0, &length) == 0 && transfers == 0);
    assert(emela_i2c_master_transmit(0, 64, data, 2, 0) == EMELA_PERIPHERAL_TIMEOUT && transfers == 0);
    assert(emela_i2c_enable_interrupt(0, 0) == EMELA_PERIPHERAL_INVALID_STATE);
    assert(emela_i2c_bind_interrupt(0) == 0 && emela_i2c_enable_interrupt(0, 2) == 0);
    assert(emela_i2c_configure(0, c) == EMELA_PERIPHERAL_INVALID_STATE);
    assert(emela_i2c_master_transmit(0, 64, data, 0, 0) == 0 && notifications == 0);
    assert(emela_i2c_master_transmit(0, 64, bad, 2, 100) == EMELA_PERIPHERAL_INVALID_ARGUMENT && notifications == 0);
    assert(emela_i2c_master_transmit(0, 64, data, 2, 0) == EMELA_PERIPHERAL_TIMEOUT && notifications == 0);
    assert(emela_i2c_master_transmit(0, 64, data, 2, 100) == 0 && transfers == 1 && notifications == 1 && last_operation == EMELA_I2C_OPERATION_MASTER_TRANSMIT);
    for (int size = 1; size <= 64; size++) {
        reset_transfer();
        assert(emela_i2c_master_receive(0, 8, size, 100, out, 64, &length) == 0 && length == size && out[size-1] == size-1);
    }
    reset_transfer(); expected_prefix_size = 2; expected_prefix[0] = 0x12; expected_prefix[1] = 0x34;
    assert(emela_i2c_memory_read(0, 0x77, 0x1234, 1, 3, 100, out, 64, &length) == 0 && length == 3 && last_operation == EMELA_I2C_OPERATION_MEMORY_READ);
    reset_transfer(); expected_prefix_size = 1; expected_prefix[0] = 255;
    assert(emela_i2c_memory_write(0, 64, 255, 0, data, 2, 100) == 0 && last_operation == EMELA_I2C_OPERATION_MEMORY_WRITE);
    for (int32_t status = -5; status >= -15; status--) {
        reset_transfer(); hardware_status = status;
        assert(emela_i2c_master_receive(0, 64, 3, 100, out, 64, &length) == status && length == 0 && last_status == status);
    }
    reset_transfer();
    unsigned before = notifications;
    results[0] = results[1] = EMELA_PERIPHERAL_NACK;
    assert(emela_i2c_is_device_ready(0, 64, 3, 100, &ready) == 0 && ready == 1 && transfers == 3 && notifications == before + 1 && last_ready == 1);
    reset_transfer(); hardware_status = EMELA_PERIPHERAL_NACK;
    assert(emela_i2c_is_device_ready(0, 64, 3, 100, &ready) == 0 && ready == 0 && transfers == 3 && last_ready == 0);
    reset_transfer(); hardware_status = EMELA_PERIPHERAL_BUS_ERROR;
    assert(emela_i2c_is_device_ready(0, 64, 3, 100, &ready) == hardware_status && transfers == 1 && last_status == hardware_status);
    reset_transfer(); cost = 6; hardware_status = EMELA_PERIPHERAL_NACK;
    assert(emela_i2c_is_device_ready(0, 64, 3, 10, &ready) == EMELA_PERIPHERAL_TIMEOUT && transfers == 2 && last_status == EMELA_PERIPHERAL_TIMEOUT);
    reset_transfer(); cost = 6; hardware_status = EMELA_PERIPHERAL_NACK;
    assert(emela_i2c_is_device_ready(0, 64, 2, 10, &ready) == EMELA_PERIPHERAL_TIMEOUT && transfers == 2);
    reset_transfer(); cost = 100;
    assert(emela_i2c_master_transmit(0, 64, data, 2, 10) == EMELA_PERIPHERAL_TIMEOUT && last_status == EMELA_PERIPHERAL_TIMEOUT);
    reset_transfer(); reusable = false; hardware_status = EMELA_PERIPHERAL_NACK;
    assert(emela_i2c_is_device_ready(0, 64, 3, 100, &ready) == EMELA_PERIPHERAL_NACK && transfers == 1);
    assert(emela_i2c_master_transmit(0, 64, data, 2, 100) == EMELA_PERIPHERAL_INVALID_STATE);
    assert(emela_i2c_disable_interrupt(0) == 0 && emela_i2c_disable_interrupt(0) == 0);
    before = notifications;
    in_isr = true; i2c_completion_irq(); in_isr = false;
    assert(notifications == before && !i2c_pending);
    assert(emela_i2c_configure(0, c) == 0);
    reset_transfer();
    before = notifications;
    assert(emela_i2c_master_transmit(0, 64, data, 2, INT64_MAX) == 0 && notifications == before);
    assert(emela_i2c_is_device_ready(0, 64, 0, 100, &ready) == EMELA_PERIPHERAL_INVALID_ARGUMENT);
    assert(emela_i2c_is_device_ready(0, 64, 3, 0, &ready) == EMELA_PERIPHERAL_TIMEOUT);
    configuration_status = EMELA_PERIPHERAL_OTHER;
    assert(emela_i2c_configure(0, c) == configuration_status && !i2c_configured);
    assert(emela_i2c_master_transmit(0, 64, data, 0, 0) == EMELA_PERIPHERAL_NOT_CONFIGURED);
    assert(!in_isr && !software_irq);
}
