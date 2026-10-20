use emela_codegen_emb::{ErrorKind, Options, compile, compile_flow, flow_from_core, verify_flow};
use emela_core::{Builtin, TypeId};
use emela_lir::{
    Callee, Const, FnBuilder, Module, OnFail, RuntimeOp, Stage, Stmt, Suspend, Term, Value,
};

#[test]
fn emits_c_files_for_an_empty_module() {
    let output = compile(&emela_core::Module::new(), &Options::default())
        .expect("an empty Core module should produce an empty C translation unit");

    assert_eq!(output.files.len(), 3);
    assert_eq!(output.files[0].path, "emela_embedded.h");
    assert_eq!(output.files[1].path, "emela_board.h");
    assert!(
        output.files[1]
            .contents
            .contains("EMELA_DEFECT_DIVISION_BY_ZERO")
    );
    assert!(!output.files[1].contents.contains("emela_board_defect"));
    assert_eq!(output.files[2].path, "emela_embedded.c");
    assert!(output.files[2].contents.contains("emela_embedded.h"));
    assert!(output.files[2].contents.contains("emela_board.h"));
}

#[test]
fn lowers_and_verifies_core_as_flow_ir() {
    let mut core = emela_core::Module::new();
    let main = core.declare("main", true);
    core.define(
        main,
        Vec::new(),
        emela_core::TypeId::UNIT,
        emela_core::Expr::Lit(emela_core::Lit::Unit),
    );

    let flow = flow_from_core(&core).expect("Core should lower to valid Flow IR");

    assert_eq!(flow.stage, emela_lir::Stage::Flow);
    assert!(
        flow.functions
            .iter()
            .any(|(_, function)| function.name == "main")
    );
    verify_flow(&flow).expect("the Flow IR verifier should accept the result");
}

fn rejects_flow(module: &Module, expected: &str) {
    for errors in [
        verify_flow(module).expect_err("reject the non-Flow construct at the public verifier"),
        compile_flow(module, &Options::default())
            .expect_err("reject the non-Flow construct before backend lowering"),
    ] {
        assert!(!errors.is_empty());
        assert!(errors.iter().all(|e| e.kind == ErrorKind::InvalidFlowIr));
        assert!(
            errors.iter().any(|e| e.message.contains(expected)),
            "{errors:?}"
        );
    }
}

#[test]
fn rejects_every_heap_stage_before_codegen() {
    for stage in [
        Stage::ClosuresConverted,
        Stage::DefectsExplicit,
        Stage::StateMachines,
        Stage::RefCounted,
    ] {
        let mut module = Module::new(emela_core::Module::new());
        module.stage = stage;
        rejects_flow(&module, "expected Flow IR");
    }
}

#[test]
fn rejects_heap_only_values_and_reference_count_statements_at_flow_stage() {
    for variant in ["MakeClosure", "EnvGet", "Dup", "Drop"] {
        let mut module = Module::new(emela_core::Module::new());
        let mut target = FnBuilder::new(&module.core, "target");
        target.param("env", TypeId::ANY);
        target.term(Term::Return(None));
        let mut target = target.finish();
        target.env = Some(vec![]);
        let target = module.add(target);
        let mut builder = FnBuilder::new(&module.core, variant);
        builder.set_exported();
        let env = builder.param("env", TypeId::ANY);
        match variant {
            "MakeClosure" => builder.let_(
                None,
                Value::MakeClosure {
                    function: target,
                    captures: vec![],
                },
            ),
            "EnvGet" => {
                builder.assign("capture", TypeId::INT, Value::EnvGet { env, index: 0 });
            }
            "Dup" => {
                builder.stmt(Stmt::Dup(env));
            }
            "Drop" => {
                builder.stmt(Stmt::Drop(env));
            }
            _ => unreachable!(),
        }
        builder.term(Term::Return(None));
        let mut function = builder.finish();
        if variant == "EnvGet" {
            function.env = Some(vec![TypeId::INT]);
        }
        module.add(function);
        rejects_flow(
            &module,
            match variant {
                "MakeClosure" => "closure record",
                "EnvGet" => "env_get",
                _ => "dup and drop",
            },
        );
    }
}

