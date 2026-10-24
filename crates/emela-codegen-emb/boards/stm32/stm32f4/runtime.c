/* SPDX-License-Identifier: Apache-2.0 */
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#include "board_config.h"
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
#if __has_include("emela_i2c.h")
#include "emela_i2c.h"
#define BOARD_HAS_I2C 1
#endif

#define REG(address) (*(volatile uint32_t *)(uintptr_t)(address))
#define GPIO_REG(port, offset) REG(UINT32_C(0x40020000) + (port) * UINT32_C(0x400) + (offset))
#define GPIO_MODER(port) GPIO_REG(port, UINT32_C(0x00))
#define GPIO_OTYPER(port) GPIO_REG(port, UINT32_C(0x04))
#define GPIO_PUPDR(port) GPIO_REG(port, UINT32_C(0x0c))
#define GPIO_IDR(port) GPIO_REG(port, UINT32_C(0x10))
#define GPIO_ODR(port) GPIO_REG(port, UINT32_C(0x14))
#define GPIO_BSRR(port) GPIO_REG(port, UINT32_C(0x18))
#define GPIO_AFR(port, pin) GPIO_REG(port, UINT32_C(0x20) + ((pin) / 8) * 4)
#define RCC_AHB1ENR REG(UINT32_C(0x40023830))
#define RCC_APB1ENR REG(UINT32_C(0x40023840))
#define RCC_APB2ENR REG(UINT32_C(0x40023844))
#define UART_REG(offset) REG(EMELA_CONSOLE_UART_BASE + (offset))
#define UART_SR UART_REG(UINT32_C(0x00))
#define UART_DR UART_REG(UINT32_C(0x04))
#define UART_BRR UART_REG(UINT32_C(0x08))
#define UART_CR1 UART_REG(UINT32_C(0x0c))
#define UART_CR2 UART_REG(UINT32_C(0x10))
#define UART_CR3 UART_REG(UINT32_C(0x14))
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
static volatile uint32_t output_pins[EMELA_GPIO_PORT_COUNT];
static bool uart_ready;
#if BOARD_HAS_GPIO || BOARD_HAS_I2C
static bool i2c_pins_owned;
#endif
#if BOARD_HAS_GPIO || BOARD_HAS_UART
static bool uart_pins_owned;
#endif
static volatile uint64_t elapsed_ms;
#ifdef EMELA_HAS_TASK_RUNTIME
static volatile uint32_t remaining_ms;
static volatile EmelaTask sleeping_task;
#endif

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
        while ((UART_SR & (UINT32_C(1) << 7)) == 0) __asm volatile("nop");
        UART_DR = (uint8_t)bytes[i];
        board_uart_bytes++;
    }
}

