#include <stddef.h>
#include <stdint.h>
#include <string.h>

#include "completion_probe.h"
#include "emela_board.h"

void _init(void) {}
void _fini(void) {}

volatile uint32_t emela_completion_producers;
volatile uint32_t emela_completion_debugs;
volatile uint32_t emela_completion_baseline;
volatile uint32_t emela_completion_iterations;
static char producer_bytes[] = {'4', '\0', '2'};

static void signal(unsigned pin) {
    *(volatile uint32_t *)UINT32_C(0x40020018) = UINT32_C(1) << pin;
}

static _Noreturn void stop(void) {
    for (;;) __asm volatile("wfi");
}

static void board_init(void) {
    volatile uint32_t *const rcc = (volatile uint32_t *)UINT32_C(0x40023830);
    volatile uint32_t *const mode = (volatile uint32_t *)UINT32_C(0x40020000);
    *rcc |= UINT32_C(1);
    (void)*rcc;
    *mode = (*mode & ~((UINT32_C(3) << 10) | (UINT32_C(3) << 12)
        | (UINT32_C(3) << 14) | (UINT32_C(3) << 16)))
        | (UINT32_C(1) << 10) | (UINT32_C(1) << 12)
        | (UINT32_C(1) << 14) | (UINT32_C(1) << 16);
}

EmelaString board_completion_text(void) {
    emela_completion_producers++;
    return (EmelaString){.data=producer_bytes, .length=3};
}

void emela_board_debug(const char *bytes, uint16_t length) {
    (void)bytes; (void)length;
    emela_completion_debugs++;
}

_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) {
    (void)kind; (void)message; (void)length;
    signal(8);
    stop();
}

int main(void) {
    board_init();
    if (emela_completion_check() != 42 || emela_completion_producers != 1 || emela_completion_debugs != 1) {
        signal(8); stop();
    }
    signal(5);
    memset(producer_bytes, 'x', sizeof producer_bytes);
    size_t baseline = emela_completion_heap();
    emela_completion_baseline = (uint32_t)baseline;
    if (!baseline) { signal(8); stop(); }
    for (unsigned i=0; i<1000; i++) {
        EmelaString text = emela_completion_text();
        int correct = text.length == 3 && text.owner != NULL && memcmp(text.data, "4\0" "2", 3) == 0;
        emela_embedded_release_string(&text);
        EmelaString derived = emela_completion_derived();
        static const char expected[] = "Next(End([42, 7]))";
        correct = correct && derived.length == sizeof expected - 1
            && memcmp(derived.data, expected, sizeof expected - 1) == 0;
        emela_embedded_release_string(&derived);
        if (!correct || emela_completion_check() != 42 || emela_completion_heap() != baseline) {
            signal(8); stop();
        }
        emela_completion_iterations = i + 1;
    }
    signal(6);
    if (emela_completion_producers != 1 || emela_completion_debugs != 1) { signal(8); stop(); }
    signal(7);
    stop();
}
