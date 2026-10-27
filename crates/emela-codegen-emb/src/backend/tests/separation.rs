use super::*;
use emela_core::{External, ExternalBinding, HostRef, Roots};
use emela_lir::{Callee, Const, FnBuilder, Module, Term, Value};
use std::{
    fs,
    process::Command,
    time::{Duration, Instant},
};

fn source_flow(source: &str) -> Module {
    let core = support::try_lower(&[("main.emel", source)], Roots::Exported).unwrap();
    crate::flow_from_core(&core).unwrap()
}

fn contents<'a>(output: &'a crate::Output, path: &str) -> &'a str {
    &output
        .files
        .iter()
        .find(|file| file.path == path)
        .unwrap()
        .contents
}

fn export(output: &crate::Output, name: &str) -> String {
    output
        .exports
        .iter()
        .find(|entry| entry.source_name == name)
        .unwrap()
        .c_symbol
        .clone()
}

/// Compile every produced translation unit, including the native platform implementation.
/// A deadline also verifies that the native entry returns instead of entering the board idle loop.
fn run_native(output: &crate::Output, harness: &str) {
    use crate::artifact::{FileRole, MANIFEST_FILE, Manifest};
    let manifest = Manifest::from_json(contents(output, MANIFEST_FILE)).unwrap();
    assert_eq!(manifest, Manifest::from_files(&output.files).unwrap());
    let temp = tempfile::tempdir().unwrap();
    for file in &output.files {
        fs::write(temp.path().join(&file.path), &file.contents).unwrap();
    }
    fs::write(temp.path().join("harness.c"), harness).unwrap();
    for optimization in ["-O0", "-O2"] {
        let executable = temp.path().join("probe");
        let mut cc = Command::new("cc");
        cc.args(["-std=c11", "-Wall", "-Werror", optimization, "-I"])
            .arg(temp.path());
        for file in &manifest.files {
            if file.role == FileRole::Source {
                cc.arg(temp.path().join(&file.path));
            }
        }
        let build = cc
            .arg(temp.path().join("harness.c"))
            .arg("-lm")
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap();
        assert!(
            build.status.success(),
            "{}",
            String::from_utf8_lossy(&build.stderr)
        );
        let mut child = Command::new(&executable).spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success(), "{status}");
                break;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("native program did not finish");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[test]
fn compiled_empty_module_can_be_linked_for_both_environments() {
    let compiled =
        compiler::compile_flow(&emela_lir::Module::new(emela_core::Module::new())).unwrap();
    let options = crate::Options::default();
    let board_output = board::link(&compiled, &options).unwrap();
    let native_output = cnative::link(&compiled, &options).unwrap();
    for file in &board_output.files {
        if file.path == crate::artifact::MANIFEST_FILE {
            continue;
        }
        assert_eq!(&file.contents, contents(&native_output, &file.path));
    }
    assert!(
        native_output
            .files
            .iter()
            .any(|file| file.path == "emela_native_platform.c")
    );
}

#[test]
fn common_closure_lowering_is_shared_and_native_managed_values_run() {
    let flow =
        source_flow("pub fn calculate(x: Int) -> Int { add = fn(y: Int) { x + y }\n add(2) }");
    let compiled = compiler::compile_flow(&flow).unwrap();
    assert!(
        flow.functions.iter().any(
            |(_, function)| function.stmts.iter().any(|(_, stmt)| matches!(
                stmt,
                emela_lir::Stmt::Let {
                    value: Value::Closure(_),
                    ..
                }
            ))
        )
    );
    assert!(
        !compiled
            .lowered()
            .functions
            .iter()
            .any(
                |(_, function)| function.stmts.iter().any(|(_, stmt)| matches!(
                    stmt,
                    emela_lir::Stmt::Let {
                        value: Value::Closure(_),
                        ..
                    }
                ))
            )
    );
    let before = emela_lir::dump(compiled.lowered());
    let options = crate::Options::default().with_heap_bytes(4096);
    let board_output = board::link(&compiled, &options).unwrap();
    let expected = crate::compile_flow(&flow, &options).unwrap();
    assert_eq!(board_output.files, expected.files);
    let native_output = cnative::link(&compiled, &options).unwrap();
    let name = export(&native_output, "calculate");
    run_native(
        &native_output,
        &format!(
            r#"
#include <assert.h>
#include "emela_embedded.h"
int main(void) {{ for (int i = 0; i < 1000; ++i) assert({name}(40) == 42); return 0; }}
"#
        ),
    );
    assert_eq!(emela_lir::dump(compiled.lowered()), before);
}

#[test]
fn one_compiled_binding_selects_board_or_native_implementation_after_compilation() {
    let mut flow = Module::new(emela_core::Module::new());
    let mut binding = FnBuilder::new(&flow.core, "Std.Embedded.Board.clock_now");
    binding.set_ret(TypeId::INT64);
    let now = flow.add(binding.finish_external(External {
        bindings: vec![ExternalBinding {
            target: "c".into(),
            host: HostRef::Global("__emela_peripheral_clock_now".into()),
        }],
    }));
    let mut entry = FnBuilder::new(&flow.core, "current_time");
    entry.set_exported().set_ret(TypeId::INT64);
    let value = entry.assign(
        "now",
        TypeId::INT64,
        Value::Call {
            callee: Callee::Direct(now),
            args: vec![],
            suspends: false,
            on_fail: None,
        },
    );
    entry.term(Term::Return(Some(value.into())));
    flow.add(entry.finish());
    let compiled = compiler::compile_flow(&flow).unwrap();
    let board_output = board::link(&compiled, &crate::Options::default()).unwrap();
    assert!(
        board_output
            .files
            .iter()
            .any(|file| file.path == "emela_clock.h")
    );
    assert_eq!(
        board_output.files,
        crate::compile_flow(&flow, &crate::Options::default())
            .unwrap()
            .files
    );
    let native_options = crate::Options::default().with_external_adapter(
        "__emela_peripheral_clock_now",
        crate::ExternalAdapter::new("host_clock_now", crate::ExternalResult::Owned),
    );
    let native_output = cnative::link(&compiled, &native_options).unwrap();
    assert!(
        !native_output
            .files
            .iter()
            .any(|file| file.path == "emela_clock.h")
    );
    let name = export(&native_output, "current_time");
    run_native(
        &native_output,
        &format!(
            r#"
#include <assert.h>
#include "emela_embedded.h"
int64_t host_clock_now(void) {{ return INT64_C(1700000000123); }}
int main(void) {{ assert({name}() == INT64_C(1700000000123)); return 0; }}
"#
        ),
    );
    assert_eq!(
        board::link(&compiled, &crate::Options::default())
            .unwrap()
            .files,
        board_output.files
    );
}

#[test]
fn board_binding_validation_is_separate_and_leaves_the_compiled_artifact_reusable() {
    // A valid external Flow signature can still violate the standard board contract.
    let mut flow = Module::new(emela_core::Module::new());
    let mut binding = FnBuilder::new(&flow.core, "Std.Embedded.Board.clock_now");
    binding.set_ret(TypeId::INT);
    let now = flow.add(binding.finish_external(External {
        bindings: vec![ExternalBinding {
            target: "c".into(),
            host: HostRef::Global("__emela_peripheral_clock_now".into()),
        }],
    }));
    let mut entry = FnBuilder::new(&flow.core, "current_time");
    entry.set_exported().set_ret(TypeId::INT);
    let value = entry.assign(
        "now",
        TypeId::INT,
        Value::Call {
            callee: Callee::Direct(now),
            args: vec![],
            suspends: false,
            on_fail: None,
        },
    );
    entry.term(Term::Return(Some(value.into())));
    flow.add(entry.finish());

    let compiled = compiler::compile_flow(&flow).unwrap();
    let before = emela_lir::dump(compiled.lowered());
    let options = crate::Options::default();
    let errors = board::link(&compiled, &options).unwrap_err();
    assert!(errors.iter().any(|error| {
        error
            .message
            .contains("invalid standard board binding signature")
    }));
    assert_eq!(errors, crate::compile_flow(&flow, &options).unwrap_err());

    let native_options = options.with_external_adapter(
        "__emela_peripheral_clock_now",
        crate::ExternalAdapter::new("host_clock_now", crate::ExternalResult::Owned),
    );
    let output = cnative::link(&compiled, &native_options).unwrap();
    let name = export(&output, "current_time");
    run_native(
        &output,
        &format!(
            r#"
#include <assert.h>
#include "emela_embedded.h"
int32_t host_clock_now(void) {{ return INT32_C(42); }}
int main(void) {{ assert({name}() == 42); return 0; }}
"#
        ),
    );
    assert_eq!(emela_lir::dump(compiled.lowered()), before);
    assert_eq!(board::link(&compiled, &native_options).unwrap_err(), errors);
}

#[test]
fn native_start_runs_and_returns_for_sync_and_suspend_entries() {
    for suspends in [false, true] {
        let mut flow = Module::new(emela_core::Module::new());
        let marker = flow.add(
            FnBuilder::new(&flow.core, "mark_main").finish_external(External {
                bindings: vec![ExternalBinding {
                    target: "c".into(),
                    host: HostRef::Global("mark_main".into()),
                }],
            }),
        );
        let mut entry = FnBuilder::new(&flow.core, "main");
        entry.set_exported().set_ret(TypeId::UNIT);
        if suspends {
            entry.set_suspends();
        }
        entry.let_(
            None,
            Value::Call {
                callee: Callee::Direct(marker),
                args: vec![],
                suspends: false,
                on_fail: None,
            },
        );
        entry.term(Term::Return(None));
        flow.add(entry.finish());
        let compiled = compiler::compile_flow(&flow).unwrap();
        let output = cnative::link(&compiled, &crate::Options::default()).unwrap();
        run_native(
            &output,
            r#"
#include <assert.h>
#include "emela_embedded.h"
static int calls;
void mark_main(void) { calls++; }
int main(void) { emela_native_start(); assert(calls == 1); return 0; }
"#,
        );
    }
}

#[test]
fn native_entry_preserves_nonvoid_suspending_result_storage() {
    let mut flow = Module::new(emela_core::Module::new());
    let mut entry = FnBuilder::new(&flow.core, "main");
    entry.set_exported().set_suspends().set_ret(TypeId::INT);
    entry.term(Term::Return(Some(Const::Int(42).into())));
    flow.add(entry.finish());
    let compiled = compiler::compile_flow(&flow).unwrap();
    let output = cnative::link(&compiled, &crate::Options::default()).unwrap();
    run_native(
        &output,
        "#include \"emela_embedded.h\"\nint main(void) { emela_native_start(); return 0; }\n",
    );
}

#[test]
fn native_entry_releases_managed_results_before_returning() {
    for suspends in [false, true] {
        let mut core = emela_core::Module::new();
        let list = core.types.intern(Type::List(TypeId::INT));
        let mut flow = Module::new(core);
        let mut entry = FnBuilder::new(&flow.core, "main");
        entry.set_exported().set_ret(list);
        if suspends {
            entry.set_suspends();
        }
        let result = entry.assign(
            "result",
            list,
            Value::List {
                elems: vec![Const::Int(42).into()],
                tail: None,
            },
        );
        entry.term(Term::Return(Some(result.into())));
        flow.add(entry.finish());
        let compiled = compiler::compile_flow(&flow).unwrap();
        let output =
            cnative::link(&compiled, &crate::Options::default().with_heap_bytes(1024)).unwrap();
        run_native(
            &output,
            "#include \"emela_embedded.h\"\nint main(void) { for (int i = 0; i < 1000; ++i) emela_native_start(); return 0; }\n",
        );
    }
}

#[test]
fn board_and_native_entries_supply_dynamic_string_output_buffers() {
    let mut flow = Module::new(emela_core::Module::new());
    let mut reader = FnBuilder::new(&flow.core, "read_number");
    reader.set_ret(TypeId::INT);
    let reader = flow.add(reader.finish_external(External {
        bindings: vec![ExternalBinding {
            target: "c".into(),
            host: HostRef::Global("read_number".into()),
        }],
    }));
    let mut entry = FnBuilder::new(&flow.core, "main");
    entry.set_exported().set_ret(TypeId::STRING);
    let number = entry.assign(
        "number",
        TypeId::INT,
        Value::Call {
            callee: Callee::Direct(reader),
            args: vec![],
            suspends: false,
            on_fail: None,
        },
    );
    let value = entry.assign(
        "text",
        TypeId::STRING,
        Value::Builtin {
            op: Builtin::IntToString,
            args: vec![number.into()],
        },
    );
    entry.term(Term::Return(Some(value.into())));
    flow.add(entry.finish());
    let compiled = compiler::compile_flow(&flow).unwrap();
    for capacity in [2, 64, 512] {
        let options = crate::Options::default().with_string_capacity(capacity);
        let board_output = board::link(&compiled, &options).unwrap();
        assert_eq!(
            board_output.files,
            crate::compile_flow(&flow, &options).unwrap().files
        );
        run_native(
            &board_output,
            r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
static int init_count;
static int calls;
int32_t read_number(void) { calls++; return INT32_C(42); }
void emela_board_init(void) { init_count++; }
void emela_board_idle(void) { assert(init_count == 1 && calls == 1); exit(0); }
_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) {
  (void)kind; (void)message; (void)length; abort();
}
int main(void) { emela_embedded_start(); return 1; }
"#,
        );
        let native_output = cnative::link(&compiled, &options).unwrap();
        let name = export(&native_output, "main");
        run_native(
            &native_output,
            &format!(
                r#"
#include <assert.h>
#include <string.h>
#include "emela_embedded.h"
static int calls;
int32_t read_number(void) {{ calls++; return INT32_C(42); }}
int main(void) {{
  for (int i = 0; i < 1000; ++i) emela_native_start();
  assert(calls == 1000);
  char buffer[2];
  EmelaString value = {name}(buffer, UINT16_C(2));
  assert(value.data == buffer && value.owner == NULL);
  assert(value.length == 2 && memcmp(value.data, "42", 2) == 0);
  assert(calls == 1001);
  return 0;
}}
"#
            ),
        );
    }
}

