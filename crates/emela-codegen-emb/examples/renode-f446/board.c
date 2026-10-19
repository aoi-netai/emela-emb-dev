#include <stdint.h>

#include "emela_embedded.h"

void _init(void) {}
void _fini(void) {}

void emela_board_init(void) {
    volatile uint32_t *const rcc_ahb1enr = (volatile uint32_t *)UINT32_C(0x40023830);
    volatile uint32_t *const gpioa_moder = (volatile uint32_t *)UINT32_C(0x40020000);
    *rcc_ahb1enr |= UINT32_C(1);
    (void)*rcc_ahb1enr;
    *gpioa_moder = (*gpioa_moder & ~((UINT32_C(3) << 10) | (UINT32_C(3) << 12)))
        | (UINT32_C(1) << 10)
        | (UINT32_C(1) << 12);
}

void renode_smoke_signal(int32_t value) {
    volatile uint32_t *const gpioa_bsrr = (volatile uint32_t *)UINT32_C(0x40020018);
    static uint32_t signal_count;
    if (signal_count == 0 && value == INT32_C(42)) {
        *gpioa_bsrr = UINT32_C(1) << 5;
    } else if (signal_count == 1 && value == INT32_C(34)) {
        *gpioa_bsrr = UINT32_C(1) << 6;
    } else {
        *gpioa_bsrr = UINT32_C(1) << 7;
    }
    signal_count += UINT32_C(1);
}

void emela_board_idle(void) {
    __asm volatile("wfi");
}

int main(void) {
    emela_embedded_start();
    return 0;
}
