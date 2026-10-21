/* SPDX-License-Identifier: Apache-2.0 */
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#include "emela_embedded_exports.h"
#include "emela_board.h"
#include "validation_stack.h"
#if __has_include("emela_gpio.h")
#include "emela_gpio.h"
#define BOARD_HAS_GPIO 1
#endif
#if __has_include("emela_uart.h")
#include "emela_uart.h"
#define BOARD_HAS_UART 1
#endif

#define REG(address) (*(volatile uint32_t *)(uintptr_t)(address))
#define GPIOA_MODER REG(UINT32_C(0x40020000))
#define GPIOA_OTYPER REG(UINT32_C(0x40020004))
#define GPIOA_PUPDR REG(UINT32_C(0x4002000c))
#define GPIOA_IDR REG(UINT32_C(0x40020010))
#define GPIOA_ODR REG(UINT32_C(0x40020014))
#define GPIOA_BSRR REG(UINT32_C(0x40020018))
#define GPIOA_AFRL REG(UINT32_C(0x40020020))
#define RCC_AHB1ENR REG(UINT32_C(0x40023830))
#define RCC_APB1ENR REG(UINT32_C(0x40023840))
#define USART2_SR REG(UINT32_C(0x40004400))
#define USART2_DR REG(UINT32_C(0x40004404))
#define USART2_BRR REG(UINT32_C(0x40004408))
#define USART2_CR1 REG(UINT32_C(0x4000440c))
#define USART2_CR2 REG(UINT32_C(0x40004410))
#define USART2_CR3 REG(UINT32_C(0x40004414))
#define SYSTICK_CTRL REG(UINT32_C(0xe000e010))
#define SYSTICK_LOAD REG(UINT32_C(0xe000e014))
#define SYSTICK_VALUE REG(UINT32_C(0xe000e018))

volatile uint32_t board_app_completed;
volatile int32_t board_app_result;
volatile uint32_t board_app_failures;
volatile uint32_t board_ticks;
volatile uint32_t board_notifications;
volatile uint32_t board_rejected_notifications;
volatile uint32_t board_uart_bytes;
/* The reset path must copy .data and clear .bss without debugger assistance. */
volatile uint32_t board_boot_data = UINT32_C(0x454d4244);
volatile uint32_t board_boot_bss;
volatile uint32_t board_boot_initialized;

static unsigned event_locked;
static uint32_t output_pins;
static bool uart_ready;
static volatile uint64_t elapsed_ms;
static volatile uint32_t remaining_ms;
static volatile EmelaTask sleeping_task;

void _init(void) {}
void _fini(void) {}

static uint32_t interrupt_state(void) {
    uint32_t value;
    __asm volatile("mrs %0, primask" : "=r"(value));
    return value;
}

static uint32_t exception_number(void) {
    uint32_t value;
    __asm volatile("mrs %0, ipsr" : "=r"(value));
    return value;
}

static void uart_bytes(const char *bytes, uint16_t length) {
    for (uint16_t i = 0; i < length; ++i) {
        while ((USART2_SR & (UINT32_C(1) << 7)) == 0) __asm volatile("nop");
        USART2_DR = (uint8_t)bytes[i];
        board_uart_bytes++;
    }
}

_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) {
    (void)kind;
    SYSTICK_CTRL = 0;
    GPIOA_BSRR = output_pins << 16;
    board_app_failures++;
    if (exception_number() == 0) {
        if (uart_ready) {
            uart_bytes(message, length);
            uart_bytes("\n", 1);
        }
        emela_validation_stack_sample();
    }
    __asm volatile("cpsid i" ::: "memory");
    for (;;) __asm volatile("wfi");
}

#define FAIL(message) emela_board_defect(EMELA_DEFECT_ASSERT, message, (uint16_t)(sizeof(message) - 1))

static void main_context(void) {
    if (exception_number() != 0 || interrupt_state() != 0 || event_locked != 0)
        FAIL("board operation requires main context");
}