#[test]
fn compiler_sources_do_not_depend_on_concrete_connections() {
    use syn::visit::{self, Visit};
    struct Dependencies;
    impl<'ast> Visit<'ast> for Dependencies {
        fn visit_path(&mut self, path: &'ast syn::Path) {
            for segment in &path.segments {
                let name = segment.ident.to_string();
                assert!(
                    !["board", "cnative", "peripheral", "interrupt_validation"]
                        .contains(&name.as_str()),
                    "compiler depends on concrete connection {name}"
                );
            }
            visit::visit_path(self, path);
        }
    }
    let mut pending =
        vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/backend/compiler")];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                Dependencies
                    .visit_file(&syn::parse_file(&fs::read_to_string(path).unwrap()).unwrap());
            }
        }
    }
}

#[test]
fn string_plans_preserve_aliases_while_other_buffers_and_calls_are_live() {
    let flow = source_flow(
        r#"
fn number(x: Int) -> String { Int.to_string(x) }
pub fn render(x: Int) -> String {
  padded = String.append(" ", number(x))
  same = String.append(padded, "")
  trimmed = String.trim(same)
  next = number(x + 1)
  String.append(String.append(trimmed, ":"), next)
}
"#,
    );
    let compiled = compiler::compile_flow(&flow).unwrap();
    let options = crate::Options::default().with_string_capacity(64);
    assert_eq!(
        board::link(&compiled, &options).unwrap().files,
        crate::compile_flow(&flow, &options).unwrap().files
    );
    let output = cnative::link(&compiled, &options).unwrap();
    let render = export(&output, "render");
    run_native(
        &output,
        &format!(
            r#"
#include <assert.h>
#include <string.h>
#include "emela_embedded.h"
int main(void) {{
  const int32_t inputs[] = {{0, 41, -1, INT32_MIN, INT32_MAX}};
  const char *expected[] = {{"0:1", "41:42", "-1:0", "-2147483648:-2147483647", "2147483647:-2147483648"}};
  for (int round = 0; round < 1000; ++round) {{
    for (unsigned i = 0; i < sizeof inputs / sizeof inputs[0]; ++i) {{
      char buffer[64];
      EmelaString value = {render}(inputs[i], buffer, UINT16_C(64));
      assert(value.length == strlen(expected[i]));
      assert(memcmp(value.data, expected[i], value.length) == 0);
      assert(value.data == buffer && value.owner == NULL);
    }}
  }}
  return 0;
}}
"#
        ),
    );
}

