use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use emela_codegen_emb::{Options, Output, compile, compile_flow};
use emela_core::{BinOp, CtorRef, EnumDef, OpTy, TypeId, VariantDef};
use emela_lir::{Const, FnBuilder, Module, Operand, PrimOp, PrimTy, Term, Value};

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

#[test]
fn emits_and_runs_int64_constants_at_the_signed_boundaries() {
    for (value, expected) in [
        (i64::MIN, "INT64_MIN"),
        (i64::MAX, "INT64_MAX"),
        (-1, "-INT64_C(1)"),
    ] {
        let output = compile_constant_function("wide", TypeId::INT64, Const::Int64(value));
        let harness = format!(
            "#include <stdint.h>\n#include \"emela_embedded.h\"\nint main(void) {{ return emela_export_wide_0() == {expected} ? 0 : 1; }}\n"
        );
        compile_and_run(output, &harness);
    }
}

#[test]
fn emits_and_runs_float_constants_including_non_finite_values() {
    for (value, expected) in [
        (1.25, "value == 1.25"),
        (1.0e100, "value == 1.0e100"),
        (-0.0, "value == 0.0 && signbit(value)"),
        (f64::INFINITY, "isinf(value) && value > 0.0"),
        (f64::NEG_INFINITY, "isinf(value) && value < 0.0"),
        (f64::NAN, "isnan(value)"),
    ] {
        let output = compile_constant_function("floating", TypeId::FLOAT, Const::Float(value));
        let harness = format!(
            "#include <math.h>\n#include \"emela_embedded.h\"\nint main(void) {{ double value = emela_export_floating_0(); return ({expected}) ? 0 : 1; }}\n"
        );
        compile_and_run(output, &harness);
    }
}

#[test]
fn emits_inline_c_layouts_for_tuple_and_generic_enum_signatures() {
    let mut tuple_core = emela_core::Module::new();
    let tuple_ty = tuple_core.tuple(vec![TypeId::INT, TypeId::BOOL]);
    let mut tuple_builder = FnBuilder::new(&tuple_core, "tuple_identity");
    let tuple_value = tuple_builder.param("value", tuple_ty);
    tuple_builder
        .set_ret(tuple_ty)
        .set_exported()
        .term(Term::Return(Some(Operand::Var(tuple_value))));
    let tuple_function = tuple_builder.finish();
    let mut tuple_flow = Module::new(tuple_core);
    tuple_flow.add(tuple_function);
    let tuple_output = compile_flow(&tuple_flow, &Options::default())
        .expect("emit tuple parameter and return types");
    let tuple_c_type = format!("emela_type_{}", tuple_ty.index());
    let tuple_harness = format!(
        "#include <stdint.h>\n#include \"emela_embedded.h\"\nint main(void) {{ {tuple_c_type} value = {{ .f0 = INT32_C(42), .f1 = INT32_C(1) }}; {tuple_c_type} result = emela_export_tuple_identity_0(value); return result.f0 == INT32_C(42) && result.f1 == INT32_C(1) ? 0 : 1; }}\n"
    );
    compile_and_run(tuple_output, &tuple_harness);

    let mut enum_core = emela_core::Module::new();
    let value_param = enum_core.type_param(0);
    let enum_id = enum_core.add_enum(EnumDef {
        name: "Maybe".to_owned(),
        params: 1,
        variants: vec![
            VariantDef::positional("Some", vec![value_param]),
            VariantDef::unit("None"),
        ],
        error: false,
        external: false,
    });
    let enum_ty = enum_core.enum_type(enum_id, vec![TypeId::INT64]);
    let mut enum_builder = FnBuilder::new(&enum_core, "enum_identity");
    let enum_value = enum_builder.param("value", enum_ty);
    enum_builder
        .set_ret(enum_ty)
        .set_exported()
        .term(Term::Return(Some(Operand::Var(enum_value))));
    let enum_function = enum_builder.finish();
    let mut enum_flow = Module::new(enum_core);
    enum_flow.add(enum_function);
    let enum_output =
        compile_flow(&enum_flow, &Options::default()).expect("emit generic enum layout");
    let enum_c_type = format!("emela_type_{}", enum_ty.index());
    let enum_harness = format!(
        "#include <stdint.h>\n#include \"emela_embedded.h\"\nint main(void) {{ {enum_c_type} value = {{ .tag = INT32_C(0), .payload.v0.f0 = INT64_C(42) }}; {enum_c_type} result = emela_export_enum_identity_0(value); return result.tag == INT32_C(0) && result.payload.v0.f0 == INT64_C(42) ? 0 : 1; }}\n"
    );
    compile_and_run(enum_output, &enum_harness);
}

