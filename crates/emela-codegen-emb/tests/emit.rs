use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use emela_codegen_emb::{ErrorKind, Options, Output, compile, compile_flow};
use emela_core::{
    BinOp, Builtin, CtorRef, EnumDef, External, ExternalBinding, HostRef, OpTy, Type, TypeId,
    VariantDef,
};
use emela_lir::{
    Callee, CaseKey, Const, FnBuilder, Module, OnFail, Operand, PrimOp, PrimTy, RuntimeOp,
    SwitchOn, Term, Value,
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
fn emits_empty_list_and_checks_list_emptiness() {
    let mut core = emela_core::Module::new();
    let list_int = core.types.intern(Type::List(TypeId::INT));

    let empty_function = {
        let mut builder = FnBuilder::new(&core, "empty_list");
        builder.set_ret(TypeId::BOOL).set_exported();
        let empty = builder.assign(
            "empty",
            list_int,
            Value::List {
                elems: Vec::new(),
                tail: None,
            },
        );
        let is_empty = builder.assign("is_empty", TypeId::BOOL, Value::ListIsEmpty(empty));
        builder.term(Term::Return(Some(is_empty.into())));
        builder.finish()
    };

    let check_function = {
        let mut builder = FnBuilder::new(&core, "check_list");
        builder.set_ret(TypeId::BOOL).set_exported();
        let list = builder.param("list", list_int);
        let is_empty = builder.assign("is_empty", TypeId::BOOL, Value::ListIsEmpty(list));
        builder.term(Term::Return(Some(is_empty.into())));
        builder.finish()
    };

    let empty_value_function = {
        let mut builder = FnBuilder::new(&core, "empty_value");
        builder.set_ret(list_int).set_exported();
        let empty = builder.assign(
            "empty",
            list_int,
            Value::List {
                elems: Vec::new(),
                tail: None,
            },
        );
        builder.term(Term::Return(Some(empty.into())));
        builder.finish()
    };

    let mut flow = Module::new(core);
    flow.add(empty_function);
    flow.add(check_function);
    flow.add(empty_value_function);

    let output = compile_flow(&flow, &Options::default()).expect("emit empty List operations");
    compile_and_run(
        output,
        "#include \"emela_embedded.h\"\nint main(void) { int value = 0; return emela_export_empty_list_0() == INT32_C(1) && emela_export_check_list_1((void *)0) == INT32_C(1) && emela_export_check_list_1(&value) == INT32_C(0) && emela_export_empty_value_2() == (void *)0 ? 0 : 1; }\n",
    );
}

#[test]
fn requires_fixed_heap_capacity_for_nonempty_list_values() {
    let mut core = emela_core::Module::new();
    let list_int = core.types.intern(Type::List(TypeId::INT));
    let function = {
        let mut builder = FnBuilder::new(&core, "first_item");
        builder.set_ret(TypeId::INT).set_exported();
        let list = builder.assign(
            "list",
            list_int,
            Value::List {
                elems: vec![Const::Int(42).into()],
                tail: None,
            },
        );
        let head = builder.assign("head", TypeId::INT, Value::ListHead(list));
        builder.term(Term::Return(Some(head.into())));
        builder.finish()
    };
    let mut flow = Module::new(core);
    flow.add(function);

    let errors = compile_flow(&flow, &Options::default()).expect_err("require a configured heap");
    assert_eq!(errors[0].kind, ErrorKind::Unsupported);
    assert!(errors[0].message.contains("heap_bytes = 0"));
    assert_eq!(Options::default().with_heap_bytes(512).heap_bytes(), 512);
}

#[test]
fn requires_fixed_heap_capacity_for_returned_list_aliases() {
    let mut core = emela_core::Module::new();
    let list_int = core.types.intern(Type::List(TypeId::INT));
    let function = {
        let mut builder = FnBuilder::new(&core, "identity_list");
        builder.set_ret(list_int).set_exported();
        let list = builder.param("list", list_int);
        builder.term(Term::Return(Some(list.into())));
        builder.finish()
    };
    let mut flow = Module::new(core);
    flow.add(function);

    let errors = compile_flow(&flow, &Options::default())
        .expect_err("returning a borrowed List requires reference management");
    assert_eq!(errors[0].kind, ErrorKind::Unsupported);
    assert!(errors[0].message.contains("heap_bytes = 0"));
}

#[test]
fn requires_fixed_heap_capacity_for_a_list_tail_alias() {
    let mut core = emela_core::Module::new();
    let list_int = core.types.intern(Type::List(TypeId::INT));
    let function = {
        let mut builder = FnBuilder::new(&core, "alias_list");
        builder.set_ret(list_int).set_exported();
        let list = builder.param("list", list_int);
        let alias = builder.assign(
            "alias",
            list_int,
            Value::List {
                elems: Vec::new(),
                tail: Some(list.into()),
            },
        );
        builder.term(Term::Return(Some(alias.into())));
        builder.finish()
    };
    let mut flow = Module::new(core);
    flow.add(function);

    let errors = compile_flow(&flow, &Options::default())
        .expect_err("a List alias needs retain/release support");
    assert_eq!(errors[0].kind, ErrorKind::Unsupported);
    assert!(errors[0].message.contains("heap_bytes = 0"));
}

#[test]
fn requires_fixed_heap_capacity_for_list_head() {
    let mut core = emela_core::Module::new();
    let list_int = core.types.intern(Type::List(TypeId::INT));
    let function = {
        let mut builder = FnBuilder::new(&core, "first_item");
        builder.set_ret(TypeId::INT).set_exported();
        let list = builder.param("list", list_int);
        let head = builder.assign("head", TypeId::INT, Value::ListHead(list));
        builder.term(Term::Return(Some(head.into())));
        builder.finish()
    };
    let mut flow = Module::new(core);
    flow.add(function);

    let errors = compile_flow(&flow, &Options::default())
        .expect_err("reading a List head requires heap-backed List support");
    assert_eq!(errors[0].kind, ErrorKind::Unsupported);
    assert!(errors[0].message.contains("heap_bytes = 0"));
}

#[test]
fn requires_fixed_heap_capacity_for_list_tail() {
    let mut core = emela_core::Module::new();
    let list_int = core.types.intern(Type::List(TypeId::INT));
    let function = {
        let mut builder = FnBuilder::new(&core, "list_tail");
        builder.set_ret(list_int).set_exported();
        let list = builder.param("list", list_int);
        let tail = builder.assign("tail", list_int, Value::ListTail(list));
        builder.term(Term::Return(Some(tail.into())));
        builder.finish()
    };
    let mut flow = Module::new(core);
    flow.add(function);

    let errors = compile_flow(&flow, &Options::default())
        .expect_err("reading a List tail requires reference management");
    assert_eq!(errors[0].kind, ErrorKind::Unsupported);
    assert!(errors[0].message.contains("heap_bytes = 0"));
}

#[test]
fn reads_list_heads_and_keeps_shared_tails_alive() {
    let mut core = emela_core::Module::new();
    let list_int = core.types.intern(Type::List(TypeId::INT));
    let identity = {
        let mut builder = FnBuilder::new(&core, "identity_list");
        builder.set_ret(list_int);
        let list = builder.param("list", list_int);
        builder.term(Term::Return(Some(list.into())));
        builder.finish()
    };
    let mut flow = Module::new(core);
    let identity_id = flow.add(identity);
    let sum = {
        let mut builder = FnBuilder::new(&flow.core, "sum_first_two");
        builder.set_ret(TypeId::INT).set_exported();
        let list = builder.assign(
            "list",
            list_int,
            Value::List {
                elems: vec![
                    Const::Int(10).into(),
                    Const::Int(20).into(),
                    Const::Int(30).into(),
                ],
                tail: None,
            },
        );
        let alias = builder.assign(
            "alias",
            list_int,
            Value::Call {
                callee: Callee::Direct(identity_id),
                args: vec![list.into()],
                suspends: false,
                on_fail: None,
            },
        );
        let first = builder.assign("first", TypeId::INT, Value::ListHead(list));
        let tail = builder.assign("tail", list_int, Value::ListTail(alias));
        let second = builder.assign("second", TypeId::INT, Value::ListHead(tail));
        let result = builder.assign(
            "sum",
            TypeId::INT,
            Value::Prim {
                op: PrimOp::Add,
                ty: PrimTy::I32,
                args: vec![first.into(), second.into()],
            },
        );
        builder.term(Term::Return(Some(result.into())));
        builder.finish()
    };
    flow.add(sum);
    let list_unit = flow.core.types.intern(Type::List(TypeId::UNIT));
    let unit_head = {
        let mut builder = FnBuilder::new(&flow.core, "unit_head");
        builder.set_ret(TypeId::BOOL).set_exported();
        let list = builder.assign(
            "list",
            list_unit,
            Value::List {
                elems: vec![Const::Unit.into()],
                tail: None,
            },
        );
        builder.let_(None, Value::ListHead(list));
        builder.term(Term::Return(Some(Const::Bool(true).into())));
        builder.finish()
    };
    flow.add(unit_head);

    let output = compile_flow(&flow, &Options::default().with_heap_bytes(1024))
        .expect("emit ListHead and ListTail operations");
    compile_and_run(
        output,
        "#include \"emela_embedded.h\"\n#include \"emela_board.h\"\n#include <stdlib.h>\n_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) { (void)message; (void)length; exit(1); }\nint main(void) { for (int i = 0; i < 1000; i++) if (emela_export_sum_first_two_1() != INT32_C(30)) return 2; return emela_export_unit_head_2() == INT32_C(1) ? 0 : 3; }\n",
    );
}

#[test]
fn stores_and_releases_nested_lists() {
    let mut core = emela_core::Module::new();
    let list_int = core.types.intern(Type::List(TypeId::INT));
    let nested_list = core.types.intern(Type::List(list_int));
    let make_nested = {
        let mut builder = FnBuilder::new(&core, "extract_inner_list");
        builder.set_ret(list_int);
        let inner = builder.assign(
            "inner",
            list_int,
            Value::List {
                elems: vec![Const::Int(42).into()],
                tail: None,
            },
        );
        let outer = builder.assign(
            "outer",
            nested_list,
            Value::List {
                elems: vec![inner.into()],
                tail: None,
            },
        );
        let extracted = builder.assign("extracted", list_int, Value::ListHead(outer));
        builder.term(Term::Return(Some(extracted.into())));
        builder.finish()
    };
    let mut flow = Module::new(core);
    let make_nested_id = flow.add(make_nested);
    let nested_head = {
        let mut builder = FnBuilder::new(&flow.core, "nested_head");
        builder.set_ret(TypeId::INT).set_exported();
        let inner = builder.assign(
            "inner",
            list_int,
            Value::Call {
                callee: Callee::Direct(make_nested_id),
                args: Vec::new(),
                suspends: false,
                on_fail: None,
            },
        );
        let head = builder.assign("head", TypeId::INT, Value::ListHead(inner));
        builder.term(Term::Return(Some(head.into())));
        builder.finish()
    };
    flow.add(nested_head);

    let output = compile_flow(&flow, &Options::default().with_heap_bytes(1024))
        .expect("emit nested List ownership");
    compile_and_run(
        output,
        "#include \"emela_embedded.h\"\n#include \"emela_board.h\"\n#include <stdlib.h>\n_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) { (void)kind; (void)message; (void)length; exit(1); }\nint main(void) { for (int i = 0; i < 1000; i++) if (emela_export_nested_head_1() != INT32_C(42)) return 2; return 0; }\n",
    );
}

#[test]
fn stores_and_releases_list_references_inside_tuples_and_list_elements() {
    let mut core = emela_core::Module::new();
    let list_int = core.types.intern(Type::List(TypeId::INT));
    let tuple_with_list = core.types.intern(Type::Tuple(vec![list_int]));
    let list_tuple = core.types.intern(Type::List(tuple_with_list));

    let tuple_function = {
        let mut builder = FnBuilder::new(&core, "tuple_list_head");
        builder.set_ret(TypeId::INT).set_exported();
        let inner = builder.assign(
            "inner",
            list_int,
            Value::List {
                elems: vec![Const::Int(42).into()],
                tail: None,
            },
        );
        let tuple = builder.assign("tuple", tuple_with_list, Value::Tuple(vec![inner.into()]));
        let extracted = builder.assign(
            "extracted",
            list_int,
            Value::TupleGet {
                base: tuple,
                index: 0,
            },
        );
        let head = builder.assign("head", TypeId::INT, Value::ListHead(extracted));
        builder.term(Term::Return(Some(head.into())));
        builder.finish()
    };

    let list_element_function = {
        let mut builder = FnBuilder::new(&core, "list_tuple_head");
        builder.set_ret(TypeId::INT).set_exported();
        let inner = builder.assign(
            "inner",
            list_int,
            Value::List {
                elems: vec![Const::Int(73).into()],
                tail: None,
            },
        );
        let tuple = builder.assign("tuple", tuple_with_list, Value::Tuple(vec![inner.into()]));
        let outer = builder.assign(
            "outer",
            list_tuple,
            Value::List {
                elems: vec![tuple.into()],
                tail: None,
            },
        );
        let extracted_tuple =
            builder.assign("extracted_tuple", tuple_with_list, Value::ListHead(outer));
        let extracted_list = builder.assign(
            "extracted_list",
            list_int,
            Value::TupleGet {
                base: extracted_tuple,
                index: 0,
            },
        );
        let head = builder.assign("head", TypeId::INT, Value::ListHead(extracted_list));
        builder.term(Term::Return(Some(head.into())));
        builder.finish()
    };

    let returned_tuple_function = {
        let mut builder = FnBuilder::new(&core, "tuple_value");
        builder.set_ret(tuple_with_list).set_exported();
        let inner = builder.assign(
            "inner",
            list_int,
            Value::List {
                elems: vec![Const::Int(5).into()],
                tail: None,
            },
        );
        let tuple = builder.assign("tuple", tuple_with_list, Value::Tuple(vec![inner.into()]));
        builder.term(Term::Return(Some(tuple.into())));
        builder.finish()
    };

    let mut flow = Module::new(core);
    flow.add(tuple_function);
    flow.add(list_element_function);
    flow.add(returned_tuple_function);

    let output = compile_flow(&flow, &Options::default().with_heap_bytes(1024))
        .expect("emit List references inside inline tuple values");
    let tuple_c_type = format!("emela_type_{}", tuple_with_list.index());
    let harness = format!(
        "#include \"emela_embedded.h\"\n#include \"emela_board.h\"\n#include <stdlib.h>\n_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) {{ (void)kind; (void)message; (void)length; exit(1); }}\nint main(void) {{ for (int i = 0; i < 1000; i++) {{ if (emela_export_tuple_list_head_0() != INT32_C(42)) return 2; if (emela_export_list_tuple_head_1() != INT32_C(73)) return 3; {tuple_c_type} value = emela_export_tuple_value_2(); emela_embedded_release_value_0(&value); }} return 0; }}\n"
    );
    compile_and_run(output, &harness);
}

#[test]
fn stores_and_releases_list_references_inside_enum_fields() {
    let mut core = emela_core::Module::new();
    let list_int = core.types.intern(Type::List(TypeId::INT));
    let enum_id = core.add_enum(EnumDef {
        name: "ListBox".to_owned(),
        params: 0,
        variants: vec![
            VariantDef::positional("Some", vec![list_int]),
            VariantDef::unit("None"),
        ],
        error: false,
        external: false,
    });
    let enum_ty = core.enum_type(enum_id, Vec::new());
    let mut builder = FnBuilder::new(&core, "enum_list_head");
    builder.set_ret(TypeId::INT).set_exported();
    let inner = builder.assign(
        "inner",
        list_int,
        Value::List {
            elems: vec![Const::Int(91).into()],
            tail: None,
        },
    );
    let boxed = builder.assign(
        "boxed",
        enum_ty,
        Value::Make {
            ctor: CtorRef {
                enum_id,
                variant: 0,
            },
            args: vec![inner.into()],
        },
    );
    let tag = builder.assign("tag", TypeId::INT, Value::Tag(boxed));
    let list = builder.assign(
        "list",
        list_int,
        Value::Field {
            base: boxed,
            ctor: CtorRef {
                enum_id,
                variant: 0,
            },
            index: 0,
        },
    );
    let head = builder.assign("head", TypeId::INT, Value::ListHead(list));
    let result = builder.assign(
        "result",
        TypeId::INT,
        Value::Prim {
            op: PrimOp::Add,
            ty: PrimTy::I32,
            args: vec![head.into(), tag.into()],
        },
    );
    builder.term(Term::Return(Some(result.into())));
    let function = builder.finish();
    let mut flow = Module::new(core);
    flow.add(function);

    let output = compile_flow(&flow, &Options::default().with_heap_bytes(1024))
        .expect("emit List references inside enum payloads");
    compile_and_run(
        output,
        "#include \"emela_embedded.h\"\n#include \"emela_board.h\"\n#include <stdlib.h>\n_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) { (void)kind; (void)message; (void)length; exit(1); }\nint main(void) { for (int i = 0; i < 1000; i++) if (emela_export_enum_list_head_0() != INT32_C(91)) return 2; return 0; }\n",
    );
}

#[test]
fn requires_fixed_heap_capacity_for_managed_aggregate_values() {
    let mut core = emela_core::Module::new();
    let list_int = core.types.intern(Type::List(TypeId::INT));
    let tuple_with_list = core.types.intern(Type::Tuple(vec![list_int]));
    let mut builder = FnBuilder::new(&core, "managed_tuple");
    builder.set_ret(TypeId::INT).set_exported();
    let list = builder.assign(
        "list",
        list_int,
        Value::List {
            elems: vec![Const::Int(1).into()],
            tail: None,
        },
    );
    let tuple = builder.assign("tuple", tuple_with_list, Value::Tuple(vec![list.into()]));
    let field = builder.assign(
        "field",
        list_int,
        Value::TupleGet {
            base: tuple,
            index: 0,
        },
    );
    let head = builder.assign("head", TypeId::INT, Value::ListHead(field));
    builder.term(Term::Return(Some(head.into())));
    let function = builder.finish();
    let mut flow = Module::new(core);
    flow.add(function);

    let errors = compile_flow(&flow, &Options::default())
        .expect_err("managed aggregate values require heap-backed reference management");
    assert_eq!(errors[0].kind, ErrorKind::Unsupported);
    assert!(errors[0].message.contains("heap_bytes = 0"));
}

#[test]
fn allocates_and_releases_lists_in_the_fixed_heap() {
    let mut core = emela_core::Module::new();
    let list_int = core.types.intern(Type::List(TypeId::INT));
    let make_list = {
        let mut builder = FnBuilder::new(&core, "make_list");
        builder.set_ret(list_int).set_exported();
        let list = builder.assign(
            "list",
            list_int,
            Value::List {
                elems: vec![Const::Int(4).into(), Const::Int(5).into()],
                tail: None,
            },
        );
        builder.term(Term::Return(Some(list.into())));
        builder.finish()
    };
    let mut flow = Module::new(core);
    flow.add(make_list);

    let output = compile_flow(&flow, &Options::default().with_heap_bytes(1024))
        .expect("emit fixed-heap List construction");
    compile_and_run(
        output,
        "#include \"emela_embedded.h\"\n#include \"emela_board.h\"\n#include <stdlib.h>\n_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) { (void)message; (void)length; exit(kind == EMELA_DEFECT_HEAP_EXHAUSTED ? 2 : 3); }\nint main(void) { for (int i = 0; i < 1000; i++) emela_embedded_release(emela_export_make_list_0()); return 0; }\n",
    );
}

#[test]
fn fixed_heap_exhaustion_calls_the_board_defect_hook() {
    let mut core = emela_core::Module::new();
    let list_int = core.types.intern(Type::List(TypeId::INT));
    let function = {
        let mut builder = FnBuilder::new(&core, "make_list");
        builder.set_ret(list_int).set_exported();
        let list = builder.assign(
            "list",
            list_int,
            Value::List {
                elems: vec![Const::Int(42).into()],
                tail: None,
            },
        );
        builder.term(Term::Return(Some(list.into())));
        builder.finish()
    };
    let mut flow = Module::new(core);
    flow.add(function);

    let output = compile_flow(&flow, &Options::default().with_heap_bytes(1))
        .expect("emit a List with an undersized fixed heap");
    compile_and_run(
        output,
        "#include \"emela_embedded.h\"\n#include \"emela_board.h\"\n#include <stdlib.h>\n_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) { (void)message; (void)length; exit(kind == EMELA_DEFECT_HEAP_EXHAUSTED ? 0 : 1); }\nint main(void) { (void)emela_export_make_list_0(); return 2; }\n",
    );
}

#[test]
fn statically_evaluates_const_arithmetic() {
    let core = emela_core::Module::new();
    let mut constant = FnBuilder::new(&core, "ANSWER");
    constant.set_ret(TypeId::INT).set_constant();
    let value = constant.assign(
        "sum",
        TypeId::INT,
        Value::Prim {
            op: PrimOp::Add,
            ty: PrimTy::I32,
            args: vec![Const::Int(20).into(), Const::Int(22).into()],
        },
    );
    constant.term(Term::Return(Some(value.into())));
    let constant = constant.finish();
    let mut flow = Module::new(core);
    let constant_id = flow.add(constant);

    let mut consumer = FnBuilder::new(&flow.core, "answer_from_const");
    consumer.set_ret(TypeId::INT).set_exported();
    let answer = consumer.assign(
        "answer",
        TypeId::INT,
        Value::Call {
            callee: Callee::Direct(constant_id),
            args: Vec::new(),
            suspends: false,
            on_fail: None,
        },
    );
    consumer.term(Term::Return(Some(answer.into())));
    flow.add(consumer.finish());

    let output = compile_flow(&flow, &Options::default()).expect("evaluate const arithmetic");
    let source = &output
        .files
        .iter()
        .find(|file| file.path == "emela_embedded.c")
        .expect("generated source")
        .contents;
    assert!(source.contains("return INT32_C(42);"));
    compile_and_run(
        output,
        "#include \"emela_embedded.h\"\nint main(void) { return emela_export_answer_from_const_1() == INT32_C(42) ? 0 : 1; }\n",
    );
}

#[test]
fn evaluates_direct_helper_calls_inside_const_functions() {
    let core = emela_core::Module::new();
    let mut helper = FnBuilder::new(&core, "twice");
    helper.set_ret(TypeId::INT);
    let value = helper.param("value", TypeId::INT);
    let result = helper.assign(
        "result",
        TypeId::INT,
        Value::Prim {
            op: PrimOp::Mul,
            ty: PrimTy::I32,
            args: vec![value.into(), Const::Int(2).into()],
        },
    );
    helper.term(Term::Return(Some(result.into())));
    let helper = helper.finish();
    let mut flow = Module::new(core);
    let helper_id = flow.add(helper);

    let mut constant = FnBuilder::new(&flow.core, "ANSWER_FROM_HELPER");
    constant.set_ret(TypeId::INT).set_constant();
    let value = constant.assign(
        "value",
        TypeId::INT,
        Value::Call {
            callee: Callee::Direct(helper_id),
            args: vec![Const::Int(21).into()],
            suspends: false,
            on_fail: None,
        },
    );
    constant.term(Term::Return(Some(value.into())));
    let constant_id = flow.add(constant.finish());

    let mut consumer = FnBuilder::new(&flow.core, "answer_from_helper");
    consumer.set_ret(TypeId::INT).set_exported();
    let answer = consumer.assign(
        "answer",
        TypeId::INT,
        Value::Call {
            callee: Callee::Direct(constant_id),
            args: Vec::new(),
            suspends: false,
            on_fail: None,
        },
    );
    consumer.term(Term::Return(Some(answer.into())));
    flow.add(consumer.finish());

    let output = compile_flow(&flow, &Options::default()).expect("evaluate const helper call");
    let source = &output
        .files
        .iter()
        .find(|file| file.path == "emela_embedded.c")
        .expect("generated source")
        .contents;
    assert!(source.contains("return INT32_C(42);"));
    assert!(!source.contains("emela_fn_0"));
    compile_and_run(
        output,
        "#include \"emela_embedded.h\"\nint main(void) { return emela_export_answer_from_helper_2() == INT32_C(42) ? 0 : 1; }\n",
    );
}

#[test]
fn emits_a_static_string_const_as_a_view() {
    let core = emela_core::Module::new();
    let mut constant = FnBuilder::new(&core, "GREETING");
    constant
        .set_ret(TypeId::STRING)
        .set_exported()
        .set_constant();
    constant.term(Term::Return(Some(Const::String("a\0é".to_owned()).into())));
    let constant = constant.finish();
    let mut flow = Module::new(core);
    flow.add(constant);

    let output = compile_flow(&flow, &Options::default()).expect("emit String const view");
    compile_and_run(
        output,
        "#include <stdint.h>\n#include <string.h>\n#include \"emela_embedded.h\"\nint main(void) { const unsigned char expected[] = { UINT8_C(0x61), UINT8_C(0x00), UINT8_C(0xc3), UINT8_C(0xa9) }; EmelaString value = emela_export_GREETING_0(); return value.length == UINT16_C(4) && memcmp(value.data, expected, sizeof expected) == 0 ? 0 : 1; }\n",
    );
}

#[test]
fn string_byte_size_counts_utf8_bytes_from_a_view() {
    let core = emela_core::Module::new();
    let mut builder = FnBuilder::new(&core, "string_byte_size");
    builder.set_ret(TypeId::INT).set_exported();
    let value = builder.param("value", TypeId::STRING);
    let byte_size = builder.assign(
        "byte_size",
        TypeId::INT,
        Value::Builtin {
            op: Builtin::StringByteSize,
            args: vec![value.into()],
        },
    );
    builder.term(Term::Return(Some(byte_size.into())));
    let function = builder.finish();
    let mut flow = Module::new(core);
    flow.add(function);

    let output = compile_flow(&flow, &Options::default()).expect("emit String.byte_size");
    let source = &output
        .files
        .iter()
        .find(|file| file.path == "emela_embedded.c")
        .expect("generated source")
        .contents;
    assert!(source.contains(".length"));
    compile_and_run(
        output,
        "#include \"emela_embedded.h\"\nint main(void) { EmelaString value = { .data = \"a\\xc3\\xa9\\xf0\\x9f\\x99\\x82\", .length = UINT16_C(7) }; return emela_export_string_byte_size_0(value) == INT32_C(7) ? 0 : 1; }\n",
    );
}

#[test]
fn int_bitwise_builtins_match_signed_32_bit_semantics() {
    let mut flow = Module::new(emela_core::Module::new());
    for (name, op) in [
        ("int_bit_and", Builtin::IntBitAnd),
        ("int_bit_or", Builtin::IntBitOr),
        ("int_bit_xor", Builtin::IntBitXor),
        ("int_bit_not", Builtin::IntBitNot),
    ] {
        let mut builder = FnBuilder::new(&flow.core, name);
        builder.set_ret(TypeId::INT).set_exported();
        let lhs = builder.param("lhs", TypeId::INT);
        let args = if op == Builtin::IntBitNot {
            vec![lhs.into()]
        } else {
            vec![lhs.into(), builder.param("rhs", TypeId::INT).into()]
        };
        let result = builder.assign("result", TypeId::INT, Value::Builtin { op, args });
        builder.term(Term::Return(Some(result.into())));
        flow.add(builder.finish());
    }

    let output = compile_flow(&flow, &Options::default()).expect("emit Int bitwise builtins");
    let source = &output
        .files
        .iter()
        .find(|file| file.path == "emela_embedded.c")
        .expect("generated source")
        .contents;
    assert!(source.contains(" & "));
    assert!(source.contains(" | "));
    assert!(source.contains(" ^ "));
    assert!(source.contains("~"));
    compile_and_run(
        output,
        "#include \"emela_embedded.h\"\nint main(void) { return emela_export_int_bit_and_0(-1, INT32_C(0x12345678)) == INT32_C(0x12345678) && emela_export_int_bit_or_1(INT32_C(0x40000000), 1) == INT32_C(0x40000001) && emela_export_int_bit_xor_2(-1, -1) == 0 && emela_export_int_bit_not_3(0) == -1 ? 0 : 1; }\n",
    );
}

#[test]
fn int64_bitwise_builtins_match_signed_64_bit_semantics() {
    let mut flow = Module::new(emela_core::Module::new());
    for (name, op) in [
        ("int64_bit_and", Builtin::Int64BitAnd),
        ("int64_bit_or", Builtin::Int64BitOr),
        ("int64_bit_xor", Builtin::Int64BitXor),
        ("int64_bit_not", Builtin::Int64BitNot),
    ] {
        let mut builder = FnBuilder::new(&flow.core, name);
        builder.set_ret(TypeId::INT64).set_exported();
        let lhs = builder.param("lhs", TypeId::INT64);
        let args = if op == Builtin::Int64BitNot {
            vec![lhs.into()]
        } else {
            vec![lhs.into(), builder.param("rhs", TypeId::INT64).into()]
        };
        let result = builder.assign("result", TypeId::INT64, Value::Builtin { op, args });
        builder.term(Term::Return(Some(result.into())));
        flow.add(builder.finish());
    }

    let output = compile_flow(&flow, &Options::default()).expect("emit Int64 bitwise builtins");
    let source = &output
        .files
        .iter()
        .find(|file| file.path == "emela_embedded.c")
        .expect("generated source")
        .contents;
    assert!(source.contains(" & "));
    assert!(source.contains(" | "));
    assert!(source.contains(" ^ "));
    assert!(source.contains("~"));
    compile_and_run(
        output,
        "#include \"emela_embedded.h\"\nint main(void) { return emela_export_int64_bit_and_0(INT64_C(-1), INT64_C(0x123456789)) == INT64_C(0x123456789) && emela_export_int64_bit_or_1(INT64_C(0x4000000000000000), INT64_C(1)) == INT64_C(0x4000000000000001) && emela_export_int64_bit_xor_2(INT64_C(-1), INT64_C(-1)) == INT64_C(0) && emela_export_int64_bit_not_3(INT64_C(0)) == INT64_C(-1) ? 0 : 1; }\n",
    );
}

#[test]
fn int_shift_builtins_handle_width_and_negative_amount_boundaries() {
    let mut flow = Module::new(emela_core::Module::new());
    for (name, op) in [
        ("int_shift_left", Builtin::IntShiftLeft),
        ("int_shift_right", Builtin::IntShiftRight),
        ("int_shift_right_unsigned", Builtin::IntShiftRightUnsigned),
    ] {
        let mut builder = FnBuilder::new(&flow.core, name);
        builder.set_ret(TypeId::INT).set_exported();
        let value = builder.param("value", TypeId::INT);
        let by = builder.param("by", TypeId::INT);
        let result = builder.assign(
            "result",
            TypeId::INT,
            Value::Builtin {
                op,
                args: vec![value.into(), by.into()],
            },
        );
        builder.term(Term::Return(Some(result.into())));
        flow.add(builder.finish());
    }

    let output = compile_flow(&flow, &Options::default()).expect("emit Int shift builtins");
    let source = &output
        .files
        .iter()
        .find(|file| file.path == "emela_embedded.c")
        .expect("generated source")
        .contents;
    assert!(source.contains("emela_i32_shift_left"));
    assert!(source.contains("emela_i32_shift_right_unsigned"));
    assert!(source.contains("emela_emb_negative_shift_defect"));
    compile_and_run(
        output,
        "#include <stdlib.h>\n#include <string.h>\n#include \"emela_embedded.h\"\n#include \"emela_board.h\"\n_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) { static const char expected[] = \"negative shift amount: -2147483648\"; if (kind == EMELA_DEFECT_NEGATIVE_SHIFT && length == sizeof expected - 1 && memcmp(message, expected, length) == 0) exit(0); exit(1); }\nint main(void) { if (emela_export_int_shift_left_0(-1, 1) != -2 || emela_export_int_shift_left_0(1, 32) != 0 || emela_export_int_shift_right_1(-9, 2) != -3 || emela_export_int_shift_right_1(-9, 32) != -1 || emela_export_int_shift_right_1(9, 32) != 0 || emela_export_int_shift_right_unsigned_2(-1, 1) != INT32_MAX || emela_export_int_shift_right_unsigned_2(-1, 32) != 0) return 2; (void)emela_export_int_shift_left_0(1, INT32_MIN); return 3; }\n",
    );
}

#[test]
fn int64_shift_builtins_handle_width_and_negative_amount_boundaries() {
    let mut flow = Module::new(emela_core::Module::new());
    for (name, op) in [
        ("int64_shift_left", Builtin::Int64ShiftLeft),
        ("int64_shift_right", Builtin::Int64ShiftRight),
        (
            "int64_shift_right_unsigned",
            Builtin::Int64ShiftRightUnsigned,
        ),
    ] {
        let mut builder = FnBuilder::new(&flow.core, name);
        builder.set_ret(TypeId::INT64).set_exported();
        let value = builder.param("value", TypeId::INT64);
        let by = builder.param("by", TypeId::INT);
        let result = builder.assign(
            "result",
            TypeId::INT64,
            Value::Builtin {
                op,
                args: vec![value.into(), by.into()],
            },
        );
        builder.term(Term::Return(Some(result.into())));
        flow.add(builder.finish());
    }

    let output = compile_flow(&flow, &Options::default()).expect("emit Int64 shift builtins");
    let source = &output
        .files
        .iter()
        .find(|file| file.path == "emela_embedded.c")
        .expect("generated source")
        .contents;
    assert!(source.contains("emela_i64_shift_left"));
    assert!(source.contains("emela_i64_shift_right_unsigned"));
    assert!(source.contains("emela_emb_negative_shift_defect"));
    compile_and_run(
        output,
        "#include <stdlib.h>\n#include <string.h>\n#include \"emela_embedded.h\"\n#include \"emela_board.h\"\n_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) { static const char expected[] = \"negative shift amount: -2147483648\"; if (kind == EMELA_DEFECT_NEGATIVE_SHIFT && length == sizeof expected - 1 && memcmp(message, expected, length) == 0) exit(0); exit(1); }\nint main(void) { if (emela_export_int64_shift_left_0(INT64_C(-1), 1) != INT64_C(-2) || emela_export_int64_shift_left_0(INT64_C(1), 64) != INT64_C(0) || emela_export_int64_shift_right_1(INT64_C(-9), 2) != INT64_C(-3) || emela_export_int64_shift_right_1(INT64_C(-9), 64) != INT64_C(-1) || emela_export_int64_shift_right_1(INT64_C(9), 64) != INT64_C(0) || emela_export_int64_shift_right_unsigned_2(INT64_C(-1), 1) != INT64_MAX || emela_export_int64_shift_right_unsigned_2(INT64_C(-1), 64) != INT64_C(0)) return 2; (void)emela_export_int64_shift_left_0(INT64_C(1), INT32_MIN); return 3; }\n",
    );
}

#[test]
fn int64_conversions_preserve_sign_and_keep_the_low_32_bits() {
    let mut flow = Module::new(emela_core::Module::new());
    for (name, op, input_type, result_type) in [
        (
            "int64_from_int",
            Builtin::Int64FromInt,
            TypeId::INT,
            TypeId::INT64,
        ),
        (
            "int64_to_int",
            Builtin::Int64ToInt,
            TypeId::INT64,
            TypeId::INT,
        ),
    ] {
        let mut builder = FnBuilder::new(&flow.core, name);
        builder.set_ret(result_type).set_exported();
        let input = builder.param("input", input_type);
        let result = builder.assign(
            "result",
            result_type,
            Value::Builtin {
                op,
                args: vec![input.into()],
            },
        );
        builder.term(Term::Return(Some(result.into())));
        flow.add(builder.finish());
    }

    let output = compile_flow(&flow, &Options::default()).expect("emit Int64 conversions");
    compile_and_run(
        output,
        "#include \"emela_embedded.h\"\nint main(void) { return emela_export_int64_from_int_0(INT32_MIN) == INT64_C(-2147483648) && emela_export_int64_from_int_0(INT32_MAX) == INT64_C(2147483647) && emela_export_int64_to_int_1(INT64_C(0x123456789)) == INT32_C(0x23456789) && emela_export_int64_to_int_1(INT64_C(-1)) == INT32_C(-1) && emela_export_int64_to_int_1(INT64_MIN) == 0 ? 0 : 1; }\n",
    );
}

#[test]
fn float_from_int_converts_the_full_signed_range_exactly() {
    let core = emela_core::Module::new();
    let mut builder = FnBuilder::new(&core, "float_from_int");
    builder.set_ret(TypeId::FLOAT).set_exported();
    let input = builder.param("input", TypeId::INT);
    let result = builder.assign(
        "result",
        TypeId::FLOAT,
        Value::Builtin {
            op: Builtin::FloatFromInt,
            args: vec![input.into()],
        },
    );
    builder.term(Term::Return(Some(result.into())));
    let function = builder.finish();
    let mut flow = Module::new(core);
    flow.add(function);

    let output = compile_flow(&flow, &Options::default()).expect("emit Float.from_int");
    compile_and_run(
        output,
        "#include \"emela_embedded.h\"\nint main(void) { return emela_export_float_from_int_0(INT32_MIN) == -2147483648.0 && emela_export_float_from_int_0(INT32_MAX) == 2147483647.0 && emela_export_float_from_int_0(-1) == -1.0 ? 0 : 1; }\n",
    );
}

#[test]
fn statically_evaluates_float_from_int() {
    let core = emela_core::Module::new();
    let mut constant = FnBuilder::new(&core, "FLOAT_ANSWER");
    constant
        .set_ret(TypeId::FLOAT)
        .set_exported()
        .set_constant();
    let result = constant.assign(
        "result",
        TypeId::FLOAT,
        Value::Builtin {
            op: Builtin::FloatFromInt,
            args: vec![Const::Int(42).into()],
        },
    );
    constant.term(Term::Return(Some(result.into())));
    let function = constant.finish();
    let mut flow = Module::new(core);
    flow.add(function);

    let output = compile_flow(&flow, &Options::default()).expect("evaluate Float.from_int const");
    compile_and_run(
        output,
        "#include \"emela_embedded.h\"\nint main(void) { return emela_export_FLOAT_ANSWER_0() == 42.0 ? 0 : 1; }\n",
    );
}

#[test]
fn float_rounding_builtins_match_spec_and_defect_out_of_range() {
    let core = emela_core::Module::new();
    let mut flow = Module::new(core);
    for (name, op) in [
        ("float_floor", Builtin::FloatFloor),
        ("float_ceil", Builtin::FloatCeil),
        ("float_round", Builtin::FloatRound),
        ("float_truncate", Builtin::FloatTruncate),
    ] {
        let mut builder = FnBuilder::new(&flow.core, name);
        builder.set_ret(TypeId::INT).set_exported();
        let input = builder.param("input", TypeId::FLOAT);
        let result = builder.assign(
            "result",
            TypeId::INT,
            Value::Builtin {
                op,
                args: vec![input.into()],
            },
        );
        builder.term(Term::Return(Some(result.into())));
        flow.add(builder.finish());
    }

    let output = compile_flow(&flow, &Options::default()).expect("emit Float rounding builtins");
    compile_and_run(
        output,
        r#"
#include <math.h>
#include <setjmp.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
static jmp_buf defect_target;
static int defect_count;
_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) {
    if (kind != EMELA_DEFECT_FLOAT_TO_INT || length != UINT16_C(27) || memcmp(message, "cannot convert Float to Int", length) != 0) exit(2);
    ++defect_count;
    longjmp(defect_target, 1);
}
int main(void) {
    if (emela_export_float_floor_0(2.5) != 2 || emela_export_float_floor_0(-2.5) != -3 || emela_export_float_floor_0(2147483647.75) != INT32_MAX || emela_export_float_ceil_1(-2.5) != -2 || emela_export_float_ceil_1(-2147483648.75) != INT32_MIN || emela_export_float_round_2(2.5) != 3 || emela_export_float_round_2(-2.5) != -3 || emela_export_float_round_2(-0.5) != -1 || emela_export_float_truncate_3(-2.9) != -2) return 1;
    if (setjmp(defect_target) == 0) { (void)emela_export_float_floor_0(NAN); return 3; }
    if (setjmp(defect_target) == 0) { (void)emela_export_float_floor_0(-2147483648.5); return 4; }
    if (setjmp(defect_target) == 0) { (void)emela_export_float_round_2(2147483647.5); return 5; }
    return defect_count == 3 ? 0 : 6;
}
"#,
    );
}

