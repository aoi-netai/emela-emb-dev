/* Fixed pools and an intrusive FIFO: no allocation or recursive C calls on resume. */
#define EMELA_SLOT_NONE UINT32_MAX
typedef struct emela_frame emela_frame;
typedef struct emela_fiber emela_fiber;
typedef void (*emela_step)(emela_fiber *, emela_frame *);
struct emela_frame {
  emela_frame *parent;
  emela_step step;
  uint32_t pc;
  uint32_t slot;
};
typedef struct {
  _Alignas(max_align_t) unsigned char data[EMELA_FRAME_BYTES];
  uint32_t next;
} emela_frame_slot;
enum { EMELA_FIBER_FREE, EMELA_FIBER_READY, EMELA_FIBER_RUNNING,
       EMELA_FIBER_WAITING, EMELA_FIBER_DONE };
struct emela_fiber {
  emela_frame *top;
  uint32_t generation;
  uint32_t next;
  uint32_t state;
};
static emela_frame_slot emela_frames[EMELA_FRAME_CAPACITY];
static emela_fiber emela_fibers[EMELA_FIBER_CAPACITY];
static uint32_t emela_frame_free;
static uint32_t emela_fiber_free;
static uint32_t emela_ready_head;
static uint32_t emela_ready_tail;
static int emela_scheduler_initialized;
static int emela_scheduler_running;

static _Noreturn void emela_scheduler_invalid(void) {
  emela_board_defect(EMELA_DEFECT_ASSERT, "invalid scheduler state", UINT16_C(23));
}
static _Noreturn void emela_scheduler_exhausted(void) {
  emela_board_defect(EMELA_DEFECT_HEAP_EXHAUSTED, "fixed scheduler pool exhausted", UINT16_C(30));
}
static void emela_scheduler_initialize(void) {
  if (emela_scheduler_initialized) return;
  for (uint32_t i = 0; i < EMELA_FRAME_CAPACITY; i++)
    emela_frames[i].next = i + 1 < EMELA_FRAME_CAPACITY ? i + 1 : EMELA_SLOT_NONE;
  for (uint32_t i = 0; i < EMELA_FIBER_CAPACITY; i++)
    emela_fibers[i].next = i + 1 < EMELA_FIBER_CAPACITY ? i + 1 : EMELA_SLOT_NONE;
  emela_frame_free = emela_fiber_free = 0;
  emela_ready_head = emela_ready_tail = EMELA_SLOT_NONE;
  emela_scheduler_initialized = 1;
}
static emela_fiber *emela_fiber_get(EmelaFiber handle) {
  if (handle.slot >= EMELA_FIBER_CAPACITY) emela_scheduler_invalid();
  emela_fiber *fiber = &emela_fibers[handle.slot];
  if (fiber->state == EMELA_FIBER_FREE || fiber->generation != handle.generation)
    emela_scheduler_invalid();
  return fiber;
}
static void emela_ready_push(emela_fiber *fiber) {
  uint32_t slot = (uint32_t)(fiber - emela_fibers);
  if (fiber->state != EMELA_FIBER_RUNNING && fiber->state != EMELA_FIBER_WAITING)
    emela_scheduler_invalid();
  fiber->state = EMELA_FIBER_READY;
  fiber->next = EMELA_SLOT_NONE;
  if (emela_ready_tail == EMELA_SLOT_NONE) emela_ready_head = slot;
  else emela_fibers[emela_ready_tail].next = slot;
  emela_ready_tail = slot;
}
static EmelaFiber emela_fiber_create(void) {
  emela_scheduler_initialize();
  if (emela_fiber_free == EMELA_SLOT_NONE) emela_scheduler_exhausted();
  uint32_t slot = emela_fiber_free;
  emela_fiber *fiber = &emela_fibers[slot];
  if (fiber->generation == UINT32_MAX) emela_scheduler_exhausted();
  emela_fiber_free = fiber->next;
  fiber->generation++;
  fiber->top = NULL;
  fiber->state = EMELA_FIBER_RUNNING;
  return (EmelaFiber){slot, fiber->generation};
}
static emela_frame *emela_frame_push(emela_fiber *fiber, size_t size, emela_step step) {
  if (size > EMELA_FRAME_BYTES || emela_frame_free == EMELA_SLOT_NONE)
    emela_scheduler_exhausted();
  if (size < sizeof(emela_frame) || step == NULL) emela_scheduler_invalid();
  uint32_t slot = emela_frame_free;
  emela_frame_free = emela_frames[slot].next;
  emela_frame *frame = (emela_frame *)(void *)emela_frames[slot].data;
  *frame = (emela_frame){.parent = fiber->top, .step = step, .slot = slot};
  fiber->top = frame;
  return frame;
}
static void emela_frame_return(emela_fiber *fiber) {
  emela_frame *frame = fiber->top;
  if (frame == NULL) emela_scheduler_invalid();
  fiber->top = frame->parent;
  uint32_t slot = frame->slot;
  emela_frames[slot].next = emela_frame_free;
  emela_frame_free = slot;
  if (fiber->top == NULL) fiber->state = EMELA_FIBER_DONE;
}
uint32_t emela_scheduler_run(uint32_t budget) {
  emela_scheduler_initialize();
  if (emela_scheduler_running) emela_scheduler_invalid();
  emela_scheduler_running = 1;
  uint32_t steps = 0;
  while (steps < budget && emela_ready_head != EMELA_SLOT_NONE) {
    emela_fiber *fiber = &emela_fibers[emela_ready_head];
    emela_ready_head = fiber->next;
    if (emela_ready_head == EMELA_SLOT_NONE) emela_ready_tail = EMELA_SLOT_NONE;
    fiber->state = EMELA_FIBER_RUNNING;
    if (fiber->top == NULL) emela_scheduler_invalid();
    fiber->top->step(fiber, fiber->top);
    steps++;
    if (fiber->state == EMELA_FIBER_RUNNING) emela_ready_push(fiber);
  }
  emela_scheduler_running = 0;
  return steps;
}
int emela_fiber_done(EmelaFiber handle) {
  return emela_fiber_get(handle)->state == EMELA_FIBER_DONE;
}
void emela_fiber_release(EmelaFiber handle) {
  emela_fiber *fiber = emela_fiber_get(handle);
  if (fiber->state != EMELA_FIBER_DONE || fiber->top != NULL) emela_scheduler_invalid();
  fiber->state = EMELA_FIBER_FREE;
  fiber->next = emela_fiber_free;
  emela_fiber_free = handle.slot;
}
