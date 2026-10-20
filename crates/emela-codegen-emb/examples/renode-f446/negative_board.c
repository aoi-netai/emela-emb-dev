#include <stdint.h>
#include "stm32f446xx.h"
#include "negative_probe.h"
#include "validation_stack.h"

volatile uint32_t emela_negative_expected = EMELA_NEGATIVE_KIND;
volatile uint32_t emela_negative_observed = UINT32_MAX;
volatile uint32_t emela_negative_stops;
volatile uint32_t emela_negative_started;
char emela_negative_bytes[3] = { (char)0xa5, (char)0x5a, (char)0xc3 };

void _init(void) {}
void _fini(void) {}
void emela_board_init(void) {
    RCC->AHB1ENR |= RCC_AHB1ENR_GPIOAEN;
    (void)RCC->AHB1ENR;
    GPIOA->MODER = (GPIOA->MODER & ~UINT32_C(0x3fc00)) | UINT32_C(0x15400);
}
void emela_board_idle(void) { __WFI(); }
int32_t negative_seed(void) { return INT32_C(42); }
#if EMELA_NEGATIVE_SCHEDULER
uint32_t emela_board_event_lock(void) { uint32_t old=__get_PRIMASK();__disable_irq();return old; }
void emela_board_event_unlock(uint32_t old) { __set_PRIMASK(old); }
#endif
#if EMELA_NEGATIVE_TASK
EmelaTask emela_negative_tasks[2];
volatile uint32_t emela_negative_first_accepted;
void negative_task_start(EmelaTask task) { if(emela_negative_started<2)emela_negative_tasks[emela_negative_started]=task;emela_negative_started++; }
#endif
_Noreturn void emela_board_defect(EmelaDefectKind kind,const char *message,uint16_t length) {
    static const char expected[]=EMELA_NEGATIVE_MESSAGE;
    uint32_t ok=(uint32_t)kind==emela_negative_expected && length==sizeof expected-1 && __get_IPSR()==0 && __get_PRIMASK()==0;
    for(uint16_t i=0;i<length && i<sizeof expected-1;i++)ok &= (unsigned char)message[i]==(unsigned char)expected[i];
#if EMELA_NEGATIVE_STRING
    ok &= (unsigned char)emela_negative_bytes[0]==UINT8_C(0xa5) && (unsigned char)emela_negative_bytes[2]==UINT8_C(0xc3);
#endif
#if EMELA_NEGATIVE_TASK
    ok &= emela_negative_started==(EMELA_NEGATIVE_EVENT ? 2u : 1u);
#if EMELA_NEGATIVE_EVENT
    ok &= emela_negative_first_accepted==1;
#endif
#endif
    emela_negative_observed=(uint32_t)kind;emela_negative_stops++;
    emela_validation_stack_sample();
    GPIOA->BSRR=ok ? UINT32_C(0xe0) : UINT32_C(0x100);
    for(;;)__WFI();
}
int main(void) {
    emela_board_init();
    emela_negative_run();
    emela_validation_stack_sample();
    GPIOA->BSRR=UINT32_C(0x100);
    for(;;)__WFI();
}
