use super::*;

fn emit_source(source: &str, heap: u32) -> Output {
    let core =
        support::try_lower(&[("main.emel", source)], Roots::Exported).expect("lower const source");
    compile(&core, &Options::default().with_heap_bytes(heap)).expect("emit runtime const")
}

fn export(output: &Output, name: &str) -> String {
    let header = &output
        .files
        .iter()
        .find(|f| f.path == "emela_embedded.h")
        .unwrap()
        .contents;
    let prefix = format!("emela_export_{name}_");
    let start = header.find(&prefix).unwrap();
    header[start..].split('(').next().unwrap().to_owned()
}

const BOARD: &str = r#"
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) {
  (void)kind; (void)message; (void)length; abort();
}
"#;

#[test]
fn evaluates_branch_loop_recursive_helper_and_closure_const_once() {
    let output = emit_source(
        r#"
fn sum(n: Int) -> Int { if n == 0 { 0 } else { n + sum(n - 1) } }
fn loop_sum(n: Int, acc: Int) -> Int { if n == 0 { acc } else { loop_sum(n - 1, acc + n) } }
const VIA_INDIRECT = { f = sum
 f(6) }
const VALUE = { dbg("once")
 if True { VIA_INDIRECT + loop_sum(6, 0) - 21 } else { 0 } }
const APPLY = { x = VALUE
 fn(y: Int) { x + y } }
pub fn read() -> Int { APPLY(VALUE) }
"#,
        4096,
    );
    let read = export(&output, "read");
    compile_and_run(
        output,
        &format!(
            r#"{BOARD}
static int debug_calls;
void emela_board_debug(const char *bytes, uint16_t length) {{ (void)bytes; (void)length; debug_calls++; }}
int main(void) {{ for (volatile int i = 0; i < 1000; i++) if ({read}() != 42) return 1; return debug_calls == 1 ? 0 : 2; }}
"#
        ),
    );
}

#[test]
fn keeps_list_tuple_enum_and_option_string_const_values_alive() {
    let output = emit_source(
        r#"
const VALUE = { dbg("once")
 (Some(42), ["a", "bc"], String.from_code_point(65)) }
pub fn read() -> Int {
  (parsed, values, point) = VALUE
  n = parsed match { Some(n) -> n
_ -> -100 }
  first = values match { [a, ..rest] -> String.byte_size(a)
_ -> 0 }
  size = point match { Some(s) -> String.byte_size(s)
_ -> 0 }
  n + first + size
}
"#,
        8192,
    );
    let read = export(&output, "read");
    compile_and_run(
        output,
        &format!(
            r#"{BOARD}
static int debug_calls;
void emela_board_debug(const char *bytes, uint16_t length) {{ (void)bytes; (void)length; debug_calls++; }}
int main(void) {{ for (volatile int i = 0; i < 1000; i++) if ({read}() != 44) return 1; return debug_calls == 1 ? 0 : 2; }}
"#
        ),
    );
}

#[test]
fn caches_runtime_const_external_results_and_discards_without_reinitializing() {
    let output = emit_source(
        r#"
@external(c, "fixture_seed")
fn raw_seed() -> Int
effect Seed { fn read() -> Int }
handler BoardSeed implements Seed { fn read() { raw_seed() } }
fn seed() -> Int { with BoardSeed { (use Seed).read() } }
const VALUE = seed()
const UNUSED = seed()
pub fn read() -> Int { VALUE }
pub fn discard() { VALUE
() }
"#,
        8192,
    );
    let read = export(&output, "read");
    let discard = export(&output, "discard");
    compile_and_run(
        output,
        &format!(
            r#"{BOARD}
static int calls;
int32_t fixture_seed(void) {{ return ++calls + 41; }}
int main(void) {{ {discard}(); for (volatile int i = 0; i < 1000; i++) if ({read}() != 42) return 1; return calls == 1 ? 0 : 2; }}
"#
        ),
    );
}

