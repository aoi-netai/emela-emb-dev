use super::*;

#[test]
fn cause_kind_reads_failure_without_consuming_the_cause() {
    let source = r#"
import Std.Fiber.{Fiber, Success, Failure}
import Std.Cause.{Cause, CauseKind, Fail, Die, Interrupt}
error Boom(n: Int)
pub fn kind(n: Int) -> Int {
 Fiber.attempt(fn() { fail Boom(n: n) }) match {
  Success(_) -> 0
  Failure(c) -> {
   Cause.kind(c)
   Cause.kind(c) match { Fail -> 42
    Die -> 1
    Interrupt -> 2
   }
  }
 }
}
"#;
    let core = support::try_lower(&[("main.emel", source)], Roots::Exported).unwrap();
    compile_and_run(
        compile(&core, &options().with_heap_bytes(1024)).unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *m,uint16_t n) { (void)k;(void)m;(void)n;abort(); }
int main(void) { for(int i=0;i<1000;i++) assert(emela_export_kind_0(i)==42); }
"#,
    );
}

#[test]
fn cause_to_string_uses_error_show_and_caller_output_buffer() {
    let source = r#"
import Std.Fiber.{Fiber, Success, Failure}
import Std.Cause.{Cause}
error Boom(n: Int)
pub fn make(n: Int) -> Cause[Boom] {
 Fiber.attempt(fn() { fail Boom(n: n) }) match { Success(_) -> panic("unexpected")
  Failure(c) -> c
 }
}
pub fn show(c: Cause[Boom]) -> String { Cause.to_string(c) }
"#;
    let core = support::try_lower(&[("main.emel", source)], Roots::Exported).unwrap();
    compile_and_run(
        compile(&core, &options().with_heap_bytes(2048)).unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *m,uint16_t n) { fprintf(stderr,"%d: %.*s\n",k,n,m);abort(); }
int main(void) {
 for(int i=0;i<1000;i++) {
  void *cause=emela_export_make_0(-2147483647-1);
  char output[256];
  EmelaString s=emela_export_show_1(cause,output,256);
  const char *expected="error: Boom(n: -2147483648)";
  assert(s.data==output && s.length==strlen(expected) && memcmp(s.data,expected,s.length)==0);
  emela_embedded_release(cause);
 }
}
"#,
    );
}

#[test]
fn cause_raise_preserves_the_error_and_runs_native_cleanup_before_escape() {
    let source = r#"
import Std.Fiber.{Fiber, Success, Failure}
import Std.Cause.{Cause}
@external(c, "mark")
fn mark(n: Int) -> () use {}
error Boom(n: Int)
effect Resource { fn read() -> Int }
handler Res implements Resource { fn read() { 0 }
 release { mark(1) }
}
fn retry(c: Cause[Boom]) -> Int fails Boom { with Res { Cause.raise(c) } }
pub fn rethrow(n: Int) -> Int {
 Fiber.attempt(fn() { fail Boom(n: n) }) match {
  Success(_) -> 0
  Failure(c) -> retry(c) escape { Boom(n) -> n }
 }
}
"#;
    let core = support::try_lower(&[("main.emel", source)], Roots::Exported).unwrap();
    compile_and_run(
        compile(&core, &options().with_heap_bytes(1024)).unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
static int releases;
void mark(int32_t n) { assert(n==1);releases++; }
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *m,uint16_t n) { (void)k;(void)m;(void)n;abort(); }
int main(void) { for(int i=0;i<1000;i++) { assert(emela_export_rethrow_0(42)==42);assert(releases==i+1); } }
"#,
    );
}

#[test]
fn raised_interruption_crosses_sync_calls_and_attempt_without_becoming_a_value() {
    let source = r#"
import Std.Fiber.{Fiber, Success, Failure}
import Std.Cause.{Cause, Fail, Die, Interrupt}
@external(c, "host_wait")
suspend fn wait(n: Int) -> Int
@external(c, "mark")
fn mark(n: Int) -> () use {}
error Boom
effect Wait { suspend fn get(n: Int) -> Int }
handler Waiting implements Wait { suspend fn get(n,resume) { resume(wait(n)) } }
effect Resource { fn read() -> Int }
handler Inner implements Resource { fn read() { 0 }
 release { mark(1) }
}
handler Outer implements Resource { fn read() { 0 }
 release { mark(2) }
}
fn bounce(c: Cause[Boom]) -> Int fails Boom { with Inner { Cause.raise(c) } }
pub fn make() -> Cause[{}] use Async {
 with Waiting {
  Fiber.scope(fn() {
   first = Fiber.fork(fn() { (use Wait).get(1) })
   (use Wait).get(2)
   Fiber.interrupt(first)
   Fiber.await(first) match {
    Success(_) -> panic("unexpected")
    Failure(c) -> c
   }
  })
 }
}
pub fn raised(c: Cause[Boom], asynchronous: Bool) -> Int use Async {
 with Waiting { Fiber.scope(fn() {
     second = Fiber.fork(fn() { with Outer {
      if asynchronous { Fiber.attempt(fn() { (use Wait).get(3)
       bounce(c)
      }) } else { Fiber.attempt(fn() { bounce(c) }) }
      mark(99)
      0
     } })
     Fiber.await(second) match {
      Success(_) -> -1
      Failure(again) -> {
       mark(3)
       Cause.kind(again) match { Interrupt -> 42
        Fail -> -2
        Die -> -3
       }
      }
     }
 }) }
}
pub fn show(c: Cause[Boom]) -> String { Cause.to_string(c) }
"#;
    let core = support::try_lower(&[("main.emel", source)], Roots::Exported).unwrap();
    compile_and_run(
        compile(
            &core,
            &options().with_heap_bytes(8192).with_fiber_capacity(10),
        )
        .unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
static EmelaTask tasks[3];static int waits, marks;
uint32_t emela_board_event_lock(void) { return 0; }
void emela_board_event_unlock(uint32_t p) { (void)p; }
void board_wait_start(EmelaTask t,int32_t n) { assert(n>=1 && n<=3);tasks[n-1]=t;waits++; }
void mark(int32_t n) { assert(n==++marks); }
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *m,uint16_t n) { fprintf(stderr,"%d: %.*s\n",k,n,m);abort(); }
int main(void) {
 for(int i=0;i<1000;i++) {
  waits=marks=0;int32_t result=-1;
  void *cause=NULL;
  EmelaFiber first=emela_export_make_0_start(&cause);
  while(emela_scheduler_run(1)) {}
  assert(waits==2);assert(emela_task_complete_int(tasks[1],0));
  while(!emela_fiber_done(first)) assert(emela_scheduler_run(1));
  emela_fiber_release(first);
  char output[32];EmelaString text=emela_export_show_2(cause,output,32);
  assert(text.length==11 && memcmp(text.data,"interrupted",11)==0);
  EmelaFiber f=emela_export_raised_1_start(cause,i%2,&result);
  while(emela_scheduler_run(1)) {}
  if(i%2) { assert(marks==0 && !emela_fiber_done(f));assert(emela_task_complete_int(tasks[2],0)); }
  while(!emela_fiber_done(f)) assert(emela_scheduler_run(1));
  assert(result==42 && marks==3 && !emela_task_complete_int(tasks[0],0));
  emela_fiber_release(f);emela_embedded_release(cause);
 }
}
"#,
    );
}
