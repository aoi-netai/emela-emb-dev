#include <stddef.h>
#include <stdint.h>
#include <string.h>
#include "runtime_probe.h"
#include "emela_board.h"
#include "validation_stack.h"

void _init(void) {}
void _fini(void) {}

volatile uint32_t emela_runtime_iterations;
volatile uint32_t emela_runtime_cleanup_iterations;
volatile uint32_t emela_runtime_failures;
volatile uint32_t emela_runtime_failure_line;
volatile uint32_t emela_runtime_notifications;
volatile uint32_t emela_runtime_isr_calls;
volatile uint32_t emela_runtime_rejected_notifications;
volatile uint32_t emela_runtime_main_callbacks;
volatile uint32_t emela_runtime_error_produced;
volatile uint32_t emela_runtime_error_released;
volatile uint32_t emela_runtime_success_produced;
volatile uint32_t emela_runtime_success_released;
volatile uint32_t emela_runtime_board_rejected_leases;
volatile uint32_t emela_runtime_cancelled_cleanups;
volatile uint32_t emela_runtime_stopped_pending;
volatile uint32_t emela_runtime_old_generation_checks;
volatile uint32_t emela_runtime_heap_after;
volatile uint32_t emela_runtime_pools_clean;
volatile uint32_t emela_runtime_sanity_checks;
volatile float emela_runtime_fp_main;
volatile float emela_runtime_fp_irq;
static unsigned locked;
static EmelaTask request_task, cleanup_tasks[3];
static unsigned requests, expected_request, cleanup_calls, cleanup_order;
static unsigned produced, released, success_produced, success_released;
typedef struct { char message[3]; void *parts; } Context;
static Context context;
static volatile unsigned irq_done;
static unsigned irq_kind;
static EmelaTask irq_task;
static int irq_accepted;
static int32_t irq_value;

