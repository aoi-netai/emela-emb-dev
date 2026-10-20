use super::*;

#[test]
fn suspend_failures_cross_calls_and_preserve_cleanup_before_escape() {
    let source = r#"
@external(c, "host_wait")
suspend fn wait(n: Int) -> Int
@external(c, "mark")
fn mark(n: Int) -> () use {}
error Boom(n: Int)
effect Wait { suspend fn get(n: Int) -> Int }
handler Waiting implements Wait {
  suspend fn get(n, resume) { resume(wait(n)) }
  release { mark(2) }
}
fn failure(n: Int) -> Int fails Boom { fail Boom(n: n) }
fn work(n: Int) -> Int fails Boom use Async {
  with Waiting {
    x = (use Wait).get(n)
    mark(1)
    failure(x)
  }
}
pub fn caught(n: Int) -> Int use Async {
  work(n) escape { Boom(n) -> {
    mark(3)
    n
  } }
}
"#;
    let core = support::try_lower(&[("main.emel", source)], Roots::Exported).unwrap();
    compile_and_run(
        compile(&core, &options().with_heap_bytes(4096)).unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include "emela_embedded.h"
#include "emela_board.h"
static EmelaTask task;
static int order;
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t p) { (void)p; }
void board_wait_start(EmelaTask t, int32_t n) { assert(n == 41); task = t; }
void mark(int32_t n) { assert(n == ++order); }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { fprintf(stderr, "%d: %.*s\n", k,n,m); abort(); }
int main(void) {
  for (int i=0; i<1000; i++) {
    int32_t result=-1; order=0;
    EmelaFiber f=emela_export_caught_0_start(41, &result);
    while (emela_scheduler_run(1)) {}
    assert(!emela_fiber_done(f) && order==0);
    assert(emela_task_complete_int(task, 42));
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(result==42 && order==3);
    emela_fiber_release(f);
  }
}
"#,
    );
}

#[test]
fn failing_suspend_closure_keeps_error_through_suspending_release() {
    let source = r#"
@external(c, "host_wait")
suspend fn wait(n: Int) -> Int
@external(c, "mark")
fn mark(n: Int) -> () use {}
error Boom(n: Int)
effect Wait { suspend fn get(n: Int) -> Int }
handler Waiting implements Wait { suspend fn get(n, resume) { resume(wait(n)) } }
effect Resource { fn read() -> Int }
handler Res implements Resource {
  fn read() { 0 }
  release { mark(2)
    (use Wait).get(2)
    mark(3)
  }
}
fn invoke(f: fn() -> Int fails Boom use Wait) -> Int fails Boom use Wait { f() }
pub fn caught(n: Int) -> Int use Async {
  with Waiting {
    invoke(fn() {
      with Res {
        (use Wait).get(1)
        mark(1)
        fail Boom(n: n)
      }
    }) escape { Boom(n) -> { mark(4)
      n
    } }
  }
}
"#;
    let core = support::try_lower(&[("main.emel", source)], Roots::Exported).unwrap();
    compile_and_run(
        compile(&core, &options().with_heap_bytes(4096)).unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include "emela_embedded.h"
#include "emela_board.h"
static EmelaTask task;
static int order, waits;
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t p) { (void)p; }
void board_wait_start(EmelaTask t, int32_t n) { assert(n==++waits); task=t; }
void mark(int32_t n) { assert(n==++order); }
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { fprintf(stderr,"%d: %.*s\n",k,n,m); abort(); }
int main(void) {
 for(int i=0;i<1000;i++) {
  order=waits=0; int32_t result=-1;
  EmelaFiber f=emela_export_caught_0_start(42,&result);
  while(emela_scheduler_run(1)) {}
  assert(waits==1 && order==0);
  assert(emela_task_complete_int(task,0));
  while(emela_scheduler_run(1)) {}
  assert(waits==2 && order==2 && result==-1 && !emela_fiber_done(f));
  assert(emela_task_complete_int(task,0));
  while(!emela_fiber_done(f)) assert(emela_scheduler_run(1));
  assert(order==4 && result==42); emela_fiber_release(f);
 }
}
"#,
    );
}

#[test]
fn uncaught_suspend_failure_stops_at_public_boundary_after_release() {
    let source = r#"
@external(c, "host_wait")
suspend fn wait(n: Int) -> Int
@external(c, "mark")
fn mark(n: Int) -> () use {}
error Boom
effect Wait { suspend fn get() -> Int }
handler Waiting implements Wait {
 suspend fn get(resume) { resume(wait(1)) }
 release { mark(1) }
}
pub fn failure() -> Int fails Boom use Async {
 with Waiting { (use Wait).get()
  fail Boom
 }
}
"#;
    let core = support::try_lower(&[("main.emel", source)], Roots::Exported).unwrap();
    compile_and_run(
        compile(&core, &options()).unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
static EmelaTask task;
static int released;
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t p) { (void)p; }
void board_wait_start(EmelaTask t,int32_t n) { (void)n;task=t; }
void mark(int32_t n) { assert(n==1); released++; }
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *m,uint16_t n) {
 assert(k==EMELA_DEFECT_UNHANDLED_ERROR && n==15 && memcmp(m,"unhandled error",15)==0 && released==1); exit(0);
}
int main(void) {
 EmelaFiber f=emela_export_failure_0_start(NULL);
 while(emela_scheduler_run(1)) {}
 assert(emela_task_complete_int(task,0));
 while(!emela_fiber_done(f)) assert(emela_scheduler_run(1));
 abort();
}
"#,
    );
}

