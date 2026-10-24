#include <assert.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include "emela_i2c.h"
#include "emela_embedded.h"
#include "emela_board.h"
static bool in_isr, irq_enabled, pending;
static unsigned transfers, callbacks;
static uint64_t now;
#define I2C_REQUIRE_MAIN() assert(!in_isr)
static void i2c_completion_irq(void);
static uint64_t i2c_hw_now(void) { return now++; }
static int32_t i2c_hw_validate(EmelaI2cConfig c) { assert(c.clock_speed_hz == 100000); return 0; }
static int32_t i2c_hw_configure(EmelaI2cConfig c) { (void)c; return 0; }
static void i2c_hw_clear_completion(void) { pending = false; }
static void i2c_hw_notification(bool enabled, EmelaInterruptPriority p) { (void)p; irq_enabled = enabled; }
static void i2c_hw_pend_completion(void) { assert(irq_enabled); pending = true; }
static void i2c_hw_wait_notification(void) { assert(pending); pending = false; in_isr = true; i2c_completion_irq(); callbacks++; in_isr = false; }
static int32_t i2c_hw_exchange(uint8_t address, bool read, const uint8_t *prefix, unsigned n,
        const uint16_t *data, uint16_t size, uint16_t *out, uint64_t start, int64_t timeout, bool *usable) {
    assert(!in_isr && now >= start && timeout == 100); transfers++; *usable = true;
    if (address == 65) {
        assert(!read && n == 0 && (size == 0 || (size == 1 && data[0] == 255)));
        return EMELA_PERIPHERAL_NACK;
    }
    if (address == 66) { assert(read && size == 2); out[0] = 42; return EMELA_PERIPHERAL_TIMEOUT; }
    assert(address == 64 && read && n == 1 && prefix[0] == 0 && size == 2);
    out[0] = 0; out[1] = 255; return 0;
}
/* @DRIVER@ */
uint32_t emela_board_event_lock(void) { return in_isr ? 1u : 0u; }
void emela_board_event_unlock(uint32_t previous) { assert(previous == (in_isr ? 1u : 0u)); }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { (void)k; (void)m; (void)n; abort(); }
int main(void) {
    assert(recover() == 42);
    assert(transfers == 6 && callbacks == 4 && !pending && !irq_enabled && !in_isr);
}
