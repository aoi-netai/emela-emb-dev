/* A resume value owns this record, never a raw pointer to a recycled frame. */
struct emela_resume {
  EmelaFiber caller;
  EmelaFiber root;
  void *result;
  const char *duplicate_message;
  uint16_t duplicate_length;
  int initial;
  int used;
};
static emela_fiber *emela_resume_live(EmelaFiber handle) {
  if (handle.slot >= EMELA_FIBER_CAPACITY) return NULL;
  emela_fiber *fiber = &emela_fibers[handle.slot];
  if (fiber->state == EMELA_FIBER_FREE || fiber->generation != handle.generation) return NULL;
  return fiber;
}
static int emela_resume_over(EmelaFiber root) {
  emela_fiber *fiber = emela_resume_live(root);
  return fiber == NULL || fiber->state == EMELA_FIBER_DONE;
}
static int emela_same_fiber(EmelaFiber a, EmelaFiber b) {
  return a.slot == b.slot && a.generation == b.generation;
}
static void emela_ready_remove(emela_fiber *fiber) {
  uint32_t slot = (uint32_t)(fiber - emela_fibers), previous = EMELA_SLOT_NONE;
  for (uint32_t at = emela_ready_head; at != EMELA_SLOT_NONE; at = emela_fibers[at].next) {
    if (at == slot) {
      if (previous == EMELA_SLOT_NONE) emela_ready_head = fiber->next;
      else emela_fibers[previous].next = fiber->next;
      if (emela_ready_tail == slot) emela_ready_tail = previous;
      return;
    }
    previous = at;
  }
  emela_scheduler_invalid();
}
static void emela_resume_wake(emela_resume *resume) {
  if (emela_resume_over(resume->root)) return;
  emela_fiber *caller = emela_resume_live(resume->caller);
  if (caller == NULL || caller->state != EMELA_FIBER_WAITING) emela_scheduler_invalid();
  emela_ready_push(caller);
}
static void emela_resume_boundary(emela_fiber *fiber) {
  /* Propagate a real suspension through the initial phases of enclosing operations. */
  while (fiber != NULL && fiber->perform != NULL && fiber->perform->initial) {
    emela_resume *resume = fiber->perform;
    resume->initial = 0;
    fiber = emela_resume_live(resume->caller);
    if (resume->used) emela_resume_wake(resume);
  }
}
static emela_fiber *emela_operation_begin(emela_fiber *caller, void *result,
                                        const char *message, uint16_t length) {
  EmelaFiber handle = emela_fiber_create();
  emela_fiber *implementation = emela_fiber_get(handle);
  implementation->root = caller->root;
  emela_resume *resume = (emela_resume *)emela_heap_allocate(sizeof(emela_resume), EMELA_RESUME_KIND);
  *resume = (emela_resume){
    .caller = {(uint32_t)(caller - emela_fibers), caller->generation},
    .root = caller->root, .result = result,
    .duplicate_message = message, .duplicate_length = length, .initial = 1
  };
  implementation->perform = resume;
  caller->state = EMELA_FIBER_WAITING;
  return implementation;
}
static emela_resume *emela_resume_prepare(void *environment) {
  emela_resume *resume = (emela_resume *)environment;
  if (resume == NULL) emela_scheduler_invalid();
  /* Checking the root first also makes callbacks from previous invocations harmless. */
  if (emela_resume_over(resume->root)) return NULL;
  if (resume->used)
    emela_board_defect(EMELA_DEFECT_PANIC, resume->duplicate_message, resume->duplicate_length);
  resume->used = 1;
  return resume;
}
static void emela_resume_after_step(emela_fiber *fiber) {
  if (fiber->state != EMELA_FIBER_DONE || fiber->perform == NULL) return;
  emela_resume *resume = fiber->perform;
  if (resume->initial) {
    resume->initial = 0;
    if (resume->used) emela_resume_wake(resume);
    else emela_resume_boundary(emela_resume_live(resume->caller));
  }
  fiber->perform = NULL;
  emela_heap_release(resume);
  emela_fiber_release((EmelaFiber){(uint32_t)(fiber - emela_fibers), fiber->generation});
}
static void emela_resume_sweep(void) {
  for (uint32_t slot = 0; slot < EMELA_FIBER_CAPACITY; slot++) {
    emela_fiber *fiber = &emela_fibers[slot];
    if (fiber->state == EMELA_FIBER_FREE || fiber->perform == NULL || !emela_resume_over(fiber->root)) continue;
    if (fiber->state == EMELA_FIBER_READY) emela_ready_remove(fiber);
#ifdef EMELA_TASK_CAPACITY
    emela_task_abandon((EmelaFiber){slot, fiber->generation});
#endif
    while (fiber->top != NULL) emela_frame_return(fiber);
    fiber->state = EMELA_FIBER_DONE;
    emela_heap_release(fiber->perform);
    fiber->perform = NULL;
    emela_fiber_release((EmelaFiber){slot, fiber->generation});
  }
}
static void emela_resume_drive(emela_fiber *target) {
  emela_fiber *saved = emela_current_fiber, *work = target;
  target->state = EMELA_FIBER_RUNNING;
  for (;;) {
    emela_current_fiber = work;
    EmelaFiber parent = work->perform == NULL ? (EmelaFiber){EMELA_SLOT_NONE, 0} : work->perform->caller;
    work->top->step(work, work->top);
    int ended = work->state == EMELA_FIBER_DONE;
    int target_ended = ended && work == target;
    emela_resume_after_step(work);
    if (target_ended) break;
    if (ended) {
      work = emela_resume_live(parent);
      if (work == NULL || work->state != EMELA_FIBER_READY) break;
      emela_ready_remove(work);
      work->state = EMELA_FIBER_RUNNING;
    }
    if (work->state == EMELA_FIBER_RUNNING) continue;
    /* A nested operation that has not reached an actual suspension runs in this breath. */
    emela_fiber *child = NULL;
    for (uint32_t slot = 0; slot < EMELA_FIBER_CAPACITY; slot++) {
      emela_fiber *candidate = &emela_fibers[slot];
      if (candidate->state == EMELA_FIBER_READY && candidate->perform != NULL &&
          candidate->perform->initial && candidate->perform->caller.slot == (uint32_t)(work - emela_fibers) &&
          candidate->perform->caller.generation == work->generation) { child = candidate; break; }
    }
    if (child == NULL) break;
    emela_ready_remove(child);
    child->state = EMELA_FIBER_RUNNING;
    work = child;
  }
  emela_current_fiber = saved;
}
static void emela_resume_commit(emela_resume *resume) {
  if (resume->initial) return;
  emela_fiber *caller = emela_resume_live(resume->caller);
  if (caller == NULL || caller->state != EMELA_FIBER_WAITING) emela_scheduler_invalid();
  if (emela_current_fiber == NULL || emela_same_fiber(emela_current_fiber->root, resume->root))
    emela_resume_drive(caller);
  else emela_ready_push(caller);
}
