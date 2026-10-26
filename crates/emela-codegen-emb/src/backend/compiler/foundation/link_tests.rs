use super::*;
use emela_core::{External, ExternalBinding, FnType, HostRef};
use emela_lir::FnBuilder;

fn unknown_function() -> FnId {
    FnId::from_raw(999_999u32.into())
}

fn fixture(capture: bool) -> (CompiledModule, FnId, FnId, VarId, VarId) {
    let mut core = emela_core::Module::new();
    let callback_type = core
        .types
        .intern(Type::Fn(FnType::pure(vec![], TypeId::INT)));
    let mut module = Module::new(core);
    let mut registration = FnBuilder::new(&module.core, "registration");
    registration.param("id", TypeId::INT);
    registration.param("callback", callback_type);
    let registration = module.add(registration.finish_external(External {
        bindings: vec![ExternalBinding {
            target: "c".into(),
            host: HostRef::Global("registration".into()),
        }],
    }));
    let mut callback = FnBuilder::new(&module.core, "callback");
    callback.set_ret(TypeId::INT);
    callback.term(Term::Return(Some(Const::Int(42).into())));
    let callback = module.add(callback.finish());
    let mut entry = FnBuilder::new(&module.core, "entry");
    entry.set_exported();
    let value = entry.assign("registered_callback", callback_type, Value::FnRef(callback));
    let unrelated = entry.assign("unrelated", callback_type, Value::FnRef(callback));
    entry.let_(
        None,
        Value::Call {
            callee: Callee::Direct(registration),
            args: vec![Const::Int(1).into(), value.into()],
            suspends: false,
            on_fail: None,
        },
    );
    if capture {
        entry.set_ret(callback_type);
        let closure = entry.var("closure", callback_type);
        entry.closure(closure, vec![], vec![value], false, |b| {
            let result = b.assign(
                "result",
                TypeId::INT,
                Value::Call {
                    callee: Callee::Indirect(value),
                    args: vec![],
                    suspends: false,
                    on_fail: None,
                },
            );
            b.term(Term::Return(Some(result.into())));
        });
        entry.term(Term::Return(Some(closure.into())));
    } else {
        entry.term(Term::Return(None));
    }
    let entry = module.add(entry.finish());
    (
        compile_flow(&module).unwrap(),
        registration,
        entry,
        value,
        unrelated,
    )
}

#[test]
fn link_additions_keep_existing_ids_types_and_artifact_unchanged() {
    let (compiled, registration, _, _, _) = fixture(false);
    let before = emela_lir::dump(compiled.lowered());
    let types: Vec<_> = compiled.lowered().core.types.iter().cloned().collect();
    let mut preparation = compiled.prepare_link();
    preparation
        .tuple_type(vec![TypeId::INT, TypeId::INT64])
        .unwrap();
    let mut helper = FnBuilder::new(&preparation.module().core, "connection_helper");
    helper.term(Term::Return(None));
    let id = preparation.add_function(helper.finish()).unwrap();
    assert_eq!(
        preparation.module().functions[registration].name,
        "registration"
    );
    assert_eq!(
        preparation
            .module()
            .core
            .types
            .iter()
            .take(types.len())
            .cloned()
            .collect::<Vec<_>>(),
        types
    );
    let linked = preparation.finish(vec![id]).unwrap();
    assert!(runtime_reachable_from(&linked.compiled.lowered, &linked.roots).contains(&id));
    assert_eq!(emela_lir::dump(compiled.lowered()), before);
}

#[test]
fn connections_cannot_redefine_existing_functions_or_add_exports_and_consts() {
    let (compiled, registration, _, _, _) = fixture(false);
    let mut preparation = compiled.prepare_link();
    let original = preparation.module().functions[registration].clone();
    assert!(
        preparation
            .define_added_function(registration, original.clone())
            .unwrap_err()[0]
            .message
            .contains("only newly added")
    );
    assert!(
        preparation.add_function(original).unwrap_err()[0]
            .message
            .contains("duplicate")
    );
    for constant in [false, true] {
        let mut function = Function::new("forbidden");
        function.constant = constant;
        function.exported = !constant;
        assert!(preparation.add_function(function).is_err());
    }
}

#[test]
fn unfinished_definitions_invalid_ids_and_bad_call_signatures_cannot_be_emitted() {
    let (compiled, registration, _, _, _) = fixture(false);
    assert!(
        compiled
            .prepare_link()
            .finish(vec![unknown_function()])
            .err()
            .unwrap()[0]
            .message
            .contains("root")
    );
    let mut preparation = compiled.prepare_link();
    preparation
        .add_function(Function::new("unfinished"))
        .unwrap();
    assert!(preparation.finish(vec![]).is_err());
    for callee in [registration, unknown_function()] {
        let mut preparation = compiled.prepare_link();
        let mut caller = FnBuilder::new(&preparation.module().core, "bad_caller");
        caller.let_(
            None,
            Value::Call {
                callee: Callee::Direct(callee),
                args: vec![],
                suspends: false,
                on_fail: None,
            },
        );
        caller.term(Term::Return(None));
        let added = preparation.add_function(caller.finish());
        if callee == registration {
            let id = added.unwrap();
            assert!(preparation.finish(vec![id]).is_err());
        } else {
            assert!(added.unwrap_err()[0].message.contains("arena reference"));
        }
    }
}

