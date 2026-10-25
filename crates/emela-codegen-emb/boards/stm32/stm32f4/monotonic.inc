/* Shared peripheral time, independent of the Clock effect's epoch. */
static inline uint64_t peripheral_now_ms(void) {
    uint32_t previous = interrupt_state();
    __asm volatile("cpsid i" ::: "memory");
    uint64_t now = elapsed_ms;
    __asm volatile("msr primask, %0" :: "r"(previous) : "memory");
    return now;
}
static inline uint64_t peripheral_now_us(void) {
    uint32_t previous = interrupt_state();
    __asm volatile("cpsid i" ::: "memory");
    uint64_t milliseconds = elapsed_ms;
    uint32_t value = SYSTICK_VALUE;
    if (REG(0xe000ed04) & (1u << 26)) { milliseconds++; value = SYSTICK_VALUE; }
    uint64_t now = milliseconds * 1000u + (value ? (16000u - value) / 16u : 0u);
    __asm volatile("msr primask, %0" :: "r"(previous) : "memory");
    return now;
}
