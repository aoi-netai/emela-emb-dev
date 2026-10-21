#include <stddef.h>
#include <stdint.h>

#include "validation_stack.h"

extern uint32_t _stack_guard_start[];
extern uint32_t _stack_bottom[];
extern uint32_t _stack_top[];

volatile uint32_t emela_validation_stack_used;
volatile uint32_t emela_validation_stack_capacity;
volatile uint32_t emela_validation_stack_painted;
volatile uint32_t emela_validation_stack_guard_ok;
volatile uint32_t emela_validation_stack_samples;
volatile uint32_t emela_validation_stack_initial_sp;
volatile uint32_t emela_validation_stack_min_sp;

/* Caller registers only, no prologue: valid from both main and an IRQ. */
__attribute__((naked, noinline)) void emela_validation_stack_record_sp(void) {
    __asm volatile(
        "csrrci t2, mstatus, 8\n"
        "la t0, emela_validation_stack_min_sp\n"
        "lw t1, 0(t0)\n"
        "bgeu sp, t1, 1f\n"
        "sw sp, 0(t0)\n"
        "1: andi t2, t2, 8\n"
        "beqz t2, 2f\n"
        "csrsi mstatus, 8\n"
        "2: ret\n"
    );
}

void emela_validation_stack_sample(void) {
    if (emela_validation_stack_initial_sp == 0) {
        emela_validation_stack_initial_sp = (uint32_t)(uintptr_t)_stack_top;
        emela_validation_stack_min_sp = (uint32_t)(uintptr_t)_stack_top;
    }
    emela_validation_stack_record_sp();
    uintptr_t lowest = (uintptr_t)_stack_bottom;
    const uintptr_t top = (uintptr_t)_stack_top;
    uint32_t guard_ok = 1;
    for (volatile uint32_t *word = _stack_guard_start; word < _stack_bottom; ++word) {
        if (*word != UINT32_C(0xdeadc0de)) guard_ok = 0;
    }
    while (lowest < top && *(volatile uint32_t *)lowest == UINT32_C(0xa5a5a5a5)) {
        lowest += sizeof(uint32_t);
    }
    if (emela_validation_stack_min_sp < lowest) lowest = emela_validation_stack_min_sp;
    emela_validation_stack_capacity = (uint32_t)(top - (uintptr_t)_stack_bottom);
    emela_validation_stack_painted = emela_validation_stack_capacity;
    emela_validation_stack_guard_ok = guard_ok;
    emela_validation_stack_used = lowest <= top ? (uint32_t)(top - lowest) : UINT32_MAX;
    emela_validation_stack_samples++;
}