#[test]
fn constructs_payload_and_unit_enum_variants_as_inline_values() {
    let mut core = emela_core::Module::new();
    let enum_id = core.add_enum(EnumDef {
        name: "Maybe".to_owned(),
        params: 0,
        variants: vec![
            VariantDef::positional("Some", vec![TypeId::INT64]),
            VariantDef::unit("None"),
        ],
        error: false,
        external: false,
    });
    let enum_ty = core.enum_type(enum_id, Vec::new());

    let mut some_builder = FnBuilder::new(&core, "some");
    some_builder.set_ret(enum_ty).set_exported();
    let some_value = some_builder.assign(
        "value",
        enum_ty,
        Value::Make {
            ctor: CtorRef {
                enum_id,
                variant: 0,
            },
            args: vec![Operand::Const(Const::Int64(42))],
        },
    );
    some_builder.term(Term::Return(Some(Operand::Var(some_value))));
    let some_function = some_builder.finish();

    let mut none_builder = FnBuilder::new(&core, "none");
    none_builder.set_ret(enum_ty).set_exported();
    let none_value = none_builder.assign(
        "value",
        enum_ty,
        Value::Make {
            ctor: CtorRef {
                enum_id,
                variant: 1,
            },
            args: Vec::new(),
        },
    );
    none_builder.term(Term::Return(Some(Operand::Var(none_value))));
    let none_function = none_builder.finish();

    let mut flow = Module::new(core);
    flow.add(some_function);
    flow.add(none_function);
    let output = compile_flow(&flow, &Options::default()).expect("emit enum constructors");
    let enum_c_type = format!("emela_type_{}", enum_ty.index());
    let harness = format!(
        "#include <stdint.h>\n#include \"emela_embedded.h\"\nint main(void) {{ {enum_c_type} some = emela_export_some_0(); if (some.tag != INT32_C(0) || some.payload.v0.f0 != INT64_C(42)) return 1; {enum_c_type} none = emela_export_none_1(); return none.tag == INT32_C(1) ? 0 : 2; }}\n"
    );
    compile_and_run(output, &harness);
}

#[test]
fn reads_generic_enum_fields_from_inline_values() {
    let mut core = emela_core::Module::new();
    let value_param = core.type_param(0);
    let enum_id = core.add_enum(EnumDef {
        name: "Boxed".to_owned(),
        params: 1,
        variants: vec![VariantDef::positional("Box", vec![value_param])],
        error: false,
        external: false,
    });
    let enum_ty = core.enum_type(enum_id, vec![TypeId::INT64]);
    let mut builder = FnBuilder::new(&core, "field_value");
    builder.set_ret(TypeId::INT64).set_exported();
    let wrapped = builder.assign(
        "wrapped",
        enum_ty,
        Value::Make {
            ctor: CtorRef {
                enum_id,
                variant: 0,
            },
            args: vec![Operand::Const(Const::Int64(73))],
        },
    );
    let field = builder.assign(
        "field",
        TypeId::INT64,
        Value::Field {
            base: wrapped,
            ctor: CtorRef {
                enum_id,
                variant: 0,
            },
            index: 0,
        },
    );
    builder.term(Term::Return(Some(Operand::Var(field))));
    let function = builder.finish();
    let mut flow = Module::new(core);
    flow.add(function);
    let output = compile_flow(&flow, &Options::default()).expect("emit enum field read");
    compile_and_run(
        output,
        "#include <stdint.h>\n#include \"emela_embedded.h\"\nint main(void) { return emela_export_field_value_0() == INT64_C(73) ? 0 : 1; }\n",
    );
}