#[test]
fn statically_evaluates_float_rounding_builtin() {
    let mut flow = Module::new(emela_core::Module::new());
    let mut constant = FnBuilder::new(&flow.core, "ROUND_ANSWER");
    constant.set_ret(TypeId::INT).set_exported().set_constant();
    let result = constant.assign(
        "result",
        TypeId::INT,
        Value::Builtin {
            op: Builtin::FloatRound,
            args: vec![Const::Float(-2.5).into()],
        },
    );
    constant.term(Term::Return(Some(result.into())));
    flow.add(constant.finish());

    let output = compile_flow(&flow, &Options::default()).expect("evaluate Float.round const");
    compile_and_run(
        output,
        "#include \"emela_embedded.h\"\nint main(void) { return emela_export_ROUND_ANSWER_0() == INT32_C(-3) ? 0 : 1; }\n",
    );
}

#[test]
fn statically_formats_float_values_without_runtime_scratch() {
    let mut flow = Module::new(emela_core::Module::new());
    let mut constant = FnBuilder::new(&flow.core, "FLOAT_TEXT");
    constant
        .set_ret(TypeId::STRING)
        .set_exported()
        .set_constant();
    let result = constant.assign(
        "result",
        TypeId::STRING,
        Value::Builtin {
            op: Builtin::FloatToString,
            args: vec![Const::Float(1.0e-6).into()],
        },
    );
    constant.term(Term::Return(Some(result.into())));
    flow.add(constant.finish());

    let output = compile_flow(&flow, &Options::default()).expect("evaluate Float.to_string const");
    compile_and_run(
        output,
        "#include <string.h>\n#include \"emela_embedded.h\"\nint main(void) { EmelaString value = emela_export_FLOAT_TEXT_0(); return value.length == UINT16_C(8) && memcmp(value.data, \"0.000001\", 8) == 0 ? 0 : 1; }\n",
    );
}

