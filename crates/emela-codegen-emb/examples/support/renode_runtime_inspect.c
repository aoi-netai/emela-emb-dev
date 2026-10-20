
volatile uint32_t emela_runtime_heap_peak;
volatile uint32_t emela_runtime_children_peak;
volatile uint32_t emela_runtime_frames_peak;
volatile uint32_t emela_runtime_fibers_peak;

size_t emela_runtime_heap(void) {
  size_t used = 0;
  for (unsigned char *p = emela_heap_storage; emela_heap_initialized && p < emela_heap_end();) {
    emela_heap_block *block = (emela_heap_block *)(void *)p;
    if (!block->is_free) used += block->size;
    p += block->size;
  }
  return used;
}
unsigned emela_runtime_children(void) {
  unsigned used = 0;
  for (unsigned char *p = emela_heap_storage; emela_heap_initialized && p < emela_heap_end();) {
    emela_heap_block *block = (emela_heap_block *)(void *)p;
    if (!block->is_free && block->kind == EMELA_CHILD_KIND) used++;
    p += block->size;
  }
  return used;
}
static void emela_runtime_heap_sample(void) {
  uint32_t heap = (uint32_t)emela_runtime_heap(), children = emela_runtime_children();
  if (heap > emela_runtime_heap_peak) emela_runtime_heap_peak = heap;
  if (children > emela_runtime_children_peak) emela_runtime_children_peak = children;
}
void emela_runtime_cancel(EmelaFiber handle) { emela_fiber_get(handle)->cancelled = 1; }
void emela_runtime_sample(void) {
  unsigned free_frames = 0, fibers = 0;
  for (uint32_t i = emela_frame_free; i != EMELA_SLOT_NONE && free_frames <= EMELA_FRAME_CAPACITY; i = emela_frames[i].next) free_frames++;
  for (unsigned i = 0; i < EMELA_FIBER_CAPACITY; i++) if (emela_fibers[i].state != EMELA_FIBER_FREE) fibers++;
  unsigned frames = EMELA_FRAME_CAPACITY - free_frames;
  if (frames > emela_runtime_frames_peak) emela_runtime_frames_peak = frames;
  if (fibers > emela_runtime_fibers_peak) emela_runtime_fibers_peak = fibers;
}
int emela_runtime_pools_empty(void) {
  unsigned frames = 0, fibers = 0, tasks = 0, deferreds = 0;
  if (emela_scheduler_running || emela_current_fiber != NULL || emela_ready_head != EMELA_SLOT_NONE || emela_ready_tail != EMELA_SLOT_NONE || emela_event_count != 0) return 0;
  for (uint32_t i = emela_frame_free; i != EMELA_SLOT_NONE;) {
    if (i >= EMELA_FRAME_CAPACITY || ++frames > EMELA_FRAME_CAPACITY) return 0;
    i = emela_frames[i].next;
  }
  for (uint32_t i = emela_fiber_free; i != EMELA_SLOT_NONE;) {
    if (i >= EMELA_FIBER_CAPACITY || ++fibers > EMELA_FIBER_CAPACITY || emela_fibers[i].state != EMELA_FIBER_FREE) return 0;
    i = emela_fibers[i].next;
  }
  for (uint32_t i = emela_task_free; i != EMELA_SLOT_NONE;) {
    if (i >= EMELA_TASK_CAPACITY || ++tasks > EMELA_TASK_CAPACITY || emela_tasks[i].state != EMELA_TASK_FREE) return 0;
    i = emela_tasks[i].next;
  }
  for (uint32_t i = emela_deferred_free; i != EMELA_SLOT_NONE;) {
    if (i >= EMELA_DEFERRED_CAPACITY || ++deferreds > EMELA_DEFERRED_CAPACITY) return 0;
    i = emela_deferred_slots[i].next;
  }
  return frames == EMELA_FRAME_CAPACITY && fibers == EMELA_FIBER_CAPACITY && tasks == EMELA_TASK_CAPACITY && deferreds == EMELA_DEFERRED_CAPACITY;
}
