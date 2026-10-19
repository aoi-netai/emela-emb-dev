use std::collections::{HashMap, HashSet};

use emela_core::EffectId;
use emela_lir::{Callee, Const, Function, Module, Operand, RuntimeOp, Stmt, Term, Value, VarId};

pub(crate) fn lower(module: &mut Module) {
    let reachable = crate::emit::runtime_reachable(module);
    for function_id in reachable {
        let function = &mut module.functions[function_id];
        let handlers: HashMap<_, _> = function
            .stmts
            .iter()
            .filter_map(|(_, statement)| {
                let Stmt::Let {
                    dst: Some(handler),
                    value: Value::Handler { effect, ops, .. },
                    ..
                } = statement
                else {
                    return None;
                };
                Some((*handler, (*effect, ops.clone())))
            })
            .collect();

        let call_updates: Vec<_> = function
            .stmts
            .iter()
            .filter_map(|(statement, value)| {
                let Stmt::Let {
                    value:
                        Value::Call {
                            callee:
                                Callee::Op {
                                    handler,
                                    effect,
                                    op,
                                },
                            suspends: false,
                            on_fail: None,
                            ..
                        },
                    ..
                } = value
                else {
                    return None;
                };
                let (static_handler, operations) =
                    resolve_handler(function, &handlers, *handler, *effect, &mut HashSet::new())?;
                if handler_is_retired(function, static_handler) {
                    return None;
                }
                operations
                    .get(*op)
                    .copied()
                    .map(|operation| (statement, operation))
            })
            .collect();
        for (statement, operation) in call_updates {
            if let Stmt::Let {
                value: Value::Call { callee, .. },
                ..
            } = &mut function.stmts[statement]
            {
                *callee = Callee::Indirect(operation);
            }
        }

        erase_unused_static_values(function);
    }
}

fn resolve_handler(
    function: &Function,
    handlers: &HashMap<VarId, (EffectId, Vec<VarId>)>,
    value_id: VarId,
    effect: EffectId,
    seen: &mut HashSet<VarId>,
) -> Option<(VarId, Vec<VarId>)> {
    if !seen.insert(value_id) {
        return None;
    }
    if let Some((handler_effect, operations)) = handlers.get(&value_id)
        && *handler_effect == effect
    {
        return Some((value_id, operations.clone()));
    }
    let definition = function
        .stmts
        .iter()
        .find_map(|(_, statement)| match statement {
            Stmt::Let {
                dst: Some(dst),
                value,
                ..
            } if *dst == value_id => Some(value),
            _ => None,
        })?;
    let Value::EvLookup {
        ev,
        effect: lookup_effect,
    } = definition
    else {
        return None;
    };
    if *lookup_effect != effect {
        return None;
    }
    resolve_evidence(function, handlers, *ev, effect, seen)
}

fn resolve_evidence(
    function: &Function,
    handlers: &HashMap<VarId, (EffectId, Vec<VarId>)>,
    evidence: VarId,
    effect: EffectId,
    seen: &mut HashSet<VarId>,
) -> Option<(VarId, Vec<VarId>)> {
    if !seen.insert(evidence) {
        return None;
    }
    let definition = function
        .stmts
        .iter()
        .find_map(|(_, statement)| match statement {
            Stmt::Let {
                dst: Some(dst),
                value: Value::EvExtend { base, bindings },
                ..
            } if *dst == evidence => Some((base, bindings)),
            _ => None,
        })?;
    let (base, bindings) = definition;
    if let Some((_, handler)) = bindings
        .iter()
        .rev()
        .find(|(bound_effect, _)| *bound_effect == effect)
    {
        return resolve_handler(function, handlers, *handler, effect, seen);
    }
    resolve_evidence(function, handlers, (*base)?, effect, seen)
}

fn handler_is_retired(function: &Function, handler: VarId) -> bool {
    function.stmts.iter().any(|(_, statement)| {
        matches!(statement, Stmt::Let { value: Value::Runtime { op: RuntimeOp::Retire, args }, .. }
            if matches!(args.first(), Some(Operand::Var(variable)) if *variable == handler))
    })
}

fn erase_unused_static_values(function: &mut Function) {
    loop {
        let unused: Vec<_> = function
            .stmts
            .iter()
            .filter_map(|(statement_id, statement)| {
                let Stmt::Let {
                    dst: Some(dst),
                    value: Value::Handler { .. } | Value::EvLookup { .. } | Value::EvExtend { .. },
                    ..
                } = statement
                else {
                    return None;
                };
                (!function_uses(function, *dst)).then_some(statement_id)
            })
            .collect();
        if unused.is_empty() {
            break;
        }
        for statement in unused {
            function.stmts[statement] = Stmt::Let {
                dst: None,
                value: Value::Const(Const::Unit),
                on_defect: None,
                on_suspend: None,
            };
        }
    }
}

fn function_uses(function: &Function, needle: VarId) -> bool {
    function
        .stmts
        .iter()
        .any(|(_, statement)| statement_uses(statement, needle))
        || function
            .seqs
            .iter()
            .any(|(_, sequence)| term_uses(&sequence.term, needle))
}

fn statement_uses(statement: &Stmt, needle: VarId) -> bool {
    match statement {
        Stmt::Let {
            value, on_suspend, ..
        } => {
            let mut found = false;
            value.for_each_var(|variable| found |= variable == needle);
            found
                || on_suspend
                    .as_ref()
                    .is_some_and(|suspend| suspend.saved.contains(&needle))
        }
        Stmt::If { cond, .. } => operand_uses(cond, needle),
        Stmt::Switch { on, .. } => match on {
            emela_lir::SwitchOn::Tag(variable) | emela_lir::SwitchOn::Lit(variable) => {
                *variable == needle
            }
        },
        Stmt::Loop { inits, .. } => inits.iter().any(|operand| operand_uses(operand, needle)),
        Stmt::Block { .. } | Stmt::Protect { .. } => false,
        Stmt::Dup(variable) | Stmt::Drop(variable) => *variable == needle,
    }
}

fn term_uses(term: &Term, needle: VarId) -> bool {
    match term {
        Term::Yield(value) | Term::Return(value) => value
            .as_ref()
            .is_some_and(|value| operand_uses(value, needle)),
        Term::Fail(value) | Term::Defect(value) => operand_uses(value, needle),
        Term::Break { value, .. } => value
            .as_ref()
            .is_some_and(|value| operand_uses(value, needle)),
        Term::Recur(values) => values.iter().any(|value| operand_uses(value, needle)),
        Term::Unwind | Term::Unreachable => false,
    }
}

fn operand_uses(operand: &Operand, needle: VarId) -> bool {
    matches!(operand, Operand::Var(variable) if *variable == needle)
}
