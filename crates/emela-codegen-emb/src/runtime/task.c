enum { EMELA_TASK_UNIT, EMELA_TASK_INT, EMELA_TASK_INT64, EMELA_TASK_FLOAT };
enum { EMELA_TASK_FREE, EMELA_TASK_PENDING, EMELA_TASK_QUEUED };
typedef union { int32_t i32; int64_t i64; double f64; } emela_task_value;
typedef struct {
  EmelaFiber fiber;
  void *result;
  uint32_t generation;
  uint32_t next;
  uint32_t state;
  uint32_t kind;
} emela_task_slot;
typedef struct { EmelaTask task; emela_task_value value; } emela_event;
static emela_task_slot emela_tasks[EMELA_TASK_CAPACITY];
static emela_event emela_events[EMELA_EVENT_CAPACITY];
static uint32_t emela_task_free;
static uint32_t emela_event_head, emela_event_tail, emela_event_count;
static int emela_tasks_initialized;

static EmelaTask emela_task_begin(emela_fiber *fiber, void *result, uint32_t kind) {
  /* Initialization runs once in the main context, before any handle is published. */
  if (!emela_tasks_initialized) {
    for (uint32_t i = 0; i < EMELA_TASK_CAPACITY; i++)
      emela_tasks[i].next = i + 1 < EMELA_TASK_CAPACITY ? i + 1 : EMELA_SLOT_NONE;
    emela_task_free = 0;
    emela_tasks_initialized = 1;
  }
  uint32_t irq_state = emela_board_event_lock();
  if (emela_task_free == EMELA_SLOT_NONE) {
    emela_board_event_unlock(irq_state);
    emela_scheduler_exhausted();
  }
  uint32_t slot = emela_task_free;
  emela_task_slot *task = &emela_tasks[slot];
  if (task->generation == UINT32_MAX) {
    emela_board_event_unlock(irq_state);
    emela_scheduler_exhausted();
  }
  emela_task_free = task->next;
  task->generation++;
  task->fiber = (EmelaFiber){(uint32_t)(fiber - emela_fibers), fiber->generation};
  task->result = result;
  task->kind = kind;
  task->state = EMELA_TASK_PENDING;
  fiber->state = EMELA_FIBER_WAITING;
  EmelaTask handle = {slot, task->generation};
  emela_board_event_unlock(irq_state);
  return handle;
}

static int emela_task_complete(EmelaTask handle, uint32_t kind, emela_task_value value) {
  uint32_t irq_state = emela_board_event_lock();
  if (handle.slot >= EMELA_TASK_CAPACITY) {
    emela_board_event_unlock(irq_state);
    return 0;
  }
  emela_task_slot *task = &emela_tasks[handle.slot];
  if (task->generation != handle.generation || task->state != EMELA_TASK_PENDING || task->kind != kind) {
    emela_board_event_unlock(irq_state);
    return 0;
  }
  if (emela_event_count == EMELA_EVENT_CAPACITY) {
    emela_board_event_unlock(irq_state);
    emela_board_defect(EMELA_DEFECT_HEAP_EXHAUSTED, "fixed event queue exhausted", UINT16_C(27));
  }
  emela_events[emela_event_tail] = (emela_event){handle, value};
  emela_event_tail = (emela_event_tail + 1) % EMELA_EVENT_CAPACITY;
  emela_event_count++;
  task->state = EMELA_TASK_QUEUED;
  emela_board_event_unlock(irq_state);
  return 1;
}
int emela_task_complete_unit(EmelaTask task) {
  return emela_task_complete(task, EMELA_TASK_UNIT, (emela_task_value){0});
}
int emela_task_complete_int(EmelaTask task, int32_t value) {
  return emela_task_complete(task, EMELA_TASK_INT, (emela_task_value){.i32 = value});
}
int emela_task_complete_int64(EmelaTask task, int64_t value) {
  return emela_task_complete(task, EMELA_TASK_INT64, (emela_task_value){.i64 = value});
}
int emela_task_complete_float(EmelaTask task, double value) {
  return emela_task_complete(task, EMELA_TASK_FLOAT, (emela_task_value){.f64 = value});
}

static int emela_task_dispatch_one(void) {
  uint32_t irq_state = emela_board_event_lock();
  if (emela_event_count == 0) {
    emela_board_event_unlock(irq_state);
    return 0;
  }
  emela_event event = emela_events[emela_event_head];
  emela_event_head = (emela_event_head + 1) % EMELA_EVENT_CAPACITY;
  emela_event_count--;
  emela_task_slot *task = &emela_tasks[event.task.slot];
  if (task->generation != event.task.generation || task->state != EMELA_TASK_QUEUED) {
    emela_board_event_unlock(irq_state);
    emela_scheduler_invalid();
  }
  EmelaFiber owner = task->fiber;
  void *result = task->result;
  uint32_t kind = task->kind;
  task->state = EMELA_TASK_FREE;
  task->next = emela_task_free;
  emela_task_free = event.task.slot;
  emela_board_event_unlock(irq_state);

  emela_fiber *fiber = emela_fiber_get(owner);
  if (fiber->state != EMELA_FIBER_WAITING) emela_scheduler_invalid();
  if (result != NULL) {
    switch (kind) {
      case EMELA_TASK_UNIT: break;
      case EMELA_TASK_INT: *(int32_t *)result = event.value.i32; break;
      case EMELA_TASK_INT64: *(int64_t *)result = event.value.i64; break;
      case EMELA_TASK_FLOAT: *(double *)result = event.value.f64; break;
      default: emela_scheduler_invalid();
    }
  }
  emela_ready_push(fiber);
  return 1;
}
