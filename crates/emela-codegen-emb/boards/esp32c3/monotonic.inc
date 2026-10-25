/* One 52-bit timer extension for UART/I2C/SPI/Clock, including ISR callers.
 * Keep milliseconds separately so I2C's Int64 millisecond deadlines need no
 * narrowing conversion. SPI uses modular microseconds with a checked range.
 */
static uint64_t peripheral_clock_ticks, peripheral_clock_ms;
static uint32_t peripheral_clock_fraction;
static inline void peripheral_sample_time(void) {
    uint64_t current = timer_ticks();
    uint64_t delta = ((current - peripheral_clock_ticks) & ((UINT64_C(1) << 52) - 1u)) + peripheral_clock_fraction;
    peripheral_clock_ticks = current;
    peripheral_clock_ms += delta / TICKS_PER_MS;
    peripheral_clock_fraction = (uint32_t)(delta % TICKS_PER_MS);
}
static inline uint64_t peripheral_now_ms(void) {
    uint32_t previous = irq_disable();
    peripheral_sample_time();
    uint64_t now = peripheral_clock_ms;
    irq_restore(previous); return now;
}
static inline uint64_t peripheral_now_us(void) {
    uint32_t previous = irq_disable();
    peripheral_sample_time();
    uint64_t now = peripheral_clock_ms * 1000u + peripheral_clock_fraction / 16u;
    irq_restore(previous); return now;
}
