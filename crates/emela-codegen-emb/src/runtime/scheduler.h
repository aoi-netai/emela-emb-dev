/* All scheduler APIs are called from the main execution context, never from an ISR. */
typedef struct {
  uint32_t slot;
  uint32_t generation;
} EmelaFiber;

/* Runs at most budget resumable steps and returns the number actually run. */
uint32_t emela_scheduler_run(uint32_t budget);
int emela_fiber_done(EmelaFiber fiber);
/* Releases a completed handle. Result storage belongs to the caller. */
void emela_fiber_release(EmelaFiber fiber);