#[test]
fn callback_erasure_updates_calls_and_only_removes_its_own_dead_values() {
    let (compiled, registration, entry, callback, unrelated) = fixture(false);
    let before = emela_lir::dump(compiled.lowered());
    let mut preparation = compiled.prepare_link();
    preparation
        .erase_callback_parameter(&[registration], 1)
        .unwrap();
    assert_eq!(preparation.module().functions[registration].params.len(), 1);
    let function = &preparation.module().functions[entry];
    assert_eq!(function.vars[callback].ty, TypeId::INT);
    assert!(matches!(
        preparation.module().core.types[function.vars[unrelated].ty],
        Type::Fn(_)
    ));
    assert!(function.stmts.iter().any(|(_, statement)| matches!(statement,
        Stmt::Let { value: Value::Call { callee: Callee::Direct(id), args, .. }, .. } if *id == registration && args.len() == 1)));
    preparation.finish(vec![]).unwrap();
    assert_eq!(emela_lir::dump(compiled.lowered()), before);
}

#[test]
fn callback_erasure_preserves_implicit_closure_captures() {
    let (compiled, registration, entry, callback, _) = fixture(true);
    assert!(!compiled.closures.environments.is_empty());
    let mut preparation = compiled.prepare_link();
    preparation
        .erase_callback_parameter(&[registration], 1)
        .unwrap();
    assert_eq!(
        preparation.module().functions[entry].vars[callback].ty,
        compiled.lowered().functions[entry].vars[callback].ty
    );
    preparation.finish(vec![]).unwrap();
}

#[test]
fn links_reject_inconsistent_capture_metadata_and_new_closure_dependencies() {
    let (mut compiled, _, entry, callback, _) = fixture(true);
    let lifted = *compiled.closures.functions.keys().next().unwrap();
    let mut preparation = compiled.prepare_link();
    let callback_type = compiled.lowered().functions[entry].vars[callback].ty;
    let mut caller = FnBuilder::new(&preparation.module().core, "bad_capture");
    caller.assign("value", callback_type, Value::FnRef(lifted));
    caller.term(Term::Return(None));
    preparation.add_function(caller.finish()).unwrap();
    assert!(
        preparation.finish(vec![]).err().unwrap()[0]
            .message
            .contains("closure environment")
    );
    compiled.lowered.functions[entry].vars[callback].ty = TypeId::BOOL;
    assert!(
        compiled.prepare_link().finish(vec![]).err().unwrap()[0]
            .message
            .contains("metadata")
    );
}

#[test]
fn roots_include_new_functions_when_replanning_deferred_storage() {
    let compiled = compile_flow(&Module::new(emela_core::Module::new())).unwrap();
    assert!(!compiled.deferred_pool);
    let mut preparation = compiled.prepare_link();
    let mut function = FnBuilder::new(&preparation.module().core, "new_deferred");
    function.set_ret(TypeId::ANY);
    let scope = function.param("scope", TypeId::FIBER_SCOPE);
    let result = function.assign(
        "result",
        TypeId::ANY,
        Value::Fiber {
            op: emela_core::FiberOp::Deferred,
            args: vec![scope.into()],
            on_fail: None,
        },
    );
    function.term(Term::Return(Some(result.into())));
    let id = preparation.add_function(function.finish()).unwrap();
    let linked = preparation.finish(vec![id]).unwrap();
    assert!(linked.compiled.deferred_pool);
    assert!(!compiled.deferred_pool);
}

#[test]
fn runtime_uses_and_escaping_function_values_prevent_callback_abi_erasure() {
    let mut core = emela_core::Module::new();
    let callback_type = core
        .types
        .intern(Type::Fn(FnType::pure(vec![], TypeId::INT)));
    let binding_type = core
        .types
        .intern(Type::Fn(FnType::pure(vec![callback_type], TypeId::UNIT)));
    let mut module = Module::new(core);
    let mut registration = FnBuilder::new(&module.core, "registration");
    let callback = registration.param("callback", callback_type);
    registration.let_(
        None,
        Value::Call {
            callee: Callee::Indirect(callback),
            args: vec![],
            suspends: false,
            on_fail: None,
        },
    );
    registration.term(Term::Return(None));
    let registration = module.add(registration.finish());
    let mut entry = FnBuilder::new(&module.core, "entry");
    entry.set_exported().set_ret(binding_type);
    let reference = entry.assign("binding", binding_type, Value::FnRef(registration));
    entry.term(Term::Return(Some(reference.into())));
    module.add(entry.finish());
    let compiled = compile_flow(&module).unwrap();
    let mut preparation = compiled.prepare_link();
    preparation
        .erase_callback_parameter(&[registration], 0)
        .unwrap();
    assert!(preparation.finish(vec![]).is_err());

    // Even an otherwise unused parameter cannot change a function value's published ABI.
    let mut module = module.clone();
    let function = &mut module.functions[registration];
    function.seqs[function.body.unwrap()].stmts.clear();
    let compiled = compile_flow(&module).unwrap();
    let mut preparation = compiled.prepare_link();
    preparation
        .erase_callback_parameter(&[registration], 0)
        .unwrap();
    assert!(
        preparation.finish(vec![]).err().unwrap()[0]
            .message
            .contains("escapes")
    );
}
