/* Host implementation of the shared runtime hook ABI, for single-threaded execution.
 * The historical emela_board_* names are retained for generated-runtime compatibility.
 * External task adapters must deliver completions on the same thread; signal handlers
 * and concurrent completion producers need a separate platform implementation.
 */
#include "emela_board.h"
#include <stdio.h>
#include <stdlib.h>

_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) {
    fprintf(stderr, "Emela defect %d: ", (int)kind);
    if (length != 0) (void)fwrite(message, 1, length, stderr);
    fputc('\n', stderr);
    abort();
}

void emela_board_debug(const char *bytes, uint16_t length) {
    if (length != 0) (void)fwrite(bytes, 1, length, stdout);
}

void emela_board_init(void) {}
void emela_board_idle(void) {}
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t previous_state) { (void)previous_state; }
