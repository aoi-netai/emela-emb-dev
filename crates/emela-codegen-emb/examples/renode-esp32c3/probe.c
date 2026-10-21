#include <stddef.h>
#include <stdint.h>

/* Register offsets follow ESP-IDF v5.4.1 ESP32-C3 SoC headers. */
#define MMIO32(address) (*(volatile uint32_t *)(uintptr_t)(address))
#define UART_FIFO MMIO32(0x60000000u)
#define UART_STATUS MMIO32(0x6000001cu)
#define GPIO_OUT MMIO32(0x60004004u)
#define GPIO_OUT_SET MMIO32(0x60004008u)
#define GPIO_OUT_CLEAR MMIO32(0x6000400cu)
#define GPIO_ENABLE MMIO32(0x60004020u)
#define GPIO_ENABLE_SET MMIO32(0x60004024u)
#define GPIO_ENABLE_CLEAR MMIO32(0x60004028u)
#define GPIO_IN MMIO32(0x6000403cu)
#define TIMER_REG(offset) MMIO32(0x60023000u + (offset))
#define INTMATRIX_REG(offset) MMIO32(0x600c2000u + (offset))

#define PROBE_PIN_BIT (1u << 3)
#define CPU_INTERRUPT 1u
#define TIMER_UNIT_ENABLE (1u << 30)
#define TIMER_TARGET_ENABLE (1u << 24)
#define TIMER_CLOCK_ENABLE (1u << 31)
#define TIMER_DELAY_TICKS 160000u
#define PROBE_INTERRUPTS 4u

volatile uint32_t probe_status;
volatile uint32_t probe_failure;
volatile uint32_t probe_irq_count;
volatile uint32_t probe_main_count;
volatile uint32_t probe_gpio_count;
volatile uint32_t probe_gpio_trace[8];
volatile uint32_t probe_irq_causes[8];
volatile uint32_t probe_irq_pcs[8];
volatile uint32_t probe_irq_ticks[8];
volatile uint32_t probe_stack_used;
volatile uint32_t probe_stack_guard_ok;

extern uint32_t _stack_guard_start[];
extern uint32_t _stack_bottom[];
extern uint32_t _stack_top[];

static void uart_putc(char character)
{
    while (((UART_STATUS >> 16) & 0x3ffu) >= 127u) {
    }
    UART_FIFO = (uint8_t)character;
}

static void uart_text(const char *text)
{
    while (*text != '\0') {
        uart_putc(*text++);
    }
}

static void uart_unsigned(uint32_t value)
{
    char digits[10];
    size_t length = 0;
    do {
        digits[length++] = (char)('0' + value % 10u);
        value /= 10u;
    } while (value != 0);
    while (length != 0) {
        uart_putc(digits[--length]);
    }
}

static void stop_cpu(void)
{
    __asm__ volatile("csrci mstatus, 8" ::: "memory");
    for (;;) {
        __asm__ volatile("wfi");
    }
}

static void fail(uint32_t reason)
{
    probe_failure = reason;
    probe_status = 0xdeadu;
    uart_text("ESP32C3_PROBE_FAIL code=");
    uart_unsigned(reason);
    uart_text("\n");
    stop_cpu();
}

static void require(int condition, uint32_t reason)
{
    if (!condition) {
        fail(reason);
    }
}

static uint32_t timer_ticks(void)
{
    TIMER_REG(0x04u) = 1u << 30;
    while ((TIMER_REG(0x04u) & (1u << 29)) == 0) {
    }
    return TIMER_REG(0x44u);
}

static void arm_timer(void)
{
    TIMER_REG(0x00u) = TIMER_CLOCK_ENABLE | TIMER_UNIT_ENABLE;
    TIMER_REG(0x1cu) = 0;
    TIMER_REG(0x20u) = timer_ticks() + TIMER_DELAY_TICKS;
    TIMER_REG(0x34u) = 0;
    TIMER_REG(0x50u) = 1;
    TIMER_REG(0x00u) = TIMER_CLOCK_ENABLE | TIMER_UNIT_ENABLE | TIMER_TARGET_ENABLE;
}

void probe_trap(uint32_t cause, uint32_t pc)
{
    uint32_t index = probe_irq_count;
    if (index < 8u) {
        probe_irq_causes[index] = cause;
        probe_irq_pcs[index] = pc;
    }
    /* CLIC MCAUSE also carries previous privilege and interrupt-enable bits. */
    require((cause & 0x80000fffu) == (0x80000000u | CPU_INTERRUPT), 20u);
    require(index < PROBE_INTERRUPTS, 21u);
    require((TIMER_REG(0x70u) & 1u) != 0, 22u);
    probe_irq_ticks[index] = timer_ticks();
    TIMER_REG(0x6cu) = 1u;
    probe_irq_count = index + 1u;
}

