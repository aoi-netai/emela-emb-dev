#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#include "emela_embedded_exports.h"
#include "emela_board.h"
#include "emela_gpio.h"
#include "emela_uart.h"
#include "emela_clock.h"
#include "emela_console.h"
#include "validation_stack.h"

#ifndef TEMPERATURE_DEMO_SAMPLES
#define TEMPERATURE_DEMO_SAMPLES 24
#endif

_Static_assert(TEMPERATURE_DEMO_SAMPLES >= 0 && TEMPERATURE_DEMO_SAMPLES <= INT32_MAX,
               "TEMPERATURE_DEMO_SAMPLES must fit a non-negative Int");

#define REGISTER(address) (*(volatile uint32_t *)(uintptr_t)(address))
#define GPIOA_MODER REGISTER(UINT32_C(0x40020000))
#define GPIOA_ODR REGISTER(UINT32_C(0x40020014))
#define GPIOA_BSRR REGISTER(UINT32_C(0x40020018))
#define GPIOA_AFRL REGISTER(UINT32_C(0x40020020))
#define RCC_AHB1ENR REGISTER(UINT32_C(0x40023830))
#define RCC_APB1ENR REGISTER(UINT32_C(0x40023840))
#define USART2_SR REGISTER(UINT32_C(0x40004400))
#define USART2_DR REGISTER(UINT32_C(0x40004404))
#define USART2_BRR REGISTER(UINT32_C(0x40004408))
#define USART2_CR1 REGISTER(UINT32_C(0x4000440c))
#define USART2_CR2 REGISTER(UINT32_C(0x40004410))
#define USART2_CR3 REGISTER(UINT32_C(0x40004414))
#define SYSTICK_CTRL REGISTER(UINT32_C(0xe000e010))
#define SYSTICK_LOAD REGISTER(UINT32_C(0xe000e014))
#define SYSTICK_VALUE REGISTER(UINT32_C(0xe000e018))
#define SCB_ICSR REGISTER(UINT32_C(0xe000ed04))

enum { HEATER_PIN = 5, COOLER_PIN = 6, INPUT_COUNT = 24 };
static const uint32_t actuator_mask = (UINT32_C(1) << HEATER_PIN) | (UINT32_C(1) << COOLER_PIN);

/* The sensor is a reproducible simulation input, not an emulated ADC device. */
static const int32_t temperatures[INPUT_COUNT] = {
    2100, 1900, 1800, 1900, 1999, 2000, 2700, 2800,
    2700, 2601, 2600, 1800, 2900, 1700, INT32_MIN, 1900,
    1800, 12501, 1700, 2000, 2800, -4001, 2900, 2600,
};

/* Observations are separate from the controller's decisions in controller.emel. */
volatile uint32_t temperature_completed;
volatile uint32_t temperature_samples;
volatile uint32_t temperature_processed;
volatile uint32_t temperature_failures;
volatile uint32_t temperature_failure_line;
volatile uint32_t temperature_waits;
volatile uint32_t temperature_ticks;
volatile uint32_t temperature_notifications;
volatile uint32_t temperature_overlap;
volatile uint32_t temperature_uart_bytes;
volatile int32_t temperature_result;
volatile int32_t temperature_input_centicelsius[INPUT_COUNT];
volatile uint32_t temperature_output_masks[INPUT_COUNT];
volatile uint32_t temperature_switch_off_masks[INPUT_COUNT];

static uint32_t next_input;
static uint32_t sample_slot;
static bool unlogged_sample;
static bool uart_ready;
static unsigned event_locked;
static volatile EmelaTask sleeping_task;
static volatile uint32_t remaining_milliseconds;
static volatile uint64_t elapsed_milliseconds;

void _init(void) {}
void _fini(void) {}

static uint32_t interrupt_state(void) {
    uint32_t state;
    __asm volatile("mrs %0, primask" : "=r"(state));
    return state;
}