#[test]
fn float_to_string_constant_contributes_its_exact_append_bound() {
    let mut flow = Module::new(emela_core::Module::new());
    let mut builder = FnBuilder::new(&flow.core, "concat_float_and_int");
    builder.set_ret(TypeId::INT).set_exported();
    let input = builder.param("input", TypeId::INT);
    let float_text = builder.assign(
        "float_text",
        TypeId::STRING,
        Value::Builtin {
            op: Builtin::FloatToString,
            args: vec![Const::Float(1.0e-6).into()],
        },
    );
    let int_text = builder.assign(
        "int_text",
        TypeId::STRING,
        Value::Builtin {
            op: Builtin::IntToString,
            args: vec![input.into()],
        },
    );
    let combined = builder.assign(
        "combined",
        TypeId::STRING,
        Value::Builtin {
            op: Builtin::StringAppend,
            args: vec![float_text.into(), int_text.into()],
        },
    );
    let length = builder.assign(
        "length",
        TypeId::INT,
        Value::Builtin {
            op: Builtin::StringByteSize,
            args: vec![combined.into()],
        },
    );
    builder.term(Term::Return(Some(length.into())));
    flow.add(builder.finish());

    let output = compile_flow(&flow, &Options::default()).expect("emit bounded String.append");
    let source = &output
        .files
        .iter()
        .find(|file| file.path == "emela_embedded.c")
        .expect("C source")
        .contents;
    assert!(source.contains("UINT16_C(19)"));
    compile_and_run(
        output,
        "#include <stdlib.h>\n#include \"emela_embedded.h\"\n#include \"emela_board.h\"\n_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) { (void)kind; (void)message; (void)length; exit(2); }\nint main(void) { return emela_export_concat_float_and_int_0(INT32_C(42)) == INT32_C(10) ? 0 : 1; }\n",
    );
}