#[test]
fn memoizes_nested_const_defects_and_retries_self_reference_without_prefix() {
    let output = emit_source(
        r#"
@external(c, "fixture_zero")
fn raw_zero() -> Int
effect Zero { fn read() -> Int }
handler BoardZero implements Zero { fn read() { raw_zero() } }
fn zero() -> Int { with BoardZero { (use Zero).read() } }
const B = 1 / zero()
const A = { dbg("A")
 B }
const SELF = if zero() == 0 { SELF } else { 42 }
pub fn bad() -> Int { A }
pub fn self_ref() -> Int { SELF }
"#,
        8192,
    );
    let bad = export(&output, "bad");
    let self_ref = export(&output, "self_ref");
    compile_and_run(
        output,
        &format!(
            r#"
#include <setjmp.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
static jmp_buf target;
static int calls, debug_calls, mode;
int32_t fixture_zero(void) {{ calls++; return 0; }}
void emela_board_debug(const char *bytes, uint16_t length) {{ (void)bytes; (void)length; debug_calls++; }}
_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) {{
  const char *expected = mode == 0 ? "while evaluating const A: while evaluating const B: division by zero" : "const SELF refers to itself while being evaluated";
  if (length != strlen(expected) || memcmp(message, expected, length) != 0 || (mode == 0 && kind != EMELA_DEFECT_DIVISION_BY_ZERO)) abort();
  longjmp(target, 1);
}}
int main(void) {{
  for (volatile int i = 0; i < 1000; i++) if (setjmp(target) == 0) {{ (void){bad}(); return 1; }}
  if (calls != 1 || debug_calls != 1) return 2;
  mode = 1;
  for (volatile int i = 0; i < 1000; i++) if (setjmp(target) == 0) {{ (void){self_ref}(); return 3; }}
  return calls == 1001 ? 0 : 4;
}}
"#
        ),
    );
}

#[test]
fn rejects_runtime_const_reachable_from_isr_but_preserves_static_fold() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
@external(c, "fixture_seed")
fn raw_seed() -> Int
effect Seed { fn read() -> Int }
handler BoardSeed implements Seed { fn read() { raw_seed() } }
const V = with BoardSeed { (use Seed).read() }
pub fn read() -> Int { V }
"#,
        )],
        Roots::Exported,
    )
    .unwrap();
    let errors = compile(
        &core,
        &Options::default()
            .with_heap_bytes(8192)
            .with_isr_entry("read")
            .with_isr_safe_external("fixture_seed"),
    )
    .unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("runtime const initialization"))
    );
    let core = support::try_lower(
        &[("main.emel", "const V = 40 + 2\npub fn read() -> Int { V }")],
        Roots::Exported,
    )
    .unwrap();
    let output = compile(&core, &Options::default().with_isr_entry("read")).unwrap();
    assert!(
        !output
            .files
            .iter()
            .find(|f| f.path == "emela_embedded.c")
            .unwrap()
            .contents
            .contains("EmelaConstState")
    );
}

fn external_const_flow() -> Module {
    let core = emela_core::Module::new();
    let mut flow = Module::new(core);
    let mut b = FnBuilder::new(&flow.core, "seed");
    b.set_ret(TypeId::INT);
    let seed = flow.add(b.finish_external(External {
        bindings: vec![ExternalBinding {
            target: "c".into(),
            host: HostRef::Global("fixture_seed".into()),
        }],
    }));
    let mut b = FnBuilder::new(&flow.core, "VALUE");
    b.set_ret(TypeId::INT).set_constant().set_exported();
    let result = b.assign(
        "result",
        TypeId::INT,
        Value::Call {
            callee: Callee::Direct(seed),
            args: vec![],
            suspends: false,
            on_fail: None,
        },
    );
    b.term(Term::Return(Some(result.into())));
    let value = flow.add(b.finish());
    let mut b = FnBuilder::new(&flow.core, "discard");
    b.set_exported();
    b.let_(
        None,
        Value::Call {
            callee: Callee::Direct(value),
            args: vec![],
            suspends: false,
            on_fail: None,
        },
    );
    b.term(Term::Return(None));
    flow.add(b.finish());
    flow
}