#[test]
fn emits_and_runs_int32_primitive_operations() {
    let cases = [
        (
            PrimOp::Add,
            vec![Const::Int(i32::MAX), Const::Int(1)],
            TypeId::INT,
            "INT32_MIN",
        ),
        (
            PrimOp::Sub,
            vec![Const::Int(i32::MIN), Const::Int(1)],
            TypeId::INT,
            "INT32_MAX",
        ),
        (
            PrimOp::Mul,
            vec![Const::Int(i32::MAX), Const::Int(2)],
            TypeId::INT,
            "-INT32_C(2)",
        ),
        (
            PrimOp::Neg,
            vec![Const::Int(i32::MIN)],
            TypeId::INT,
            "INT32_MIN",
        ),
        (
            PrimOp::Not,
            vec![Const::Bool(false)],
            TypeId::BOOL,
            "INT32_C(1)",
        ),
        (
            PrimOp::Not,
            vec![Const::Bool(true)],
            TypeId::BOOL,
            "INT32_C(0)",
        ),
        (
            PrimOp::Eq,
            vec![Const::Int(42), Const::Int(42)],
            TypeId::BOOL,
            "INT32_C(1)",
        ),
        (
            PrimOp::Ne,
            vec![Const::Int(42), Const::Int(41)],
            TypeId::BOOL,
            "INT32_C(1)",
        ),
        (
            PrimOp::Lt,
            vec![Const::Int(-2), Const::Int(-1)],
            TypeId::BOOL,
            "INT32_C(1)",
        ),
        (
            PrimOp::Le,
            vec![Const::Int(2), Const::Int(2)],
            TypeId::BOOL,
            "INT32_C(1)",
        ),
        (
            PrimOp::Gt,
            vec![Const::Int(3), Const::Int(2)],
            TypeId::BOOL,
            "INT32_C(1)",
        ),
        (
            PrimOp::Ge,
            vec![Const::Int(-1), Const::Int(-1)],
            TypeId::BOOL,
            "INT32_C(1)",
        ),
    ];

    for (op, args, result_type, expected) in cases {
        let core = emela_core::Module::new();
        let mut builder = FnBuilder::new(&core, "probe");
        builder.set_ret(result_type).set_exported();
        let result = builder.assign(
            "result",
            result_type,
            Value::Prim {
                op,
                ty: PrimTy::I32,
                args: args.into_iter().map(Operand::Const).collect(),
            },
        );
        builder.term(Term::Return(Some(Operand::Var(result))));
        let function = builder.finish();
        let mut flow = Module::new(core);
        flow.add(function);

        let output = compile_flow(&flow, &Options::default()).expect("emit Int32 primitive");
        let harness = format!(
            "#include \"emela_embedded.h\"\nint main(void) {{ return emela_export_probe_0() == {expected} ? 0 : 1; }}\n"
        );
        compile_and_run(output, &harness);
    }
}