static uint32_t exception_number(void) {
    uint32_t number;
    __asm volatile("mrs %0, ipsr" : "=r"(number));
    return number;
}

static void stop_timer(void) {
    SYSTICK_CTRL = 0;
    SCB_ICSR = UINT32_C(1) << 25;
    __asm volatile("dsb\nisb" ::: "memory");
}

static void all_off(void) {
    GPIOA_BSRR = actuator_mask << 16;
}

static uint32_t output_mask(void) {
    uint32_t mask = GPIOA_ODR & actuator_mask;
    if (mask == actuator_mask) temperature_overlap++;
    return mask;
}

static void uart_bytes(const char *bytes, uint16_t length) {
    for (uint16_t i = 0; i < length; i++) {
        while ((USART2_SR & (UINT32_C(1) << 7)) == 0) __asm volatile("nop");
        USART2_DR = (uint8_t)bytes[i];
        temperature_uart_bytes++;
    }
}

_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) {
    (void)kind;
    stop_timer();
    all_off();
    temperature_failures++;
    if (exception_number() == 0) {
        if (uart_ready) {
            static const char prefix[] = "controller stopped: ";
            uart_bytes(prefix, (uint16_t)(sizeof prefix - 1));
            uart_bytes(message, length);
            uart_bytes("\n", 1);
        }
        emela_validation_stack_sample();
    }
    for (;;) __asm volatile("wfi");
}

static _Noreturn void board_failure(uint32_t line, const char *message, uint16_t length) {
    temperature_failure_line = line;
    emela_board_defect(EMELA_DEFECT_ASSERT, message, length);
}

#define BOARD_FAILURE(message) board_failure(__LINE__, message, (uint16_t)(sizeof(message) - 1))

static void require_main_context(void) {
    if (exception_number() != 0 || interrupt_state() != 0 || event_locked != 0)
        BOARD_FAILURE("board operation requires main context");
}

static void board_init(void) {
    /* CMSIS SystemInit leaves HSI and APB1 at 16 MHz; no PLL is enabled here. */
    RCC_AHB1ENR |= UINT32_C(1);
    RCC_APB1ENR |= UINT32_C(1) << 17;
    (void)RCC_AHB1ENR;
    (void)RCC_APB1ENR;
    all_off();
    /* Peripheral configuration is requested through the standard Emela effects. */
}

void emela_gpio_configure(EmelaGpioPin pin, EmelaGpioMode mode, EmelaGpioPull pull) {
    require_main_context();
    if ((pin.id != HEATER_PIN && pin.id != COOLER_PIN)
        || mode != EMELA_GPIO_MODE_OUTPUT_PUSH_PULL || pull != EMELA_GPIO_PULL_NO_PULL)
        BOARD_FAILURE("unsupported GPIO configuration");
    uint32_t shift = (uint32_t)pin.id * 2;
    GPIOA_MODER = (GPIOA_MODER & ~(UINT32_C(3) << shift)) | (UINT32_C(1) << shift);
}

int32_t emela_uart_configure(EmelaUartId id, EmelaUartConfig config) {
    require_main_context();
    if (id != EMELA_UART_ID_UART2 || config.baud_rate != 115200
        || config.data_bits != EMELA_UART_DATA_BITS_BITS8
        || config.stop_bits != EMELA_UART_STOP_BITS_ONE
        || config.parity != EMELA_UART_PARITY_NO_PARITY
        || config.direction != EMELA_UART_DIRECTION_TRANSMIT
        || config.flow_control != EMELA_UART_FLOW_CONTROL_NO_FLOW_CONTROL
        || config.oversampling != EMELA_UART_OVERSAMPLING_BY16) return 1;
    GPIOA_MODER = (GPIOA_MODER & ~(UINT32_C(3) << 4)) | (UINT32_C(2) << 4);
    GPIOA_AFRL = (GPIOA_AFRL & ~(UINT32_C(15) << 8)) | (UINT32_C(7) << 8);
    USART2_CR1 = 0;
    USART2_CR2 = 0;
    USART2_CR3 = 0;
    USART2_BRR = 139; /* 16 MHz / 115200 baud, rounded to the nearest divider. */
    USART2_CR1 = (UINT32_C(1) << 13) | (UINT32_C(1) << 3);
    uart_ready = true;
    return 0;
}

