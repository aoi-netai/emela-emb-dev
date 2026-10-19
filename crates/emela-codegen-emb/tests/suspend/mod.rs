use super::*;
#[path = "../../examples/support/suspend_flow.rs"]
mod suspend_flow;

#[test]
fn suspend_recursion_resumes_into_caller_storage_and_reuses_frames() {
    let options = Options::default()
        .with_fiber_capacity(2)
        .with_frame_capacity(64)
        .with_frame_bytes(128);
    let output = compile_flow(&suspend_flow::recursive_flow(), &options)
        .expect("emit recursive state machine");
    let source = &output
        .files
        .iter()
        .find(|f| f.path == "emela_embedded.c")
        .unwrap()
        .contents;
    assert!(!source.contains("emela_heap_storage"));
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) {
  (void)kind; (void)message; (void)length; abort();
}
int main(void) {
  for (unsigned repeat = 0; repeat < 1000; repeat++) {
    int32_t a = -1, b = -1;
    EmelaFiber x = emela_export_descend_0_start(30, &a);
    EmelaFiber y = emela_export_descend_0_start(4, &b);
    assert(emela_scheduler_run(0) == 0 && a == -1 && b == -1);
    assert(emela_scheduler_run(1) == 1 && a == -1 && b == -1);
    unsigned steps = 1;
    while (!emela_fiber_done(x) || !emela_fiber_done(y)) {
      steps += emela_scheduler_run(3);
      assert(steps <= 100);
    }
    assert(a == 465 && b == 10);
    emela_fiber_release(x); emela_fiber_release(y);
  }
  return 0;
}
"#,
    );
}

#[test]
fn suspend_loop_preserves_parallel_recur_and_block_result() {
    let mut flow = Module::new(emela_core::Module::new());
    let identity = {
        let mut b = FnBuilder::new(&flow.core, "identity");
        b.set_suspends().set_ret(TypeId::INT);
        let x = b.param("x", TypeId::INT);
        b.term(Term::Return(Some(x.into())));
        b.finish()
    };
    let identity = flow.add(identity);
    let mut b = FnBuilder::new(&flow.core, "rotate");
    b.set_exported().set_suspends().set_ret(TypeId::INT);
    let n = b.param("n", TypeId::INT);
    let a = b.var("a", TypeId::INT);
    let c = b.var("c", TypeId::INT);
    let left = b.var("left", TypeId::INT);
    let result = b.var("result", TypeId::INT);
    b.block(Some(result), "exit", |b, exit| {
        b.loop_(
            None,
            vec![
                (a, Const::Int(10).into()),
                (c, Const::Int(20).into()),
                (left, n.into()),
            ],
            |b| {
                let x = b.assign(
                    "x",
                    TypeId::INT,
                    Value::Call {
                        callee: Callee::Direct(identity),
                        args: vec![a.into()],
                        suspends: true,
                        on_fail: None,
                    },
                );
                let done = b.assign(
                    "done",
                    TypeId::BOOL,
                    Value::Prim {
                        op: PrimOp::Eq,
                        ty: PrimTy::I32,
                        args: vec![left.into(), Const::Int(0).into()],
                    },
                );
                b.if_(
                    None,
                    done,
                    |b| {
                        b.term(Term::Break {
                            label: exit,
                            value: Some(x.into()),
                        })
                    },
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
                b.term(Term::Recur(vec![c.into(), a.into(), less.into()]));
            },
        );
        b.term(Term::Unreachable);
    });
    b.term(Term::Return(Some(result.into())));
    let function = b.finish();
    flow.add(function);
    let output = compile_flow(&flow, &Options::default()).expect("emit suspend loop");
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) {
  (void)kind; (void)message; (void)length; abort();
}
int main(void) {
  for (int32_t n = 0; n < 100; n++) {
    int32_t result = 0;
    EmelaFiber f = emela_export_rotate_1_start(n, &result);
    while (!emela_fiber_done(f)) assert(emela_scheduler_run(1) == 1);
    assert(result == (n % 2 ? 20 : 10));
    emela_fiber_release(f);
  }
  return 0;
}
"#,
    );
}

#[test]
fn synchronous_program_omits_scheduler_storage_and_zero_suspend_pool_is_diagnosed() {
    let output = compile_constant_function("answer", TypeId::INT, Const::Int(42));
    assert!(
        output
            .files
            .iter()
            .all(|f| !f.contents.contains("emela_frames") && !f.contents.contains("EmelaFiber"))
    );
    let errors = compile_flow(
        &suspend_flow::recursive_flow(),
        &Options::default().with_frame_capacity(0),
    )
    .unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.kind == ErrorKind::InvalidOptions)
    );
}

#[test]
fn suspend_frame_exhaustion_is_a_defect_and_isr_entry_is_rejected() {
    let flow = suspend_flow::recursive_flow();
    let errors = compile_flow(&flow, &Options::default().with_isr_entry("descend")).unwrap_err();
    assert!(errors.iter().any(|error| error.message.contains("ISR")));
    let output = compile_flow(&flow, &Options::default().with_frame_capacity(2)).unwrap();
    compile_and_run(
        output,
        r#"
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) {
  exit(kind == EMELA_DEFECT_HEAP_EXHAUSTED && length == 30 &&
       memcmp(message, "fixed scheduler pool exhausted", length) == 0 ? 0 : 2);
}
int main(void) {
  int32_t result = 0;
  (void)emela_export_descend_0_start(3, &result);
  (void)emela_scheduler_run(10);
  return 1;
}
"#,
    );
}
