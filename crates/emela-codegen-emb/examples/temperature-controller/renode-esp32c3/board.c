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

/* Register offsets follow the ESP-IDF v5.4.1 ESP32-C3 SoC headers. */
#define REGISTER(address) (*(volatile uint32_t *)(uintptr_t)(address))
#define UART_FIFO REGISTER(UINT32_C(0x60000000))
#define UART_CLKDIV REGISTER(UINT32_C(0x60000014))
#define UART_STATUS REGISTER(UINT32_C(0x6000001c))
#define UART_CONF0 REGISTER(UINT32_C(0x60000020))
#define GPIO_OUT REGISTER(UINT32_C(0x60004004))
#define GPIO_OUT_SET REGISTER(UINT32_C(0x60004008))
#define GPIO_OUT_CLEAR REGISTER(UINT32_C(0x6000400c))
#define GPIO_ENABLE_SET REGISTER(UINT32_C(0x60004024))
#define GPIO_PIN(pin) REGISTER(UINT32_C(0x60004074) + 4u * (pin))
#define GPIO_OUT_SELECT(pin) REGISTER(UINT32_C(0x60004554) + 4u * (pin))
#define TIMER_REG(offset) REGISTER(UINT32_C(0x60023000) + (offset))
#define INTMATRIX_REG(offset) REGISTER(UINT32_C(0x600c2000) + (offset))

#define CPU_INTERRUPT UINT32_C(1)
#define TIMER_UNIT_ENABLE (UINT32_C(1) << 30)
#define TIMER_TARGET_ENABLE (UINT32_C(1) << 24)
#define TIMER_CLOCK_ENABLE (UINT32_C(1) << 31)
#define TIMER_TICKS_PER_MILLISECOND UINT32_C(16000)
#define INTERRUPT_ENABLE UINT32_C(8)

enum { HEATER_PIN = 3, COOLER_PIN = 4, INPUT_COUNT = 24 };
static const uint32_t actuator_mask = (UINT32_C(1) << HEATER_PIN) | (UINT32_C(1) << COOLER_PIN);

/* Reproducible simulation input; no physical temperature ADC is modeled. */
static const int32_t temperatures[INPUT_COUNT] = {
    2100, 1900, 1800, 1900, 1999, 2000, 2700, 2800,
    2700, 2601, 2600, 1800, 2900, 1700, INT32_MIN, 1900,
    1800, 12501, 1700, 2000, 2800, -4001, 2900, 2600,
};

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
static volatile unsigned in_trap;
static volatile EmelaTask sleeping_task;
static volatile uint32_t remaining_milliseconds;

void _init(void) {}
void _fini(void) {}

static uint32_t interrupt_state(void) {
    uint32_t state;
    __asm volatile("csrr %0, mstatus" : "=r"(state));
    return state & INTERRUPT_ENABLE;
}

static uint32_t interrupt_disable(void) {
    uint32_t previous;
    __asm volatile("csrrci %0, mstatus, 8" : "=r"(previous) :: "memory");
    return previous & INTERRUPT_ENABLE;
}

static void interrupt_restore(uint32_t previous) {
    if (previous != 0) __asm volatile("csrsi mstatus, 8" ::: "memory");
}

static void stop_timer(void) {
    TIMER_REG(0x64u) = 0;
    TIMER_REG(0x00u) = TIMER_CLOCK_ENABLE | TIMER_UNIT_ENABLE;
    TIMER_REG(0x6cu) = 1;
    __asm volatile("fence iorw, iorw" ::: "memory");
}

static uint64_t timer_ticks(void) {
    TIMER_REG(0x04u) = UINT32_C(1) << 30;
    while ((TIMER_REG(0x04u) & (UINT32_C(1) << 29)) == 0) {}
    uint32_t hi = TIMER_REG(0x40u);
    uint32_t lo = TIMER_REG(0x44u);
    return ((uint64_t)hi << 32) | lo;
}

static void all_off(void) {
    GPIO_OUT_CLEAR = actuator_mask;
}