static void measure_stack(void)
{
    probe_stack_guard_ok = 1;
    for (volatile uint32_t *word = _stack_guard_start; word < _stack_bottom; ++word) {
        if (*word != 0xdeadc0deu) {
            probe_stack_guard_ok = 0;
        }
    }
    volatile uint32_t *word = _stack_bottom;
    while (word < _stack_top && *word == 0xa5a5a5a5u) {
        ++word;
    }
    probe_stack_used = (uint32_t)((uintptr_t)_stack_top - (uintptr_t)word);
}

void probe_main(void)
{
    probe_status = 1;
    uart_text("ESP32C3_PROBE_START\n");

    /* The upstream GPIO model exposes register loopback, not physical pads. */
    GPIO_OUT = 0;
    GPIO_ENABLE_SET = PROBE_PIN_BIT;
    require((GPIO_ENABLE & PROBE_PIN_BIT) != 0, 1u);
    GPIO_OUT_SET = PROBE_PIN_BIT;
    require((GPIO_OUT & PROBE_PIN_BIT) != 0, 2u);
    require((GPIO_IN & PROBE_PIN_BIT) != 0, 3u);
    GPIO_OUT_CLEAR = PROBE_PIN_BIT;
    require((GPIO_OUT & PROBE_PIN_BIT) == 0, 4u);
    require((GPIO_IN & PROBE_PIN_BIT) == 0, 5u);
    GPIO_ENABLE_CLEAR = PROBE_PIN_BIT;
    require((GPIO_ENABLE & PROBE_PIN_BIT) == 0, 6u);
    GPIO_ENABLE_SET = PROBE_PIN_BIT;

    TIMER_REG(0x00u) = TIMER_CLOCK_ENABLE | TIMER_UNIT_ENABLE;
    TIMER_REG(0x64u) = 0;
    TIMER_REG(0x6cu) = 7;
    INTMATRIX_REG(0x094u) = CPU_INTERRUPT;
    INTMATRIX_REG(0x108u) = 0;
    INTMATRIX_REG(0x118u) = 1;
    INTMATRIX_REG(0x194u) = 1;
    INTMATRIX_REG(0x104u) = 1u << CPU_INTERRUPT;
    TIMER_REG(0x64u) = 1;
    __asm__ volatile("csrsi mstatus, 8" ::: "memory");

    for (uint32_t round = 0; round < PROBE_INTERRUPTS; ++round) {
        arm_timer();
        while (probe_irq_count == round) {
            __asm__ volatile("nop");
        }
        require(probe_irq_count == round + 1u, 30u);
        require((TIMER_REG(0x70u) & 1u) == 0, 31u);
        GPIO_OUT_SET = PROBE_PIN_BIT;
        probe_gpio_trace[probe_gpio_count++] = GPIO_OUT & PROBE_PIN_BIT;
        require((GPIO_IN & PROBE_PIN_BIT) != 0, 32u);
        GPIO_OUT_CLEAR = PROBE_PIN_BIT;
        probe_gpio_trace[probe_gpio_count++] = GPIO_OUT & PROBE_PIN_BIT;
        require((GPIO_IN & PROBE_PIN_BIT) == 0, 33u);
        probe_main_count = round + 1u;
        uart_text("TIMER irq=");
        uart_unsigned(probe_irq_count);
        uart_text(" main=");
        uart_unsigned(probe_main_count);
        uart_text("\n");
    }

    __asm__ volatile("csrci mstatus, 8" ::: "memory");
    TIMER_REG(0x64u) = 0;
    TIMER_REG(0x00u) = 0;
    INTMATRIX_REG(0x104u) = 0;
    measure_stack();
    require(probe_stack_guard_ok != 0, 40u);
    require(probe_irq_count == PROBE_INTERRUPTS && probe_main_count == PROBE_INTERRUPTS, 41u);
    probe_status = 0x600du;
    uart_text("ESP32C3_PROBE_PASS irq=");
    uart_unsigned(probe_irq_count);
    uart_text(" main=");
    uart_unsigned(probe_main_count);
    uart_text(" gpio=");
    uart_unsigned(probe_gpio_count);
    uart_text("\n");
    stop_cpu();
}