#[test]
fn rejects_heap_only_unwind_and_explicit_landing_edges_at_flow_stage() {
    let mut module = Module::new(emela_core::Module::new());
    let mut builder = FnBuilder::new(&module.core, "unwind");
    builder.set_exported();
    builder.term(Term::Unwind);
    let mut function = builder.finish();
    function.defects = true;
    module.add(function);
    rejects_flow(&module, "unwind");

    for suspend in [false, true] {
        let mut module = Module::new(emela_core::Module::new());
        let mut builder = FnBuilder::new(&module.core, "landing");
        builder.set_exported().set_suspends();
        let landing = builder.seq(|b| b.term(Term::Return(None)));
        builder.stmt(Stmt::Let {
            dst: None,
            value: Value::Const(Const::Unit),
            on_defect: (!suspend).then_some(landing),
            on_suspend: suspend.then_some(Suspend {
                state: 0,
                saved: vec![],
            }),
        });
        builder.term(Term::Return(None));
        module.add(builder.finish());
        rejects_flow(
            &module,
            if suspend {
                "suspension edge"
            } else {
                "defect landing"
            },
        );
    }
}

#[test]
fn rejects_heap_only_failure_handle_at_flow_stage() {
    let mut module = Module::new(emela_core::Module::new());
    let mut callee = FnBuilder::new(&module.core, "callee");
    callee.set_fails();
    let error = callee.param("error", TypeId::ANY);
    callee.term(Term::Fail(error.into()));
    let callee = module.add(callee.finish());
    let mut caller = FnBuilder::new(&module.core, "caller");
    caller.set_exported();
    let error = caller.param("error", TypeId::ANY);
    let caught = caller.var("caught", TypeId::ANY);
    let landing = caller.seq(|b| b.term(Term::Return(None)));
    caller.let_(
        None,
        Value::Call {
            callee: Callee::Direct(callee),
            args: vec![error.into()],
            suspends: false,
            on_fail: Some(OnFail::Handle {
                error: caught,
                body: landing,
            }),
        },
    );
    caller.term(Term::Return(None));
    module.add(caller.finish());
    rejects_flow(&module, "failure landing");
}

#[test]
fn rejects_heap_only_runtime_operations_at_flow_stage() {
    for op in [
        RuntimeOp::Defect,
        RuntimeOp::DefectTake,
        RuntimeOp::DefectResume,
        RuntimeOp::Yield,
        RuntimeOp::YieldPerform,
    ] {
        let mut module = Module::new(emela_core::Module::new());
        let mut builder = FnBuilder::new(&module.core, op.name());
        builder.set_exported().set_suspends();
        let args = match op {
            RuntimeOp::Defect => vec![Const::String("defect".into()).into()],
            RuntimeOp::DefectResume => vec![builder.param("pending", TypeId::ANY).into()],
            _ => vec![],
        };
        builder.let_(None, Value::Runtime { op, args });
        builder.term(Term::Return(None));
        module.add(builder.finish());
        rejects_flow(
            &module,
            if matches!(op, RuntimeOp::Yield | RuntimeOp::YieldPerform) {
                "fairness yield"
            } else {
                "Heap IR runtime"
            },
        );
    }
}

#[test]
fn rejects_source_builtins_that_must_lower_to_flow_control_or_runtime() {
    for op in [Builtin::Panic, Builtin::Todo, Builtin::Dbg] {
        let mut module = Module::new(emela_core::Module::new());
        let mut builder = FnBuilder::new(&module.core, "unlowered_builtin");
        builder.set_exported();
        let args = match op {
            Builtin::Panic => vec![Const::String("panic".into()).into()],
            Builtin::Todo => vec![],
            Builtin::Dbg => vec![Const::Int(42).into()],
            _ => unreachable!(),
        };
        builder.let_(None, Value::Builtin { op, args });
        builder.term(Term::Return(None));
        module.add(builder.finish());
        rejects_flow(&module, "unlowered builtin");
    }
}

#[test]
fn rejects_recur_crossing_protect_and_return_leaving_cleanup() {
    let mut module = Module::new(emela_core::Module::new());
    let mut builder = FnBuilder::new(&module.core, "recur_across_protect");
    builder.set_exported();
    let n = builder.var("n", TypeId::INT);
    builder.loop_(None, vec![(n, Const::Int(0).into())], |b| {
        b.protect(None, |b| b.term(Term::Recur(vec![n.into()])), |_| {});
    });
    builder.term(Term::Return(None));
    module.add(builder.finish());
    rejects_flow(
        &module,
        "recur outside a loop of the same body, or across protect",
    );

    let mut module = Module::new(emela_core::Module::new());
    let mut builder = FnBuilder::new(&module.core, "return_from_cleanup");
    builder.set_exported();
    builder.protect(None, |_| {}, |b| b.term(Term::Return(None)));
    builder.term(Term::Return(None));
    module.add(builder.finish());
    rejects_flow(&module, "return leaves a cleanup");
}