#[test]
fn verified_flow_runtime_scalar_const_uses_no_heap_and_caches_discarded_first_reference() {
    let output = compile_flow(&external_const_flow(), &Options::default()).unwrap();
    assert!(
        !output
            .files
            .iter()
            .find(|f| f.path == "emela_embedded.c")
            .unwrap()
            .contents
            .contains("emela_heap_storage")
    );
    compile_and_run(
        output,
        &format!(
            r#"{BOARD}
static int calls;
int32_t fixture_seed(void) {{ return ++calls + 41; }}
int main(void) {{ emela_export_discard_2(); for (volatile int i=0; i<1000; i++) if (emela_export_VALUE_1() != 42) return 1; return calls == 1 ? 0 : 2; }}
"#
        ),
    );
}

#[test]
fn public_defect_report_snapshots_borrowed_message_once_without_using_heap() {
    let output = compile_flow(&external_const_flow(), &Options::default()).unwrap();
    compile_and_run(
        output,
        r#"
#include <setjmp.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
static jmp_buf target;
static int calls;
static char message[] = "borrowed failure";
int32_t fixture_seed(void) { calls++; emela_embedded_report_defect(EMELA_DEFECT_ASSERT, message, sizeof message - 1); }
_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *bytes, uint16_t length) {
  static const char expected[] = "while evaluating const VALUE: borrowed failure";
  if (kind != EMELA_DEFECT_ASSERT || length != sizeof expected - 1 || memcmp(bytes, expected, length)) abort();
  memset(message, 'x', sizeof message - 1);
  longjmp(target, 1);
}
int main(void) { for (volatile int i=0; i<1000; i++) if (setjmp(target)==0) { (void)emela_export_VALUE_1(); return 1; } return calls == 1 ? 0 : 2; }
"#,
    );
}

#[test]
fn diagnostic_capacity_exhaustion_reports_active_const_without_allocating_or_recursing() {
    let output = compile_flow(
        &external_const_flow(),
        &Options::default().with_const_diagnostic_bytes(1),
    )
    .unwrap();
    compile_and_run(
        output,
        r#"
#include <setjmp.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
static jmp_buf target;
static int calls;
int32_t fixture_seed(void) { calls++; emela_embedded_report_defect(EMELA_DEFECT_ASSERT, "bad", 3); }
_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *bytes, uint16_t length) {
  static const char expected[] = "while evaluating const VALUE: const diagnostic capacity exceeded";
  if (kind != EMELA_DEFECT_STRING_CAPACITY || length != sizeof expected - 1 || memcmp(bytes, expected, length)) abort();
  longjmp(target, 1);
}
int main(void) { for (volatile int i=0; i<1000; i++) if (setjmp(target)==0) { (void)emela_export_VALUE_1(); return 1; } return calls == 1000 ? 0 : 2; }
"#,
    );
    assert_eq!(
        compile_flow(
            &external_const_flow(),
            &Options::default().with_const_diagnostic_bytes(0)
        )
        .unwrap_err()[0]
            .kind,
        ErrorKind::InvalidOptions
    );
}

#[test]
fn heap_exhaustion_during_const_initialization_keeps_its_name_and_cached_defect() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
const VALUE = [1, 2, 3]
pub fn read() -> Int { VALUE match { [head, ..tail] -> head
_ -> 0 } }
"#,
        )],
        Roots::Exported,
    )
    .unwrap();
    let output = compile(&core, &Options::default().with_heap_bytes(64)).unwrap();
    let read = export(&output, "read");
    compile_and_run(
        output,
        &format!(
            r#"
#include <setjmp.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
static jmp_buf target;
_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *bytes, uint16_t length) {{
  static const char expected[] = "while evaluating const VALUE: fixed heap exhausted";
  if (kind != EMELA_DEFECT_HEAP_EXHAUSTED || length != sizeof expected - 1 || memcmp(bytes, expected, length)) abort();
  longjmp(target, 1);
}}
int main(void) {{ for (volatile int i=0; i<1000; i++) if (setjmp(target)==0) {{ (void){read}(); return 1; }} return 0; }}
"#
        ),
    );
}