#[test]
fn statically_parses_float_literals_with_the_float_parse_grammar() {
    let mut flow = Module::new(emela_core::Module::new());
    let option = flow.core.option_enum();
    let option_float = flow.core.enum_type(option, vec![TypeId::FLOAT]);
    for (name, text) in [
        ("parse_decimal", "+12.5e-1"),
        ("parse_negative_infinity", "-Infinity"),
        ("parse_nan", "NaN"),
        ("reject_leading_decimal", ".5"),
        ("reject_trailing_decimal", "1."),
        ("reject_whitespace", " 1"),
        ("parse_overflow", "1e999"),
        ("parse_underflow", "1e-999"),
    ] {
        let mut builder = FnBuilder::new(&flow.core, name);
        builder.set_ret(option_float).set_exported();
        let result = builder.assign(
            "result",
            option_float,
            Value::Builtin {
                op: Builtin::FloatParse,
                args: vec![Const::String(text.to_owned()).into()],
            },
        );
        builder.term(Term::Return(Some(result.into())));
        flow.add(builder.finish());
    }

    let output = compile_flow(&flow, &Options::default()).expect("fold Float.parse literals");
    let c_type = format!("emela_type_{}", option_float.index());
    let harness = format!(
        "#include <math.h>\n#include \"emela_embedded.h\"\nint main(void) {{ {c_type} value = emela_export_parse_decimal_0(); if (value.tag != 0 || value.payload.v0.f0 != 1.25) return 1; value = emela_export_parse_negative_infinity_1(); if (value.tag != 0 || !isinf(value.payload.v0.f0) || value.payload.v0.f0 > 0.0) return 2; value = emela_export_parse_nan_2(); if (value.tag != 0 || !isnan(value.payload.v0.f0)) return 3; value = emela_export_reject_leading_decimal_3(); if (value.tag != INT32_C({none})) return 4; value = emela_export_reject_trailing_decimal_4(); if (value.tag != INT32_C({none})) return 5; value = emela_export_reject_whitespace_5(); if (value.tag != INT32_C({none})) return 6; value = emela_export_parse_overflow_6(); if (value.tag != 0 || !isinf(value.payload.v0.f0)) return 7; value = emela_export_parse_underflow_7(); if (value.tag != 0 || value.payload.v0.f0 != 0.0) return 8; return 0; }}\n",
        none = emela_core::OPTION_NONE,
    );
    compile_and_run(output, &harness);
}

#[test]
fn dynamic_float_formatting_and_parsing_report_the_runtime_requirement() {
    let mut flow = Module::new(emela_core::Module::new());
    let mut format = FnBuilder::new(&flow.core, "dynamic_float_to_string");
    format.set_ret(TypeId::INT).set_exported();
    let input = format.param("input", TypeId::FLOAT);
    let rendered = format.assign(
        "rendered",
        TypeId::STRING,
        Value::Builtin {
            op: Builtin::FloatToString,
            args: vec![input.into()],
        },
    );
    let length = format.assign(
        "length",
        TypeId::INT,
        Value::Builtin {
            op: Builtin::StringByteSize,
            args: vec![rendered.into()],
        },
    );
    format.term(Term::Return(Some(length.into())));
    flow.add(format.finish());

    let error =
        compile_flow(&flow, &Options::default()).expect_err("reject dynamic Float.to_string");
    assert_eq!(error[0].kind, ErrorKind::Unsupported);
    assert!(
        error[0]
            .message
            .contains("allocation-free shortest-decimal runtime")
    );

    let mut flow = Module::new(emela_core::Module::new());
    let option = flow.core.option_enum();
    let option_float = flow.core.enum_type(option, vec![TypeId::FLOAT]);
    let mut parse = FnBuilder::new(&flow.core, "dynamic_float_parse");
    parse.set_ret(option_float).set_exported();
    let input = parse.param("input", TypeId::STRING);
    let result = parse.assign(
        "result",
        option_float,
        Value::Builtin {
            op: Builtin::FloatParse,
            args: vec![input.into()],
        },
    );
    parse.term(Term::Return(Some(result.into())));
    flow.add(parse.finish());

    let error = compile_flow(&flow, &Options::default()).expect_err("reject dynamic Float.parse");
    assert_eq!(error[0].kind, ErrorKind::Unsupported);
    assert!(
        error[0]
            .message
            .contains("allocation-free bounded decimal parser")
    );
}

#[test]
fn int_checked_add_returns_some_or_none_at_int_boundaries() {
    let mut flow = Module::new(emela_core::Module::new());
    let option = flow.core.option_enum();
    let option_int = flow.core.enum_type(option, vec![TypeId::INT]);
    let mut builder = FnBuilder::new(&flow.core, "int_checked_add");
    builder.set_ret(option_int).set_exported();
    let left = builder.param("left", TypeId::INT);
    let right = builder.param("right", TypeId::INT);
    let result = builder.assign(
        "result",
        option_int,
        Value::Builtin {
            op: Builtin::IntCheckedAdd,
            args: vec![left.into(), right.into()],
        },
    );
    builder.term(Term::Return(Some(result.into())));
    flow.add(builder.finish());

    let output = compile_flow(&flow, &Options::default()).expect("emit Int.checked_add");
    let c_type = format!("emela_type_{}", option_int.index());
    let harness = format!(
        "#include \"emela_embedded.h\"\nint main(void) {{ {c_type} value = emela_export_int_checked_add_0(INT32_MAX, 1); if (value.tag != INT32_C({})) return 1; value = emela_export_int_checked_add_0(INT32_MIN, -1); if (value.tag != INT32_C({})) return 2; value = emela_export_int_checked_add_0(INT32_MAX, 0); if (value.tag != INT32_C({}) || value.payload.v{}.f0 != INT32_MAX) return 3; value = emela_export_int_checked_add_0(-2, 1); if (value.tag != INT32_C({}) || value.payload.v{}.f0 != -1) return 4; return 0; }}\n",
        emela_core::OPTION_NONE,
        emela_core::OPTION_NONE,
        emela_core::OPTION_SOME,
        emela_core::OPTION_SOME,
        emela_core::OPTION_SOME,
        emela_core::OPTION_SOME,
    );
    compile_and_run(output, &harness);
}

#[test]
fn int_parse_accepts_decimal_signs_and_rejects_invalid_or_overflowing_text() {
    let mut flow = Module::new(emela_core::Module::new());
    let option = flow.core.option_enum();
    let option_int = flow.core.enum_type(option, vec![TypeId::INT]);
    let mut builder = FnBuilder::new(&flow.core, "int_parse");
    builder.set_ret(option_int).set_exported();
    let input = builder.param("input", TypeId::STRING);
    let result = builder.assign(
        "result",
        option_int,
        Value::Builtin {
            op: Builtin::IntParse,
            args: vec![input.into()],
        },
    );
    builder.term(Term::Return(Some(result.into())));
    let function = builder.finish();
    flow.add(function);

    let output = compile_flow(&flow, &Options::default()).expect("emit Int.parse");
    let c_type = format!("emela_type_{}", option_int.index());
    let harness = format!(
        "#include \"emela_embedded.h\"\nstatic EmelaString text(const char *data, uint16_t length) {{ return (EmelaString){{ .data = data, .length = length }}; }}\nint main(void) {{ {c_type} value = emela_export_int_parse_0(text(\"+2147483647\", 11)); if (value.tag != 0 || value.payload.v0.f0 != INT32_MAX) return 1; value = emela_export_int_parse_0(text(\"-2147483648\", 11)); if (value.tag != 0 || value.payload.v0.f0 != INT32_MIN) return 2; value = emela_export_int_parse_0(text(\"2147483648\", 10)); if (value.tag != INT32_C({})) return 3; value = emela_export_int_parse_0(text(\" 1\", 2)); if (value.tag != INT32_C({})) return 4; value = emela_export_int_parse_0(text(\"+\", 1)); if (value.tag != INT32_C({})) return 5; value = emela_export_int_parse_0(text(\"12x\", 3)); if (value.tag != INT32_C({})) return 6; value = emela_export_int_parse_0(text(\"\", 0)); if (value.tag != INT32_C({})) return 7; return 0; }}\n",
        emela_core::OPTION_NONE,
        emela_core::OPTION_NONE,
        emela_core::OPTION_NONE,
        emela_core::OPTION_NONE,
        emela_core::OPTION_NONE,
    );
    compile_and_run(output, &harness);
}

