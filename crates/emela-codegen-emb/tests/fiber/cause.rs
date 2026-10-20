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
