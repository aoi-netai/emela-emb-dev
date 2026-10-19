use emela_lir::{Callee, Const, Function, Module, Operand, Stmt, StmtId, Term, Value, VarId};

pub(crate) fn lower(module: &mut Module) {
    let reachable = crate::emit::runtime_reachable(module);
    for function_id in reachable {
        let function = &mut module.functions[function_id];
        let handlers: Vec<_> = function
            .stmts
            .iter()
            .filter_map(|(statement, value)| {
                let Stmt::Let {
                    dst: Some(handler),
                    value: Value::Handler { effect, ops, .. },
                    ..
                } = value
                else {
                    return None;
                };
                Some((statement, *handler, *effect, ops.clone()))
            })
            .collect();

        for (definition, handler, effect, operations) in handlers {
            let Some(calls) =
                exclusive_operation_calls(function, handler, effect, operations.len())
            else {
                continue;
            };
            for (statement, op) in calls {
                if let Stmt::Let {
                    value: Value::Call { callee, .. },
                    ..
                } = &mut function.stmts[statement]
                {
                    *callee = Callee::Indirect(operations[op]);
                }
            }
            function.stmts[definition] = Stmt::Let {
                dst: None,
                value: Value::Const(Const::Unit),
                on_defect: None,
                on_suspend: None,
            };
        }
    }
}

fn exclusive_operation_calls(
    function: &Function,
    handler: VarId,
    effect: emela_core::EffectId,
    operation_count: usize,
) -> Option<Vec<(StmtId, usize)>> {
    let mut calls = Vec::new();
    for (statement_id, statement) in function.stmts.iter() {
        if let Stmt::Let {
            value:
                Value::Call {
                    callee:
                        Callee::Op {
                            handler: used_handler,
                            effect: used_effect,
                            op,
                        },
                    suspends: false,
                    on_fail: None,
                    ..
                },
            ..
        } = statement
            && *used_handler == handler
            && *used_effect == effect
        {
            if *op >= operation_count {
                return None;
            }
            calls.push((statement_id, *op));
            continue;
        }
        if statement_uses(statement, handler) {
            return None;
        }
    }
    if function
        .seqs
        .iter()
        .any(|(_, sequence)| term_uses(&sequence.term, handler))
    {
        return None;
    }
    Some(calls)
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