#[test]
fn string_from_code_point_encodes_valid_scalars_and_rejects_invalid_ones() {
    let mut core = emela_core::Module::new();
    let option = core.option_enum();
    let option_string = core.enum_type(option, vec![TypeId::STRING]);
    let mut builder = FnBuilder::new(&core, "code_point_matches");
    builder.set_ret(TypeId::BOOL).set_exported();
    let code_point = builder.param("code_point", TypeId::INT);
    let expected = builder.param("expected", TypeId::STRING);
    let value = builder.assign(
        "value",
        option_string,
        Value::Builtin {
            op: Builtin::StringFromCodePoint,
            args: vec![code_point.into()],
        },
    );
    let tag = builder.assign("tag", TypeId::INT, Value::Tag(value));
    let is_some = builder.assign(
        "is_some",
        TypeId::BOOL,
        Value::Prim {
            op: PrimOp::Eq,
            ty: emela_lir::PrimTy::I32,
            args: vec![
                tag.into(),
                Const::Int(emela_core::OPTION_SOME as i32).into(),
            ],
        },
    );
    let result = builder.var("result", TypeId::BOOL);
    builder.if_(
        Some(result),
        is_some,
        |some| {
            let string = some.assign(
                "string",
                TypeId::STRING,
                Value::Field {
                    base: value,
                    ctor: CtorRef {
                        enum_id: option,
                        variant: emela_core::OPTION_SOME,
                    },
                    index: 0,
                },
            );
            let matches = some.assign(
                "matches",
                TypeId::BOOL,
                Value::Builtin {
                    op: Builtin::StringContains,
                    args: vec![string.into(), expected.into()],
                },
            );
            some.term(Term::Yield(Some(matches.into())));
        },
        |none| none.term(Term::Yield(Some(Const::Bool(false).into()))),
    );
    builder.term(Term::Return(Some(result.into())));
    let function = builder.finish();
    let mut flow = Module::new(core);
    flow.add(function);

    let output = compile_flow(&flow, &Options::default()).expect("emit String.from_code_point");
    let source = &output
        .files
        .iter()
        .find(|file| file.path == "emela_embedded.c")
        .expect("generated source")
        .contents;
    assert!(source.contains("char emela_string_buffer_0[4];"));
    compile_and_run(
        output,
        "#include <stdlib.h>\n#include \"emela_embedded.h\"\n#include \"emela_board.h\"\n_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) { (void)kind; (void)message; (void)length; exit(2); }\nstatic EmelaString text(const char *data, uint16_t length) { return (EmelaString){ .data = data, .length = length }; }\nint main(void) { if (!emela_export_code_point_matches_0(65, text(\"A\", 1))) return 1; if (!emela_export_code_point_matches_0(INT32_C(0x20ac), text(\"\\xe2\\x82\\xac\", 3))) return 2; if (!emela_export_code_point_matches_0(INT32_C(0x1f642), text(\"\\xf0\\x9f\\x99\\x82\", 4))) return 3; if (!emela_export_code_point_matches_0(0, text(\"\\x00\", 1))) return 4; if (emela_export_code_point_matches_0(-1, text(\"\", 0))) return 5; if (emela_export_code_point_matches_0(INT32_C(0xd800), text(\"\", 0))) return 6; if (emela_export_code_point_matches_0(INT32_C(0x110000), text(\"\", 0))) return 7; return 0; }\n",
    );
}

#[test]
fn statically_encodes_string_from_code_point_without_stack_scratch() {
    let mut core = emela_core::Module::new();
    let option = core.option_enum();
    let option_string = core.enum_type(option, vec![TypeId::STRING]);
    let mut builder = FnBuilder::new(&core, "static_code_point");
    builder.set_ret(option_string).set_exported();
    let value = builder.assign(
        "value",
        option_string,
        Value::Builtin {
            op: Builtin::StringFromCodePoint,
            args: vec![Const::Int(0x1f642).into()],
        },
    );
    builder.term(Term::Return(Some(value.into())));
    let function = builder.finish();
    let mut flow = Module::new(core);
    flow.add(function);

    let output = compile_flow(&flow, &Options::default()).expect("fold code point to literal");
    let source = &output
        .files
        .iter()
        .find(|file| file.path == "emela_embedded.c")
        .expect("generated source")
        .contents;
    assert!(!source.contains("char emela_string_buffer_"));
    let c_type = format!("emela_type_{}", option_string.index());
    let harness = format!(
        "#include <string.h>\n#include \"emela_embedded.h\"\nint main(void) {{ {c_type} value = emela_export_static_code_point_0(); return value.tag == INT32_C({}) && value.payload.v{}.f0.length == UINT16_C(4) && memcmp(value.payload.v{}.f0.data, \"\\xf0\\x9f\\x99\\x82\", 4) == 0 ? 0 : 1; }}\n",
        emela_core::OPTION_SOME,
        emela_core::OPTION_SOME,
        emela_core::OPTION_SOME,
    );
    compile_and_run(output, &harness);
}

#[test]
fn dynamic_string_from_code_point_checks_capacity_before_writing() {
    let mut core = emela_core::Module::new();
    let option = core.option_enum();
    let option_string = core.enum_type(option, vec![TypeId::STRING]);
    let mut builder = FnBuilder::new(&core, "code_point_tag");
    builder.set_ret(TypeId::INT).set_exported();
    let code_point = builder.param("code_point", TypeId::INT);
    let value = builder.assign(
        "value",
        option_string,
        Value::Builtin {
            op: Builtin::StringFromCodePoint,
            args: vec![code_point.into()],
        },
    );
    let tag = builder.assign("tag", TypeId::INT, Value::Tag(value));
    builder.term(Term::Return(Some(tag.into())));
    let function = builder.finish();
    let mut flow = Module::new(core);
    flow.add(function);

    let output = compile_flow(&flow, &Options::default().with_string_capacity(2))
        .expect("emit capacity-bounded code point output");
    compile_and_run(
        output,
        "#include <stdlib.h>\n#include <string.h>\n#include \"emela_embedded.h\"\n#include \"emela_board.h\"\n_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) { if (kind == EMELA_DEFECT_STRING_CAPACITY && length == UINT16_C(24) && memcmp(message, \"string capacity exceeded\", length) == 0) exit(0); exit(1); }\nint main(void) { (void)emela_export_code_point_tag_0(INT32_C(0x20ac)); return 2; }\n",
    );
}

#[test]
fn int_to_string_uses_small_scratch_and_matches_signed_decimal_output() {
    let core = emela_core::Module::new();
    let mut builder = FnBuilder::new(&core, "int_to_string_matches");
    builder.set_ret(TypeId::BOOL).set_exported();
    let input = builder.param("input", TypeId::INT);
    let expected = builder.param("expected", TypeId::STRING);
    let actual = builder.assign(
        "actual",
        TypeId::STRING,
        Value::Builtin {
            op: Builtin::IntToString,
            args: vec![input.into()],
        },
    );
    let matches = builder.assign(
        "matches",
        TypeId::BOOL,
        Value::Builtin {
            op: Builtin::StringContains,
            args: vec![actual.into(), expected.into()],
        },
    );
    builder.term(Term::Return(Some(matches.into())));
    let function = builder.finish();
    let mut flow = Module::new(core);
    flow.add(function);

    let output = compile_flow(&flow, &Options::default()).expect("emit Int.to_string");
    let source = &output
        .files
        .iter()
        .find(|file| file.path == "emela_embedded.c")
        .expect("generated source")
        .contents;
    assert!(source.contains("char emela_string_buffer_0[11];"));
    assert!(source.contains("emela_emb_int_to_string"));
    compile_and_run(
        output,
        "#include <stdlib.h>\n#include \"emela_embedded.h\"\n#include \"emela_board.h\"\n_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) { (void)kind; (void)message; (void)length; exit(2); }\nint main(void) { EmelaString min = { .data = \"-2147483648\", .length = UINT16_C(11) }; EmelaString zero = { .data = \"0\", .length = UINT16_C(1) }; EmelaString positive = { .data = \"1234567890\", .length = UINT16_C(10) }; return emela_export_int_to_string_matches_0(INT32_MIN, min) && emela_export_int_to_string_matches_0(0, zero) && emela_export_int_to_string_matches_0(INT32_C(1234567890), positive) ? 0 : 1; }\n",
    );
}

#[test]
fn statically_evaluates_int_to_string_without_runtime_scratch() {
    let core = emela_core::Module::new();
    let mut constant = FnBuilder::new(&core, "INT_MIN_TEXT");
    constant
        .set_ret(TypeId::STRING)
        .set_exported()
        .set_constant();
    let text = constant.assign(
        "text",
        TypeId::STRING,
        Value::Builtin {
            op: Builtin::IntToString,
            args: vec![Const::Int(i32::MIN).into()],
        },
    );
    constant.term(Term::Return(Some(text.into())));
    let constant = constant.finish();
    let mut flow = Module::new(core);
    flow.add(constant);

    let output = compile_flow(&flow, &Options::default()).expect("evaluate Int.to_string const");
    let source = &output
        .files
        .iter()
        .find(|file| file.path == "emela_embedded.c")
        .expect("generated source")
        .contents;
    assert!(!source.contains("emela_emb_int_to_string"));
    compile_and_run(
        output,
        "#include <string.h>\n#include \"emela_embedded.h\"\nint main(void) { EmelaString value = emela_export_INT_MIN_TEXT_0(); return value.length == UINT16_C(11) && memcmp(value.data, \"-2147483648\", 11) == 0 ? 0 : 1; }\n",
    );
}

#[test]
fn int_to_string_capacity_overflow_uses_the_board_defect_hook() {
    let core = emela_core::Module::new();
    let mut builder = FnBuilder::new(&core, "int_to_string_overflow");
    builder.set_ret(TypeId::INT).set_exported();
    let input = builder.param("input", TypeId::INT);
    let text = builder.assign(
        "text",
        TypeId::STRING,
        Value::Builtin {
            op: Builtin::IntToString,
            args: vec![input.into()],
        },
    );
    let length = builder.assign(
        "length",
        TypeId::INT,
        Value::Builtin {
            op: Builtin::StringByteSize,
            args: vec![text.into()],
        },
    );
    builder.term(Term::Return(Some(length.into())));
    let function = builder.finish();
    let mut flow = Module::new(core);
    flow.add(function);

    let output = compile_flow(&flow, &Options::default().with_string_capacity(3))
        .expect("emit bounded Int.to_string");
    compile_and_run(
        output,
        "#include <stdlib.h>\n#include <string.h>\n#include \"emela_embedded.h\"\n#include \"emela_board.h\"\n_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) { if (kind == EMELA_DEFECT_STRING_CAPACITY && length == UINT16_C(24) && memcmp(message, \"string capacity exceeded\", length) == 0) exit(0); exit(1); }\nint main(void) { (void)emela_export_int_to_string_overflow_0(INT32_C(1234)); return 2; }\n",
    );
}

#[test]
fn int64_to_string_uses_twenty_byte_scratch_and_matches_decimal_output() {
    let core = emela_core::Module::new();
    let mut builder = FnBuilder::new(&core, "int64_to_string_matches");
    builder.set_ret(TypeId::BOOL).set_exported();
    let input = builder.param("input", TypeId::INT64);
    let expected = builder.param("expected", TypeId::STRING);
    let actual = builder.assign(
        "actual",
        TypeId::STRING,
        Value::Builtin {
            op: Builtin::Int64ToString,
            args: vec![input.into()],
        },
    );
    let matches = builder.assign(
        "matches",
        TypeId::BOOL,
        Value::Builtin {
            op: Builtin::StringContains,
            args: vec![actual.into(), expected.into()],
        },
    );
    builder.term(Term::Return(Some(matches.into())));
    let function = builder.finish();
    let mut flow = Module::new(core);
    flow.add(function);

    let output = compile_flow(&flow, &Options::default()).expect("emit Int64.to_string");
    let source = &output
        .files
        .iter()
        .find(|file| file.path == "emela_embedded.c")
        .expect("generated source")
        .contents;
    assert!(source.contains("char emela_string_buffer_0[20];"));
    assert!(source.contains("emela_emb_int_to_string"));
    compile_and_run(
        output,
        "#include <stdlib.h>\n#include \"emela_embedded.h\"\n#include \"emela_board.h\"\n_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) { (void)kind; (void)message; (void)length; exit(2); }\nint main(void) { EmelaString min = { .data = \"-9223372036854775808\", .length = UINT16_C(20) }; EmelaString max = { .data = \"9223372036854775807\", .length = UINT16_C(19) }; EmelaString zero = { .data = \"0\", .length = UINT16_C(1) }; return emela_export_int64_to_string_matches_0(INT64_MIN, min) && emela_export_int64_to_string_matches_0(INT64_MAX, max) && emela_export_int64_to_string_matches_0(INT64_C(0), zero) ? 0 : 1; }\n",
    );
}

