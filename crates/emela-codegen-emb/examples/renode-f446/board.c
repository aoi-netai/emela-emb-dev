#include <stdint.h>

#include "emela_embedded.h"

void _init(void) {}
void _fini(void) {}

void emela_board_init(void) {
    volatile uint32_t *const rcc_ahb1enr = (volatile uint32_t *)UINT32_C(0x40023830);
    volatile uint32_t *const gpioa_moder = (volatile uint32_t *)UINT32_C(0x40020000);
    *rcc_ahb1enr |= UINT32_C(1);
    (void)*rcc_ahb1enr;
    *gpioa_moder = (*gpioa_moder & ~(UINT32_C(3) << 10)) | (UINT32_C(1) << 10);
}

void renode_smoke_signal(void) {
    volatile uint32_t *const gpioa_bsrr = (volatile uint32_t *)UINT32_C(0x40020018);
    *gpioa_bsrr = UINT32_C(1) << 5;
}

void emela_board_idle(void) {
    __asm volatile("wfi");
}

int main(void) {
    emela_embedded_start();
    return 0;
}
