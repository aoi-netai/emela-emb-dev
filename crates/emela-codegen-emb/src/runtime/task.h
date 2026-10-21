typedef struct {
  uint32_t slot;
  uint32_t generation;
} EmelaTask;

/* May be called from an ISR. Copies a completion into the fixed event queue.
 * Returns zero for a stale, duplicate, or mismatched completion; never resumes Emela here.
 * A full event queue calls the board defect hook after restoring the interrupt state. */
int emela_task_complete_unit(EmelaTask task);
int emela_task_complete_int(EmelaTask task, int32_t value);
int emela_task_complete_int64(EmelaTask task, int64_t value);
int emela_task_complete_float(EmelaTask task, double value);