#[test]
fn statically_evaluates_int64_to_string_without_runtime_scratch() {
    let core = emela_core::Module::new();
    let mut constant = FnBuilder::new(&core, "INT64_MIN_TEXT");
    constant
        .set_ret(TypeId::STRING)
        .set_exported()
        .set_constant();
    let text = constant.assign(
        "text",
        TypeId::STRING,
        Value::Builtin {
            op: Builtin::Int64ToString,
            args: vec![Const::Int64(i64::MIN).into()],
        },
    );
    constant.term(Term::Return(Some(text.into())));
    let function = constant.finish();
    let mut flow = Module::new(core);
    flow.add(function);

    let output = compile_flow(&flow, &Options::default()).expect("evaluate Int64.to_string const");
    let source = &output
        .files
        .iter()
        .find(|file| file.path == "emela_embedded.c")
        .expect("generated source")
        .contents;
    assert!(!source.contains("emela_emb_int_to_string"));
    compile_and_run(
        output,
        "#include <string.h>\n#include \"emela_embedded.h\"\nint main(void) { EmelaString value = emela_export_INT64_MIN_TEXT_0(); return value.length == UINT16_C(20) && memcmp(value.data, \"-9223372036854775808\", 20) == 0 ? 0 : 1; }\n",
    );
}

#[test]
fn appends_dynamic_strings_into_one_inferred_stack_buffer() {
    let core = emela_core::Module::new();
    let mut builder = FnBuilder::new(&core, "string_append_contains");
    builder.set_ret(TypeId::BOOL).set_exported();
    let input = builder.param("input", TypeId::STRING);
    let first = builder.assign(
        "first",
        TypeId::STRING,
        Value::Builtin {
            op: Builtin::StringAppend,
            args: vec![input.into(), Const::String("X".to_owned()).into()],
        },
    );
    let second = builder.assign(
        "second",
        TypeId::STRING,
        Value::Builtin {
            op: Builtin::StringAppend,
            args: vec![first.into(), Const::String("Y".to_owned()).into()],
        },
    );
    let result = builder.assign(
        "result",
        TypeId::BOOL,
        Value::Builtin {
            op: Builtin::StringContains,
            args: vec![second.into(), Const::String("éXY".to_owned()).into()],
        },
    );
    builder.term(Term::Return(Some(result.into())));
    let function = builder.finish();
    let mut flow = Module::new(core);
    flow.add(function);

    let output = compile_flow(&flow, &Options::default()).expect("emit dynamic String.append");
    let source = &output
        .files
        .iter()
        .find(|file| file.path == "emela_embedded.c")
        .expect("generated source")
        .contents;
    assert_eq!(source.matches("char emela_string_buffer_").count(), 1);
    assert!(source.contains("[256];"));
    compile_and_run(
        output,
        "#include <stdlib.h>\n#include \"emela_embedded.h\"\n#include \"emela_board.h\"\n_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) { (void)kind; (void)message; (void)length; exit(2); }\nint main(void) { EmelaString input = { .data = \"a\\xc3\\xa9\", .length = UINT16_C(3) }; return emela_export_string_append_contains_0(input) ? 0 : 1; }\n",
    );
}

#[test]
fn reuses_a_string_scratch_buffer_after_its_last_read() {
    let core = emela_core::Module::new();
    let mut builder = FnBuilder::new(&core, "string_scratch_reuse");
    builder.set_ret(TypeId::BOOL).set_exported();
    let input = builder.param("input", TypeId::STRING);
    let first = builder.assign(
        "first",
        TypeId::STRING,
        Value::Builtin {
            op: Builtin::StringAppend,
            args: vec![input.into(), Const::String("a".to_owned()).into()],
        },
    );
    let first_matches = builder.assign(
        "first_matches",
        TypeId::BOOL,
        Value::Builtin {
            op: Builtin::StringContains,
            args: vec![first.into(), Const::String("xa".to_owned()).into()],
        },
    );
    let result = builder.var("result", TypeId::BOOL);
    builder.if_(
        Some(result),
        first_matches,
        |then| {
            let second = then.assign(
                "second",
                TypeId::STRING,
                Value::Builtin {
                    op: Builtin::StringAppend,
                    args: vec![input.into(), Const::String("b".to_owned()).into()],
                },
            );
            let second_matches = then.assign(
                "second_matches",
                TypeId::BOOL,
                Value::Builtin {
                    op: Builtin::StringContains,
                    args: vec![second.into(), Const::String("xb".to_owned()).into()],
                },
            );
            then.term(Term::Yield(Some(second_matches.into())));
        },
        |otherwise| otherwise.term(Term::Yield(Some(Const::Bool(false).into()))),
    );
    builder.term(Term::Return(Some(result.into())));
    let function = builder.finish();
    let mut flow = Module::new(core);
    flow.add(function);

    let output = compile_flow(&flow, &Options::default()).expect("emit reusable String scratch");
    let source = &output
        .files
        .iter()
        .find(|file| file.path == "emela_embedded.c")
        .expect("generated source")
        .contents;
    assert_eq!(source.matches("char emela_string_buffer_").count(), 1);
    compile_and_run(
        output,
        "#include <stdlib.h>\n#include \"emela_embedded.h\"\n#include \"emela_board.h\"\n_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) { (void)kind; (void)message; (void)length; exit(2); }\nint main(void) { EmelaString input = { .data = \"x\", .length = UINT16_C(1) }; return emela_export_string_scratch_reuse_0(input) ? 0 : 1; }\n",
    );
}

#[test]
fn keeps_overlapping_string_scratch_values_in_distinct_buffers() {
    let core = emela_core::Module::new();
    let mut builder = FnBuilder::new(&core, "string_scratch_overlap");
    builder.set_ret(TypeId::BOOL).set_exported();
    let input = builder.param("input", TypeId::STRING);
    let left = builder.assign(
        "left",
        TypeId::STRING,
        Value::Builtin {
            op: Builtin::StringAppend,
            args: vec![input.into(), Const::String("a".to_owned()).into()],
        },
    );
    let right = builder.assign(
        "right",
        TypeId::STRING,
        Value::Builtin {
            op: Builtin::StringAppend,
            args: vec![input.into(), Const::String("b".to_owned()).into()],
        },
    );
    let combined = builder.assign(
        "combined",
        TypeId::STRING,
        Value::Builtin {
            op: Builtin::StringAppend,
            args: vec![left.into(), right.into()],
        },
    );
    let matches = builder.assign(
        "matches",
        TypeId::BOOL,
        Value::Builtin {
            op: Builtin::StringContains,
            args: vec![combined.into(), Const::String("xaxb".to_owned()).into()],
        },
    );
    builder.term(Term::Return(Some(matches.into())));
    let function = builder.finish();
    let mut flow = Module::new(core);
    flow.add(function);

    let output = compile_flow(&flow, &Options::default()).expect("emit overlapping scratch");
    let source = &output
        .files
        .iter()
        .find(|file| file.path == "emela_embedded.c")
        .expect("generated source")
        .contents;
    assert_eq!(source.matches("char emela_string_buffer_").count(), 2);
    compile_and_run(
        output,
        "#include <stdlib.h>\n#include \"emela_embedded.h\"\n#include \"emela_board.h\"\n_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) { (void)kind; (void)message; (void)length; exit(2); }\nint main(void) { EmelaString input = { .data = \"x\", .length = UINT16_C(1) }; return emela_export_string_scratch_overlap_0(input) ? 0 : 1; }\n",
    );
}

#[test]
fn preserves_a_string_scratch_buffer_while_a_trim_view_is_live() {
    let core = emela_core::Module::new();
    let mut builder = FnBuilder::new(&core, "string_trim_scratch_lifetime");
    builder.set_ret(TypeId::BOOL).set_exported();
    let first_input = builder.param("first_input", TypeId::STRING);
    let second_input = builder.param("second_input", TypeId::STRING);
    let first = builder.assign(
        "first",
        TypeId::STRING,
        Value::Builtin {
            op: Builtin::StringAppend,
            args: vec![first_input.into(), Const::String(" ".to_owned()).into()],
        },
    );
    let trimmed = builder.assign(
        "trimmed",
        TypeId::STRING,
        Value::Builtin {
            op: Builtin::StringTrim,
            args: vec![first.into()],
        },
    );
    let second = builder.assign(
        "second",
        TypeId::STRING,
        Value::Builtin {
            op: Builtin::StringAppend,
            args: vec![second_input.into(), Const::String("b".to_owned()).into()],
        },
    );
    let second_size = builder.assign(
        "second_size",
        TypeId::INT,
        Value::Builtin {
            op: Builtin::StringByteSize,
            args: vec![second.into()],
        },
    );
    let has_expected_size = builder.assign(
        "has_expected_size",
        TypeId::BOOL,
        Value::Prim {
            op: PrimOp::Eq,
            ty: PrimTy::I32,
            args: vec![second_size.into(), Const::Int(2).into()],
        },
    );
    let still_contains_x = builder.assign(
        "still_contains_x",
        TypeId::BOOL,
        Value::Builtin {
            op: Builtin::StringContains,
            args: vec![trimmed.into(), Const::String("x".to_owned()).into()],
        },
    );
    let result = builder.var("result", TypeId::BOOL);
    builder.if_(
        Some(result),
        has_expected_size,
        |then| then.term(Term::Yield(Some(still_contains_x.into()))),
        |otherwise| otherwise.term(Term::Yield(Some(Const::Bool(false).into()))),
    );
    builder.term(Term::Return(Some(result.into())));
    let function = builder.finish();
    let mut flow = Module::new(core);
    flow.add(function);

    let output = compile_flow(&flow, &Options::default()).expect("emit trim-view lifetime");
    let source = &output
        .files
        .iter()
        .find(|file| file.path == "emela_embedded.c")
        .expect("generated source")
        .contents;
    assert_eq!(source.matches("char emela_string_buffer_").count(), 2);
    compile_and_run(
        output,
        "#include <stdlib.h>\n#include \"emela_embedded.h\"\n#include \"emela_board.h\"\n_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) { (void)kind; (void)message; (void)length; exit(2); }\nint main(void) { EmelaString first = { .data = \" x\", .length = UINT16_C(2) }; EmelaString second = { .data = \"y\", .length = UINT16_C(1) }; return emela_export_string_trim_scratch_lifetime_0(first, second) ? 0 : 1; }\n",
    );
}

#[test]
fn string_append_capacity_overflow_uses_the_board_defect_hook() {
    let core = emela_core::Module::new();
    let mut builder = FnBuilder::new(&core, "append_one");
    builder.set_ret(TypeId::INT).set_exported();
    let input = builder.param("input", TypeId::STRING);
    let appended = builder.assign(
        "appended",
        TypeId::STRING,
        Value::Builtin {
            op: Builtin::StringAppend,
            args: vec![input.into(), Const::String("d".to_owned()).into()],
        },
    );
    let size = builder.assign(
        "size",
        TypeId::INT,
        Value::Builtin {
            op: Builtin::StringByteSize,
            args: vec![appended.into()],
        },
    );
    builder.term(Term::Return(Some(size.into())));
    let function = builder.finish();
    let mut flow = Module::new(core);
    flow.add(function);

    let output = compile_flow(&flow, &Options::default().with_string_capacity(3))
        .expect("emit String.append capacity check");
    compile_and_run(
        output,
        "#include <stdlib.h>\n#include <string.h>\n#include \"emela_embedded.h\"\n#include \"emela_board.h\"\n_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) { if (kind == EMELA_DEFECT_STRING_CAPACITY && length == UINT16_C(24) && memcmp(message, \"string capacity exceeded\", length) == 0) exit(0); exit(1); }\nint main(void) { EmelaString input = { .data = \"abc\", .length = UINT16_C(3) }; (void)emela_export_append_one_0(input); return 2; }\n",
    );
}

#[test]
fn statically_evaluates_string_append_in_const_functions() {
    let core = emela_core::Module::new();
    let mut builder = FnBuilder::new(&core, "CONCATENATED");
    builder
        .set_ret(TypeId::STRING)
        .set_exported()
        .set_constant();
    let appended = builder.assign(
        "appended",
        TypeId::STRING,
        Value::Builtin {
            op: Builtin::StringAppend,
            args: vec![
                Const::String("a".to_owned()).into(),
                Const::String("é".to_owned()).into(),
            ],
        },
    );
    builder.term(Term::Return(Some(appended.into())));
    let function = builder.finish();
    let mut flow = Module::new(core);
    flow.add(function);

    let output = compile_flow(&flow, &Options::default()).expect("evaluate const String.append");
    let source = &output
        .files
        .iter()
        .find(|file| file.path == "emela_embedded.c")
        .expect("generated source")
        .contents;
    assert!(!source.contains("emela_emb_string_append"));
    compile_and_run(
        output,
        "#include <string.h>\n#include \"emela_embedded.h\"\nint main(void) { EmelaString value = emela_export_CONCATENATED_0(); return value.length == UINT16_C(3) && memcmp(value.data, \"a\\xc3\\xa9\", 3) == 0 ? 0 : 1; }\n",
    );
}