#[test]
fn emits_and_runs_int64_primitive_operations_with_wrapping_semantics() {
    let cases = [
        (
            PrimOp::Add,
            vec![Const::Int64(i64::MAX), Const::Int64(1)],
            TypeId::INT64,
            "INT64_MIN",
        ),
        (
            PrimOp::Sub,
            vec![Const::Int64(i64::MIN), Const::Int64(1)],
            TypeId::INT64,
            "INT64_MAX",
        ),
        (
            PrimOp::Mul,
            vec![Const::Int64(i64::MAX), Const::Int64(2)],
            TypeId::INT64,
            "-INT64_C(2)",
        ),
        (
            PrimOp::Neg,
            vec![Const::Int64(i64::MIN)],
            TypeId::INT64,
            "INT64_MIN",
        ),
        (
            PrimOp::Div,
            vec![Const::Int64(-7), Const::Int64(3)],
            TypeId::INT64,
            "-INT64_C(2)",
        ),
        (
            PrimOp::Rem,
            vec![Const::Int64(-7), Const::Int64(3)],
            TypeId::INT64,
            "-INT64_C(1)",
        ),
        (
            PrimOp::Div,
            vec![Const::Int64(i64::MIN), Const::Int64(-1)],
            TypeId::INT64,
            "INT64_MIN",
        ),
        (
            PrimOp::Rem,
            vec![Const::Int64(i64::MIN), Const::Int64(-1)],
            TypeId::INT64,
            "INT64_C(0)",
        ),
        (
            PrimOp::Eq,
            vec![Const::Int64(i64::MIN), Const::Int64(i64::MIN)],
            TypeId::BOOL,
            "INT32_C(1)",
        ),
        (
            PrimOp::Ne,
            vec![Const::Int64(1), Const::Int64(2)],
            TypeId::BOOL,
            "INT32_C(1)",
        ),
        (
            PrimOp::Lt,
            vec![Const::Int64(-2), Const::Int64(-1)],
            TypeId::BOOL,
            "INT32_C(1)",
        ),
        (
            PrimOp::Le,
            vec![Const::Int64(2), Const::Int64(2)],
            TypeId::BOOL,
            "INT32_C(1)",
        ),
        (
            PrimOp::Gt,
            vec![Const::Int64(3), Const::Int64(2)],
            TypeId::BOOL,
            "INT32_C(1)",
        ),
        (
            PrimOp::Ge,
            vec![Const::Int64(i64::MAX), Const::Int64(i64::MIN)],
            TypeId::BOOL,
            "INT32_C(1)",
        ),
    ];

    for (op, args, result_type, expected) in cases {
        let output = compile_i64_prim(op, args, result_type);
        let harness = format!(
            "#include <stdint.h>\n#include <stdlib.h>\n#include \"emela_embedded.h\"\n#include \"emela_board.h\"\n_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) {{ (void)kind; (void)message; (void)length; exit(1); }}\nint main(void) {{ return emela_export_probe_0() == {expected} ? 0 : 1; }}\n"
        );
        compile_and_run(output, &harness);
    }
}

#[test]
fn int64_division_by_zero_calls_the_board_defect_hook() {
    let output = compile_i64_prim(
        PrimOp::Div,
        vec![Const::Int64(7), Const::Int64(0)],
        TypeId::INT64,
    );
    compile_and_run(
        output,
        "#include <stdlib.h>\n#include <string.h>\n#include \"emela_embedded.h\"\n#include \"emela_board.h\"\n_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) { if (kind == EMELA_DEFECT_DIVISION_BY_ZERO && length == UINT16_C(16) && memcmp(message, \"division by zero\", 16) == 0) exit(0); exit(1); }\nint main(void) { (void)emela_export_probe_0(); return 1; }\n",
    );
}

