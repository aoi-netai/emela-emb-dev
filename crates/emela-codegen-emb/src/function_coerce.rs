use std::collections::{HashMap, HashSet};

use emela_core::Type;
use emela_lir::{Callee, Function, Module, Operand, Seq, Stmt, Term, Value, VarData};

pub(crate) fn lower(module: &mut Module, closure_functions: &HashSet<emela_lir::FnId>) {
    let reachable = crate::emit::runtime_reachable(module);
    let mut adapters = HashMap::new();
    let mut pending = Vec::new();
    for function_id in &reachable {
        let function = &module.functions[*function_id];
        for (statement, value) in function.stmts.iter() {
            let Stmt::Let {
                dst: Some(dst),
                value: Value::Coerce(source),
                ..
            } = value
            else {
                continue;
            };
            let (Type::Fn(from), Type::Fn(to)) = (
                module.core.types[function.vars[*source].ty].clone(),
                module.core.types[function.vars[*dst].ty].clone(),
            ) else {
                continue;
            };
            if from.fails
                || !to.fails
                || from.suspends
                || to.suspends
                || from.ev
                || to.ev
                || from.sets.is_some()
                || to.sets.is_some()
                || from.params != to.params
                || from.ret != to.ret
            {
                continue;
            }
            let target = function
                .stmts
                .iter()
                .find_map(|(_, statement)| match statement {
                    Stmt::Let {
                        dst: Some(variable),
                        value: Value::FnRef(target),
                        ..
                    } if *variable == *source => Some(*target),
                    _ => None,
                });
            if let Some(target) = target {
                if closure_functions.contains(&target) {
                    continue;
                }
                pending.push((*function_id, statement, target, function.vars[*dst].ty));
            }
        }
    }

    for (caller, statement, target, function_type) in pending {
        let adapter = if let Some(adapter) = adapters.get(&(target, function_type)) {
            *adapter
        } else {
            let target_function = module.functions[target].clone();
            if target_function.fails
                || target_function.suspends
                || target_function.constant
                || target_function.env.is_some()
            {
                continue;
            }
            let name = unique_name(module, &target_function.name, adapters.len());
            let adapter_function = make_adapter(&module.core, target, &target_function, &name);
            let adapter = module.add(adapter_function);
            adapters.insert((target, function_type), adapter);
            adapter
        };
        if let Stmt::Let { value, .. } = &mut module.functions[caller].stmts[statement] {
            *value = Value::FnRef(adapter);
        }
    }
}

fn make_adapter(
    core: &emela_core::Module,
    target_id: emela_lir::FnId,
    target: &Function,
    name: &str,
) -> Function {
    let mut adapter = Function::new(name);
    adapter.ret = target.ret;
    adapter.fails = true;
    adapter.params = target
        .params
        .iter()
        .map(|&parameter| adapter.vars.alloc(target.vars[parameter].clone()))
        .collect();

    let result = (emela_lir::repr_of(core, target.ret) != emela_lir::Repr::Void).then(|| {
        adapter.vars.alloc(VarData {
            hint: "coerced_result".to_owned(),
            repr: emela_lir::repr_of(core, target.ret),
            ty: target.ret,
        })
    });
    let call = adapter.stmts.alloc(Stmt::Let {
        dst: result,
        value: Value::Call {
            callee: Callee::Direct(target_id),
            args: adapter.params.iter().copied().map(Operand::Var).collect(),
            suspends: false,
            on_fail: None,
        },
        on_defect: None,
        on_suspend: None,
    });
    let body = adapter.seqs.alloc(Seq {
        stmts: vec![call],
        term: Term::Return(result.map(Operand::Var)),
    });
    adapter.body = Some(body);
    adapter
}

fn unique_name(module: &Module, target: &str, mut index: usize) -> String {
    loop {
        let name = format!("{target}$fail_adapter_{index}");
        if module
            .functions
            .iter()
            .all(|(_, function)| function.name != name)
        {
            return name;
        }
        index += 1;
    }
}