#[test]
fn shows_dynamic_strings_into_a_bounded_scratch_buffer() {
    let core = emela_core::Module::new();
    let mut builder = FnBuilder::new(&core, "show_runtime");
    builder.set_ret(TypeId::BOOL).set_exported();
    let input = builder.param("input", TypeId::STRING);
    let rendered = builder.assign(
        "rendered",
        TypeId::STRING,
        Value::Runtime {
            op: RuntimeOp::ShowString,
            args: vec![input.into()],
        },
    );
    let size = builder.assign(
        "size",
        TypeId::INT,
        Value::Builtin {
            op: Builtin::StringByteSize,
            args: vec![rendered.into()],
        },
    );
    let expected = [
        "\"q",
        r#"\""#,
        " ",
        r#"\\"#,
        " ",
        r#"\#{"#,
        " # ",
        r#"\n"#,
        r#"\t"#,
        r#"\r"#,
        r#"\u{0}"#,
        r#"\u{7}"#,
        r#"\u{7f}"#,
        "é",
        "\"",
    ]
    .concat();
    let size_matches = builder.assign(
        "size_matches",
        TypeId::BOOL,
        Value::Prim {
            op: PrimOp::Eq,
            ty: PrimTy::I32,
            args: vec![size.into(), Const::Int(expected.len() as i32).into()],
        },
    );
    let content_matches = builder.assign(
        "content_matches",
        TypeId::BOOL,
        Value::Builtin {
            op: Builtin::StringStartsWith,
            args: vec![rendered.into(), Const::String(expected).into()],
        },
    );
    let result = builder.var("result", TypeId::BOOL);
    builder.if_(
        Some(result),
        size_matches,
        |then| then.term(Term::Yield(Some(content_matches.into()))),
        |otherwise| otherwise.term(Term::Yield(Some(Const::Bool(false).into()))),
    );
    builder.term(Term::Return(Some(result.into())));
    let function = builder.finish();
    let mut flow = Module::new(core);
    flow.add(function);

    let output = compile_flow(&flow, &Options::default()).expect("emit RuntimeOp::ShowString");
    let source = &output
        .files
        .iter()
        .find(|file| file.path == "emela_embedded.c")
        .expect("generated source")
        .contents;
    assert!(source.contains("char emela_string_buffer_0[256];"));
    compile_and_run(
        output,
        r#"
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) {
    (void)kind; (void)message; (void)length; exit(2);
}
int main(void) {
    static const unsigned char input[] = {
        'q', '"', ' ', '\\', ' ', '#', '{', ' ', '#', ' ', '\n', '\t', '\r',
        '\0', 0x07, 0x7f, 0xc3, 0xa9
    };
    EmelaString value = { .data = (const char *)input, .length = sizeof input };
    return emela_export_show_runtime_0(value) ? 0 : 1;
}
"#,
    );
}

#[test]
fn statically_shows_a_string_without_runtime_scratch() {
    let core = emela_core::Module::new();
    let mut builder = FnBuilder::new(&core, "show_static");
    builder.set_ret(TypeId::STRING).set_exported();
    let rendered = builder.assign(
        "rendered",
        TypeId::STRING,
        Value::Runtime {
            op: RuntimeOp::ShowString,
            args: vec![Const::String("a\"b#{\n".to_owned()).into()],
        },
    );
    builder.term(Term::Return(Some(rendered.into())));
    let function = builder.finish();
    let mut flow = Module::new(core);
    flow.add(function);

    let output = compile_flow(&flow, &Options::default()).expect("fold static show string");
    let source = &output
        .files
        .iter()
        .find(|file| file.path == "emela_embedded.c")
        .expect("generated source")
        .contents;
    assert!(!source.contains("emela_emb_string_show"));
    assert!(!source.contains("char emela_string_buffer_"));
    compile_and_run(
        output,
        r#"
#include <string.h>
#include "emela_embedded.h"
static const unsigned char expected[] = {
    0x22, 'a', 0x5c, 0x22, 'b', 0x5c, '#', '{', 0x5c, 'n', 0x22
};
int main(void) {
    EmelaString value = emela_export_show_static_0();
    return value.length == sizeof expected && memcmp(value.data, expected, sizeof expected) == 0 ? 0 : 1;
}
"#,
    );
}

#[test]
fn runtime_show_string_capacity_overflow_uses_the_board_defect_hook() {
    let core = emela_core::Module::new();
    let mut builder = FnBuilder::new(&core, "show_too_large");
    builder.set_ret(TypeId::INT).set_exported();
    let input = builder.param("input", TypeId::STRING);
    let rendered = builder.assign(
        "rendered",
        TypeId::STRING,
        Value::Runtime {
            op: RuntimeOp::ShowString,
            args: vec![input.into()],
        },
    );
    let size = builder.assign(
        "size",
        TypeId::INT,
        Value::Builtin {
            op: Builtin::StringByteSize,
            args: vec![rendered.into()],
        },
    );
    builder.term(Term::Return(Some(size.into())));
    let function = builder.finish();
    let mut flow = Module::new(core);
    flow.add(function);

    let output = compile_flow(&flow, &Options::default().with_string_capacity(2))
        .expect("emit the show capacity check");
    compile_and_run(
        output,
        "#include <stdlib.h>\n#include <string.h>\n#include \"emela_embedded.h\"\n#include \"emela_board.h\"\n_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) { if (kind == EMELA_DEFECT_STRING_CAPACITY && length == UINT16_C(24) && memcmp(message, \"string capacity exceeded\", length) == 0) exit(0); exit(1); }\nint main(void) { EmelaString input = { .data = \"x\", .length = UINT16_C(1) }; (void)emela_export_show_too_large_0(input); return 2; }\n",
    );
}

#[test]
fn debug_print_calls_the_board_hook_with_the_string_view() {
    let core = emela_core::Module::new();
    let mut builder = FnBuilder::new(&core, "debug_line");
    builder.set_ret(TypeId::UNIT).set_exported();
    let input = builder.param("input", TypeId::STRING);
    builder.let_(
        None,
        Value::Runtime {
            op: RuntimeOp::DebugPrint,
            args: vec![input.into()],
        },
    );
    builder.term(Term::Return(None));
    let function = builder.finish();
    let mut flow = Module::new(core);
    flow.add(function);

    let output = compile_flow(&flow, &Options::default()).expect("emit RuntimeOp::DebugPrint");
    let board_header = &output
        .files
        .iter()
        .find(|file| file.path == "emela_board.h")
        .expect("board header")
        .contents;
    assert!(board_header.contains("void emela_board_debug(const char *bytes, uint16_t length);"));
    assert!(!board_header.contains("emela_board_defect"));
    compile_and_run(
        output,
        "#include <stdlib.h>\n#include <string.h>\n#include \"emela_embedded.h\"\n#include \"emela_board.h\"\nstatic int calls;\nvoid emela_board_debug(const char *bytes, uint16_t length) { if (length != UINT16_C(3) || memcmp(bytes, \"abc\", 3) != 0) exit(1); ++calls; }\nint main(void) { EmelaString input = { .data = \"abc\", .length = UINT16_C(3) }; emela_export_debug_line_0(input); return calls == 1 ? 0 : 2; }\n",
    );
}

#[test]
fn debug_print_can_consume_a_runtime_show_view_before_return() {
    let core = emela_core::Module::new();
    let mut builder = FnBuilder::new(&core, "debug_shown_string");
    builder.set_ret(TypeId::UNIT).set_exported();
    let input = builder.param("input", TypeId::STRING);
    let rendered = builder.assign(
        "rendered",
        TypeId::STRING,
        Value::Runtime {
            op: RuntimeOp::ShowString,
            args: vec![input.into()],
        },
    );
    builder.let_(
        None,
        Value::Runtime {
            op: RuntimeOp::DebugPrint,
            args: vec![rendered.into()],
        },
    );
    builder.term(Term::Return(None));
    let function = builder.finish();
    let mut flow = Module::new(core);
    flow.add(function);

    let output = compile_flow(&flow, &Options::default()).expect("emit show then debug print");
    compile_and_run(
        output,
        r#"
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
static int calls;
static const unsigned char expected[] = { 0x22, 'a', 0x5c, 0x22, 'b', 0x5c, 'n', 0x22 };
void emela_board_debug(const char *bytes, uint16_t length) {
    if (length != sizeof expected || memcmp(bytes, expected, sizeof expected) != 0) exit(1);
    ++calls;
}
_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) {
    (void)kind; (void)message; (void)length; exit(2);
}
int main(void) {
    EmelaString input = { .data = "a\"b\n", .length = UINT16_C(4) };
    emela_export_debug_shown_string_0(input);
    return calls == 1 ? 0 : 3;
}
"#,
    );
}

#[test]
fn rejects_dynamic_string_append_that_returns_its_stack_buffer() {
    let core = emela_core::Module::new();
    let mut builder = FnBuilder::new(&core, "return_appended");
    builder.set_ret(TypeId::STRING).set_exported();
    let input = builder.param("input", TypeId::STRING);
    let appended = builder.assign(
        "appended",
        TypeId::STRING,
        Value::Builtin {
            op: Builtin::StringAppend,
            args: vec![input.into(), Const::String("x".to_owned()).into()],
        },
    );
    builder.term(Term::Return(Some(appended.into())));
    let function = builder.finish();
    let mut flow = Module::new(core);
    flow.add(function);

    let errors = compile_flow(&flow, &Options::default()).expect_err("reject an escaping buffer");
    assert!(errors.iter().any(|error| {
        error.kind == ErrorKind::Unsupported && error.message.contains("outlive its function stack")
    }));
}

#[test]
fn allows_a_local_string_buffer_in_a_scalar_fallible_function() {
    let core = emela_core::Module::new();
    let mut builder = FnBuilder::new(&core, "append_size_fallible");
    builder.set_ret(TypeId::INT).set_exported().set_fails();
    let input = builder.param("input", TypeId::STRING);
    let appended = builder.assign(
        "appended",
        TypeId::STRING,
        Value::Builtin {
            op: Builtin::StringAppend,
            args: vec![input.into(), Const::String("x".to_owned()).into()],
        },
    );
    let size = builder.assign(
        "size",
        TypeId::INT,
        Value::Builtin {
            op: Builtin::StringByteSize,
            args: vec![appended.into()],
        },
    );
    builder.term(Term::Return(Some(size.into())));
    let function = builder.finish();
    let mut flow = Module::new(core);
    flow.add(function);

    let output = compile_flow(&flow, &Options::default())
        .expect("emit a non-escaping buffer in a fallible function");
    compile_and_run(
        output,
        "#include <stdlib.h>\n#include \"emela_embedded.h\"\n#include \"emela_board.h\"\n_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) { (void)kind; (void)message; (void)length; exit(2); }\nint main(void) { EmelaString input = { .data = \"abc\", .length = UINT16_C(3) }; return emela_export_append_size_fallible_0(input) == INT32_C(4) ? 0 : 1; }\n",
    );
}

#[test]
fn string_search_predicates_match_utf8_substrings_without_copying() {
    let core = emela_core::Module::new();
    let mut contains = FnBuilder::new(&core, "string_contains");
    contains.set_ret(TypeId::BOOL).set_exported();
    let value = contains.param("value", TypeId::STRING);
    let needle = contains.param("needle", TypeId::STRING);
    let result = contains.assign(
        "result",
        TypeId::BOOL,
        Value::Builtin {
            op: Builtin::StringContains,
            args: vec![value.into(), needle.into()],
        },
    );
    contains.term(Term::Return(Some(result.into())));
    let contains = contains.finish();

    let mut starts_with = FnBuilder::new(&core, "string_starts_with");
    starts_with.set_ret(TypeId::BOOL).set_exported();
    let value = starts_with.param("value", TypeId::STRING);
    let prefix = starts_with.param("prefix", TypeId::STRING);
    let result = starts_with.assign(
        "result",
        TypeId::BOOL,
        Value::Builtin {
            op: Builtin::StringStartsWith,
            args: vec![value.into(), prefix.into()],
        },
    );
    starts_with.term(Term::Return(Some(result.into())));
    let starts_with = starts_with.finish();

    let mut ends_with = FnBuilder::new(&core, "string_ends_with");
    ends_with.set_ret(TypeId::BOOL).set_exported();
    let value = ends_with.param("value", TypeId::STRING);
    let suffix = ends_with.param("suffix", TypeId::STRING);
    let result = ends_with.assign(
        "result",
        TypeId::BOOL,
        Value::Builtin {
            op: Builtin::StringEndsWith,
            args: vec![value.into(), suffix.into()],
        },
    );
    ends_with.term(Term::Return(Some(result.into())));
    let ends_with = ends_with.finish();

    let mut flow = Module::new(core);
    flow.add(contains);
    flow.add(starts_with);
    flow.add(ends_with);
    let output = compile_flow(&flow, &Options::default()).expect("emit String search builtins");
    compile_and_run(
        output,
        "#include \"emela_embedded.h\"\nint main(void) { EmelaString value = { .data = \"a\\xe2\\x82\\xac\\xf0\\x9f\\x99\\x82z\", .length = UINT16_C(9) }; EmelaString middle = { .data = \"\\xe2\\x82\\xac\\xf0\\x9f\\x99\\x82\", .length = UINT16_C(7) }; EmelaString prefix = { .data = \"a\\xe2\\x82\\xac\", .length = UINT16_C(4) }; EmelaString suffix = { .data = \"\\xf0\\x9f\\x99\\x82z\", .length = UINT16_C(5) }; EmelaString empty = { .data = \"\", .length = UINT16_C(0) }; return emela_export_string_contains_0(value, middle) && emela_export_string_contains_0(value, empty) && emela_export_string_starts_with_1(value, prefix) && emela_export_string_starts_with_1(value, empty) && emela_export_string_ends_with_2(value, suffix) && emela_export_string_ends_with_2(value, empty) ? 0 : 1; }\n",
    );
}