static uint32_t output_mask(void) {
    uint32_t mask = GPIO_OUT & actuator_mask;
    if (mask == actuator_mask) temperature_overlap++;
    return mask;
}

static void uart_bytes(const char *bytes, uint16_t length) {
    for (uint16_t i = 0; i < length; i++) {
        while (((UART_STATUS >> 16) & UINT32_C(0x3ff)) >= 127u) __asm volatile("nop");
        UART_FIFO = (uint8_t)bytes[i];
        temperature_uart_bytes++;
    }
}

_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) {
    (void)kind;
    interrupt_disable();
    stop_timer();
    all_off();
    temperature_failures++;
    if (in_trap == 0) {
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
    if (in_trap != 0 || interrupt_state() == 0 || event_locked != 0)
        BOARD_FAILURE("board operation requires main context");
}

static void board_init(void) {
    all_off();
    TIMER_REG(0x00u) = TIMER_CLOCK_ENABLE | TIMER_UNIT_ENABLE;
    TIMER_REG(0x64u) = 0;
    TIMER_REG(0x6cu) = 7;
    INTMATRIX_REG(0x094u) = CPU_INTERRUPT;
    INTMATRIX_REG(0x108u) = 0;
    INTMATRIX_REG(0x118u) = 1;
    INTMATRIX_REG(0x194u) = 1;
    INTMATRIX_REG(0x104u) = UINT32_C(1) << CPU_INTERRUPT;
    __asm volatile("csrsi mstatus, 8" ::: "memory");
}

void emela_gpio_configure(EmelaGpioPin pin, EmelaGpioMode mode, EmelaGpioPull pull) {
    require_main_context();
    if ((pin.id != HEATER_PIN && pin.id != COOLER_PIN)
        || mode != EMELA_GPIO_MODE_OUTPUT_PUSH_PULL || pull != EMELA_GPIO_PULL_NO_PULL)
        BOARD_FAILURE("unsupported GPIO configuration");
    uint32_t number = (uint32_t)pin.id;
    GPIO_PIN(number) = 0;
    /* GPIO matrix routes the software output and software output-enable signals. */
    GPIO_OUT_SELECT(number) = UINT32_C(128) | (UINT32_C(1) << 9);
    GPIO_ENABLE_SET = UINT32_C(1) << number;
}

int32_t emela_uart_configure(EmelaUartId id, EmelaUartConfig config) {
    require_main_context();
    if (id != EMELA_UART_ID_UART0 || config.baud_rate != 115200
        || config.data_bits != EMELA_UART_DATA_BITS_BITS8
        || config.stop_bits != EMELA_UART_STOP_BITS_ONE
        || config.parity != EMELA_UART_PARITY_NO_PARITY
        || config.direction != EMELA_UART_DIRECTION_TRANSMIT
        || config.flow_control != EMELA_UART_FLOW_CONTROL_NO_FLOW_CONTROL
        || config.oversampling != EMELA_UART_OVERSAMPLING_BY16) return 1;
    /* The model accepts UART divider/configuration registers; pads are not modeled. */
    UART_CLKDIV = UINT32_C(694) | (UINT32_C(7) << 20);
    UART_CONF0 = UINT32_C(0x1c);
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
    /* Timed word transfers are outside this Console-only example board. */
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
    if (state == EMELA_GPIO_PIN_STATE_HIGH) GPIO_OUT_SET = bit;
    else GPIO_OUT_CLEAR = bit;
    uint32_t mask = output_mask();
    if (unlogged_sample && pin.id == COOLER_PIN && state == EMELA_GPIO_PIN_STATE_LOW)
        temperature_switch_off_masks[sample_slot] = mask;
}

EmelaGpioPinState emela_gpio_read(EmelaGpioPin pin) {
    require_main_context();
    if (pin.id != HEATER_PIN && pin.id != COOLER_PIN) BOARD_FAILURE("unsupported GPIO read");
    return (GPIO_OUT & (UINT32_C(1) << (uint32_t)pin.id))
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
    uint32_t previous = interrupt_disable();
    uint64_t ticks = timer_ticks();
    interrupt_restore(previous);
    /* A virtual Unix epoch beginning at 1970-01-01, without a physical RTC. */
    return (int64_t)(ticks / TIMER_TICKS_PER_MILLISECOND);
}

uint32_t emela_board_event_lock(void) {
    uint32_t previous = interrupt_disable();
    if (event_locked != 0) BOARD_FAILURE("nested task event lock");
    event_locked = 1;
    return previous;
}

void emela_board_event_unlock(uint32_t previous) {
    if (event_locked != 1 || interrupt_state() != 0) BOARD_FAILURE("invalid task event unlock");
    event_locked = 0;
    interrupt_restore(previous);
}

void emela_board_clock_sleep(EmelaTask task, int32_t milliseconds) {
    require_main_context();
    if (remaining_milliseconds != 0) BOARD_FAILURE("invalid timer request");
    if (milliseconds <= 0) {
        if (!emela_task_complete_unit(task)) BOARD_FAILURE("immediate timer completion was rejected");
        return;
    }
    uint32_t previous = interrupt_disable();
    stop_timer();
    sleeping_task = task;
    remaining_milliseconds = (uint32_t)milliseconds;
    temperature_waits++;
    uint64_t target = timer_ticks() + TIMER_TICKS_PER_MILLISECOND;
    TIMER_REG(0x1cu) = (uint32_t)(target >> 32);
    TIMER_REG(0x20u) = (uint32_t)target;
    TIMER_REG(0x34u) = (UINT32_C(1) << 30) | TIMER_TICKS_PER_MILLISECOND;
    TIMER_REG(0x50u) = 1;
    TIMER_REG(0x64u) = 1;
    TIMER_REG(0x00u) = TIMER_CLOCK_ENABLE | TIMER_UNIT_ENABLE | TIMER_TARGET_ENABLE;
    __asm volatile("fence iorw, iorw" ::: "memory");
    interrupt_restore(previous);
}

void temperature_trap(uint32_t cause, uint32_t pc) {
    (void)pc;
    in_trap = 1;
    emela_validation_stack_record_sp();
    /* CLIC MCAUSE also contains the saved MPP and MPIE state. */
    if ((cause & UINT32_C(0x80000fff)) != (UINT32_C(0x80000000) | CPU_INTERRUPT)
        || (TIMER_REG(0x70u) & 1u) == 0 || remaining_milliseconds == 0)
        BOARD_FAILURE("unexpected CPU trap");
    TIMER_REG(0x6cu) = 1;
    temperature_ticks++;
    remaining_milliseconds--;
    if (remaining_milliseconds == 0) {
        stop_timer();
        EmelaTask task = sleeping_task;
        /* Notification only: all Emela work and heap access remain in main. */
        if (emela_task_complete_unit(task)) temperature_notifications++;
        else BOARD_FAILURE("timer completion was rejected");
    }
    __asm volatile("fence iorw, iorw" ::: "memory");
    in_trap = 0;
}

void temperature_main(void) {
    emela_validation_stack_sample();
    board_init();
    int32_t result = -1;
    uint32_t sampled_processed = 0;
    EmelaFiber controller = temperature_controller_run_start(TEMPERATURE_DEMO_SAMPLES, &result);
    while (!emela_fiber_done(controller)) {
        uint32_t observed_tick = temperature_ticks;
        while (emela_scheduler_run(1)) {}
        if (temperature_processed != sampled_processed) {
            sampled_processed = temperature_processed;
            emela_validation_stack_sample();
        }
        if (emela_fiber_done(controller)) break;
        /* A pending enabled interrupt wakes WFI even while global MIE is clear. */
        uint32_t previous = interrupt_disable();
        if (temperature_ticks == observed_tick) __asm volatile("fence iorw, iorw\nwfi" ::: "memory");
        interrupt_restore(previous);
    }
    stop_timer();
    all_off();
    emela_fiber_release(controller);
    temperature_result = result;
    emela_validation_stack_sample();
    temperature_completed = 1;
    interrupt_disable();
    for (;;) __asm volatile("wfi");
}
