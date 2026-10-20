//! Backend-only preparation for structured fibers; the input stays Flow IR.

use std::collections::{HashMap, HashSet};

use emela_core::{FiberOp, Type, TypeId, TypeTable};
use emela_lir::{Module, Stmt, Term, Value};

use crate::static_closure::ClosureLowering;

pub(crate) fn has_scopes(module: &Module, emitted: &HashSet<emela_lir::FnId>) -> bool {
    module.functions.iter().any(|(id, f)| {
        emitted.contains(&id)
            && f.stmts.iter().any(|(_, s)| {
                matches!(
                    s,
                    Stmt::Let {
                        value: Value::Fiber {
                            op: FiberOp::Scope { .. } | FiberOp::Fork | FiberOp::Deferred,
                            ..
                        },
                        ..
                    }
                )
            })
    })
}

// Generic Scope lowering can conservatively mark wrappers as failing even when E is empty.
// Erase those flags only in a closed, failure-free program, never at an unknown C boundary.
pub(crate) fn erase_inert_failures(module: &mut Module, closures: &mut ClosureLowering) {
    let reachable = crate::emit::runtime_reachable(module);
    if !has_scopes(module, &reachable) {
        return;
    }
    for (id, f) in module.functions.iter() {
        if !reachable.contains(&id) {
            continue;
        }
        if ((f.exported || f.external.is_some()) && f.fails)
            || f.seqs
                .iter()
                .any(|(_, seq)| matches!(seq.term, Term::Fail(_) | Term::Unwind))
            || f.stmts.iter().any(|(_, s)| {
                matches!(
                    s,
                    Stmt::Let {
                        value: Value::Fiber {
                            op: FiberOp::CauseRaise,
                            ..
                        },
                        ..
                    }
                )
            })
        {
            return;
        }
        let boundary_types: Vec<_> = if f.external.is_some() {
            f.params
                .iter()
                .map(|p| f.vars[*p].ty)
                .chain(std::iter::once(f.ret))
                .collect()
        } else if f.exported {
            f.params.iter().map(|p| f.vars[*p].ty).collect()
        } else {
            vec![]
        };
        if boundary_types
            .into_iter()
            .any(|ty| contains_failing_function(&module.core, ty, &mut HashSet::new()))
        {
            return;
        }
    }
    // Definitions may contain function-valued fields, so remap those as well as local types.
    let mut map = HashMap::new();
    let mut types = Vec::new();
    for (_, f) in module.functions.iter() {
        types.push(f.ret);
        types.extend(f.vars.iter().map(|(_, v)| v.ty));
        types.extend(f.env.iter().flatten().copied());
    }
    types.extend(
        module
            .core
            .enums
            .iter()
            .flat_map(|(_, d)| &d.variants)
            .flat_map(|v| &v.tys)
            .copied(),
    );
    for (_, effect) in module.core.effects.iter() {
        for op in &effect.ops {
            types.extend(&op.params);
            types.push(op.ret);
        }
    }
    for ty in types {
        remap(&mut module.core.types, ty, &mut map);
    }
    for (_, f) in module.functions.iter_mut() {
        f.ret = map[&f.ret];
        if let Some(env) = &mut f.env {
            for ty in env {
                *ty = map[ty];
            }
        }
        for (_, var) in f.vars.iter_mut() {
            var.ty = map[&var.ty];
        }
    }
    for (id, f) in module.functions.iter_mut() {
        if !reachable.contains(&id) {
            continue;
        }
        f.fails = false;
        for (_, s) in f.stmts.iter_mut() {
            if let Stmt::Let { value, .. } = s {
                match value {
                    Value::Call { on_fail, .. } => *on_fail = None,
                    Value::Fiber { op, on_fail, .. } => {
                        *on_fail = None;
                        if let FiberOp::Scope { fails } = op {
                            *fails = false;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    for (_, def) in module.core.enums.iter_mut() {
        for variant in &mut def.variants {
            for ty in &mut variant.tys {
                *ty = map[ty];
            }
        }
    }
    for (_, effect) in module.core.effects.iter_mut() {
        for op in &mut effect.ops {
            for ty in &mut op.params {
                *ty = map[ty];
            }
            op.ret = map[&op.ret];
            op.fails = false;
        }
    }
    for env in &mut closures.environments {
        env.function_type = map[&env.function_type];
        for capture in &mut env.captures {
            capture.ty = map[&capture.ty];
        }
    }
}

fn contains_failing_function(
    core: &emela_core::Module,
    ty: TypeId,
    seen: &mut HashSet<TypeId>,
) -> bool {
    if !seen.insert(ty) {
        return false;
    }
    match &core.types[ty] {
        Type::Fn(ft) => {
            ft.fails
                || ft
                    .params
                    .iter()
                    .chain(std::iter::once(&ft.ret))
                    .any(|&t| contains_failing_function(core, t, seen))
        }
        Type::List(t) | Type::Cell(t) => contains_failing_function(core, *t, seen),
        Type::Tuple(ts) => ts.iter().any(|&t| contains_failing_function(core, t, seen)),
        Type::Enum(id, args) => {
            args.iter()
                .any(|&t| contains_failing_function(core, t, seen))
                || core.enums[*id]
                    .variants
                    .iter()
                    .flat_map(|v| &v.tys)
                    .any(|&t| contains_failing_function(core, t, seen))
        }
        Type::Any | Type::Handler(_) | Type::Evidence => true,
        _ => false,
    }
}

fn remap(types: &mut TypeTable, ty: TypeId, map: &mut HashMap<TypeId, TypeId>) -> TypeId {
    if let Some(&mapped) = map.get(&ty) {
        return mapped;
    }
    let mapped = match types[ty].clone() {
        Type::Fn(mut ft) => {
            ft.fails = false;
            ft.params = ft
                .params
                .into_iter()
                .map(|t| remap(types, t, map))
                .collect();
            ft.ret = remap(types, ft.ret, map);
            types.intern(Type::Fn(ft))
        }
        Type::Tuple(ts) => {
            let ts = ts.into_iter().map(|t| remap(types, t, map)).collect();
            types.tuple(ts)
        }
        Type::Enum(id, ts) => {
            let ts = ts.into_iter().map(|t| remap(types, t, map)).collect();
            types.enum_type(id, ts)
        }
        Type::List(t) => {
            let t = remap(types, t, map);
            types.list(t)
        }
        Type::Cell(t) => {
            let t = remap(types, t, map);
            types.cell(t)
        }
        _ => ty,
    };
    map.insert(ty, mapped);
    mapped
}

#[cfg(test)]
mod tests {
    use super::*;
    use emela_core::FnType;
    use emela_lir::{Const, FnBuilder};

    fn program(foreign_callback: bool, actual_failure: bool) -> Module {
        let mut m = Module::new(emela_core::Module::new());
        let ft = m.core.types.intern(Type::Fn(FnType {
            fails: true,
            suspends: true,
            ..FnType::pure(vec![], TypeId::INT)
        }));
        let mut b = FnBuilder::new(&m.core, "entry");
        b.set_exported().set_suspends().set_ret(TypeId::INT);
        if foreign_callback {
            b.param("callback", ft);
        }
        b.let_(
            None,
            Value::Fiber {
                op: FiberOp::Scope { fails: false },
                args: vec![],
                on_fail: None,
            },
        );
        b.term(Term::Return(Some(Const::Int(1).into())));
        m.add(b.finish());
        let mut b = FnBuilder::new(&m.core, "wrapper");
        b.set_exported().set_ret(ft);
        let x = b.param("value", ft);
        b.term(if actual_failure {
            Term::Fail(Const::Int(1).into())
        } else {
            Term::Return(Some(x.into()))
        });
        m.add(b.finish());
        m
    }

    #[test]
    fn unknown_callbacks_and_actual_failure_prevent_erasure() {
        for (callback, failure) in [(true, false), (false, true)] {
            let mut m = program(callback, failure);
            let before = emela_lir::dump(&m);
            erase_inert_failures(&mut m, &mut ClosureLowering::default());
            assert_eq!(before, emela_lir::dump(&m));
        }
    }

    #[test]
    fn closed_failure_free_wrappers_can_erase_conservative_flags() {
        let mut m = program(false, false);
        // Remove the callback input at the public boundary before proving closure.
        for (_, f) in m.functions.iter_mut() {
            f.params.clear();
        }
        erase_inert_failures(&mut m, &mut ClosureLowering::default());
        for (_, f) in m.functions.iter() {
            for (_, v) in f.vars.iter() {
                if let Type::Fn(ft) = &m.core.types[v.ty] {
                    assert!(!ft.fails);
                }
            }
        }
    }
}