#[path = "../../examples/support/gc_anchor.rs"]
mod gc_anchor;

#[test]
fn persistent_const_roots_survive_cycle_collection_and_caller_release() {
    let core = support::try_lower(
        &[(
            "main.emel",
            include_str!("../../examples/support/const_runtime.emel"),
        )],
        Roots::Exported,
    )
    .unwrap();
    let mut flow = emela_codegen_emb::flow_from_core(&core).unwrap();
    gc_anchor::add(&mut flow);
    let mut output = compile_flow(&flow, &Options::default().with_heap_bytes(4096)).unwrap();
    let read = export(&output, "read");
    output
        .files
        .iter_mut()
        .find(|f| f.path == "emela_embedded.c")
        .unwrap()
        .contents
        .push_str(
            r#"
size_t test_const_heap(void) {
  emela_heap_collect_cycles();
  size_t used = 0;
  for (unsigned char *p = emela_heap_storage; p < emela_heap_end();) {
    emela_heap_block *block = (emela_heap_block *)(void *)p;
    if (!block->is_free) used += block->size;
    p += block->size;
  }
  return used;
}
"#,
        );
    compile_and_run(
        output,
        &format!(
            r#"{BOARD}
static int debug_calls;
void emela_board_debug(const char *bytes, uint16_t length) {{ (void)bytes; (void)length; debug_calls++; }}
size_t test_const_heap(void);
int main(void) {{ if ({read}()!=42) return 1; size_t baseline=test_const_heap(); if (!baseline) return 2; for (volatile int i=0; i<1000; i++) if ({read}()!=42 || test_const_heap()!=baseline) return 3; return debug_calls==1 ? 0 : 4; }}
"#
        ),
    );
}

#[test]
fn deep_const_contexts_share_cached_diagnostic_suffixes_and_bound_static_folding() {
    let mut flow = external_const_flow();
    flow.functions[emela_lir::FnId::from_raw(1.into())].exported = false;
    let ids: Vec<_> = (0..80).map(|i| flow.declare(&format!("C{i}"))).collect();
    for (i, &id) in ids.iter().enumerate() {
        let target = ids
            .get(i + 1)
            .copied()
            .unwrap_or(emela_lir::FnId::from_raw(0.into()));
        let mut b = FnBuilder::new(&flow.core, &format!("C{i}"));
        b.set_ret(TypeId::INT).set_constant();
        if i == 0 || i == 79 {
            b.set_exported();
        }
        let value = b.assign(
            "value",
            TypeId::INT,
            Value::Call {
                callee: Callee::Direct(target),
                args: vec![],
                suspends: false,
                on_fail: None,
            },
        );
        b.term(Term::Return(Some(value.into())));
        flow.define(id, b.finish());
    }
    let output =
        compile_flow(&flow, &Options::default().with_const_diagnostic_bytes(8192)).unwrap();
    let outer = export(&output, "C0");
    let inner = export(&output, "C79");
    let expected: String = (0..80)
        .map(|i| format!("while evaluating const C{i}: "))
        .collect::<String>()
        + "bad";
    compile_and_run(
        output,
        &format!(
            r#"
#include <setjmp.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
static jmp_buf target;
static int calls, mode;
int32_t fixture_seed(void) {{ calls++; emela_embedded_report_defect(EMELA_DEFECT_PANIC, "bad", 3); }}
_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *bytes, uint16_t length) {{
  const char *expected = mode == 0 ? "{expected}" : "while evaluating const C79: bad";
  if (kind != EMELA_DEFECT_PANIC || length != strlen(expected) || memcmp(bytes, expected, length)) abort();
  longjmp(target, 1);
}}
int main(void) {{
  for (volatile int i=0; i<1000; i++) if (setjmp(target)==0) {{ (void){outer}(); return 1; }}
  mode=1;
  for (volatile int i=0; i<1000; i++) if (setjmp(target)==0) {{ (void){inner}(); return 2; }}
  return calls == 1 ? 0 : 3;
}}
"#
        ),
    );
}

