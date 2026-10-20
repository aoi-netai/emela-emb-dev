//! Source const origins carried through verified Flow IR as private, unreachable metadata.

use std::collections::{HashMap, HashSet};

use emela_core::{FnType, Type, TypeId};
use emela_lir::{FnBuilder, FnId, Module, Operand, Stmt, Term, Value};

use crate::{Error, ErrorKind};

const PREFIX: &str = "$emb_const_origins$v1$";

#[derive(Debug, Clone)]
pub(crate) struct Group {
    pub(crate) canonical: FnId,
    pub(crate) specializations: Vec<FnId>,
}

pub(crate) struct Prepared {
    pub(crate) module: emela_core::Module,
    pub(crate) groups: Vec<Group>,
}

pub(crate) fn monomorphize(core: &emela_core::Module) -> Prepared {
    let roots: Vec<_> = core
        .functions
        .iter()
        .filter_map(|(id, f)| f.exported.then_some(id))
        .collect();
    let first = emela_core::monomorphize_traced(core, &roots);
    let mut counts = HashMap::new();
    for (_, &origin) in first.origins.iter() {
        let f = &core.functions[origin];
        if f.constant && f.type_params != 0 {
            *counts.entry(origin).or_insert(0) += 1;
        }
    }
    // A single specialization already has one cache and needs no erased representation bridge.
    let mut shared: Vec<_> = counts
        .into_iter()
        .filter_map(|(id, count)| (count > 1).then_some(id))
        .collect();
    shared.sort_by_key(|id| u32::from(id.into_raw()));
    if shared.is_empty() {
        return Prepared {
            module: first.module,
            groups: vec![],
        };
    }
    let all_roots: Vec<_> = roots
        .iter()
        .copied()
        .chain(shared.iter().copied())
        .collect();
    let mono = emela_core::monomorphize_traced(core, &all_roots);
    let groups = shared
        .iter()
        .enumerate()
        .map(|(index, &origin)| {
            let canonical = mono.roots[roots.len() + index];
            let specializations = mono
                .origins
                .iter()
                .filter_map(|(id, &source)| {
                    (source == origin && id != canonical).then_some(FnId::from_raw(id.into_raw()))
                })
                .collect();
            Group {
                canonical: FnId::from_raw(canonical.into_raw()),
                specializations,
            }
        })
        .collect();
    Prepared {
        module: mono.module,
        groups,
    }
}

pub(crate) fn attach(flow: &mut Module, groups: &[Group]) {
    for (index, group) in groups.iter().enumerate() {
        let types: Vec<_> = std::iter::once(group.canonical)
            .chain(group.specializations.iter().copied())
            .map(|id| {
                flow.core
                    .types
                    .intern(Type::Fn(FnType::pure(vec![], flow.functions[id].ret)))
            })
            .collect();
        let mut name = format!("{PREFIX}{index}");
        while flow.functions.iter().any(|(_, f)| f.name == name) {
            name.push('$');
        }
        let mut b = FnBuilder::new(&flow.core, &name);
        b.set_constant();
        for (index, (id, ty)) in std::iter::once(group.canonical)
            .chain(group.specializations.iter().copied())
            .zip(types)
            .enumerate()
        {
            b.assign(
                if index == 0 {
                    "canonical"
                } else {
                    "specialization"
                },
                ty,
                Value::FnRef(id),
            );
        }
        b.term(Term::Return(None));
        flow.add(b.finish());
    }
}

pub(crate) fn groups(flow: &Module) -> Result<Vec<Group>, Vec<Error>> {
    let mut result = Vec::new();
    let mut members = HashSet::new();
    for (_, marker) in flow
        .functions
        .iter()
        .filter(|(_, f)| f.name.starts_with(PREFIX))
    {
        let invalid = || {
            vec![Error {
                kind: ErrorKind::InvalidFlowIr,
                message: format!("invalid embedded const origin marker `{}`", marker.name),
            }]
        };
        if marker.name[PREFIX.len()..]
            .trim_end_matches('$')
            .parse::<usize>()
            .is_err()
            || marker.defects
            || !marker.labels.is_empty()
            || marker.exported
            || !marker.constant
            || !marker.params.is_empty()
            || marker.ret != TypeId::UNIT
            || marker.fails
            || marker.suspends
            || marker.external.is_some()
            || marker.env.is_some()
        {
            return Err(invalid());
        }
        let Some(body) = marker.body else {
            return Err(invalid());
        };
        let seq = &marker.seqs[body];
        if !matches!(
            seq.term,
            Term::Return(None) | Term::Return(Some(Operand::Const(emela_lir::Const::Unit)))
        ) || seq.stmts.len() < 2
            || marker.seqs.len() != 1
            || marker.stmts.len() != seq.stmts.len()
            || marker.vars.len() != seq.stmts.len()
        {
            return Err(invalid());
        }
        let mut ids = Vec::new();
        for &stmt in &seq.stmts {
            let Stmt::Let {
                dst: Some(dst),
                value: Value::FnRef(target),
                on_defect: None,
                on_suspend: None,
            } = marker.stmts[stmt]
            else {
                return Err(invalid());
            };
            if u32::from(target.into_raw()) as usize >= flow.functions.len() {
                return Err(invalid());
            }
            let target_function = &flow.functions[target];
            if !target_function.constant
                || !target_function.params.is_empty()
                || target_function.fails
                || target_function.suspends
                || target_function.external.is_some()
                || !members.insert(target)
            {
                return Err(invalid());
            }
            let Type::Fn(ft) = &flow.core.types[marker.vars[dst].ty] else {
                return Err(invalid());
            };
            if !ft.params.is_empty()
                || ft.ret != target_function.ret
                || ft.fails
                || ft.ev
                || ft.suspends
                || ft.sets.is_some()
            {
                return Err(invalid());
            }
            ids.push(target);
        }
        result.push(Group {
            canonical: ids[0],
            specializations: ids[1..].to_vec(),
        });
    }
    Ok(result)
}
