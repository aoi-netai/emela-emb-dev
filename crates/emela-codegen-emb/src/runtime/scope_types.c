typedef struct emela_child emela_child;
typedef struct emela_scope emela_scope;
typedef struct emela_deferred emela_deferred;
struct emela_scope {
  emela_child *children;
  EmelaFiber closer;
  uint32_t running;
  int closed;
  int ended;
#ifdef EMELA_DEFERRED_CAPACITY
  emela_deferred *deferreds; /* Non-owning: a live Deferred retains its scope. */
#endif
};
struct emela_child {
  emela_scope *scope;
  emela_child *next; /* Non-owning link; the scope holds one reference per child. */
  EmelaFiber execution;
  EmelaAny result;
  int status;
};
/* The constructor is part of Scope's generated handler, even if never called. */
struct emela_deferred {
  emela_scope *scope;
#ifdef EMELA_DEFERRED_CAPACITY
  emela_deferred *previous;
  emela_deferred *next;
  uint32_t slot;
  uint32_t generation;
#endif
};