int32_t emela_uart_transmit(EmelaUartId id, const uint16_t *data, uint16_t length,
                            int64_t timeout_ms) {
    require_main_context();
    (void)id;
    (void)data;
    (void)length;
    (void)timeout_ms;
    /* Timed peripheral transfers require a free-running clock not used by this demo. */
    return 2;
}

int32_t emela_uart_receive(EmelaUartId id, int32_t size, int64_t timeout_ms,
                           uint16_t *out, uint16_t capacity, uint16_t *length) {
    require_main_context();
    (void)id;
    (void)size;
    (void)timeout_ms;
    (void)out;
    (void)capacity;
    *length = 0;
    /* This example configures USART2 for transmission only. */
    return 2;
}

int32_t temperature_sensor_read(void) {
    require_main_context();
    sample_slot = next_input;
    int32_t value = temperatures[next_input];
    next_input = (next_input + 1) % INPUT_COUNT;
    temperature_input_centicelsius[sample_slot] = value;
    temperature_samples++;
    unlogged_sample = true;
    return value;
}

void emela_gpio_write(EmelaGpioPin pin, EmelaGpioPinState state) {
    require_main_context();
    if ((pin.id != HEATER_PIN && pin.id != COOLER_PIN)
        || (state != EMELA_GPIO_PIN_STATE_LOW && state != EMELA_GPIO_PIN_STATE_HIGH))
        BOARD_FAILURE("unsupported GPIO write");
    (void)output_mask();
    uint32_t bit = UINT32_C(1) << (uint32_t)pin.id;
    GPIOA_BSRR = state == EMELA_GPIO_PIN_STATE_HIGH ? bit : bit << 16;
    uint32_t mask = output_mask();
    if (unlogged_sample && pin.id == COOLER_PIN && state == EMELA_GPIO_PIN_STATE_LOW)
        temperature_switch_off_masks[sample_slot] = mask;
}

EmelaGpioPinState emela_gpio_read(EmelaGpioPin pin) {
    require_main_context();
    if (pin.id != HEATER_PIN && pin.id != COOLER_PIN) BOARD_FAILURE("unsupported GPIO read");
    return (GPIOA_ODR & (UINT32_C(1) << (uint32_t)pin.id))
        ? EMELA_GPIO_PIN_STATE_HIGH : EMELA_GPIO_PIN_STATE_LOW;
}

void emela_gpio_toggle(EmelaGpioPin pin) {
    emela_gpio_write(pin, emela_gpio_read(pin) == EMELA_GPIO_PIN_STATE_HIGH
        ? EMELA_GPIO_PIN_STATE_LOW : EMELA_GPIO_PIN_STATE_HIGH);
}

void emela_gpio_enable_interrupt(EmelaGpioPin pin, EmelaGpioEdge edge) {
    (void)pin;
    (void)edge;
    BOARD_FAILURE("GPIO interrupts are not implemented by this example board");
}

void emela_gpio_disable_interrupt(EmelaGpioPin pin) {
    (void)pin;
    BOARD_FAILURE("GPIO interrupts are not implemented by this example board");
}

static void print_line(const char *data, uint16_t length) {
    require_main_context();
    if (!uart_ready) BOARD_FAILURE("console UART is not configured");
    uart_bytes(data, length);
    uart_bytes("\n", 1);
    if (unlogged_sample) {
        temperature_output_masks[sample_slot] = output_mask();
        temperature_processed++;
        unlogged_sample = false;
    }
}

void emela_board_console_print_line(const char *data, uint16_t length) {
    print_line(data, length);
}