static void init_uart(void) {
    RCC_AHB1ENR |= UINT32_C(1);
    RCC_APB1ENR |= UINT32_C(1) << 17;
    (void)RCC_APB1ENR;
    /* PA2/PA3, AF7, USART2 at HSI/APB1 16 MHz; no PLL or HSE dependency. */
    GPIOA_MODER = (GPIOA_MODER & ~UINT32_C(0xf0)) | UINT32_C(0xa0);
    GPIOA_AFRL = (GPIOA_AFRL & ~UINT32_C(0xff00)) | UINT32_C(0x7700);
    USART2_CR1 = 0;
    USART2_CR2 = 0;
    USART2_CR3 = 0;
    USART2_BRR = 139;
    USART2_CR1 = (UINT32_C(1) << 13) | (UINT32_C(1) << 3) | (UINT32_C(1) << 2);
    uart_ready = true;
}

#if BOARD_HAS_GPIO
static uint32_t pin_bit(EmelaGpioPin pin) {
    if (pin.id < 0 || pin.id > 15 || pin.id == 2 || pin.id == 3)
        FAIL("GPIO must be PA0..PA15 excluding USART2 PA2/PA3");
    return UINT32_C(1) << (uint32_t)pin.id;
}

void emela_gpio_configure(EmelaGpioPin pin, EmelaGpioMode mode, EmelaGpioPull pull) {
    main_context();
    uint32_t bit = pin_bit(pin);
    if (mode < EMELA_GPIO_MODE_INPUT || mode > EMELA_GPIO_MODE_ANALOG
        || pull < EMELA_GPIO_PULL_NO_PULL || pull > EMELA_GPIO_PULL_DOWN)
        FAIL("unsupported GPIO configuration");
    uint32_t shift = (uint32_t)pin.id * 2;
    uint32_t hardware_mode = mode == EMELA_GPIO_MODE_ANALOG ? 3
        : mode == EMELA_GPIO_MODE_INPUT ? 0 : 1;
    GPIOA_BSRR = bit << 16;
    GPIOA_OTYPER = mode == EMELA_GPIO_MODE_OUTPUT_OPEN_DRAIN
        ? GPIOA_OTYPER | bit : GPIOA_OTYPER & ~bit;
    GPIOA_PUPDR = (GPIOA_PUPDR & ~(UINT32_C(3) << shift)) | ((uint32_t)pull << shift);
    GPIOA_MODER = (GPIOA_MODER & ~(UINT32_C(3) << shift)) | (hardware_mode << shift);
    output_pins = hardware_mode == 1 ? output_pins | bit : output_pins & ~bit;
}

void emela_gpio_write(EmelaGpioPin pin, EmelaGpioPinState state) {
    main_context();
    uint32_t bit = pin_bit(pin);
    if (!(output_pins & bit) || (state != EMELA_GPIO_PIN_STATE_LOW && state != EMELA_GPIO_PIN_STATE_HIGH))
        FAIL("GPIO write requires a configured output");
    GPIOA_BSRR = state == EMELA_GPIO_PIN_STATE_HIGH ? bit : bit << 16;
}

EmelaGpioPinState emela_gpio_read(EmelaGpioPin pin) {
    main_context();
    return (GPIOA_IDR & pin_bit(pin)) ? EMELA_GPIO_PIN_STATE_HIGH : EMELA_GPIO_PIN_STATE_LOW;
}

void emela_gpio_toggle(EmelaGpioPin pin) {
    main_context();
    uint32_t bit = pin_bit(pin);
    emela_gpio_write(pin, (GPIOA_ODR & bit) ? EMELA_GPIO_PIN_STATE_LOW : EMELA_GPIO_PIN_STATE_HIGH);
}

void emela_gpio_enable_interrupt(EmelaGpioPin pin, EmelaGpioEdge edge) {
    (void)pin;
    (void)edge;
    FAIL("GPIO interrupts require a project-specific driver");
}

void emela_gpio_disable_interrupt(EmelaGpioPin pin) {
    (void)pin;
    FAIL("GPIO interrupts require a project-specific driver");
}
#endif

#if BOARD_HAS_UART
int32_t emela_uart_configure(EmelaUartId id, EmelaUartConfig config) {
    main_context();
    if (id != EMELA_UART_ID_UART2 || config.baud_rate != 115200
        || config.data_bits != EMELA_UART_DATA_BITS_BITS8
        || config.stop_bits != EMELA_UART_STOP_BITS_ONE
        || config.parity != EMELA_UART_PARITY_NO_PARITY
        || config.direction != EMELA_UART_DIRECTION_TRANSMIT
        || config.flow_control != EMELA_UART_FLOW_CONTROL_NO_FLOW_CONTROL
        || config.oversampling != EMELA_UART_OVERSAMPLING_BY16) return 1;
    init_uart();
    return 0;
}

