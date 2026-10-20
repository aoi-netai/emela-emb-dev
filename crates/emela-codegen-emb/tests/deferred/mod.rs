use super::*;

fn options() -> Options {
    Options::default()
        .with_heap_bytes(8192)
        .with_fiber_capacity(8)
        .with_frame_capacity(24)
        .with_frame_bytes(512)
        .with_deferred_capacity(2)
}

#[test]
fn deferred_constructor_reuses_slots_and_scope_only_programs_omit_the_pool() {
    let core = support::try_lower(
        &[(
            "main.emel",
            include_str!("../../examples/support/deferred.emel"),
        )],
        Roots::Exported,
    )
    .unwrap();
    let output = compile(&core, &options().with_deferred_capacity(1)).unwrap();
    assert!(output.files.iter().any(|f| f.path == "emela_embedded.c"
        && f.contents.contains("#define EMELA_DEFERRED_CAPACITY 1")));
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { (void)k; (void)m; (void)n; abort(); }
int main(void) {
  for (int i = 0; i < 1000; i++) {
    EmelaFiber f = emela_export_work_0_start();
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    emela_fiber_release(f);
  }
}
"#,
    );
    assert!(
        compile(&core, &options().with_deferred_capacity(0))
            .unwrap_err()
            .iter()
            .any(|e| e.kind == ErrorKind::InvalidOptions)
    );
    let scope = support::try_lower(&[("main.emel", "import Std.Fiber.{Fiber}\npub fn work() -> Int use Async { Fiber.scope(fn() { 1 }) }\n")], Roots::Exported).unwrap();
    let output = compile(&scope, &options().with_deferred_capacity(0)).unwrap();
    assert!(
        !output.files.iter().any(|f| f.path == "emela_embedded.c"
            && f.contents.contains("#define EMELA_DEFERRED_CAPACITY"))
    );
}

#[test]
fn discarded_deferreds_reuse_one_slot_within_a_long_lived_scope() {
    let mut flow = Module::new(emela_core::Module::new());
    let ft = flow.core.types.intern(Type::Fn(FnType {
        suspends: true,
        ..FnType::pure(vec![TypeId::FIBER_SCOPE], TypeId::UNIT)
    }));
    let mut b = FnBuilder::new(&flow.core, "body");
    b.set_suspends();
    let scope = b.param("scope", TypeId::FIBER_SCOPE);
    let left = b.var("left", TypeId::INT);
    b.loop_(None, vec![(left, Const::Int(1000).into())], |b| {
        b.assign(
            "d",
            TypeId::ANY,
            Value::Fiber {
                op: emela_core::FiberOp::Deferred,
                args: vec![scope.into()],
                on_fail: None,
            },
        );
        let zero = b.assign(
            "zero",
            TypeId::BOOL,
            Value::Prim {
                op: PrimOp::Eq,
                ty: PrimTy::I32,
                args: vec![left.into(), Const::Int(0).into()],
            },
        );
        b.if_(
            None,
            zero,
            |b| b.term(Term::Return(None)),
            |b| b.term(Term::Yield(None)),
        );
        let less = b.assign(
            "less",
            TypeId::INT,
            Value::Prim {
                op: PrimOp::Sub,
                ty: PrimTy::I32,
                args: vec![left.into(), Const::Int(1).into()],
            },
        );
        b.term(Term::Recur(vec![less.into()]));
    });
    b.term(Term::Return(None));
    let body = flow.add(b.finish());
    let mut b = FnBuilder::new(&flow.core, "work");
    b.set_exported().set_suspends();
    let body = b.assign("body", ft, Value::FnRef(body));
    b.let_(
        None,
        Value::Fiber {
            op: emela_core::FiberOp::Scope { fails: false },
            args: vec![body.into()],
            on_fail: None,
        },
    );
    b.term(Term::Return(None));
    flow.add(b.finish());
    compile_and_run(
        compile_flow(
            &flow,
            &options().with_deferred_capacity(1).with_heap_bytes(2048),
        )
        .unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { (void)k; (void)m; (void)n; abort(); }
int main(void) {
  for (int i = 0; i < 1000; i++) {
    EmelaFiber f = emela_export_work_1_start();
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    emela_fiber_release(f);
  }
}
"#,
    );
}

#[test]
fn complete_stores_once_and_scope_end_breaks_cycles_through_captured_deferreds() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
import Std.Fiber.{Fiber, Scope}
import Std.Deferred.{Deferred}
pub fn once(n: Int) -> Bool use Async {
  Fiber.scope(fn() {
    d: Deferred[Int] = (use Scope).deferred()
    first = Deferred.complete(d, n)
    second = Deferred.complete(d, n + 1)
    first && !second
  })
}
pub fn cycle() -> Bool use Async {
  Fiber.scope(fn() {
    d: Deferred[fn() -> Bool] = (use Scope).deferred()
    callback = fn() { Deferred.complete(d, fn() { False }) }
    Deferred.complete(d, callback)
  })
}
"#,
        )],
        Roots::Exported,
    )
    .unwrap();
    compile_and_run(
        compile(&core, &options().with_deferred_capacity(1)).unwrap(),
        r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { fprintf(stderr, "%d: %.*s\n", k, n, m); abort(); }
int main(void) {
  for (int i = 0; i < 1000; i++) {
    int32_t result = 0;
    EmelaFiber f = emela_export_once_0_start(i, &result);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(result == 1); emela_fiber_release(f);
    f = emela_export_cycle_1_start(&result);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    assert(result == 1); emela_fiber_release(f);
  }
}
"#,
    );
}

#[test]
fn leaked_complete_callback_checks_scope_before_recycled_slot() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
import Std.Fiber.{Fiber, Scope}
import Std.Deferred.{Deferred}
pub fn leak() -> (fn() -> Bool) use Async {
  Fiber.scope(fn() {
    d: Deferred[Int] = (use Scope).deferred()
    fn() { Deferred.complete(d, 42) }
  })
}
"#,
        )],
        Roots::Exported,
    )
    .unwrap();
    let flow = emela_codegen_emb::flow_from_core(&core).unwrap();
    let ty = flow
        .functions
        .iter()
        .find(|(_, f)| f.name == "leak")
        .unwrap()
        .1
        .ret
        .index();
    compile_and_run(
        compile(&core, &options().with_deferred_capacity(1)).unwrap(),
        &format!(
            r#"
#include <assert.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
static int expected;
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) {{
  assert(expected && k == EMELA_DEFECT_PANIC && n == 44);
  assert(memcmp(m, "deferred was completed after its scope ended", 44) == 0); exit(0);
}}
int main(void) {{
  emela_function_type_{ty} old = {{0}};
  EmelaFiber f = emela_export_leak_0_start(&old);
  while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
  emela_fiber_release(f);
  for (int i = 0; i < 1000; i++) {{
    emela_function_type_{ty} next = {{0}};
    f = emela_export_leak_0_start(&next);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1));
    emela_fiber_release(f); emela_embedded_release_function_{ty}(&next);
  }}
  expected = 1; (void)old.call(old.env); abort();
}}
"#
        ),
    );
}
