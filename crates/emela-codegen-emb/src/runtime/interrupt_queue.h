/* Four compiler-owned fixed Int queues. Short IRQ-masked sections never wait.
 * No ordinary Emela cells, allocation, retained pointers, or scheduler access.
 */
static volatile int32_t emela_shared_words[4][16];
static volatile uint32_t emela_shared_head[4], emela_shared_tail[4];
static inline int32_t emela_shared_queue_push(int32_t channel, int32_t value) {
    if (channel < 0 || channel >= 4) return 0;
    uint32_t previous = emela_board_event_lock();
    uint32_t head = emela_shared_head[channel];
    int32_t accepted = head - emela_shared_tail[channel] < 16;
    if (accepted) {
        emela_shared_words[channel][head % 16] = value;
        emela_shared_head[channel] = head + 1;
    }
    emela_board_event_unlock(previous);
    return accepted;
}
static inline int32_t emela_shared_queue_pop(int32_t channel, int32_t *value) {
    *value = 0;
    if (channel < 0 || channel >= 4) return 0;
    uint32_t previous = emela_board_event_lock();
    uint32_t tail = emela_shared_tail[channel];
    int32_t available = emela_shared_head[channel] != tail;
    if (available) {
        *value = emela_shared_words[channel][tail % 16];
        emela_shared_tail[channel] = tail + 1;
    }
    emela_board_event_unlock(previous);
    return available;
}