#[test]
fn emits_and_runs_float_primitive_operations() {
    let cases = [
        (
            PrimOp::Add,
            vec![Const::Float(1.25), Const::Float(2.75)],
            TypeId::FLOAT,
            "actual == 4.0",
        ),
        (
            PrimOp::Sub,
            vec![Const::Float(4.5), Const::Float(1.25)],
            TypeId::FLOAT,
            "actual == 3.25",
        ),
        (
            PrimOp::Mul,
            vec![Const::Float(1.5), Const::Float(2.0)],
            TypeId::FLOAT,
            "actual == 3.0",
        ),
        (
            PrimOp::Div,
            vec![Const::Float(7.0), Const::Float(2.0)],
            TypeId::FLOAT,
            "actual == 3.5",
        ),
        (
            PrimOp::Rem,
            vec![Const::Float(7.0), Const::Float(2.0)],
            TypeId::FLOAT,
            "actual == 1.0",
        ),
        (
            PrimOp::Neg,
            vec![Const::Float(-2.5)],
            TypeId::FLOAT,
            "actual == 2.5",
        ),
        (
            PrimOp::Eq,
            vec![Const::Float(1.25), Const::Float(1.25)],
            TypeId::BOOL,
            "actual == INT32_C(1)",
        ),
        (
            PrimOp::Ne,
            vec![Const::Float(1.25), Const::Float(2.0)],
            TypeId::BOOL,
            "actual == INT32_C(1)",
        ),
        (
            PrimOp::Lt,
            vec![Const::Float(-1.0), Const::Float(0.0)],
            TypeId::BOOL,
            "actual == INT32_C(1)",
        ),
        (
            PrimOp::Le,
            vec![Const::Float(2.0), Const::Float(2.0)],
            TypeId::BOOL,
            "actual == INT32_C(1)",
        ),
        (
            PrimOp::Gt,
            vec![Const::Float(3.0), Const::Float(2.0)],
            TypeId::BOOL,
            "actual == INT32_C(1)",
        ),
        (
            PrimOp::Ge,
            vec![Const::Float(2.0), Const::Float(2.0)],
            TypeId::BOOL,
            "actual == INT32_C(1)",
        ),
        (
            PrimOp::Eq,
            vec![Const::Float(f64::NAN), Const::Float(0.0)],
            TypeId::BOOL,
            "actual == INT32_C(0)",
        ),
        (
            PrimOp::Ne,
            vec![Const::Float(f64::NAN), Const::Float(0.0)],
            TypeId::BOOL,
            "actual == INT32_C(1)",
        ),
    ];

    for (op, args, result_type, expected) in cases {
        let output = compile_f64_prim(op, args, result_type);
        let actual_type = if result_type == TypeId::FLOAT {
            "double"
        } else {
            "int32_t"
        };
        let harness = format!(
            "#include <math.h>\n#include <stdint.h>\n#include \"emela_embedded.h\"\nint main(void) {{ {actual_type} actual = emela_export_probe_0(); return ({expected}) ? 0 : 1; }}\n"
        );
        compile_and_run(output, &harness);
    }
}

#[test]
fn compiles_core_int_add_with_twos_complement_wraparound() {
    let mut core = emela_core::Module::new();
    let main = core.declare("main", true);
    core.define(
        main,
        Vec::new(),
        TypeId::INT,
        emela_core::build::bin(
            BinOp::Add,
            OpTy::Int,
            emela_core::build::int(i32::MAX),
            emela_core::build::int(1),
        ),
    );

    let output = compile(&core, &Options::default()).expect("compile Core Int addition");
    compile_and_run(
        output,
        "#include \"emela_embedded.h\"\nint main(void) { return emela_export_main_0() == INT32_MIN ? 0 : 1; }\n",
    );
}

#[test]
fn emits_int32_division_and_remainder_with_emela_semantics() {
    for (op, args, expected) in [
        (
            PrimOp::Div,
            vec![Const::Int(-7), Const::Int(3)],
            "-INT32_C(2)",
        ),
        (
            PrimOp::Rem,
            vec![Const::Int(-7), Const::Int(3)],
            "-INT32_C(1)",
        ),
        (
            PrimOp::Div,
            vec![Const::Int(i32::MIN), Const::Int(-1)],
            "INT32_MIN",
        ),
        (
            PrimOp::Rem,
            vec![Const::Int(i32::MIN), Const::Int(-1)],
            "INT32_C(0)",
        ),
    ] {
        let output = compile_i32_prim(op, args, TypeId::INT);
        let harness = format!(
            "#include <stdlib.h>\n#include \"emela_embedded.h\"\n#include \"emela_board.h\"\n_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) {{ (void)kind; (void)message; (void)length; exit(1); }}\nint main(void) {{ return emela_export_probe_0() == {expected} ? 0 : 1; }}\n"
        );
        compile_and_run(output, &harness);
    }
}

#[test]
fn int32_division_by_zero_calls_the_board_defect_hook() {
    let output = compile_i32_prim(PrimOp::Div, vec![Const::Int(7), Const::Int(0)], TypeId::INT);
    compile_and_run(
        output,
        "#include <stdlib.h>\n#include <string.h>\n#include \"emela_embedded.h\"\n#include \"emela_board.h\"\n_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) { if (kind == EMELA_DEFECT_DIVISION_BY_ZERO && length == UINT16_C(16) && memcmp(message, \"division by zero\", 16) == 0) exit(0); exit(1); }\nint main(void) { (void)emela_export_probe_0(); return 1; }\n",
    );
}

