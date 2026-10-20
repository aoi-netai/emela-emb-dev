/* Structured lifetime for non-failing fibers. All work runs in the main context. */
static void emela_scope_finish(emela_scope *scope) {
  if (scope->ended || !scope->closed || scope->running != 0) return;
  scope->ended = 1;
  emela_child *child = scope->children;
  scope->children = NULL;
  while (child != NULL) {
    emela_child *next = child->next;
    child->next = NULL;
    emela_heap_release(child);
    child = next;
  }
}
static void emela_scope_close(emela_scope *scope, EmelaFiber closer) {
  if (!scope->closed) {
    scope->closed = 1;
    scope->closer = closer;
    for (emela_child *child = scope->children; child != NULL; child = child->next) {
      if (child->status != 0) continue;
      for (uint32_t slot = 0; slot < EMELA_FIBER_CAPACITY; slot++) {
        emela_fiber *fiber = &emela_fibers[slot];
        if (fiber->state != EMELA_FIBER_FREE && emela_same_fiber(fiber->root, child->execution))
          fiber->cancelled = 1;
      }
    }
  } else if (closer.slot == EMELA_SLOT_NONE) {
    scope->closer = closer;
  }
}
static void emela_child_after_step(emela_fiber *fiber) {
  if (fiber->state != EMELA_FIBER_DONE || fiber->child == NULL) return;
  emela_child *child = fiber->child;
  emela_scope *scope = child->scope;
  emela_heap_retain(scope);
  child->status = fiber->cancelled ? 2 : 1;
  if (scope->running == 0) emela_scheduler_invalid();
  scope->running--;
  fiber->child = NULL;
  emela_fiber_release(child->execution);
  emela_heap_release(child);
  if (scope->closed && scope->running == 0) {
    emela_fiber *closer = emela_resume_live(scope->closer);
    if (closer != NULL && !closer->cancelled && closer->state == EMELA_FIBER_WAITING)
      emela_ready_push(closer);
    if (scope->closer.slot == EMELA_SLOT_NONE) emela_scope_finish(scope);
  }
  emela_heap_release(scope);
}
static void emela_scope_sweep(void) {
  int progress;
  do {
    progress = 0;
    for (uint32_t slot = 0; slot < EMELA_FIBER_CAPACITY; slot++) {
      emela_fiber *fiber = &emela_fibers[slot];
      if (fiber->state == EMELA_FIBER_FREE || !fiber->cancelled || fiber->active != 0) continue;
      if (fiber->state == EMELA_FIBER_READY) emela_ready_remove(fiber);
#ifdef EMELA_TASK_CAPACITY
      emela_task_abandon((EmelaFiber){slot, fiber->generation});
#endif
      while (fiber->top != NULL) emela_frame_return(fiber);
      fiber->state = EMELA_FIBER_DONE;
      if (fiber->child != NULL) emela_child_after_step(fiber);
      else if (fiber->perform != NULL) emela_resume_after_step(fiber);
      else emela_scheduler_invalid();
      progress = 1;
    }
  } while (progress);
}
typedef struct {
  emela_frame base;
  emela_scope *scope;
} emela_scope_frame;
_Static_assert(sizeof(emela_scope_frame) <= EMELA_FRAME_BYTES, "increase frame_bytes for Scope");
static void emela_scope_drop(emela_frame *base) {
  emela_scope *scope = ((emela_scope_frame *)base)->scope;
  emela_scope_close(scope, (EmelaFiber){EMELA_SLOT_NONE, 0});
  emela_scope_finish(scope);
  emela_heap_release(scope);
}
static void emela_scope_step(emela_fiber *fiber, emela_frame *base) {
  emela_scope *scope = ((emela_scope_frame *)base)->scope;
  emela_scope_close(scope, (EmelaFiber){(uint32_t)(fiber - emela_fibers), fiber->generation});
  if (scope->running != 0) {
    fiber->state = EMELA_FIBER_WAITING;
    emela_resume_boundary(fiber);
    return;
  }
  emela_scope_finish(scope);
  emela_frame_return(fiber);
}
static inline emela_scope *emela_scope_begin(emela_fiber *fiber) {
  emela_scope *scope = (emela_scope *)emela_heap_allocate(sizeof(emela_scope), EMELA_SCOPE_KIND);
  *scope = (emela_scope){.closer = {EMELA_SLOT_NONE, 0}};
  emela_scope_frame *frame = (emela_scope_frame *)emela_frame_push(fiber, sizeof(*frame), emela_scope_step);
  frame->base.drop = emela_scope_drop;
  frame->scope = scope;
  return scope;
}
static void emela_scope_check(emela_scope *scope) {
  if (scope == NULL || scope->closed)
    emela_board_defect(EMELA_DEFECT_PANIC, "fiber scope was used after it ended", UINT16_C(34));
}
static inline emela_fiber *emela_child_begin(emela_scope *scope) {
  emela_scope_check(scope);
  EmelaFiber handle = emela_fiber_create();
  emela_fiber *fiber = emela_fiber_get(handle);
  emela_child *child = (emela_child *)emela_heap_allocate(sizeof(emela_child), EMELA_CHILD_KIND);
  *child = (emela_child){.scope = scope, .next = scope->children, .execution = handle};
  emela_heap_retain(scope);
  emela_heap_retain(child); /* The execution context and scope each hold one reference. */
  scope->children = child;
  scope->running++;
  fiber->child = child;
  return fiber;
}
static inline EmelaAny emela_deferred_create(emela_scope *scope) {
  emela_scope_check(scope);
  emela_deferred *deferred = (emela_deferred *)emela_heap_allocate(sizeof(*deferred), EMELA_DEFERRED_KIND);
  deferred->scope = scope;
  emela_heap_retain(scope);
  return (EmelaAny){.payload.any_box = deferred};
}
