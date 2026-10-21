#include <stdint.h>

/* Reproducible application input; replace with a physical sensor driver as needed. */
int32_t temperature_sensor_read(void) {
    static const int32_t temperatures[] = {
        2100, 1900, 1800, 1900, 1999, 2000, 2700, 2800,
        2700, 2601, 2600, 1800, 2900, 1700, INT32_MIN, 1900,
        1800, 12501, 1700, 2000, 2800, -4001, 2900, 2600,
    };
    static uint32_t index;
    int32_t result = temperatures[index];
    index = (index + 1) % (sizeof temperatures / sizeof temperatures[0]);
    return result;
}
