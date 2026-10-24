#include <assert.h>
#include <stdlib.h>
#include "emela_board.h"
#include "emela_embedded.h"
#include "emela_uart.h"
#include "emela_gpio.h"
static unsigned writes;
static int32_t last_pin, last_level;
int32_t emela_uart_configure(EmelaUartId id, EmelaUartConfig c) { assert(id == 1 && c.baud_rate == 115200); return 0; }
int32_t emela_uart_set_receive_mode(EmelaUartId id, EmelaUartReceiveMode mode) { assert(id == 1 && mode == 1); return 0; }
int32_t emela_uart_bind_interrupt(EmelaUartId id) { assert((id == 1 || id == 2) && emela_uart_has_binding(id)); return 0; }
int32_t emela_uart_enable_interrupt(EmelaUartId id, EmelaInterruptPriority priority, int32_t tx) { assert(id == 1 && priority == 2 && tx); return 0; }
int32_t emela_uart_disable_interrupt(EmelaUartId id) { assert(id == 1); return 0; }
int32_t emela_gpio_isr_write(EmelaGpioPin pin, EmelaGpioPinState state) { writes++; last_pin = pin.id; last_level = state; return 0; }
int32_t emela_uart_receive_some(EmelaUartId id, int32_t size, int64_t timeout, uint16_t *out, uint16_t cap, uint16_t *length) {
  assert(id == 1 && size == 4 && timeout == 0 && cap == 4); out[0] = 0; out[1] = 255; *length = 2; return 0;
}
int32_t emela_uart_discard_input(EmelaUartId id, int32_t size, int32_t *out) { assert(id == 1 && size == 7); *out = 3; return 0; }
unsigned fixture_allocations(void);
int main(void) {
  assert(initialize() == 5);
  assert(emela_uart_has_binding(1) && emela_uart_has_binding(2) && !emela_uart_has_binding(0));
  unsigned before = fixture_allocations();
  emela_uart_received(1, 511); assert(writes == 1 && last_pin == 1 && last_level == 1);
  emela_uart_received(2, 0); assert(writes == 2 && last_pin == 2 && last_level == 0);
  emela_uart_transmit_complete(1); assert(writes == 3 && last_level == 1);
  emela_uart_receive_error(2, 1, 1, 1, 1); assert(writes == 4 && last_pin == 2 && last_level == 1);
  emela_uart_receive_error(1, 1, 0, 0, 0); assert(writes == 5 && last_level == 0);
  emela_uart_receive_error(1, 0, 0, 0, 0); emela_uart_received(0, 1); emela_uart_received(1, 512); assert(writes == 5);
  assert(fixture_allocations() == before);
  for (int status = -1; status >= -12; status--) assert(classify(status) == -status);
  assert(classify(0) == 0 && classify(99) == 9);
  assert(utilities() == 5);
}

_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t size) { (void)kind; (void)message; (void)size; abort(); }
int32_t emela_uart_transmit(EmelaUartId id, const uint16_t *data, uint16_t size, int64_t timeout) { (void)id; (void)data; (void)size; (void)timeout; abort(); }
int32_t emela_uart_receive(EmelaUartId id, int32_t size, int64_t timeout, uint16_t *out, uint16_t cap, uint16_t *length) { (void)id; (void)size; (void)timeout; (void)out; (void)cap; (void)length; abort(); }
