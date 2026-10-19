use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use emela_codegen_emb::{Options, Output, compile, compile_flow};
use emela_core::{
    BinOp, CtorRef, EnumDef, External, ExternalBinding, HostRef, OpTy, TypeId, VariantDef,
};
use emela_lir::{
    Callee, CaseKey, Const, FnBuilder, Module, OnFail, Operand, PrimOp, PrimTy, SwitchOn, Term,
    Value,
};

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
fn emits_utf8_and_embedded_nul_string_constants_as_views() {
    let core = emela_core::Module::new();
    let mut builder = FnBuilder::new(&core, "string_literal");
    builder.set_ret(TypeId::STRING).set_exported();
    builder.term(Term::Return(Some(Operand::Const(Const::String(
        "a\0é".to_owned(),
    )))));
    let function = builder.finish();
    let mut flow = Module::new(core);
    flow.add(function);
    let output = compile_flow(&flow, &Options::default()).expect("emit a String view constant");
    compile_and_run(
        output,
        "#include <stdint.h>\n#include <string.h>\n#include \"emela_embedded.h\"\nint main(void) { const unsigned char expected[] = { UINT8_C(0x61), UINT8_C(0x00), UINT8_C(0xc3), UINT8_C(0xa9) }; EmelaString value = emela_export_string_literal_0(); return value.length == UINT16_C(4) && memcmp(value.data, expected, sizeof expected) == 0 ? 0 : 1; }\n",
    );
}

#[test]
fn sends_dynamic_defect_string_views_to_the_board_hook() {
    let core = emela_core::Module::new();
    let mut builder = FnBuilder::new(&core, "panic_message");
    let message = builder.param("message", TypeId::STRING);
    builder.set_ret(TypeId::NEVER).set_exported();
    builder.term(Term::Defect(Operand::Var(message)));
    let function = builder.finish();
    let mut flow = Module::new(core);
    flow.add(function);
    let output = compile_flow(&flow, &Options::default()).expect("emit dynamic defect message");
    compile_and_run(
        output,
        "#include <stdlib.h>\n#include <string.h>\n#include \"emela_embedded.h\"\n#include \"emela_board.h\"\nstatic const char expected[] = \"assertion failed: x\";\n_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) { if (kind == EMELA_DEFECT_ASSERT && length == sizeof expected - 1 && memcmp(message, expected, length) == 0) exit(0); exit(1); }\nint main(void) { EmelaString message = { expected, (uint16_t)(sizeof expected - 1) }; emela_export_panic_message_0(message); return 2; }\n",
    );
}