int32_t emela_uart_transmit(EmelaUartId id, const uint16_t *data, uint16_t length, int64_t timeout_ms) {
    main_context();
    (void)id; (void)data; (void)length; (void)timeout_ms;
    return 2;
}

int32_t emela_uart_receive(EmelaUartId id, int32_t size, int64_t timeout_ms,
                           uint16_t *out, uint16_t capacity, uint16_t *length) {
    main_context();
    (void)id; (void)size; (void)timeout_ms; (void)out; (void)capacity;
    *length = 0;
    return 2;
}
#endif

void emela_board_console_print_line(const char *data, uint16_t length) {
    main_context();
    uart_bytes(data, length);
    uart_bytes("\n", 1);
}

void emela_board_console_print_error_line(const char *data, uint16_t length) {
    emela_board_console_print_line(data, length);
}

uint32_t emela_board_event_lock(void) {
    uint32_t previous = interrupt_state();
    __asm volatile("cpsid i" ::: "memory");
    if (event_locked != 0) FAIL("nested task event lock");
    event_locked = 1;
    return previous;
}

void emela_board_event_unlock(uint32_t previous) {
    if (event_locked != 1 || interrupt_state() == 0) FAIL("invalid task event unlock");
    event_locked = 0;
    __asm volatile("msr primask, %0" :: "r"(previous) : "memory");
}

int64_t emela_board_clock_now(void) {
    main_context();
    uint32_t previous = interrupt_state();
    __asm volatile("cpsid i" ::: "memory");
    uint64_t value = elapsed_ms;
    __asm volatile("msr primask, %0" :: "r"(previous) : "memory");
    /* The epoch is boot-time zero; this package does not supply a wall-clock RTC. */
    return (int64_t)value;
}

void emela_board_clock_sleep(EmelaTask task, int32_t milliseconds) {
    main_context();
    if (milliseconds <= 0) {
        if (!emela_task_complete_unit(task)) FAIL("immediate timer completion rejected");
        return;
    }
    if (remaining_ms != 0) FAIL("only one concurrent Clock.sleep is supported");
    uint32_t previous = interrupt_state();
    __asm volatile("cpsid i" ::: "memory");
    sleeping_task = task;
    remaining_ms = (uint32_t)milliseconds;
    __asm volatile("msr primask, %0" :: "r"(previous) : "memory");
}

void SysTick_Handler(void) {
    emela_validation_stack_record_sp();
    board_ticks++;
    elapsed_ms++;
    if (remaining_ms != 0 && --remaining_ms == 0) {
        /* Cancellation can invalidate a task before the physical timer expires. */
        if (emela_task_complete_unit(sleeping_task)) board_notifications++;
        else board_rejected_notifications++;
    }
}

int main(void) {
    if (board_boot_data != UINT32_C(0x454d4244) || board_boot_bss != 0)
        FAIL("reset data/bss initialization failed");
    board_boot_bss = UINT32_C(0x4253534f);
    board_boot_initialized = 1;
    init_uart();
    SYSTICK_LOAD = 16000 - 1;
    SYSTICK_VALUE = 0;
    SYSTICK_CTRL = 7;
    int32_t result = 0;
    EmelaFiber app = board_main_start(&result);
    while (!emela_fiber_done(app)) {
        uint32_t observed_tick = board_ticks;
        bool did_work = false;
        while (emela_scheduler_run(1)) did_work = true;
        if (did_work) emela_validation_stack_sample();
        if (emela_fiber_done(app)) break;
        /* Masked WFI wakes on pending interrupts and closes the empty-queue race. */
        uint32_t previous = interrupt_state();
        __asm volatile("cpsid i" ::: "memory");
        if (board_ticks == observed_tick) __asm volatile("dsb\nwfi" ::: "memory");
        __asm volatile("msr primask, %0" :: "r"(previous) : "memory");
    }
    SYSTICK_CTRL = 0;
    emela_fiber_release(app);
    board_app_result = result;
    emela_validation_stack_sample();
    board_app_completed = 1;
    for (;;) __asm volatile("wfi");
}