#[test]
fn folded_self_reference_with_long_name_does_not_copy_into_short_runtime_emergency_storage() {
    let mut flow = external_const_flow();
    let name = "LONG".repeat(1024);
    let id = flow.declare(&name);
    let mut b = FnBuilder::new(&flow.core, &name);
    b.set_ret(TypeId::INT).set_constant().set_exported();
    let value = b.assign(
        "value",
        TypeId::INT,
        Value::Call {
            callee: Callee::Direct(id),
            args: vec![],
            suspends: false,
            on_fail: None,
        },
    );
    b.term(Term::Return(Some(value.into())));
    flow.define(id, b.finish());
    let output = compile_flow(&flow, &Options::default()).unwrap();
    let call = export(&output, &name);
    compile_and_run(
        output,
        &format!(
            r#"
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
int32_t fixture_seed(void) {{ return 42; }}
_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *bytes, uint16_t length) {{
  static const char expected[]="const {name} refers to itself while being evaluated";
  exit(kind==EMELA_DEFECT_PANIC && length==sizeof expected-1 && !memcmp(bytes,expected,length) ? 0 : 1);
}}
int main(void) {{ (void){call}(); return 2; }}
"#
        ),
    );
}

#[test]
fn unit_and_never_const_cache_states_do_not_require_a_represented_return_value() {
    let mut flow = Module::new(emela_core::Module::new());
    let b = FnBuilder::new(&flow.core, "tick");
    let tick = flow.add(b.finish_external(External {
        bindings: vec![ExternalBinding {
            target: "c".into(),
            host: HostRef::Global("fixture_tick".into()),
        }],
    }));
    for (name, ret) in [("ONCE", TypeId::UNIT), ("BROKEN", TypeId::NEVER)] {
        let mut b = FnBuilder::new(&flow.core, name);
        b.set_ret(ret).set_constant().set_exported();
        b.let_(
            None,
            Value::Call {
                callee: Callee::Direct(tick),
                args: vec![],
                suspends: false,
                on_fail: None,
            },
        );
        b.term(if ret == TypeId::UNIT {
            Term::Return(None)
        } else {
            Term::Defect(Const::String("boom".into()).into())
        });
        flow.add(b.finish());
    }
    let output = compile_flow(&flow, &Options::default()).unwrap();
    compile_and_run(
        output,
        r#"
#include <setjmp.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
static jmp_buf target;
static int calls;
void fixture_tick(void) { calls++; }
_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *bytes, uint16_t length) {
  static const char expected[]="while evaluating const BROKEN: boom";
  if(kind!=EMELA_DEFECT_PANIC || length!=sizeof expected-1 || memcmp(bytes,expected,length)) abort();
  longjmp(target,1);
}
int main(void) {
  for (volatile int i=0;i<1000;i++) emela_export_ONCE_1();
  if(calls!=1) return 1;
  for (volatile int i=0;i<1000;i++) if(setjmp(target)==0) { emela_export_BROKEN_2(); return 2; }
  return calls==2 ? 0 : 3;
}
"#,
    );
}

#[test]
fn source_polymorphic_const_origins_keep_a_canonical_value_and_typed_members_in_verified_flow() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
const EMPTY = { dbg("once")
 [] }