void emela_board_console_print_error_line(const char *data, uint16_t length) {
    print_line(data, length);
}

int64_t emela_board_clock_now(void) {
    require_main_context();
    uint32_t previous = interrupt_state();
    __asm volatile("cpsid i" ::: "memory");
    uint64_t milliseconds = elapsed_milliseconds;
    __asm volatile("msr primask, %0" :: "r"(previous) : "memory");
    /* The example's virtual Unix epoch is 1970-01-01T00:00:00Z, not a real RTC. */
    return (int64_t)milliseconds;
}

uint32_t emela_board_event_lock(void) {
    uint32_t previous = interrupt_state();
    __asm volatile("cpsid i" ::: "memory");
    if (event_locked != 0) BOARD_FAILURE("nested task event lock");
    event_locked = 1;
    return previous;
}

void emela_board_event_unlock(uint32_t previous) {
    if (event_locked != 1 || interrupt_state() == 0) BOARD_FAILURE("invalid task event unlock");
    event_locked = 0;
    __asm volatile("msr primask, %0" :: "r"(previous) : "memory");
}

void emela_board_clock_sleep(EmelaTask task, int32_t milliseconds) {
    require_main_context();
    if (remaining_milliseconds != 0) BOARD_FAILURE("invalid timer request");
    if (milliseconds <= 0) {
        if (!emela_task_complete_unit(task)) BOARD_FAILURE("immediate timer completion was rejected");
        return;
    }
    uint32_t previous = interrupt_state();
    __asm volatile("cpsid i" ::: "memory");
    stop_timer();
    sleeping_task = task;
    remaining_milliseconds = (uint32_t)milliseconds;
    temperature_waits++;
    /* A 1 ms tick supports any positive Int duration without reload overflow. */
    SYSTICK_LOAD = 16000 - 1;
    SYSTICK_VALUE = 0;
    __asm volatile("dmb" ::: "memory");
    SYSTICK_CTRL = 7;
    __asm volatile("msr primask, %0" :: "r"(previous) : "memory");
}

void SysTick_Handler(void) {
    emela_validation_stack_record_sp();
    temperature_ticks++;
    elapsed_milliseconds++;
    if (remaining_milliseconds == 0) {
        stop_timer();
        return;
    }
    remaining_milliseconds--;
    if (remaining_milliseconds == 0) {
        stop_timer();
        EmelaTask task = sleeping_task;
        /* ISR work ends at notification; the scheduler resumes the app in main. */
        if (emela_task_complete_unit(task)) temperature_notifications++;
        else BOARD_FAILURE("timer completion was rejected");
        __asm volatile("dmb" ::: "memory");
    }
}

int main(void) {
    board_init();
    int32_t result = -1;
    uint32_t sampled_processed = 0;
    /* Set TEMPERATURE_DEMO_SAMPLES=0 at compile time for continuous operation. */
    EmelaFiber controller = temperature_controller_run_start(TEMPERATURE_DEMO_SAMPLES, &result);
    while (!emela_fiber_done(controller)) {
        uint32_t observed_tick = temperature_ticks;
        while (emela_scheduler_run(1)) {}
        if (temperature_processed != sampled_processed) {
            sampled_processed = temperature_processed;
            emela_validation_stack_sample();
        }
        if (emela_fiber_done(controller)) break;
        /* Close the empty-queue -> sleep race. Pending IRQs wake WFI while masked. */
        uint32_t previous = interrupt_state();
        __asm volatile("cpsid i" ::: "memory");
        if (temperature_ticks == observed_tick) __asm volatile("dsb\nwfi" ::: "memory");
        __asm volatile("msr primask, %0" :: "r"(previous) : "memory");
    }
    stop_timer();
    all_off();
    emela_fiber_release(controller);
    temperature_result = result;
    emela_validation_stack_sample();
    temperature_completed = 1;
    for (;;) __asm volatile("wfi");
}