#[test]
fn compiles_direct_calls_to_internal_and_external_c_functions() {
    let mut core = emela_core::Module::new();
    let main = core.declare("main", true);
    let helper = core.declare("helper", false);
    let external = core.declare("external_next", false);
    let external_value = core.local("value", TypeId::INT);
    core.define_external(
        external,
        vec![external_value],
        TypeId::INT,
        emela_core::External {
            bindings: vec![emela_core::ExternalBinding {
                target: "c".to_owned(),
                host: emela_core::HostRef::Global("fixture_increment".to_owned()),
            }],
        },
    );

    let helper_value = core.local("value", TypeId::INT);
    core.define(
        helper,
        vec![helper_value],
        TypeId::INT,
        emela_core::build::call(external, vec![emela_core::build::var(helper_value)]),
    );
    core.define(
        main,
        Vec::new(),
        TypeId::INT,
        emela_core::build::call(helper, vec![emela_core::build::int(41)]),
    );

    let output = compile(&core, &Options::default()).expect("compile direct calls");
    compile_and_run(
        output,
        "#include \"emela_embedded.h\"\n#include <stdint.h>\nint32_t fixture_increment(int32_t value) { return value + INT32_C(1); }\nint main(void) { return emela_export_main_0() == INT32_C(42) ? 0 : 1; }\n",
    );
}

#[test]
fn compiles_if_statements_and_boolean_short_circuiting() {
    let if_expr = emela_core::build::if_(
        emela_core::build::bin(
            BinOp::Lt,
            OpTy::Int,
            emela_core::build::int(1),
            emela_core::build::int(2),
        ),
        emela_core::build::int(42),
        emela_core::build::int(7),
    );
    compile_core_main_and_expect(if_expr, TypeId::INT, "INT32_C(42)");

    let and_expr = emela_core::build::bin(
        BinOp::And,
        OpTy::Bool,
        emela_core::build::bool(false),
        emela_core::build::bool(true),
    );
    compile_core_main_and_expect(and_expr, TypeId::BOOL, "INT32_C(0)");

    let or_expr = emela_core::build::bin(
        BinOp::Or,
        OpTy::Bool,
        emela_core::build::bool(false),
        emela_core::build::bool(true),
    );
    compile_core_main_and_expect(or_expr, TypeId::BOOL, "INT32_C(1)");
}

#[test]
fn embedded_start_runs_board_hooks_around_emela_main() {
    let mut core = emela_core::Module::new();
    let main = core.declare("main", true);
    let marker = core.declare("mark_main", false);
    core.define_external(
        marker,
        Vec::new(),
        TypeId::UNIT,
        emela_core::External {
            bindings: vec![emela_core::ExternalBinding {
                target: "c".to_owned(),
                host: emela_core::HostRef::Global("mark_main".to_owned()),
            }],
        },
    );
    core.define(
        main,
        Vec::new(),
        TypeId::UNIT,
        emela_core::build::call(marker, Vec::new()),
    );

    let output = compile(&core, &Options::default()).expect("compile embedded entry");
    compile_and_run(
        output,
        "#include <stdlib.h>\n#include \"emela_embedded.h\"\n#include \"emela_board.h\"\nstatic int init_count;\nstatic int main_count;\nstatic int idle_count;\nvoid mark_main(void) { main_count += 1; }\nvoid emela_board_init(void) { init_count += 1; }\nvoid emela_board_idle(void) { if (init_count == 1 && main_count == 1) { if (++idle_count == 2) exit(0); return; } exit(1); }\nint main(void) { emela_embedded_start(); return 1; }\n",
    );
}

fn compile_core_main_and_expect(expression: emela_core::Expr, result_type: TypeId, expected: &str) {
    let mut core = emela_core::Module::new();
    let main = core.declare("main", true);
    core.define(main, Vec::new(), result_type, expression);
    let output = compile(&core, &Options::default()).expect("compile Core control flow");
    let harness = format!(
        "#include \"emela_embedded.h\"\nint main(void) {{ return emela_export_main_0() == {expected} ? 0 : 1; }}\n"
    );
    compile_and_run(output, &harness);
}