#[test]
fn child_failure_closes_scope_and_waits_for_sibling_release_before_escape() {
    let source = r#"
import Std.Fiber.{Fiber}
@external(c, "host_wait")
suspend fn wait(n: Int) -> Int
@external(c, "mark")
fn mark(n: Int) -> () use {}
error Boom(n: Int)
effect Wait { suspend fn get(n: Int) -> Int }
handler Waiting implements Wait { suspend fn get(n,resume) { resume(wait(n)) } }
effect Resource { fn read() -> Int }
handler Res implements Resource {
 fn read() { 0 }
 release { mark(1)
  (use Wait).get(3)
  mark(2)
 }
}
pub fn caught(mode: Int) -> Int use Async {
 with Waiting {
  Fiber.scope(fn() {
   Fiber.fork(fn() { with Res { (use Wait).get(2) } })
   Fiber.fork(fn() { if mode == 0 { fail Boom(n: 42) } else { 0 } })
   (use Wait).get(1)
   if mode != 0 { fail Boom(n: 42) }
   mark(99)
   0
  }) escape { Boom(n) -> { mark(3)
   n
  } }
 }
}
"#;
    let core = support::try_lower(&[("main.emel", source)], Roots::Exported).unwrap();
    compile_and_run(
        compile(
            &core,
            &options()
                .with_heap_bytes(8192)
                .with_task_capacity(3)
                .with_event_capacity(3),
        )
        .unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include "emela_embedded.h"
#include "emela_board.h"
static EmelaTask tasks[3];
static int marks,waits;
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t p) { (void)p; }
void board_wait_start(EmelaTask t,int32_t n) { assert(n==++waits);tasks[n-1]=t; }
void mark(int32_t n) { assert(n==++marks); }
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *m,uint16_t n) { fprintf(stderr,"%d: %.*s\n",k,n,m);abort(); }
int main(void) {
 for(int i=0;i<1000;i++) {
  marks=waits=0;int32_t result=-1;
  EmelaFiber f=emela_export_caught_0_start(i%2, &result);
  while(emela_scheduler_run(1)) {}
  if(i%2) {
   assert(waits==2 && marks==0 && !emela_fiber_done(f));
   assert(emela_task_complete_int(tasks[0],0));
   while(emela_scheduler_run(1)) {}
  }
  assert(waits==3 && marks==1 && result==-1 && !emela_fiber_done(f));
  assert(!emela_task_complete_int(tasks[0],99));
  assert(!emela_task_complete_int(tasks[1],99));
  assert(emela_task_complete_int(tasks[2],0));
  while(!emela_fiber_done(f)) assert(emela_scheduler_run(1));
  assert(result==42 && marks==3);emela_fiber_release(f);
 }
}
"#,
    );
}

#[test]
fn suspend_operation_failure_before_resume_reaches_its_call_edge() {
    let source = r#"
@external(c, "host_wait")
suspend fn wait(n: Int) -> Int
error Boom(n: Int)
effect Wait { suspend fn get(n: Int) -> Int fails Boom }
handler Waiting implements Wait {
 suspend fn get(n,resume) {
  x = wait(n)
  if n == 0 { resume(x) } else { fail Boom(n: x) }
 }
}
pub fn caught(n: Int) -> Int use Async {
 with Waiting { (use Wait).get(n) escape { Boom(n) -> n } }
}
"#;
    let core = support::try_lower(&[("main.emel", source)], Roots::Exported).unwrap();
    compile_and_run(
        compile(&core, &options().with_heap_bytes(2048)).unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include "emela_embedded.h"
#include "emela_board.h"
static EmelaTask task;
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t p) { (void)p; }
void board_wait_start(EmelaTask t,int32_t n) { (void)n;task=t; }
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *m,uint16_t n) { fprintf(stderr,"%d: %.*s\n",k,n,m);abort(); }
int main(void) {
 for(int i=0;i<1000;i++) {
  int32_t result=-1;EmelaFiber f=emela_export_caught_0_start(i%2,&result);
  while(emela_scheduler_run(1)) {}
  assert(emela_task_complete_int(task,42));
  while(!emela_fiber_done(f)) assert(emela_scheduler_run(1));
  assert(result==42);emela_fiber_release(f);
 }
}
"#,
    );
}

#[test]
fn operation_failure_after_resume_stops_without_writing_a_stale_error_pointer() {
    let source = r#"
error Boom
effect Tick { suspend fn tick() -> Int fails Boom }
handler Ticking implements Tick {
 suspend fn tick(resume) { resume(42)
  fail Boom
 }
}
pub fn work() -> Int use Async {
 with Ticking { (use Tick).tick() escape { Boom -> 0 } }
}
"#;
    let core = support::try_lower(&[("main.emel", source)], Roots::Exported).unwrap();
    compile_and_run(
        compile(&core, &options()).unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *m,uint16_t n) {
 assert(k==EMELA_DEFECT_PANIC && n==29 && memcmp(m,"operation failed after resume",29)==0); exit(0);
}
int main(void) {
 EmelaFiber f=emela_export_work_0_start(NULL);
 while(!emela_fiber_done(f)) assert(emela_scheduler_run(1));
 abort();
}
"#,
    );
}