#[test]
fn shared_string_helpers_support_multiple_consumers_in_one_translation_unit() {
    let flow = source_flow(
        r#"
pub fn has_a(text: String) -> Bool { String.contains(text, "a") }
pub fn starts_a(text: String) -> Bool { String.starts_with(text, "a") }
pub fn ends_a(text: String) -> Bool { String.ends_with(text, "a") }
"#,
    );
    let compiled = compiler::compile_flow(&flow).unwrap();
    let options = crate::Options::default();
    assert_eq!(
        board::link(&compiled, &options).unwrap().files,
        crate::compile_flow(&flow, &options).unwrap().files
    );
    let output = cnative::link(&compiled, &options).unwrap();
    let has = export(&output, "has_a");
    let starts = export(&output, "starts_a");
    let ends = export(&output, "ends_a");
    run_native(
        &output,
        &format!(
            r#"
#include <assert.h>
#include "emela_embedded.h"
int main(void) {{
  EmelaString both = {{.data = "aba", .length = 3}};
  EmelaString middle = {{.data = "bab", .length = 3}};
  EmelaString empty = {{.data = "", .length = 0}};
  assert({has}(both) && {starts}(both) && {ends}(both));
  assert({has}(middle) && !{starts}(middle) && !{ends}(middle));
  assert(!{has}(empty) && !{starts}(empty) && !{ends}(empty));
  return 0;
}}
"#
        ),
    );
}

#[test]
fn shared_planners_use_feature_facts_instead_of_concrete_operations() {
    use syn::visit::{self, Visit};
    struct Operations;
    impl<'ast> Visit<'ast> for Operations {
        fn visit_path(&mut self, path: &'ast syn::Path) {
            for segment in &path.segments {
                assert!(
                    !["Builtin", "PrimOp", "RuntimeOp", "FiberOp"]
                        .contains(&segment.ident.to_string().as_str()),
                    "shared planner inspects concrete operation {}",
                    segment.ident
                );
            }
            visit::visit_path(self, path);
        }
    }
    for source in [
        include_str!("../compiler/foundation/string_buffers.rs"),
        include_str!("../compiler/foundation/heap_plan.rs"),
        include_str!("../compiler/foundation/runtime_plan.rs"),
    ] {
        Operations.visit_file(&syn::parse_file(source).unwrap());
    }
}
