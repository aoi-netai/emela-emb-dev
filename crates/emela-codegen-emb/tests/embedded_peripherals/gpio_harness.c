#include <assert.h>
#include <stdlib.h>
#include "emela_gpio.h"
#include "emela_embedded.h"
#include "emela_board.h"
unsigned fixture_allocations(void);
static unsigned configurations, bindings, reads, writes;
static int32_t read_status, write_status, last_level;
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { (void)k; (void)m; (void)n; abort(); }
int32_t emela_gpio_configure(EmelaGpioPin pin, EmelaGpioMode mode, EmelaGpioPull pull, EmelaGpioPinState initial) {
  static const int32_t modes[] = {1,0,2,3}, pulls[] = {1,2,0,0}, levels[] = {1,0,0,0};
  assert(pin.id == (int32_t)configurations + 5);
  assert(mode == modes[configurations] && pull == pulls[configurations] && initial == levels[configurations]);
  configurations++;
  return 0;
}
int32_t emela_gpio_bind_interrupt(EmelaGpioPin pin) { assert(pin.id == (int32_t)bindings + 6); bindings++; return 0; }
int32_t emela_gpio_enable_interrupt(EmelaGpioPin p, EmelaGpioEdge e, EmelaInterruptPriority priority) { assert(p.id == 6 && e == 2 && priority == 1); return 0; }
int32_t emela_gpio_disable_interrupt(EmelaGpioPin p) { assert(p.id == 6); return 0; }
int32_t emela_gpio_read(EmelaGpioPin p, EmelaGpioPinState *out) { (void)p; (void)out; abort(); }
int32_t emela_gpio_write(EmelaGpioPin p, EmelaGpioPinState s) { (void)p; (void)s; abort(); }
int32_t emela_gpio_toggle(EmelaGpioPin p) { (void)p; abort(); }
int32_t emela_gpio_isr_read(EmelaGpioPin p, EmelaGpioPinState *out) { assert(p.id == 9); reads++; *out = 1; return read_status; }
int32_t emela_gpio_isr_write(EmelaGpioPin p, EmelaGpioPinState s) { assert(p.id == 5); writes++; last_level = s; return write_status; }
int main(void) {
  assert(initialize() == 7 && configurations == 4 && bindings == 2);
  assert(fixture_allocations() > 0);
  assert(emela_gpio_has_binding(6) && emela_gpio_has_binding(7) && !emela_gpio_has_binding(8));
  unsigned before = fixture_allocations();
  emela_gpio_dispatch(6, 1); assert(reads == 1 && writes == 1 && last_level == 1);
  emela_gpio_dispatch(7, 0); assert(reads == 1 && writes == 2 && last_level == 0);
  read_status = EMELA_PERIPHERAL_NOT_CONFIGURED;
  emela_gpio_dispatch(6, 1); assert(reads == 2 && writes == 3 && last_level == 0);
  write_status = EMELA_PERIPHERAL_INVALID_STATE;
  emela_gpio_dispatch(7, 0); assert(writes == 4);
  emela_gpio_dispatch(99, 1); emela_gpio_dispatch(6, 2); assert(writes == 4);
  assert(fixture_allocations() == before);
  const int32_t status[] = {0,-1,-2,-3,-4,-5,-6,-7,-8,-9,71};
  const int32_t kind[] = {0,1,7,2,3,7,4,5,6,7,7};
  for (unsigned i = 0; i < sizeof(status)/sizeof(status[0]); i++) assert(classify(status[i]) == kind[i]);
  return 0;
}