#[test]
fn statically_evaluates_string_search_builtins_in_const_functions() {
    let core = emela_core::Module::new();
    let mut builder = FnBuilder::new(&core, "HAS_SUBSTRING");
    builder.set_ret(TypeId::BOOL).set_exported().set_constant();
    let result = builder.assign(
        "result",
        TypeId::BOOL,
        Value::Builtin {
            op: Builtin::StringContains,
            args: vec![
                Const::String("aé🙂z".to_owned()).into(),
                Const::String("é🙂".to_owned()).into(),
            ],
        },
    );
    builder.term(Term::Return(Some(result.into())));
    let function = builder.finish();
    let mut flow = Module::new(core);
    flow.add(function);

    let output = compile_flow(&flow, &Options::default()).expect("evaluate String.contains");
    compile_and_run(
        output,
        "#include \"emela_embedded.h\"\nint main(void) { return emela_export_HAS_SUBSTRING_0() == INT32_C(1) ? 0 : 1; }\n",
    );
}

#[test]
fn string_trim_returns_a_unicode_whitespace_view() {
    let core = emela_core::Module::new();
    let mut builder = FnBuilder::new(&core, "string_trim");
    builder.set_ret(TypeId::STRING).set_exported();
    let value = builder.param("value", TypeId::STRING);
    let trimmed = builder.assign(
        "trimmed",
        TypeId::STRING,
        Value::Builtin {
            op: Builtin::StringTrim,
            args: vec![value.into()],
        },
    );
    builder.term(Term::Return(Some(trimmed.into())));
    let function = builder.finish();
    let mut flow = Module::new(core);
    flow.add(function);

    let output = compile_flow(&flow, &Options::default()).expect("emit String.trim");
    compile_and_run(
        output,
        "#include <string.h>\n#include \"emela_embedded.h\"\nint main(void) { EmelaString input = { .data = \"\\t\\xc2\\xa0\\xe3\\x80\\x80\" \"a\\xe2\\x82\\xac\" \"b\\xe2\\x80\\xaf\\r\\n\", .length = UINT16_C(16) }; EmelaString value = emela_export_string_trim_0(input); if (value.length != UINT16_C(5) || memcmp(value.data, \"a\\xe2\\x82\\xac\" \"b\", 5) != 0) return 1; EmelaString whitespace = { .data = \" \\xe2\\x80\\xa8\\xe3\\x80\\x80\", .length = UINT16_C(7) }; if (emela_export_string_trim_0(whitespace).length != UINT16_C(0)) return 2; EmelaString bom = { .data = \" \\xef\\xbb\\xbf \", .length = UINT16_C(5) }; value = emela_export_string_trim_0(bom); if (value.length != UINT16_C(3) || memcmp(value.data, \"\\xef\\xbb\\xbf\", 3) != 0) return 3; EmelaString empty = { .data = 0, .length = UINT16_C(0) }; if (emela_export_string_trim_0(empty).length != UINT16_C(0)) return 4; return 0; }\n",
    );
}

#[test]
fn statically_evaluates_string_trim_with_the_fixed_whitespace_set() {
    let core = emela_core::Module::new();
    let mut builder = FnBuilder::new(&core, "TRIMMED");
    builder
        .set_ret(TypeId::STRING)
        .set_exported()
        .set_constant();
    let trimmed = builder.assign(
        "trimmed",
        TypeId::STRING,
        Value::Builtin {
            op: Builtin::StringTrim,
            args: vec![Const::String("\t\u{00a0}\u{3000}x\u{202f}\n".to_owned()).into()],
        },
    );
    builder.term(Term::Return(Some(trimmed.into())));
    let function = builder.finish();
    let mut flow = Module::new(core);
    flow.add(function);

    let output = compile_flow(&flow, &Options::default()).expect("evaluate String.trim");
    compile_and_run(
        output,
        "#include \"emela_embedded.h\"\nint main(void) { EmelaString value = emela_export_TRIMMED_0(); return value.length == UINT16_C(1) && value.data[0] == 'x' ? 0 : 1; }\n",
    );
}

#[test]
fn defers_a_statically_known_const_defect_until_reference() {
    let core = emela_core::Module::new();
    let mut constant = FnBuilder::new(&core, "BROKEN");
    constant
        .set_ret(TypeId::NEVER)
        .set_exported()
        .set_constant();
    constant.term(Term::Defect(
        Const::String("division by zero".to_owned()).into(),
    ));
    let constant = constant.finish();
    let mut flow = Module::new(core);
    flow.add(constant);

    let output = compile_flow(&flow, &Options::default()).expect("emit deferred const defect");
    let source = &output
        .files
        .iter()
        .find(|file| file.path == "emela_embedded.c")
        .expect("generated source")
        .contents;
    assert!(source.contains("EMELA_DEFECT_DIVISION_BY_ZERO"));
    compile_and_run(
        output,
        "#include <stdlib.h>\n#include <string.h>\n#include \"emela_embedded.h\"\n#include \"emela_board.h\"\nstatic const char expected[] = \"while evaluating const BROKEN: division by zero\";\n_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) { if (kind == EMELA_DEFECT_DIVISION_BY_ZERO && length == sizeof expected - 1 && memcmp(message, expected, length) == 0) exit(0); exit(1); }\nint main(void) { emela_export_BROKEN_0(); return 1; }\n",
    );
}

#[test]
fn folds_const_division_by_zero_into_a_deferred_defect() {
    let core = emela_core::Module::new();
    let mut constant = FnBuilder::new(&core, "ZERO_DIVISION");
    constant.set_ret(TypeId::INT).set_exported().set_constant();
    let value = constant.assign(
        "quotient",
        TypeId::INT,
        Value::Prim {
            op: PrimOp::Div,
            ty: PrimTy::I32,
            args: vec![Const::Int(1).into(), Const::Int(0).into()],
        },
    );
    constant.term(Term::Return(Some(value.into())));
    let constant = constant.finish();
    let mut flow = Module::new(core);
    flow.add(constant);

    let output = compile_flow(&flow, &Options::default()).expect("emit deferred division defect");
    compile_and_run(
        output,
        "#include <stdlib.h>\n#include <string.h>\n#include \"emela_embedded.h\"\n#include \"emela_board.h\"\nstatic const char expected[] = \"while evaluating const ZERO_DIVISION: division by zero\";\n_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length) { if (kind == EMELA_DEFECT_DIVISION_BY_ZERO && length == sizeof expected - 1 && memcmp(message, expected, length) == 0) exit(0); exit(1); }\nint main(void) { (void)emela_export_ZERO_DIVISION_0(); return 1; }\n",
    );
}

#[test]
fn ignores_an_unevaluable_unreferenced_internal_const_and_reports_its_use() {
    let core = emela_core::Module::new();
    let mut unused = FnBuilder::new(&core, "UNUSED");
    unused.set_ret(TypeId::INT).set_constant();
    let unused_value = unused.var("value", TypeId::INT);
    unused.if_(
        Some(unused_value),
        Const::Bool(true),
        |branch| branch.term(Term::Yield(Some(Const::Int(1).into()))),
        |branch| branch.term(Term::Yield(Some(Const::Int(2).into()))),
    );
    unused.term(Term::Return(Some(unused_value.into())));
    let mut unused_flow = Module::new(core.clone());
    unused_flow.add(unused.finish());
    compile_flow(&unused_flow, &Options::default())
        .expect("an unused const should not block code generation");

    let mut constant = FnBuilder::new(&core, "CHOICE");
    constant.set_ret(TypeId::INT).set_constant();
    let value = constant.var("value", TypeId::INT);
    constant.if_(
        Some(value),
        Const::Bool(true),
        |branch| branch.term(Term::Yield(Some(Const::Int(1).into()))),
        |branch| branch.term(Term::Yield(Some(Const::Int(2).into()))),
    );
    constant.term(Term::Return(Some(value.into())));
    let constant = constant.finish();
    let mut flow = Module::new(core);
    let constant_id = flow.add(constant);
    let mut caller = FnBuilder::new(&flow.core, "PUBLIC_USER");
    caller.set_ret(TypeId::INT).set_exported();
    let result = caller.assign(
        "result",
        TypeId::INT,
        Value::Call {
            callee: Callee::Direct(constant_id),
            args: Vec::new(),
            suspends: false,
            on_fail: None,
        },
    );
    caller.term(Term::Return(Some(result.into())));
    flow.add(caller.finish());

    let errors = compile_flow(&flow, &Options::default())
        .expect_err("a referenced const must be statically evaluable");
    assert_eq!(errors[0].kind, ErrorKind::Unsupported);
    assert!(errors[0].message.contains("PUBLIC_USER"));
    assert!(errors[0].message.contains("CHOICE"));
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
fn validates_isr_entries_and_external_safety() {
    let core = emela_core::Module::new();
    let external = {
        let builder = FnBuilder::new(&core, "timer_tick");
        builder.finish_external(External {
            bindings: vec![ExternalBinding {
                target: "c".to_owned(),
                host: HostRef::Global("board_timer_tick".to_owned()),
            }],
        })
    };
    let mut flow = Module::new(core);
    let external_id = flow.add(external);
    let entry = {
        let mut builder = FnBuilder::new(&flow.core, "on_timer");
        builder.set_exported();
        builder.let_(
            None,
            Value::Call {
                callee: Callee::Direct(external_id),
                args: Vec::new(),
                suspends: false,
                on_fail: None,
            },
        );
        builder.term(Term::Return(None));
        builder.finish()
    };
    flow.add(entry);

    let errors = compile_flow(&flow, &Options::default().with_isr_entry("on_timer"))
        .expect_err("require an ISR-safe annotation for external C calls");
    assert!(errors[0].message.contains("not marked ISR-safe"));

    let output = compile_flow(
        &flow,
        &Options::default()
            .with_isr_entry("on_timer")
            .with_isr_safe_external("board_timer_tick"),
    )
    .expect("accept an explicitly ISR-safe external C function");
    compile_and_run(
        output,
        "#include \"emela_embedded.h\"\nstatic int tick_count;\nvoid board_timer_tick(void) { tick_count++; }\nint main(void) { emela_export_on_timer_1(); return tick_count == 1 ? 0 : 1; }\n",
    );
}

#[test]
fn rejects_unknown_or_heap_using_isr_entries() {
    let core = emela_core::Module::new();
    let function = {
        let mut builder = FnBuilder::new(&core, "plain_interrupt");
        builder.set_ret(TypeId::UNIT).set_exported();
        builder.term(Term::Return(None));
        builder.finish()
    };
    let mut flow = Module::new(core);
    flow.add(function);

    let errors = compile_flow(
        &flow,
        &Options::default().with_isr_entry("missing_interrupt"),
    )
    .expect_err("require configured ISR entries to name exported functions");
    assert_eq!(errors[0].kind, ErrorKind::InvalidOptions);

    let mut core = emela_core::Module::new();
    let list_int = core.types.intern(Type::List(TypeId::INT));
    let function = {
        let mut builder = FnBuilder::new(&core, "list_interrupt");
        builder.set_ret(TypeId::BOOL).set_exported();
        let list = builder.assign(
            "list",
            list_int,
            Value::List {
                elems: vec![Const::Int(1).into()],
                tail: None,
            },
        );
        let empty = builder.assign("empty", TypeId::BOOL, Value::ListIsEmpty(list));
        builder.term(Term::Return(Some(empty.into())));
        builder.finish()
    };
    let mut flow = Module::new(core);
    flow.add(function);

    let errors = compile_flow(
        &flow,
        &Options::default()
            .with_heap_bytes(1024)
            .with_isr_entry("list_interrupt"),
    )
    .expect_err("reject heap-backed List values reachable from an ISR");
    assert!(errors[0].message.contains("heap-backed List"));
}

#[test]
fn rejects_recursive_calls_reachable_from_isr_entries() {
    let core = emela_core::Module::new();
    let mut flow = Module::new(core);
    let entry_id = flow.declare("recursive_interrupt");
    let entry = {
        let mut builder = FnBuilder::new(&flow.core, "recursive_interrupt");
        builder.set_ret(TypeId::UNIT).set_exported();
        builder.let_(
            None,
            Value::Call {
                callee: Callee::Direct(entry_id),
                args: Vec::new(),
                suspends: false,
                on_fail: None,
            },
        );
        builder.term(Term::Return(None));
        builder.finish()
    };
    flow.define(entry_id, entry);

    let errors = compile_flow(
        &flow,
        &Options::default().with_isr_entry("recursive_interrupt"),
    )
    .expect_err("reject recursive call graphs reachable from an ISR");
    assert!(errors[0].message.contains("recursive call"));
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
