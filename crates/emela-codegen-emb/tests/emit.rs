use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use emela_codegen_emb::{Options, Output, compile, compile_flow};
use emela_core::TypeId;
use emela_lir::{Const, FnBuilder, Module, Operand, Term};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("emela-codegen-emb-{}-{id}", std::process::id()));
        fs::create_dir_all(&path).expect("create temporary C output directory");
        TempDir(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn emits_c_for_a_function_returning_an_int_constant() {
    let core = emela_core::Module::new();
    let mut builder = FnBuilder::new(&core, "answer");
    builder.set_ret(TypeId::INT).set_exported();
    builder.term(Term::Return(Some(Operand::Const(Const::Int(42)))));
    let function = builder.finish();
    let mut flow = Module::new(core);
    flow.add(function);

    let output = compile_flow(&flow, &Options::default()).expect("emit C from Flow IR");
    compile_and_run(
        output,
        "#include \"emela_embedded.h\"\nint main(void) { return emela_export_answer_0() == INT32_C(42) ? 0 : 1; }\n",
    );
}

#[test]
fn compiles_core_int_main_through_flow_ir_and_runs_generated_c() {
    let mut core = emela_core::Module::new();
    let main = core.declare("main", true);
    core.define(
        main,
        Vec::new(),
        TypeId::INT,
        emela_core::Expr::Lit(emela_core::Lit::Int(42)),
    );

    let output = compile(&core, &Options::default()).expect("compile Core through Flow IR");
    compile_and_run(
        output,
        "#include \"emela_embedded.h\"\nint main(void) { return emela_export_main_0() == INT32_C(42) ? 0 : 1; }\n",
    );
}

#[test]
fn compiles_core_bool_and_unit_constants() {
    let mut bool_core = emela_core::Module::new();
    let bool_main = bool_core.declare("main", true);
    bool_core.define(
        bool_main,
        Vec::new(),
        TypeId::BOOL,
        emela_core::Expr::Lit(emela_core::Lit::Bool(true)),
    );
    let bool_output = compile(&bool_core, &Options::default()).expect("compile Bool main");
    compile_and_run(
        bool_output,
        "#include \"emela_embedded.h\"\nint main(void) { return emela_export_main_0() == INT32_C(1) ? 0 : 1; }\n",
    );

    let mut unit_core = emela_core::Module::new();
    let unit_main = unit_core.declare("main", true);
    unit_core.define(
        unit_main,
        Vec::new(),
        TypeId::UNIT,
        emela_core::Expr::Lit(emela_core::Lit::Unit),
    );
    let unit_output = compile(&unit_core, &Options::default()).expect("compile Unit main");
    compile_and_run(
        unit_output,
        "#include \"emela_embedded.h\"\nint main(void) { emela_export_main_0(); return 0; }\n",
    );
}

fn compile_and_run(output: Output, harness_source: &str) {
    let temp = TempDir::new();
    for file in output.files {
        fs::write(temp.0.join(file.path), file.contents).expect("write generated C file");
    }
    let harness = temp.0.join("harness.c");
    fs::write(&harness, harness_source).expect("write C harness");

    let executable = temp.0.join("probe");
    let compile = Command::new("cc")
        .arg("-std=c11")
        .arg("-Wall")
        .arg("-Werror")
        .arg("-I")
        .arg(&temp.0)
        .arg(temp.0.join("emela_embedded.c"))
        .arg(&harness)
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("run the host C compiler");
    assert!(
        compile.status.success(),
        "C compilation failed:\n{}",
        String::from_utf8_lossy(&compile.stderr)
    );

    let run = Command::new(&executable)
        .status()
        .expect("run generated C program");
    assert!(run.success(), "generated C returned an unexpected result");
}