fn ints(xs: List[Int]) -> Int { List.length(xs) }
fn strings(xs: List[String]) -> Int { List.length(xs) }
pub fn read() -> Int { ints(EMPTY) + strings(EMPTY) }
"#,
        )],
        Roots::Exported,
    )
    .unwrap();
    let flow = emela_codegen_emb::flow_from_core(&core).unwrap();
    emela_lir::verify(&flow).unwrap();
    let (_, marker) = flow
        .functions
        .iter()
        .find(|(_, f)| f.name.starts_with("$emb_const_origins$v1$"))
        .expect("origin metadata survives the public Flow result");
    assert!(marker.constant && !marker.exported && marker.params.is_empty());
    let seq = &marker.seqs[marker.body.unwrap()];
    let targets: Vec<_> = seq
        .stmts
        .iter()
        .map(|&id| {
            let emela_lir::Stmt::Let {
                value: Value::FnRef(target),
                ..
            } = marker.stmts[id]
            else {
                panic!("origin marker contains only typed function references")
            };
            target
        })
        .collect();
    assert_eq!(targets.len(), 3);
    assert_eq!(flow.functions[targets[0]].name, "EMPTY");
    assert!(matches!(
        flow.core.types[flow.functions[targets[0]].ret],
        Type::List(TypeId::ANY)
    ));
    for &target in &targets[1..] {
        assert!(flow.functions[target].name.starts_with("EMPTY["));
    }
}

#[test]
fn malformed_reserved_const_origin_markers_are_invalid_flow() {
    let mut flow = external_const_flow();
    let mut b = FnBuilder::new(&flow.core, "$emb_const_origins$v1$0");
    b.set_constant();
    b.term(Term::Return(None));
    flow.add(b.finish());
    let errors = compile_flow(&flow, &Options::default()).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.kind == ErrorKind::InvalidFlowIr
                && error.message.contains("const origin marker"))
    );
}

#[test]
fn distinct_direct_flow_const_ids_keep_independent_caches_without_source_markers() {
    let mut flow = external_const_flow();
    let mut second = flow.functions[emela_lir::FnId::from_raw(1.into())].clone();
    second.name = "OTHER".into();
    flow.add(second);
    let output = compile_flow(&flow, &Options::default()).unwrap();
    compile_and_run(
        output,
        &format!(
            r#"{BOARD}
static int calls;
int32_t fixture_seed(void) {{ return ++calls + 41; }}
int main(void) {{ for (int i=0;i<1000;i++) if(emela_export_VALUE_1()!=42 || emela_export_OTHER_3()!=43) return 1; return calls==2 ? 0 : 2; }}
"#
        ),
    );
}

#[test]
fn generic_function_references_returned_by_consts_keep_their_source_family() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
fn identity[A](value: A, unit: ()) -> A { value }
const IDENTITY = identity
pub fn integer() -> Int { IDENTITY(42, ()) }
pub fn text() -> String { IDENTITY("42", ()) }
"#,
        )],
        Roots::Exported,
    )
    .unwrap();
    let flow = emela_codegen_emb::flow_from_core(&core).unwrap();
    let (_, marker) = flow
        .functions
        .iter()
        .find(|(_, f)| f.name.starts_with("$emb_fn_origins$v1$"))
        .expect("ordinary FnRef specializations retain their source origin");
    assert_eq!(marker.stmts.len(), 3);
    for (_, stmt) in marker.stmts.iter() {
        let emela_lir::Stmt::Let {
            value: Value::FnRef(target),
            ..
        } = stmt
        else {
            panic!("family is a list of FnRef values")
        };
        assert!(flow.functions[*target].name.starts_with("identity["));
        assert!(!flow.functions[*target].constant);
    }
    compile_flow(&flow, &Options::default())
        .expect("well-shaped function family markers are unreachable metadata");
    let mut malformed = flow.clone();
    let (id, _) = malformed
        .functions
        .iter()
        .find(|(_, f)| f.name.starts_with("$emb_fn_origins$v1$"))
        .unwrap();
    malformed.functions[id].exported = true;
    let errors = compile_flow(&malformed, &Options::default()).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| e.kind == ErrorKind::InvalidFlowIr && e.message.contains("origin marker"))
    );
}
