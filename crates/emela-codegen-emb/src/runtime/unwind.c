/* A context is reclaimed only after its operation contexts and cleanup frames have ended. */
static int emela_unwind_has_operations(EmelaFiber owner) {
#ifdef EMELA_RESUME_RUNTIME
  for (uint32_t slot = 0; slot < EMELA_FIBER_CAPACITY; slot++) {
    emela_fiber *candidate = &emela_fibers[slot];
    if (candidate->state != EMELA_FIBER_FREE && candidate->perform != NULL &&
        emela_same_fiber(candidate->perform->caller, owner)) return 1;
  }
#else
  (void)owner;
#endif
  return 0;
}
static int emela_unwind_context(emela_fiber *fiber) {
  if (fiber->state == EMELA_FIBER_READY) emela_ready_remove(fiber);
#ifdef EMELA_TASK_CAPACITY
  emela_task_abandon((EmelaFiber){(uint32_t)(fiber - emela_fibers), fiber->generation});
#endif
  fiber->state = EMELA_FIBER_RUNNING;
  while (fiber->top != NULL) {
    if (fiber->top->unwind != NULL) {
      int action = fiber->top->unwind(fiber, fiber->top);
      if (action != 0) {
        if (action == 1) emela_ready_push(fiber);
        return action != 3;
      }
    }
    emela_frame_return(fiber);
  }
  fiber->finishing = 1;
  fiber->state = EMELA_FIBER_WAITING;
  return 1;
}
static void emela_unwind_sweep(void) {
  int progress;
  do {
    progress = 0;
    for (uint32_t slot = 0; slot < EMELA_FIBER_CAPACITY; slot++) {
      emela_fiber *fiber = &emela_fibers[slot];
      if (fiber->state == EMELA_FIBER_FREE || fiber->state == EMELA_FIBER_DONE) continue;
#ifdef EMELA_RESUME_RUNTIME
      if (fiber->perform != NULL) {
        emela_fiber *root = emela_resume_live(fiber->root);
        emela_fiber *caller = emela_resume_live(fiber->perform->caller);
        if (root == NULL) emela_scheduler_invalid();
        if (caller == NULL) emela_scheduler_invalid();
        if (root->finishing || root->cancelled || caller->finishing) fiber->cancelled = 1;
      }
#endif
      if (fiber->active != 0) continue;
      EmelaFiber owner = {slot, fiber->generation};
      if (emela_unwind_has_operations(owner)) continue;
      if (fiber->cancelled && fiber->holds == 0 && !fiber->finishing)
        progress |= emela_unwind_context(fiber);
      if (!fiber->finishing) continue;
      fiber->state = EMELA_FIBER_DONE;
#ifdef EMELA_RESUME_RUNTIME
      emela_resume_after_step(fiber);
#endif
      progress = 1;
    }
  } while (progress);
}
static void emela_resume_drive(emela_fiber *target) {
  emela_fiber *saved = emela_current_fiber, *work = target;
  target->state = EMELA_FIBER_RUNNING;
  for (;;) {
    if (work->cancelled && work->holds == 0) { emela_unwind_sweep(); break; }
    work->active++;
    emela_current_fiber = work;
#ifdef EMELA_RESUME_RUNTIME
    EmelaFiber parent = work->perform == NULL ? (EmelaFiber){EMELA_SLOT_NONE, 0} : work->perform->caller;
#endif
    work->top->step(work, work->top);
    work->active--;
    emela_unwind_sweep();
    int ended = work->state == EMELA_FIBER_DONE || work->state == EMELA_FIBER_FREE || work->finishing;
    if (work == target && ended) break;
    if (ended) {
#ifdef EMELA_RESUME_RUNTIME
      work = emela_resume_live(parent);
      if (work == NULL || work->state != EMELA_FIBER_READY) break;
      emela_ready_remove(work);
      work->state = EMELA_FIBER_RUNNING;
#else
      break;
#endif
    }
    if (work->state == EMELA_FIBER_RUNNING) continue;
#ifdef EMELA_RESUME_RUNTIME
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
#else
    break;
#endif
  }
  emela_current_fiber = saved;
}