#[test]
fn classifies_a_static_todo_defect() {
    let core = emela_core::Module::new();
    let mut flow = Module::new(core);
    let mut cleanup = FnBuilder::new(&flow.core, "cleanup_probe");
    cleanup.set_ret(TypeId::UNIT);
    let cleanup_id = flow.add(cleanup.finish_external(External {
        bindings: vec![ExternalBinding {
            target: "c".to_owned(),
            host: HostRef::Global("fixture_cleanup".to_owned()),
        }],
    }));
    let mut builder = FnBuilder::new(&flow.core, "todo");
    builder.set_ret(TypeId::NEVER).set_exported();
    builder.protect(
        None,
        |builder| {
            builder.term(Term::Defect(Operand::Const(Const::String(
                "not yet implemented".to_owned(),
            ))));
        },
        |builder| {
            builder.let_(
                None,
                Value::Call {
                    callee: Callee::Direct(cleanup_id),
                    args: Vec::new(),
                    suspends: false,
                    on_fail: None,
                },
            );
        },
    );
    builder.term(Term::Return(None));
    flow.add(builder.finish());
    let output = compile_flow(&flow, &Options::default()).expect("emit Todo defect");
    compile_and_run(
        output,
        "#include <stdlib.h>\n#include <string.h>\n#include \"emela_embedded.h\"\n#include \"emela_board.h\"\nstatic int cleanup_calls;\nstatic const char expected[] = \"not yet implemented\";\nvoid fixture_cleanup(void) { cleanup_calls += 1; }\n_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) { if (kind == EMELA_DEFECT_TODO && cleanup_calls == 0 && length == sizeof expected - 1 && memcmp(message, expected, length) == 0) exit(0); exit(1); }\nint main(void) { emela_export_todo_1(); return 2; }\n",
    );
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
fn reads_tags_from_payload_and_nullary_enums() {
    let mut core = emela_core::Module::new();
    let payload_id = core.add_enum(EnumDef {
        name: "Maybe".to_owned(),
        params: 0,
        variants: vec![
            VariantDef::positional("Some", vec![TypeId::INT64]),
            VariantDef::unit("None"),
        ],
        error: false,
        external: false,
    });
    let payload_ty = core.enum_type(payload_id, Vec::new());
    let nullary_id = core.add_enum(EnumDef {
        name: "Color".to_owned(),
        params: 0,
        variants: vec![VariantDef::unit("Red"), VariantDef::unit("Blue")],
        error: false,
        external: false,
    });
    let nullary_ty = core.enum_type(nullary_id, Vec::new());

    let mut payload_builder = FnBuilder::new(&core, "payload_tag");
    payload_builder.set_ret(TypeId::INT).set_exported();
    let payload = payload_builder.assign(
        "payload",
        payload_ty,
        Value::Make {
            ctor: CtorRef {
                enum_id: payload_id,
                variant: 1,
            },
            args: Vec::new(),
        },
    );
    let payload_tag = payload_builder.assign("tag", TypeId::INT, Value::Tag(payload));
    payload_builder.term(Term::Return(Some(Operand::Var(payload_tag))));
    let payload_function = payload_builder.finish();

    let mut nullary_builder = FnBuilder::new(&core, "nullary_tag");
    nullary_builder.set_ret(TypeId::INT).set_exported();
    let nullary = nullary_builder.assign(
        "nullary",
        nullary_ty,
        Value::Make {
            ctor: CtorRef {
                enum_id: nullary_id,
                variant: 1,
            },
            args: Vec::new(),
        },
    );
    let nullary_tag = nullary_builder.assign("tag", TypeId::INT, Value::Tag(nullary));
    nullary_builder.term(Term::Return(Some(Operand::Var(nullary_tag))));
    let nullary_function = nullary_builder.finish();

    let mut flow = Module::new(core);
    flow.add(payload_function);
    flow.add(nullary_function);
    let output = compile_flow(&flow, &Options::default()).expect("emit enum tags");
    compile_and_run(
        output,
        "#include <stdint.h>\n#include \"emela_embedded.h\"\nint main(void) { return emela_export_payload_tag_0() == INT32_C(1) && emela_export_nullary_tag_1() == INT32_C(1) ? 0 : 1; }\n",
    );
}

#[test]
fn constructs_inline_tuples_and_keeps_unit_field_positions() {
    let mut core = emela_core::Module::new();
    let tuple_ty = core.tuple(vec![TypeId::INT64, TypeId::UNIT, TypeId::BOOL]);
    let mut builder = FnBuilder::new(&core, "tuple_value");
    builder.set_ret(tuple_ty).set_exported();
    let tuple = builder.assign(
        "tuple",
        tuple_ty,
        Value::Tuple(vec![
            Operand::Const(Const::Int64(91)),
            Operand::Const(Const::Unit),
            Operand::Const(Const::Bool(true)),
        ]),
    );
    builder.term(Term::Return(Some(Operand::Var(tuple))));
    let function = builder.finish();
    let mut flow = Module::new(core);
    flow.add(function);
    let output = compile_flow(&flow, &Options::default()).expect("emit tuple construction");
    let tuple_c_type = format!("emela_type_{}", tuple_ty.index());
    let harness = format!(
        "#include <stdint.h>\n#include \"emela_embedded.h\"\nint main(void) {{ {tuple_c_type} value = emela_export_tuple_value_0(); return value.f0 == INT64_C(91) && value.f2 == INT32_C(1) ? 0 : 1; }}\n"
    );
    compile_and_run(output, &harness);
}

#[test]
fn reads_tuple_fields_and_ignores_unit_projections() {
    let mut core = emela_core::Module::new();
    let tuple_ty = core.tuple(vec![TypeId::INT64, TypeId::UNIT, TypeId::BOOL]);
    let mut builder = FnBuilder::new(&core, "tuple_field");
    builder.set_ret(TypeId::BOOL).set_exported();
    let tuple = builder.assign(
        "tuple",
        tuple_ty,
        Value::Tuple(vec![
            Operand::Const(Const::Int64(91)),
            Operand::Const(Const::Unit),
            Operand::Const(Const::Bool(true)),
        ]),
    );
    builder.let_(
        None,
        Value::TupleGet {
            base: tuple,
            index: 1,
        },
    );
    let value = builder.assign(
        "value",
        TypeId::BOOL,
        Value::TupleGet {
            base: tuple,
            index: 2,
        },
    );
    builder.term(Term::Return(Some(Operand::Var(value))));
    let function = builder.finish();
    let mut flow = Module::new(core);
    flow.add(function);
    let output = compile_flow(&flow, &Options::default()).expect("emit tuple projections");
    compile_and_run(
        output,
        "#include <stdint.h>\n#include \"emela_embedded.h\"\nint main(void) { return emela_export_tuple_field_0() == INT32_C(1) ? 0 : 1; }\n",
    );
}

#[test]
fn emits_and_runs_int32_and_int64_literal_switches() {
    let core = emela_core::Module::new();
    let mut int_builder = FnBuilder::new(&core, "choose_int");
    let int_input = int_builder.param("input", TypeId::INT);
    int_builder.set_ret(TypeId::INT).set_exported();
    let int_result = int_builder.var("result", TypeId::INT);
    let int_min = int_builder.seq(|builder| {
        builder.term(Term::Yield(Some(Operand::Const(Const::Int(11)))));
    });
    let int_zero = int_builder.seq(|builder| {
        builder.term(Term::Yield(Some(Operand::Const(Const::Int(22)))));
    });
    let int_default = int_builder.seq(|builder| {
        builder.term(Term::Yield(Some(Operand::Const(Const::Int(33)))));
    });
    int_builder.switch(
        Some(int_result),
        SwitchOn::Lit(int_input),
        vec![
            (CaseKey::Lit(Const::Int(i32::MIN)), int_min),
            (CaseKey::Lit(Const::Int(0)), int_zero),
        ],
        Some(int_default),
    );
    int_builder.term(Term::Return(Some(Operand::Var(int_result))));
    let int_function = int_builder.finish();

    let mut int64_builder = FnBuilder::new(&core, "choose_int64");
    let int64_input = int64_builder.param("input", TypeId::INT64);
    int64_builder.set_ret(TypeId::INT).set_exported();
    let int64_result = int64_builder.var("result", TypeId::INT);
    let int64_min = int64_builder.seq(|builder| {
        builder.term(Term::Yield(Some(Operand::Const(Const::Int(44)))));
    });
    let int64_max = int64_builder.seq(|builder| {
        builder.term(Term::Yield(Some(Operand::Const(Const::Int(55)))));
    });
    let int64_default = int64_builder.seq(|builder| {
        builder.term(Term::Yield(Some(Operand::Const(Const::Int(66)))));
    });
    int64_builder.switch(
        Some(int64_result),
        SwitchOn::Lit(int64_input),
        vec![
            (CaseKey::Lit(Const::Int64(i64::MIN)), int64_min),
            (CaseKey::Lit(Const::Int64(i64::MAX)), int64_max),
        ],
        Some(int64_default),
    );
    int64_builder.term(Term::Return(Some(Operand::Var(int64_result))));
    let int64_function = int64_builder.finish();

    let mut flow = Module::new(core);
    flow.add(int_function);
    flow.add(int64_function);
    let output = compile_flow(&flow, &Options::default()).expect("emit literal switches");
    compile_and_run(
        output,
        "#include <stdint.h>\n#include \"emela_embedded.h\"\nint main(void) { return emela_export_choose_int_0(INT32_MIN) == INT32_C(11) && emela_export_choose_int_0(INT32_C(0)) == INT32_C(22) && emela_export_choose_int_0(INT32_C(1)) == INT32_C(33) && emela_export_choose_int64_1(INT64_MIN) == INT32_C(44) && emela_export_choose_int64_1(INT64_MAX) == INT32_C(55) && emela_export_choose_int64_1(INT64_C(0)) == INT32_C(66) ? 0 : 1; }\n",
    );
}

#[test]
fn emits_and_runs_exhaustive_payload_enum_switch_and_defects_on_invalid_tag() {
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
    let mut builder = FnBuilder::new(&core, "classify");
    let input = builder.param("input", enum_ty);
    builder.set_ret(TypeId::INT).set_exported();
    let result = builder.var("result", TypeId::INT);
    let some = builder.seq(|builder| {
        builder.term(Term::Yield(Some(Operand::Const(Const::Int(71)))));
    });
    let none = builder.seq(|builder| {
        builder.term(Term::Yield(Some(Operand::Const(Const::Int(82)))));
    });
    builder.switch(
        Some(result),
        SwitchOn::Tag(input),
        vec![(CaseKey::Variant(0), some), (CaseKey::Variant(1), none)],
        None,
    );
    builder.term(Term::Return(Some(Operand::Var(result))));
    let function = builder.finish();
    let mut flow = Module::new(core);
    flow.add(function);
    let output = compile_flow(&flow, &Options::default()).expect("emit enum tag switch");
    let enum_c_type = format!("emela_type_{}", enum_ty.index());
    let harness = format!(
        "#include <stdint.h>\n#include <stdlib.h>\n#include <string.h>\n#include \"emela_embedded.h\"\n#include \"emela_board.h\"\n_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) {{ if (kind == EMELA_DEFECT_NO_MATCH && length == UINT16_C(8) && memcmp(message, \"no match\", 8) == 0) exit(0); exit(1); }}\nint main(void) {{ {enum_c_type} some = {{ .tag = INT32_C(0), .payload.v0.f0 = INT64_C(7) }}; {enum_c_type} none = {{ .tag = INT32_C(1) }}; if (emela_export_classify_0(some) != INT32_C(71) || emela_export_classify_0(none) != INT32_C(82)) return 1; {enum_c_type} invalid = {{ .tag = INT32_C(7) }}; (void)emela_export_classify_0(invalid); return 2; }}\n"
    );
    compile_and_run(output, &harness);
}

#[test]
fn emits_and_runs_switches_on_nullary_enum_representations() {
    let mut core = emela_core::Module::new();
    let enum_id = core.add_enum(EnumDef {
        name: "Color".to_owned(),
        params: 0,
        variants: vec![VariantDef::unit("Red"), VariantDef::unit("Blue")],
        error: false,
        external: false,
    });
    let enum_ty = core.enum_type(enum_id, Vec::new());
    let mut builder = FnBuilder::new(&core, "color_code");
    let input = builder.param("input", enum_ty);
    builder.set_ret(TypeId::INT).set_exported();
    let result = builder.var("result", TypeId::INT);
    let red = builder.seq(|builder| {
        builder.term(Term::Yield(Some(Operand::Const(Const::Int(3)))));
    });
    let blue = builder.seq(|builder| {
        builder.term(Term::Yield(Some(Operand::Const(Const::Int(5)))));
    });
    builder.switch(
        Some(result),
        SwitchOn::Tag(input),
        vec![(CaseKey::Variant(0), red), (CaseKey::Variant(1), blue)],
        None,
    );
    builder.term(Term::Return(Some(Operand::Var(result))));
    let function = builder.finish();
    let mut flow = Module::new(core);
    flow.add(function);
    let output = compile_flow(&flow, &Options::default()).expect("emit nullary enum switch");
    compile_and_run(
        output,
        "#include <stdlib.h>\n#include \"emela_embedded.h\"\n#include \"emela_board.h\"\n_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) { (void)kind; (void)message; (void)length; exit(1); }\nint main(void) { return emela_export_color_code_0(INT32_C(0)) == INT32_C(3) && emela_export_color_code_0(INT32_C(1)) == INT32_C(5) ? 0 : 1; }\n",
    );
}

#[test]
fn emits_and_runs_a_loop_with_an_initial_value_and_yield() {
    let core = emela_core::Module::new();
    let mut builder = FnBuilder::new(&core, "loop_value");
    builder.set_ret(TypeId::INT).set_exported();
    let state = builder.var("state", TypeId::INT);
    let result = builder.var("result", TypeId::INT);
    builder.loop_(
        Some(result),
        vec![(state, Operand::Const(Const::Int(97)))],
        |builder| {
            builder.term(Term::Yield(Some(Operand::Var(state))));
        },
    );
    builder.term(Term::Return(Some(Operand::Var(result))));
    let function = builder.finish();
    let mut flow = Module::new(core);
    flow.add(function);
    let output = compile_flow(&flow, &Options::default()).expect("emit a value-yielding loop");
    compile_and_run(
        output,
        "#include <stdint.h>\n#include \"emela_embedded.h\"\nint main(void) { return emela_export_loop_value_0() == INT32_C(97) ? 0 : 1; }\n",
    );
}

#[test]
fn emits_and_runs_recur_with_multiple_loop_variables() {
    let core = emela_core::Module::new();
    let mut builder = FnBuilder::new(&core, "count_down");
    let start = builder.param("start", TypeId::INT);
    builder.set_ret(TypeId::INT).set_exported();
    let counter = builder.var("counter", TypeId::INT);
    let sum = builder.var("sum", TypeId::INT);
    let result = builder.var("result", TypeId::INT);
    builder.loop_(
        Some(result),
        vec![
            (counter, Operand::Var(start)),
            (sum, Operand::Const(Const::Int(0))),
        ],
        |builder| {
            let is_zero = builder.assign(
                "is_zero",
                TypeId::BOOL,
                Value::Prim {
                    op: PrimOp::Eq,
                    ty: PrimTy::I32,
                    args: vec![Operand::Var(counter), Operand::Const(Const::Int(0))],
                },
            );
            let branch = builder.var("branch", TypeId::INT);
            builder.if_(
                Some(branch),
                is_zero,
                |builder| {
                    builder.term(Term::Yield(Some(Operand::Var(sum))));
                },
                |builder| {
                    let next_counter = builder.assign(
                        "next_counter",
                        TypeId::INT,
                        Value::Prim {
                            op: PrimOp::Sub,
                            ty: PrimTy::I32,
                            args: vec![Operand::Var(counter), Operand::Const(Const::Int(1))],
                        },
                    );
                    let next_sum = builder.assign(
                        "next_sum_value",
                        TypeId::INT,
                        Value::Prim {
                            op: PrimOp::Add,
                            ty: PrimTy::I32,
                            args: vec![Operand::Var(sum), Operand::Var(counter)],
                        },
                    );
                    builder.term(Term::Recur(vec![
                        Operand::Var(next_counter),
                        Operand::Var(next_sum),
                    ]));
                },
            );
            builder.term(Term::Yield(Some(Operand::Var(branch))));
        },
    );
    builder.term(Term::Return(Some(Operand::Var(result))));
    let function = builder.finish();
    let mut flow = Module::new(core);
    flow.add(function);
    let output = compile_flow(&flow, &Options::default()).expect("emit Recur loop");
    compile_and_run(
        output,
        "#include <stdint.h>\n#include \"emela_embedded.h\"\nint main(void) { return emela_export_count_down_0(INT32_C(5)) == INT32_C(15) ? 0 : 1; }\n",
    );
}

#[test]
fn emits_and_runs_a_block_that_yields_a_value() {
    let core = emela_core::Module::new();
    let mut builder = FnBuilder::new(&core, "block_value");
    builder.set_ret(TypeId::INT).set_exported();
    let result = builder.var("result", TypeId::INT);
    builder.block(Some(result), "exit", |builder, _label| {
        builder.term(Term::Yield(Some(Operand::Const(Const::Int(123)))));
    });
    builder.term(Term::Return(Some(Operand::Var(result))));
    let function = builder.finish();
    let mut flow = Module::new(core);
    flow.add(function);
    let output = compile_flow(&flow, &Options::default()).expect("emit a yielding block");
    compile_and_run(
        output,
        "#include <stdint.h>\n#include \"emela_embedded.h\"\nint main(void) { return emela_export_block_value_0() == INT32_C(123) ? 0 : 1; }\n",
    );
}

#[test]
fn emits_and_runs_break_to_an_enclosing_block() {
    let core = emela_core::Module::new();
    let mut builder = FnBuilder::new(&core, "break_value");
    builder.set_ret(TypeId::INT).set_exported();
    let result = builder.var("result", TypeId::INT);
    builder.block(Some(result), "outer", |builder, outer_label| {
        builder.block(None, "inner", |builder, _inner_label| {
            builder.term(Term::Break {
                label: outer_label,
                value: Some(Operand::Const(Const::Int(137))),
            });
        });
        builder.term(Term::Yield(Some(Operand::Const(Const::Int(0)))));
    });
    builder.term(Term::Return(Some(Operand::Var(result))));
    let function = builder.finish();
    let mut flow = Module::new(core);
    flow.add(function);
    let output = compile_flow(&flow, &Options::default()).expect("emit Break to an outer block");
    compile_and_run(
        output,
        "#include <stdint.h>\n#include \"emela_embedded.h\"\nint main(void) { return emela_export_break_value_0() == INT32_C(137) ? 0 : 1; }\n",
    );
}

#[test]
fn runs_protect_cleanup_on_yield_break_and_return() {
    let core = emela_core::Module::new();
    let mut hook_builder = FnBuilder::new(&core, "cleanup_hook");
    hook_builder.set_ret(TypeId::UNIT);
    let hook = hook_builder.finish_external(emela_core::External {
        bindings: vec![emela_core::ExternalBinding {
            target: "c".to_owned(),
            host: emela_core::HostRef::Global("fixture_cleanup".to_owned()),
        }],
    });
    let mut flow = Module::new(core);
    let hook_id = flow.add(hook);

    let cleanup_call = || Value::Call {
        callee: Callee::Direct(hook_id),
        args: Vec::new(),
        suspends: false,
        on_fail: None,
    };

    let mut normal = FnBuilder::new(&flow.core, "protect_yield");
    normal.set_ret(TypeId::INT).set_exported();
    let normal_result = normal.var("result", TypeId::INT);
    normal.protect(
        Some(normal_result),
        |builder| builder.term(Term::Yield(Some(Operand::Const(Const::Int(42))))),
        |builder| builder.let_(None, cleanup_call()),
    );
    normal.term(Term::Return(Some(Operand::Var(normal_result))));
    flow.add(normal.finish());

    let mut returning = FnBuilder::new(&flow.core, "protect_return");
    returning.set_ret(TypeId::INT).set_exported();
    returning.protect(
        None,
        |builder| builder.term(Term::Return(Some(Operand::Const(Const::Int(58))))),
        |builder| builder.let_(None, cleanup_call()),
    );
    returning.term(Term::Return(Some(Operand::Const(Const::Int(0)))));
    flow.add(returning.finish());

    let mut breaking = FnBuilder::new(&flow.core, "protect_break");
    breaking.set_ret(TypeId::INT).set_exported();
    let break_result = breaking.var("result", TypeId::INT);
    breaking.block(Some(break_result), "exit", |builder, label| {
        builder.protect(
            None,
            |builder| {
                builder.term(Term::Break {
                    label,
                    value: Some(Operand::Const(Const::Int(61))),
                });
            },
            |builder| builder.let_(None, cleanup_call()),
        );
        builder.term(Term::Yield(Some(Operand::Const(Const::Int(0)))));
    });
    breaking.term(Term::Return(Some(Operand::Var(break_result))));
    flow.add(breaking.finish());

    let output = compile_flow(&flow, &Options::default()).expect("emit protected control flow");
    compile_and_run(
        output,
        "#include <stdint.h>\n#include \"emela_embedded.h\"\nstatic int cleanup_calls;\nvoid fixture_cleanup(void) { cleanup_calls += 1; }\nint main(void) { if (emela_export_protect_yield_1() != INT32_C(42) || cleanup_calls != 1) return 1; cleanup_calls = 0; if (emela_export_protect_return_2() != INT32_C(58) || cleanup_calls != 1) return 2; cleanup_calls = 0; if (emela_export_protect_break_3() != INT32_C(61) || cleanup_calls != 1) return 3; return 0; }\n",
    );
}

#[test]
fn propagates_catches_and_reports_fallible_calls() {
    let mut core = emela_core::Module::new();
    let error_enum = core.add_enum(EnumDef {
        name: "ProbeError".to_owned(),
        params: 0,
        variants: vec![VariantDef::positional("ProbeError", vec![TypeId::INT])],
        error: true,
        external: false,
    });
    let error_ty = core.enum_type(error_enum, Vec::new());

    let mut failure = FnBuilder::new(&core, "fail_or_return_probe_error");
    let input = failure.param("input", TypeId::INT);
    failure.set_ret(TypeId::INT).set_fails();
    let is_zero = failure.assign(
        "is_zero",
        TypeId::BOOL,
        Value::Prim {
            op: PrimOp::Eq,
            ty: PrimTy::I32,
            args: vec![Operand::Var(input), Operand::Const(Const::Int(0))],
        },
    );
    let result = failure.var("result", TypeId::INT);
    failure.if_(
        Some(result),
        Operand::Var(is_zero),
        |builder| {
            let error = builder.assign(
                "error",
                error_ty,
                Value::Make {
                    ctor: CtorRef {
                        enum_id: error_enum,
                        variant: 0,
                    },
                    args: vec![Operand::Const(Const::Int(73))],
                },
            );
            builder.term(Term::Fail(Operand::Var(error)));
        },
        |builder| builder.term(Term::Yield(Some(Operand::Var(input)))),
    );
    failure.term(Term::Return(Some(Operand::Var(result))));
    let failure = failure.finish();
    let mut flow = Module::new(core);
    let failure_id = flow.add(failure);
    let mut cleanup = FnBuilder::new(&flow.core, "cleanup_probe");
    cleanup.set_ret(TypeId::UNIT);
    let cleanup_id = flow.add(cleanup.finish_external(External {
        bindings: vec![ExternalBinding {
            target: "c".to_owned(),
            host: HostRef::Global("fixture_cleanup".to_owned()),
        }],
    }));

    let mut caught = FnBuilder::new(&flow.core, "catch_probe_error");
    let input = caught.param("input", TypeId::INT);
    caught.set_ret(TypeId::INT).set_exported();
    let result = caught.var("result", TypeId::INT);
    let caught_error = caught.var("caught_error", TypeId::ANY);
    caught.block(Some(result), "done", |builder, done_label| {
        builder.block(Some(caught_error), "caught", |builder, caught_label| {
            let call_result = builder.var("call_result", TypeId::INT);
            builder.protect(
                Some(call_result),
                |builder| {
                    let call_value = builder.assign(
                        "call_value",
                        TypeId::INT,
                        Value::Call {
                            callee: Callee::Direct(failure_id),
                            args: vec![Operand::Var(input)],
                            suspends: false,
                            on_fail: Some(OnFail::Break(caught_label)),
                        },
                    );
                    builder.term(Term::Yield(Some(Operand::Var(call_value))));
                },
                |builder| {
                    builder.let_(
                        None,
                        Value::Call {
                            callee: Callee::Direct(cleanup_id),
                            args: Vec::new(),
                            suspends: false,
                            on_fail: None,
                        },
                    );
                },
            );
            builder.term(Term::Break {
                label: done_label,
                value: Some(Operand::Var(call_result)),
            });
        });
        let is_probe_error = builder.assign(
            "is_probe_error",
            TypeId::BOOL,
            Value::IsError {
                error: caught_error,
                enum_id: error_enum,
            },
        );
        builder.if_(
            None,
            Operand::Var(is_probe_error),
            |builder| {
                let concrete_error = builder.assign(
                    "concrete_error",
                    error_ty,
                    Value::AsError {
                        error: caught_error,
                        enum_id: error_enum,
                    },
                );
                let payload = builder.assign(
                    "payload",
                    TypeId::INT,
                    Value::Field {
                        base: concrete_error,
                        ctor: CtorRef {
                            enum_id: error_enum,
                            variant: 0,
                        },
                        index: 0,
                    },
                );
                builder.term(Term::Break {
                    label: done_label,
                    value: Some(Operand::Var(payload)),
                });
            },
            |builder| {
                builder.term(Term::Break {
                    label: done_label,
                    value: Some(Operand::Const(Const::Int(-1))),
                });
            },
        );
        builder.term(Term::Return(Some(Operand::Const(Const::Int(-2)))));
    });
    caught.term(Term::Return(Some(Operand::Var(result))));
    flow.add(caught.finish());

    let mut propagate = FnBuilder::new(&flow.core, "propagate_probe_error");
    let input = propagate.param("input", TypeId::INT);
    propagate.set_ret(TypeId::INT).set_fails().set_exported();
    let result = propagate.assign(
        "result",
        TypeId::INT,
        Value::Call {
            callee: Callee::Direct(failure_id),
            args: vec![Operand::Var(input)],
            suspends: false,
            on_fail: Some(OnFail::Propagate),
        },
    );
    propagate.term(Term::Return(Some(Operand::Var(result))));
    flow.add(propagate.finish());

    let output = compile_flow(&flow, &Options::default()).expect("emit fallible functions");
    compile_and_run(
        output,
        "#include <stdlib.h>\n#include <string.h>\n#include \"emela_embedded.h\"\n#include \"emela_board.h\"\nstatic int cleanup_calls;\nvoid fixture_cleanup(void) { cleanup_calls += 1; }\n_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) { if (kind == EMELA_DEFECT_UNHANDLED_ERROR && length == UINT16_C(15) && memcmp(message, \"unhandled error\", 15) == 0) exit(0); exit(1); }\nint main(void) { if (emela_export_catch_probe_error_2(INT32_C(0)) != INT32_C(73) || cleanup_calls != 1) return 1; if (emela_export_catch_probe_error_2(INT32_C(9)) != INT32_C(9) || cleanup_calls != 2) return 2; if (emela_export_propagate_probe_error_3(INT32_C(9)) != INT32_C(9)) return 3; (void)emela_export_propagate_probe_error_3(INT32_C(0)); return 4; }\n",
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
