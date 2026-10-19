/* All scheduler APIs are called from the main execution context, never from an ISR. */
typedef struct {
  uint32_t slot;
  uint32_t generation;
} EmelaFiber;

/* Runs at most budget work units (a resumable step or a completion event).
 * Returns the number actually processed. A zero budget performs no work. */
uint32_t emela_scheduler_run(uint32_t budget);
int emela_fiber_done(EmelaFiber fiber);
/* Releases a completed handle. Result storage belongs to the caller. */
void emela_fiber_release(EmelaFiber fiber);
