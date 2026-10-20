typedef struct emela_child emela_child;
typedef struct emela_scope emela_scope;
struct emela_scope {
  emela_child *children;
  EmelaFiber closer;
  uint32_t running;
  int closed;
  int ended;
};
struct emela_child {
  emela_scope *scope;
  emela_child *next; /* Non-owning link; the scope holds one reference per child. */
  EmelaFiber execution;
  EmelaAny result;
  int status;
};
/* The constructor is part of Scope's generated handler, even if never called. */
typedef struct { emela_scope *scope; } emela_deferred;