_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) {
    (void)kind;
    SYSTICK_CTRL = 0;
    for (uint32_t port = 0; port < EMELA_GPIO_PORT_COUNT; ++port) {
        if (output_pins[port] != 0) GPIO_BSRR(port) = output_pins[port] << 16;
    }
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

static void init_uart_pin(uint32_t id) {
    uint32_t port = id / 16;
    uint32_t pin = id % 16;
    uint32_t shift = pin * 2;
    uint32_t af_shift = (pin % 8) * 4;
    RCC_AHB1ENR |= UINT32_C(1) << port;
    (void)RCC_AHB1ENR;
    GPIO_OTYPER(port) &= ~(UINT32_C(1) << pin);
    GPIO_PUPDR(port) &= ~(UINT32_C(3) << shift);
    GPIO_AFR(port, pin) = (GPIO_AFR(port, pin) & ~(UINT32_C(15) << af_shift))
        | ((uint32_t)EMELA_CONSOLE_UART_AF << af_shift);
    GPIO_MODER(port) = (GPIO_MODER(port) & ~(UINT32_C(3) << shift)) | (UINT32_C(2) << shift);
}

static void init_uart(void) {
    init_uart_pin(EMELA_CONSOLE_TX_PIN);
    init_uart_pin(EMELA_CONSOLE_RX_PIN);
#if EMELA_CONSOLE_UART_APB == 1
    RCC_APB1ENR |= UINT32_C(1) << EMELA_CONSOLE_UART_RCC_BIT;
    (void)RCC_APB1ENR;
#elif EMELA_CONSOLE_UART_APB == 2
    RCC_APB2ENR |= UINT32_C(1) << EMELA_CONSOLE_UART_RCC_BIT;
    (void)RCC_APB2ENR;
#else
#error "Console UART must be attached to APB1 or APB2"
#endif
    /* All selected profiles retain reset HSI/APB clocks at 16 MHz. */
    UART_CR1 = 0;
    UART_CR2 = 0;
    UART_CR3 = 0;
    UART_BRR = 139;
    UART_CR1 = (UINT32_C(1) << 13) | (UINT32_C(1) << 3) | (UINT32_C(1) << 2);
    uart_ready = true;
}

#if BOARD_HAS_GPIO
volatile uint32_t board_gpio_events;
static const uint16_t gpio_pin_masks[EMELA_GPIO_PORT_COUNT] = EMELA_GPIO_PIN_MASKS;
static volatile uint32_t configured_pins[EMELA_GPIO_PORT_COUNT];
static volatile uint32_t digital_pins[EMELA_GPIO_PORT_COUNT];
static uint32_t bound_pins[EMELA_GPIO_PORT_COUNT];
static volatile uint32_t interrupt_pins[EMELA_GPIO_PORT_COUNT];
static volatile int32_t exti_owner[16] = {-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1,-1};
static int32_t exti_priority[16];
#define EXTI(offset) REG(UINT32_C(0x40013c00) + (offset))
#define SYSCFG_EXTICR(n) REG(UINT32_C(0x40013808) + 4u * (n))
#define NVIC_ISER(n) REG(UINT32_C(0xe000e100) + 4u * (n))
#define NVIC_ICER(n) REG(UINT32_C(0xe000e180) + 4u * (n))
#define NVIC_ICPR(n) REG(UINT32_C(0xe000e280) + 4u * (n))

static int32_t gpio_pin_status(EmelaGpioPin pin) {
    if (pin.id < 0 || (uint32_t)pin.id >= EMELA_GPIO_PORT_COUNT * 16u)
        return EMELA_PERIPHERAL_INVALID_ARGUMENT;
    uint32_t port = (uint32_t)pin.id / 16u, bit = UINT32_C(1) << ((uint32_t)pin.id % 16u);
    if (!(gpio_pin_masks[port] & bit)) return EMELA_PERIPHERAL_INVALID_ARGUMENT;
    if (pin.id == EMELA_CONSOLE_TX_PIN || pin.id == EMELA_CONSOLE_RX_PIN
        || (uart_pins_owned && (pin.id == 9 || pin.id == 10))
        || (i2c_pins_owned && (pin.id == 22 || pin.id == 23)))
        return EMELA_PERIPHERAL_RESOURCE_CONFLICT;
    return 0;
}

static uint32_t gpio_lock(void) {
    uint32_t previous = interrupt_state();
    __asm volatile("cpsid i" ::: "memory");
    return previous;
}
static void gpio_unlock(uint32_t previous) {
    if (!previous) __asm volatile("cpsie i" ::: "memory");
}

int32_t emela_gpio_configure(EmelaGpioPin pin, EmelaGpioMode mode, EmelaGpioPull pull, EmelaGpioPinState initial) {
    main_context();
    int32_t status = gpio_pin_status(pin);
    if (status) return status;
    if (mode < EMELA_GPIO_MODE_INPUT || mode > EMELA_GPIO_MODE_ANALOG
        || pull < EMELA_GPIO_PULL_NO_PULL || pull > EMELA_GPIO_PULL_DOWN
        || (initial != EMELA_GPIO_PIN_STATE_LOW && initial != EMELA_GPIO_PIN_STATE_HIGH)
        || ((mode == EMELA_GPIO_MODE_INPUT || mode == EMELA_GPIO_MODE_ANALOG) && initial != EMELA_GPIO_PIN_STATE_LOW))
        return EMELA_PERIPHERAL_INVALID_ARGUMENT;
    uint32_t port = (uint32_t)pin.id / 16u, number = (uint32_t)pin.id % 16u;
    uint32_t bit = UINT32_C(1) << number, shift = number * 2u;
    if (interrupt_pins[port] & bit) return EMELA_PERIPHERAL_INVALID_STATE;
    uint32_t hardware_mode = mode == EMELA_GPIO_MODE_ANALOG ? 3u : mode == EMELA_GPIO_MODE_INPUT ? 0u : 1u;
    uint32_t previous = gpio_lock();
    RCC_AHB1ENR |= UINT32_C(1) << port;
    (void)RCC_AHB1ENR;
    /* Release any previous output before changing its latch or output type. */
    GPIO_MODER(port) &= ~(UINT32_C(3) << shift);
    /* The latch is set before enabling output drive, including initial High. */
    if (hardware_mode == 1u) GPIO_BSRR(port) = initial == EMELA_GPIO_PIN_STATE_HIGH ? bit : bit << 16;
    GPIO_OTYPER(port) = mode == EMELA_GPIO_MODE_OUTPUT_OPEN_DRAIN ? GPIO_OTYPER(port) | bit : GPIO_OTYPER(port) & ~bit;
    GPIO_PUPDR(port) = (GPIO_PUPDR(port) & ~(UINT32_C(3) << shift)) | ((uint32_t)pull << shift);
    GPIO_MODER(port) = (GPIO_MODER(port) & ~(UINT32_C(3) << shift)) | (hardware_mode << shift);
    output_pins[port] = hardware_mode == 1u ? output_pins[port] | bit : output_pins[port] & ~bit;
    digital_pins[port] = hardware_mode == 3u ? digital_pins[port] & ~bit : digital_pins[port] | bit;
    configured_pins[port] |= bit;
    gpio_unlock(previous);
    return 0;
}

static int32_t gpio_read(EmelaGpioPin pin, EmelaGpioPinState *out) {
    int32_t status = gpio_pin_status(pin);
    if (status) return status;
    if (!out) return EMELA_PERIPHERAL_INVALID_ARGUMENT;
    uint32_t port = (uint32_t)pin.id / 16u, bit = UINT32_C(1) << ((uint32_t)pin.id % 16u);
    if (!(configured_pins[port] & bit)) return EMELA_PERIPHERAL_NOT_CONFIGURED;
    if (!(digital_pins[port] & bit)) return EMELA_PERIPHERAL_INVALID_STATE;
    *out = (GPIO_IDR(port) & bit) ? EMELA_GPIO_PIN_STATE_HIGH : EMELA_GPIO_PIN_STATE_LOW;
    return 0;
}
static int32_t gpio_write(EmelaGpioPin pin, EmelaGpioPinState state) {
    int32_t status = gpio_pin_status(pin);
    if (status) return status;
    if (state != EMELA_GPIO_PIN_STATE_LOW && state != EMELA_GPIO_PIN_STATE_HIGH) return EMELA_PERIPHERAL_INVALID_ARGUMENT;
    uint32_t port = (uint32_t)pin.id / 16u, bit = UINT32_C(1) << ((uint32_t)pin.id % 16u);
    if (!(configured_pins[port] & bit)) return EMELA_PERIPHERAL_NOT_CONFIGURED;
    if (!(output_pins[port] & bit)) return EMELA_PERIPHERAL_INVALID_STATE;
    GPIO_BSRR(port) = state == EMELA_GPIO_PIN_STATE_HIGH ? bit : bit << 16;
    return 0;
}
int32_t emela_gpio_read(EmelaGpioPin pin, EmelaGpioPinState *out) { main_context(); return gpio_read(pin, out); }
int32_t emela_gpio_write(EmelaGpioPin pin, EmelaGpioPinState state) { main_context(); return gpio_write(pin, state); }
int32_t emela_gpio_isr_read(EmelaGpioPin pin, EmelaGpioPinState *out) {
    if (!exception_number()) return EMELA_PERIPHERAL_INVALID_STATE;
    return gpio_read(pin, out);
}
int32_t emela_gpio_isr_write(EmelaGpioPin pin, EmelaGpioPinState state) {
    if (!exception_number()) return EMELA_PERIPHERAL_INVALID_STATE;
    return gpio_write(pin, state);
}
int32_t emela_gpio_toggle(EmelaGpioPin pin) {
    main_context();
    int32_t status = gpio_pin_status(pin);
    if (status) return status;
    uint32_t port = (uint32_t)pin.id / 16u, bit = UINT32_C(1) << ((uint32_t)pin.id % 16u);
    return gpio_write(pin, GPIO_ODR(port) & bit ? EMELA_GPIO_PIN_STATE_LOW : EMELA_GPIO_PIN_STATE_HIGH);
}
int32_t emela_gpio_bind_interrupt(EmelaGpioPin pin) {
    main_context();
    int32_t status = gpio_pin_status(pin);
    if (status) return status;
    uint32_t port = (uint32_t)pin.id / 16u, bit = UINT32_C(1) << ((uint32_t)pin.id % 16u);
    if (!(configured_pins[port] & bit)) return EMELA_PERIPHERAL_NOT_CONFIGURED;
    if (!(digital_pins[port] & bit) || (interrupt_pins[port] & bit)) return EMELA_PERIPHERAL_INVALID_STATE;
    if (!emela_gpio_has_binding(pin.id)) return EMELA_PERIPHERAL_INVALID_ARGUMENT;
    bound_pins[port] |= bit;
    return 0;
}
static uint32_t exti_irq(uint32_t line) { return line < 5u ? 6u + line : line < 10u ? 23u : 40u; }
static uint32_t exti_group(uint32_t line) { return line < 5u ? UINT32_C(1) << line : line < 10u ? UINT32_C(0x3e0) : UINT32_C(0xfc00); }
int32_t emela_gpio_enable_interrupt(EmelaGpioPin pin, EmelaGpioEdge edge, EmelaInterruptPriority priority) {
    main_context();
    int32_t status = gpio_pin_status(pin);
    if (status) return status;
    if (edge < EMELA_GPIO_EDGE_RISING || edge > EMELA_GPIO_EDGE_BOTH || priority < 0 || priority > 2)
        return EMELA_PERIPHERAL_INVALID_ARGUMENT;
    uint32_t port = (uint32_t)pin.id / 16u, line = (uint32_t)pin.id % 16u, bit = UINT32_C(1) << line;
    if (!(configured_pins[port] & bit)) return EMELA_PERIPHERAL_NOT_CONFIGURED;
    if (!(digital_pins[port] & bit) || !(bound_pins[port] & bit) || (interrupt_pins[port] & bit)) return EMELA_PERIPHERAL_INVALID_STATE;
    if (exti_owner[line] >= 0 && exti_owner[line] != pin.id) return EMELA_PERIPHERAL_RESOURCE_CONFLICT;
    uint32_t group = exti_group(line), irq = exti_irq(line);
    for (uint32_t i = 0; i < 16u; i++)
        if ((group & (UINT32_C(1) << i)) && exti_owner[i] >= 0 && exti_priority[i] != priority)
            return EMELA_PERIPHERAL_RESOURCE_CONFLICT;
    uint32_t previous = gpio_lock();
    RCC_APB2ENR |= UINT32_C(1) << 14;
    (void)RCC_APB2ENR;
    uint32_t shift = (line % 4u) * 4u;
    SYSCFG_EXTICR(line / 4u) = (SYSCFG_EXTICR(line / 4u) & ~(UINT32_C(15) << shift)) | (port << shift);
    EXTI(0x08u) = edge != EMELA_GPIO_EDGE_FALLING ? EXTI(0x08u) | bit : EXTI(0x08u) & ~bit;
    EXTI(0x0cu) = edge != EMELA_GPIO_EDGE_RISING ? EXTI(0x0cu) | bit : EXTI(0x0cu) & ~bit;
    EXTI(0x14u) = bit;
    exti_owner[line] = pin.id;
    exti_priority[line] = priority;
    interrupt_pins[port] |= bit;
    *(volatile uint8_t *)(uintptr_t)(UINT32_C(0xe000e400) + irq) = (uint8_t)(0xc0u - (uint32_t)priority * 0x40u);
    EXTI(0x00u) |= bit;
    NVIC_ISER(irq / 32u) = UINT32_C(1) << (irq % 32u);
    gpio_unlock(previous);
    return 0;
}
int32_t emela_gpio_disable_interrupt(EmelaGpioPin pin) {
    main_context();
    int32_t status = gpio_pin_status(pin);
    if (status) return status;
    uint32_t port = (uint32_t)pin.id / 16u, line = (uint32_t)pin.id % 16u, bit = UINT32_C(1) << line;
    if (!(configured_pins[port] & bit)) return EMELA_PERIPHERAL_NOT_CONFIGURED;
    if (!(interrupt_pins[port] & bit)) return 0;
    uint32_t previous = gpio_lock(), irq = exti_irq(line);
    EXTI(0x00u) &= ~bit;
    EXTI(0x08u) &= ~bit;
    EXTI(0x0cu) &= ~bit;
    EXTI(0x14u) = bit;
    interrupt_pins[port] &= ~bit;
    exti_owner[line] = -1;
    if (!(EXTI(0x00u) & exti_group(line))) {
        NVIC_ICER(irq / 32u) = UINT32_C(1) << (irq % 32u);
        NVIC_ICPR(irq / 32u) = UINT32_C(1) << (irq % 32u);
    }
    __asm volatile("dsb\nisb" ::: "memory");
    gpio_unlock(previous);
    return 0;
}
static void gpio_exti(uint32_t group) {
    emela_validation_stack_record_sp();
    uint32_t pending = EXTI(0x14u) & EXTI(0x00u) & group;
    for (uint32_t line = 0; line < 16u; line++) {
        uint32_t bit = UINT32_C(1) << line;
        if (!(pending & bit)) continue;
        EXTI(0x14u) = bit;
        int32_t id = exti_owner[line];
        if (id >= 0 && (interrupt_pins[(uint32_t)id / 16u] & bit)) {
            board_gpio_events++;
            emela_gpio_dispatch(id, (GPIO_IDR((uint32_t)id / 16u) & bit) ? 1 : 0);
        }
    }
    __asm volatile("dsb" ::: "memory");
}
void EXTI0_IRQHandler(void) { gpio_exti(1u); }
void EXTI1_IRQHandler(void) { gpio_exti(2u); }
void EXTI2_IRQHandler(void) { gpio_exti(4u); }
void EXTI3_IRQHandler(void) { gpio_exti(8u); }
void EXTI4_IRQHandler(void) { gpio_exti(16u); }
void EXTI9_5_IRQHandler(void) { gpio_exti(UINT32_C(0x3e0)); }
void EXTI15_10_IRQHandler(void) { gpio_exti(UINT32_C(0xfc00)); }
#endif

#if BOARD_HAS_UART
#include "uart.inc"
#endif
#if BOARD_HAS_I2C
#include "i2c.inc"
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

#ifdef EMELA_HAS_TASK_RUNTIME
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
#endif

void SysTick_Handler(void) {
    emela_validation_stack_record_sp();
    board_ticks++;
    elapsed_ms++;
#ifdef EMELA_HAS_TASK_RUNTIME
    if (remaining_ms != 0 && --remaining_ms == 0) {
        /* Cancellation can invalidate a task before the physical timer expires. */
        if (emela_task_complete_unit(sleeping_task)) board_notifications++;
        else board_rejected_notifications++;
    }
#endif
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