fn compile_i32_prim(op: PrimOp, args: Vec<Const>, result_type: TypeId) -> Output {
    let core = emela_core::Module::new();
    let mut builder = FnBuilder::new(&core, "probe");
    builder.set_ret(result_type).set_exported();
    let result = builder.assign(
        "result",
        result_type,
        Value::Prim {
            op,
            ty: PrimTy::I32,
            args: args.into_iter().map(Operand::Const).collect(),
        },
    );
    builder.term(Term::Return(Some(Operand::Var(result))));
    let function = builder.finish();
    let mut flow = Module::new(core);
    flow.add(function);
    compile_flow(&flow, &Options::default()).expect("emit Int32 primitive")
}

fn compile_i64_prim(op: PrimOp, args: Vec<Const>, result_type: TypeId) -> Output {
    let core = emela_core::Module::new();
    let mut builder = FnBuilder::new(&core, "probe");
    builder.set_ret(result_type).set_exported();
    let result = builder.assign(
        "result",
        result_type,
        Value::Prim {
            op,
            ty: PrimTy::I64,
            args: args.into_iter().map(Operand::Const).collect(),
        },
    );
    builder.term(Term::Return(Some(Operand::Var(result))));
    let function = builder.finish();
    let mut flow = Module::new(core);
    flow.add(function);
    compile_flow(&flow, &Options::default()).expect("emit Int64 primitive")
}

fn compile_f64_prim(op: PrimOp, args: Vec<Const>, result_type: TypeId) -> Output {
    let core = emela_core::Module::new();
    let mut builder = FnBuilder::new(&core, "probe");
    builder.set_ret(result_type).set_exported();
    let result = builder.assign(
        "result",
        result_type,
        Value::Prim {
            op,
            ty: PrimTy::F64,
            args: args.into_iter().map(Operand::Const).collect(),
        },
    );
    builder.term(Term::Return(Some(Operand::Var(result))));
    let function = builder.finish();
    let mut flow = Module::new(core);
    flow.add(function);
    compile_flow(&flow, &Options::default()).expect("emit Float primitive")
}

fn compile_constant_function(name: &str, return_type: TypeId, value: Const) -> Output {
    let core = emela_core::Module::new();
    let mut builder = FnBuilder::new(&core, name);
    builder.set_ret(return_type).set_exported();
    builder.term(Term::Return(Some(Operand::Const(value))));
    let function = builder.finish();
    let mut flow = Module::new(core);
    flow.add(function);
    compile_flow(&flow, &Options::default()).expect("emit scalar constant")
}

fn compile_and_run(output: Output, harness_source: &str) {
    let temp = TempDir::new();
    let needs_board_lifecycle = output.files.iter().any(|file| {
        file.path == "emela_embedded.c" && file.contents.contains("void emela_embedded_start(void)")
    });
    for file in output.files {
        fs::write(temp.0.join(file.path), file.contents).expect("write generated C file");
    }
    let harness = temp.0.join("harness.c");
    let mut harness_contents = harness_source.to_owned();
    if needs_board_lifecycle {
        if !harness_source.contains("emela_board_init(")
            || !harness_source.contains("emela_board_idle(")
        {
            harness_contents.push_str("\n#include \"emela_board.h\"\n");
        }
        if !harness_source.contains("emela_board_init(") {
            harness_contents.push_str("void emela_board_init(void) {}\n");
        }
        if !harness_source.contains("emela_board_idle(") {
            harness_contents.push_str("void emela_board_idle(void) {}\n");
        }
    }
    fs::write(&harness, harness_contents).expect("write C harness");

    let executable = temp.0.join("probe");
    let compile = Command::new("cc")
        .arg("-std=c11")
        .arg("-Wall")
        .arg("-Werror")
        .arg("-I")
        .arg(&temp.0)
        .arg(temp.0.join("emela_embedded.c"))
        .arg(&harness)
        .arg("-lm")
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
