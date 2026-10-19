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

fn task_options() -> Options {
    Options::default()
        .with_task_adapter("host_wait", "board_wait_start")
        .with_fiber_capacity(2)
        .with_frame_capacity(2)
        .with_frame_bytes(128)
        .with_task_capacity(2)
        .with_event_capacity(2)
}

#[test]
fn suspend_tasks_queue_completion_without_resuming_in_the_callback() {
    let output =
        compile_flow(&suspend_flow::task_flow(), &task_options()).expect("emit task calls");
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
static unsigned locked;
static EmelaTask requests[4];
static int32_t inputs[4];
static unsigned count;
static int immediate;
uint32_t emela_board_event_lock(void) { unsigned old = locked; locked = 1; return old; }
void emela_board_event_unlock(uint32_t old) { assert(locked); locked = old; }
_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) {
  (void)kind; (void)message; (void)length; abort();
}
void board_wait_start(EmelaTask task, int32_t n) {
  assert(!locked);
  if (immediate) { assert(emela_task_complete_int(task, n)); return; }
  assert(count < 4); requests[count] = task; inputs[count++] = n;
}
int main(void) {
  int32_t a = -1, b = -1;
  EmelaFiber x = emela_export_work_1_start(40, &a);
  EmelaFiber y = emela_export_work_1_start(10, &b);
  assert(emela_scheduler_run(2) == 2 && count == 2);
  assert(emela_scheduler_run(10) == 0 && a == -1 && b == -1);
  assert(!emela_task_complete_float(requests[1], 10.0));
  assert(emela_task_complete_int(requests[1], inputs[1]));
  assert(!emela_task_complete_int(requests[1], 999));
  assert(!locked && count == 2 && b == -1);
  assert(emela_scheduler_run(1) == 1 && count == 2);
  assert(emela_scheduler_run(1) == 1 && count == 3);
  assert(!emela_task_complete_int(requests[1], 999));
  assert(emela_task_complete_int(requests[2], inputs[2]));
  assert(emela_scheduler_run(2) == 2 && b == 21);
  assert(emela_fiber_done(y) && !emela_fiber_done(x));
  emela_fiber_release(y);
  assert(emela_task_complete_int(requests[0], inputs[0]));
  assert(emela_scheduler_run(2) == 2 && count == 4);
  assert(emela_task_complete_int(requests[3], inputs[3]));
  assert(emela_scheduler_run(2) == 2 && a == 81);
  emela_fiber_release(x);
  assert(!emela_task_complete_int((EmelaTask){UINT32_MAX, 0}, 1));
  immediate = 1;
  for (int32_t n = 0; n < 1000; n++) {
    int32_t result = -1;
    EmelaFiber f = emela_export_work_1_start(n, &result);
    unsigned steps = 0;
    while (!emela_fiber_done(f)) {
      assert(emela_scheduler_run(1) == 1);
      assert(++steps <= 5);
    }
    assert(result == n * 2 + 1 && !locked);
    emela_fiber_release(f);
  }
  return 0;
}
"#,
    );
}

#[test]
fn suspend_task_abi_requires_an_explicit_adapter_and_nonzero_pools() {
    let flow = suspend_flow::task_flow();
    let errors = compile_flow(&flow, &Options::default()).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("with_task_adapter"))
    );
    for options in [
        task_options().with_task_capacity(0),
        task_options().with_event_capacity(0),
    ] {
        assert!(
            compile_flow(&flow, &options)
                .unwrap_err()
                .iter()
                .any(|e| e.kind == ErrorKind::InvalidOptions)
        );
    }
}

#[test]
fn suspend_task_and_event_exhaustion_restore_the_board_lock_before_defect() {
    let flow = suspend_flow::task_flow();
    let harness = r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
static int locked;
static EmelaTask pending[2];
static unsigned count;
uint32_t emela_board_event_lock(void) { int old = locked; locked = 1; return old; }
void emela_board_event_unlock(uint32_t old) { assert(locked); locked = (int)old; }
_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) {
  (void)message; (void)length;
  exit(kind == EMELA_DEFECT_HEAP_EXHAUSTED && !locked ? 0 : 2);
}
void board_wait_start(EmelaTask task, int32_t input) { (void)input; pending[count++] = task; }
int main(void) {
  static int32_t a, b;
  (void)emela_export_work_1_start(1, &a);
  (void)emela_export_work_1_start(2, &b);
  (void)emela_scheduler_run(2);
  assert(count == 2);
  assert(emela_task_complete_int(pending[0], 1));
  (void)emela_task_complete_int(pending[1], 2);
  return 1;
}
"#;
    for options in [
        task_options().with_task_capacity(1),
        task_options().with_event_capacity(1),
    ] {
        compile_and_run(compile_flow(&flow, &options).unwrap(), harness);
    }
}