static uint32_t ipsr(void) { uint32_t v; __asm volatile("mrs %0, ipsr" : "=r"(v)); return v; }
static uint32_t primask(void) { uint32_t v; __asm volatile("mrs %0, primask" : "=r"(v)); return v; }
static void signal(unsigned pin) { *(volatile uint32_t *)UINT32_C(0x40020018) = UINT32_C(1) << pin; }
static _Noreturn void stop(void) { *(volatile uint32_t *)UINT32_C(0xe000e010) = 0; for (;;) __asm volatile("wfi"); }
static _Noreturn void fail(unsigned line) {
  emela_runtime_failures++; emela_runtime_failure_line = line; signal(8);
  if (ipsr() == 0) emela_validation_stack_sample();
  stop();
}
#define CHECK(condition) do { if (!(condition)) fail(__LINE__); } while (0)
static void main_callback(void) {
  CHECK(ipsr() == 0 && primask() == 0 && locked == 0);
  emela_runtime_main_callbacks++;
}
static void board_init(void) {
  volatile uint32_t *const rcc = (volatile uint32_t *)UINT32_C(0x40023830);
  volatile uint32_t *const mode = (volatile uint32_t *)UINT32_C(0x40020000);
  *rcc |= UINT32_C(1); (void)*rcc;
  *mode = (*mode & ~((UINT32_C(3) << 10) | (UINT32_C(3) << 12) | (UINT32_C(3) << 14) | (UINT32_C(3) << 16)))
      | (UINT32_C(1) << 10) | (UINT32_C(1) << 12) | (UINT32_C(1) << 14) | (UINT32_C(1) << 16);
  /* Enable CP10/CP11 before observing exception preservation of FP registers. */
  *(volatile uint32_t *)UINT32_C(0xe000ed88) |= UINT32_C(0xf) << 20;
  __asm volatile("dsb\nisb" ::: "memory");
  emela_runtime_fp_main = 1.25f;
}
uint32_t emela_board_event_lock(void) {
  uint32_t state = primask();
  __asm volatile("cpsid i" ::: "memory");
  CHECK(locked == 0); locked = 1;
  return state;
}
void emela_board_event_unlock(uint32_t state) {
  CHECK(locked == 1 && primask() == 1); locked = 0;
  __asm volatile("msr primask, %0" :: "r"(state) : "memory");
}
void board_request_start(EmelaTask task, int32_t n) {
  main_callback(); CHECK(n == (int32_t)expected_request--); request_task = task; requests++;
}
void board_wait_start(EmelaTask task, int32_t n) {
  main_callback(); CHECK(n >= 1 && n <= 3); cleanup_tasks[n - 1] = task; cleanup_calls++;
}
void runtime_mark(int32_t n) { main_callback(); CHECK(n == (int32_t)++cleanup_order); }
static void produce_failure(void *p, EmelaError *error) {
  Context *value = p; main_callback(); produced++; emela_runtime_error_produced++;
  emela_runtime_error(error, value->message, value->parts);
}
static void release_failure(void *p) {
  Context *value = p; main_callback(); released++; emela_runtime_error_released++;
  memset(value->message, 'x', 3); emela_embedded_release(value->parts); value->parts = NULL;
}
#if EMELA_RUNTIME_MANAGED
static void produce_success(void *p, EmelaString *value) {
  Context *source = p; main_callback(); success_produced++; emela_runtime_success_produced++;
  *value = (EmelaString){.data = source->message, .length = 2};
}
static void release_success(void *p) {
  Context *value = p; main_callback(); success_released++; emela_runtime_success_released++;
  memset(value->message, 'x', 3);
}
#endif
void SysTick_Handler(void) {
  emela_validation_stack_record_sp();
  CHECK(ipsr() == 15 && primask() == 0 && locked == 0);
  *(volatile uint32_t *)UINT32_C(0xe000e010) = 0;
  emela_runtime_isr_calls++;
  /* Force lazy FP stacking, then record the actual exception MSP. */
  emela_runtime_fp_irq = emela_runtime_fp_main + 0.5f;
  emela_validation_stack_record_sp();
  if (irq_kind == 1) irq_accepted = emela_task_fail(irq_task, produce_failure, &context, release_failure);
#if EMELA_RUNTIME_MANAGED
  else if (irq_kind == 2) irq_accepted = emela_task_complete_value_4(irq_task, produce_success, &context, release_success);
#endif
  else irq_accepted = emela_task_complete_int(irq_task, irq_value);
  if (irq_accepted) emela_runtime_notifications++;
  else emela_runtime_rejected_notifications++;
  irq_done = 1;
}
static void notify(EmelaTask task, unsigned kind, int32_t value, int accepted) {
  CHECK(ipsr() == 0 && primask() == 0 && locked == 0);
  irq_task = task; irq_kind = kind; irq_value = value; irq_done = 0;
  unsigned before = produced + released + success_produced + success_released;
  *(volatile uint32_t *)UINT32_C(0xe000e014) = 127;
  *(volatile uint32_t *)UINT32_C(0xe000e018) = 0;
  *(volatile uint32_t *)UINT32_C(0xe000e010) = 7;
  for (unsigned spin = 0; !irq_done; spin++) { CHECK(spin < 1000000); __asm volatile("nop"); }
  CHECK(irq_accepted == accepted && emela_runtime_fp_irq == 1.75f && emela_runtime_fp_main == 1.25f);
  CHECK(before == produced + released + success_produced + success_released);
}
static void drain(void) {
  CHECK(ipsr() == 0 && primask() == 0);
  while (emela_scheduler_run(1)) {
    emela_runtime_sample(); CHECK(emela_runtime_children() <= 3);
  }
  emela_runtime_sample();
}
static void check_empty(void) {
  emela_runtime_heap_after = (uint32_t)emela_runtime_heap();
  emela_runtime_pools_clean = (uint32_t)emela_runtime_pools_empty();
  CHECK(emela_runtime_heap_after == 0 && emela_runtime_pools_clean && emela_runtime_children() == 0);
}
static void prepare_failure(void) {
  context = (Context){.message = {'a', '\0', 'b'}, .parts = emela_runtime_parts(42)};
  CHECK(emela_runtime_anchor());
}
static void sanity(void) {
  union { uint64_t bits; double number; } input, result;
  static const uint64_t samples[] = {UINT64_C(0x8000000000000000), UINT64_C(0xbeb4b66dc01ec6fb), UINT64_C(1), UINT64_C(0x7ff0000000000000)};
  for (unsigned i = 0; i < sizeof samples / sizeof samples[0]; i++) {
    input.bits = samples[i]; result.number = emela_runtime_float_roundtrip(input.number);
    /* Formatting intentionally canonicalizes both signed zeros to 0.0. */
    CHECK(result.bits == (i == 0 ? 0 : input.bits)); emela_runtime_sanity_checks++;
  }
  static const char unicode[] = "a\xcc\x88" "\xf0\x9f\x87\xba\xf0\x9f\x87\xb8" "\xf0\x9f\x91\xa9\xe2\x80\x8d\xf0\x9f\x91\xa9\xe2\x80\x8d\xf0\x9f\x91\xa7\xe2\x80\x8d\xf0\x9f\x91\xa6";
  CHECK(emela_runtime_unicode_signature((EmelaString){.data = unicode, .length = sizeof unicode - 1}) == 303);
  CHECK(emela_runtime_heap() == 0); emela_runtime_sanity_checks++;
}
int main(void) {
  board_init(); sanity();
  EmelaTask old = {UINT32_MAX, 0};
  for (unsigned i = 0; i < 1000; i++) {
    unsigned mode = i % 4, count = i == 0 ? 1000 : 3, discarded = i % 5 == 4;
    int32_t result = -1; requests = produced = released = success_produced = success_released = 0; expected_request = count;
    EmelaFiber fiber = emela_runtime_work_start((int32_t)count, discarded ? NULL : &result);
    emela_runtime_sample(); drain();
    if (i) {
      CHECK(old.generation != request_task.generation || old.slot != request_task.slot);
      if (old.slot == request_task.slot) emela_runtime_old_generation_checks++;
      prepare_failure(); notify(old, 1, 0, 0);
      CHECK(produced == 0 && released == 0 && context.parts != NULL);
      release_failure(&context); emela_runtime_board_rejected_leases++;
      produced = released = 0;
    }
    for (unsigned n = 0; n < count; n++) {
      CHECK(!emela_fiber_done(fiber) && requests == n + 1 && emela_runtime_children() <= 2);
#if EMELA_RUNTIME_MANAGED
      if (mode == 3) {
        context = (Context){.message = {'4', '2', '!'}}; CHECK(emela_runtime_anchor());
        notify(request_task, 2, 0, 1); emela_runtime_cancel(fiber);
        CHECK(emela_scheduler_run(0) == 0 && success_produced == 0 && success_released == 0);
        drain(); CHECK(emela_fiber_done(fiber) && success_produced == 0 && success_released == 1);
        CHECK(memcmp(context.message, "xxx", 3) == 0); break;
      }
#endif
      if (((mode == 1 || mode == 3) && n == 0) || (mode == 2 && n == count - 1)) {
        prepare_failure(); notify(request_task, 1, 0, 1);
        if (mode == 3) { emela_runtime_cancel(fiber); CHECK(emela_scheduler_run(0) == 0 && produced == 0 && released == 0); }
        drain(); CHECK(emela_fiber_done(fiber) && produced == (mode != 3) && released == 1 && context.parts == NULL);
        CHECK(memcmp(context.message, "xxx", 3) == 0); break;
      }
#if EMELA_RUNTIME_MANAGED
      context = (Context){.message = {'4', '2', '!'}}; CHECK(emela_runtime_anchor());
      unsigned produced_before = success_produced, released_before = success_released;
      notify(request_task, 2, 0, 1); drain();
      CHECK(success_produced == produced_before + 1 && success_released == released_before + 1);
      CHECK(memcmp(context.message, "xxx", 3) == 0);
#else
      notify(request_task, 0, 42, 1); drain();
#endif
    }
    CHECK(emela_fiber_done(fiber)); emela_fiber_release(fiber);
    CHECK((!discarded && mode != 3) ? result == (int32_t)(mode == 0 ? 2 * count + 2 : mode == 1 ? 11 : 6) : result == -1);
#if EMELA_RUNTIME_MANAGED
    CHECK(success_produced == (mode == 0 ? count : mode == 2 ? count - 1 : 0));
    CHECK(success_released == success_produced + (mode == 3));
#endif
    check_empty(); emela_runtime_iterations = i + 1;
    cleanup_calls = cleanup_order = 0; result = -1;
    fiber = emela_runtime_cleanup_start(&result); drain();
    CHECK(cleanup_calls == 2 && cleanup_order == 1);
    /* Scope completion interrupts the child. The board stops its pending device explicitly. */
    notify(cleanup_tasks[1], 0, 42, 1); drain();
    CHECK(cleanup_calls == 3 && cleanup_order == 2 && !emela_fiber_done(fiber) && result == -1);
    emela_runtime_stopped_pending++; notify(cleanup_tasks[0], 0, 0, 0);
    notify(cleanup_tasks[2], 0, 3, 1); drain();
    CHECK(emela_fiber_done(fiber) && result == 42 && cleanup_order == 4);
    old = cleanup_tasks[2]; emela_fiber_release(fiber); check_empty();
    emela_runtime_cancelled_cleanups++; emela_runtime_cleanup_iterations = i + 1;
  }
  CHECK(emela_runtime_old_generation_checks > 0);
  signal(5); signal(6); signal(7);
  *(volatile uint32_t *)UINT32_C(0xe000e010) = 0;
  *(volatile uint32_t *)UINT32_C(0xe000ed04) = UINT32_C(1) << 25;
  __asm volatile("dsb\nisb" ::: "memory");
  emela_validation_stack_sample(); stop();
}
_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) {
  (void)kind; (void)message; (void)length; fail(__LINE__);
}
