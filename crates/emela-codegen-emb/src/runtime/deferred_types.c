typedef struct emela_deferred_waiter emela_deferred_waiter;
typedef struct {
  EmelaAny value;
  emela_deferred_waiter *head;
  emela_deferred_waiter *tail;
  uint32_t generation;
  uint32_t next;
  int full;
} emela_deferred_slot;
static emela_deferred_slot emela_deferred_slots[EMELA_DEFERRED_CAPACITY];
static uint32_t emela_deferred_free;
static int emela_deferred_initialized;
