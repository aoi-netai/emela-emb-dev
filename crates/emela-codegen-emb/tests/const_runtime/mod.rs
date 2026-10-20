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
const VALUE = { dbg("once")
 if True { sum(6) } else { 0 } }
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
int main(void) {{ for (int i = 0; i < 1000; i++) if ({read}() != 42) return 1; return debug_calls == 1 ? 0 : 2; }}
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
int main(void) {{ for (int i = 0; i < 1000; i++) if ({read}() != 44) return 1; return debug_calls == 1 ? 0 : 2; }}
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
int main(void) {{ {discard}(); for (int i = 0; i < 1000; i++) if ({read}() != 42) return 1; return calls == 1 ? 0 : 2; }}
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
const SELF = if zero() == 0 { SELF + 0 } else { 42 }
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
  for (int i = 0; i < 1000; i++) if (setjmp(target) == 0) {{ (void){bad}(); return 1; }}
  if (calls != 1 || debug_calls != 1) return 2;
  mode = 1;
  for (int i = 0; i < 1000; i++) if (setjmp(target) == 0) {{ (void){self_ref}(); return 3; }}
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