#[test]
fn suspend_handler_source_is_diagnosed_until_closure_resume_is_implemented() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
@external(c, "host_wait")
suspend fn wait(n: Int) -> Int
effect Wait { suspend fn get(n: Int) -> Int }
handler Waiting implements Wait {
  suspend fn get(n, resume) { resume(wait(n)) }
}
pub fn work(n: Int) -> Int { with Waiting { (use Wait).get(n) + 1 } }
"#,
        )],
        Roots::Exported,
    )
    .expect("lower a valid suspend handler program");
    let errors = compile(&core, &task_options()).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.kind == ErrorKind::Unsupported
                && error.message.contains("suspending closure"))
    );
}

#[test]
fn suspend_task_completion_preserves_int64_float_bool_and_unit_abis() {
    for (ty, c_type, input, complete, check) in [
        (
            TypeId::INT64,
            "int64_t",
            "INT64_MIN",
            "emela_task_complete_int64",
            "result == INT64_MIN",
        ),
        (
            TypeId::FLOAT,
            "double",
            "-0.0",
            "emela_task_complete_float",
            "result == 0.0 && signbit(result)",
        ),
        (
            TypeId::BOOL,
            "int32_t",
            "1",
            "emela_task_complete_int",
            "result == 1",
        ),
        (TypeId::UNIT, "", "", "emela_task_complete_unit", "1"),
    ] {
        let mut flow = Module::new(emela_core::Module::new());
        let mut b = FnBuilder::new(&flow.core, "echo");
        b.set_suspends().set_ret(ty);
        if ty != TypeId::UNIT {
            b.param("input", ty);
        }
        let external = b.finish_external(External {
            bindings: vec![ExternalBinding {
                target: "c".into(),
                host: HostRef::Global("host_echo".into()),
            }],
        });
        let echo = flow.add(external);
        let mut b = FnBuilder::new(&flow.core, "work");
        b.set_exported().set_suspends().set_ret(ty);
        let args = if ty == TypeId::UNIT {
            vec![]
        } else {
            vec![b.param("input", ty).into()]
        };
        let value = Value::Call {
            callee: Callee::Direct(echo),
            args,
            suspends: true,
            on_fail: None,
        };
        if ty == TypeId::UNIT {
            b.let_(None, value);
            b.term(Term::Return(None));
        } else {
            let result = b.assign("result", ty, value);
            b.term(Term::Return(Some(result.into())));
        }
        flow.add(b.finish());
        let output = compile_flow(
            &flow,
            &Options::default().with_task_adapter("host_echo", "board_echo"),
        )
        .unwrap();
        let (extra_param, completion_args, result_decl, start_args) = if ty == TypeId::UNIT {
            (String::new(), String::new(), String::new(), String::new())
        } else {
            (
                format!(", {c_type} input"),
                ", input".into(),
                format!("{c_type} result = 0;"),
                format!("{input}, &result"),
            )
        };
        compile_and_run(
            output,
            &format!(
                r#"
#include <assert.h>
#include <math.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
uint32_t emela_board_event_lock(void) {{ return 0; }}
void emela_board_event_unlock(uint32_t previous) {{ (void)previous; }}
_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) {{
  (void)kind; (void)message; (void)length; abort();
}}
void board_echo(EmelaTask task{extra_param}) {{ assert({complete}(task{completion_args})); }}
int main(void) {{
  {result_decl}
  EmelaFiber fiber = emela_export_work_1_start({start_args});
  assert(emela_scheduler_run(3) == 3 && emela_fiber_done(fiber));
  assert({check});
  emela_fiber_release(fiber);
  return 0;
}}
"#
            ),
        );
    }
}
