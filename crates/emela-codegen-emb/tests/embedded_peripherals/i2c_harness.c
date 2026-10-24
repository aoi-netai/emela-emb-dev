#include <assert.h>
#include <stdlib.h>
#include "emela_board.h"
#include "emela_embedded.h"
#include "emela_i2c.h"
#include "emela_gpio.h"
static unsigned writes;
static int32_t last_pin, last_level;
int32_t emela_i2c_configure(EmelaI2cId id, EmelaI2cConfig c) { assert(id == 0 && c.clock_speed_hz == 400000 && c.duty_cycle == 1); return 0; }
int32_t emela_i2c_bind_interrupt(int32_t id) { assert((id == 0 || id == 1) && emela_i2c_has_binding(id)); return 0; }
int32_t emela_i2c_enable_interrupt(EmelaI2cId id, EmelaInterruptPriority priority) { assert(id == 0 && priority == 2); return 0; }
int32_t emela_i2c_disable_interrupt(EmelaI2cId id) { assert(id == 0); return 0; }
int32_t emela_gpio_isr_write(EmelaGpioPin pin, EmelaGpioPinState state) { writes++; last_pin = pin.id; last_level = state; return 0; }
unsigned fixture_allocations(void);
int main(void) {
    assert(initialize() == 42);
    assert(emela_i2c_has_binding(0) && emela_i2c_has_binding(1) && !emela_i2c_has_binding(2));
    unsigned before = fixture_allocations();
    emela_i2c_transfer_complete(0, EMELA_I2C_OPERATION_MASTER_TRANSMIT); assert(writes == 1 && last_pin == 1 && last_level == 1);
    emela_i2c_transfer_complete(1, EMELA_I2C_OPERATION_MASTER_RECEIVE); assert(writes == 2 && last_pin == 2 && last_level == 0);
    emela_i2c_transfer_complete(0, EMELA_I2C_OPERATION_MEMORY_READ); assert(writes == 3 && last_level == 1);
    emela_i2c_transfer_error(1, EMELA_I2C_OPERATION_IS_DEVICE_READY, EMELA_PERIPHERAL_NACK); assert(writes == 4 && last_pin == 2 && last_level == 1);
    emela_i2c_transfer_error(0, EMELA_I2C_OPERATION_MEMORY_WRITE, EMELA_PERIPHERAL_TIMEOUT); assert(writes == 5 && last_level == 0);
    emela_i2c_transfer_error(0, EMELA_I2C_OPERATION_IS_DEVICE_READY, 123); assert(writes == 6 && last_level == 0);
    emela_i2c_device_ready(0, 1); assert(writes == 7 && last_level == 1);
    emela_i2c_device_ready(0, 0); assert(writes == 8 && last_level == 0);
    emela_i2c_transfer_complete(0, 0); emela_i2c_transfer_complete(0, 5);
    emela_i2c_transfer_error(0, 6, -13); emela_i2c_transfer_error(0, 1, 0);
    emela_i2c_device_ready(2, 1); emela_i2c_device_ready(0, 2);
    assert(writes == 8 && fixture_allocations() == before);
    for (int status = -1; status >= -15; status--)
        assert(classify(status) == (status <= -10 && status >= -12 ? 9 : -status));
    assert(classify(0) == 0 && classify(99) == 9);
}
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { (void)k; (void)m; (void)n; abort(); }
int32_t emela_i2c_master_transmit(EmelaI2cId id, int32_t a, const uint16_t *d, uint16_t n, int64_t t) { (void)id; (void)a; (void)d; (void)n; (void)t; abort(); }
int32_t emela_i2c_master_receive(EmelaI2cId id, int32_t a, int32_t n, int64_t t, uint16_t *o, uint16_t c, uint16_t *l) { (void)id; (void)a; (void)n; (void)t; (void)o; (void)c; (void)l; abort(); }
int32_t emela_i2c_memory_write(EmelaI2cId id, int32_t a, int32_t m, EmelaI2cMemoryAddressSize s, const uint16_t *d, uint16_t n, int64_t t) { (void)id; (void)a; (void)m; (void)s; (void)d; (void)n; (void)t; abort(); }
int32_t emela_i2c_memory_read(EmelaI2cId id, int32_t a, int32_t m, EmelaI2cMemoryAddressSize s, int32_t n, int64_t t, uint16_t *o, uint16_t c, uint16_t *l) { (void)id; (void)a; (void)m; (void)s; (void)n; (void)t; (void)o; (void)c; (void)l; abort(); }
int32_t emela_i2c_is_device_ready(EmelaI2cId id, int32_t a, int32_t n, int64_t t, int32_t *o) { (void)id; (void)a; (void)n; (void)t; (void)o; abort(); }
