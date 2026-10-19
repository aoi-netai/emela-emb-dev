use std::collections::HashMap;

use emela_lir::{Const, Function, Operand, PrimOp, PrimTy, Stmt, Term, Value, VarId};

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Outcome {
    Value(Const),
    Defect { kind: &'static str, message: String },
}

pub(crate) fn evaluate(function: &Function) -> Result<Outcome, &'static str> {
    if !function.constant {
        return Err("function is not marked constant");
    }
    if !function.params.is_empty() {
        return Err("const function has parameters");
    }
    if function.fails || function.suspends || function.external.is_some() {
        return Err("const function has a failure, suspension, or external body");
    }
    let body = function.body.ok_or("const function has no body")?;
    let sequence = &function.seqs[body];
    let mut values = HashMap::new();

    for &statement_id in &sequence.stmts {
        let Stmt::Let {
            dst,
            value,
            on_defect: None,
            on_suspend: None,
        } = &function.stmts[statement_id]
        else {
            return Err("const body contains unsupported control flow");
        };

        let value = match value {
            Value::Const(value) => value.clone(),
            Value::Prim { op, ty, args } => match evaluate_primitive(
                *op,
                *ty,
                args.iter()
                    .map(|arg| operand_value(arg, &values))
                    .collect::<Result<Vec<_>, _>>()?,
            ) {
                Ok(value) => value,
                Err(message) if message == "division by zero" => {
                    return Ok(with_const_context(
                        function,
                        Outcome::Defect {
                            kind: defect_kind(&message),
                            message,
                        },
                    ));
                }
                Err(_) => return Err("const body contains an unsupported primitive"),
            },
            _ => return Err("const body contains a value that is not statically evaluable"),
        };

        if let Some(dst) = dst {
            values.insert(*dst, value);
        }
    }

    let outcome = match &sequence.term {
        Term::Return(None) => Outcome::Value(Const::Unit),
        Term::Return(Some(operand)) => Outcome::Value(operand_value(operand, &values)?),
        Term::Defect(operand) => {
            let Const::String(message) = operand_value(operand, &values)? else {
                return Err("const defect message is not a String");
            };
            Outcome::Defect {
                kind: defect_kind(&message),
                message,
            }
        }
        _ => return Err("const body has an unsupported terminator"),
    };

    Ok(match outcome {
        Outcome::Defect { .. } => with_const_context(function, outcome),
        value => value,
    })
}

fn with_const_context(function: &Function, outcome: Outcome) -> Outcome {
    match outcome {
        Outcome::Defect { kind, message } => Outcome::Defect {
            kind,
            message: format!("while evaluating const {}: {message}", function.name),
        },
        value => value,
    }
}

fn operand_value(operand: &Operand, values: &HashMap<VarId, Const>) -> Result<Const, &'static str> {
    match operand {
        Operand::Const(value) => Ok(value.clone()),
        Operand::Var(variable) => values
            .get(variable)
            .cloned()
            .ok_or("const body reads a value that is not statically available"),
    }
}

fn evaluate_primitive(op: PrimOp, ty: PrimTy, args: Vec<Const>) -> Result<Const, String> {
    if args.len() != op.arity() {
        return Err("invalid primitive arity".to_owned());
    }
    let defect = || "division by zero".to_owned();

    match ty {
        PrimTy::I32 => {
            let lhs = args.first().and_then(as_i32).ok_or("invalid Int operand")?;
            if op == PrimOp::Not {
                if args.len() != 1 || !matches!(args[0], Const::Bool(_)) {
                    return Err("invalid Bool operand".to_owned());
                }
                return Ok(Const::Bool(lhs == 0));
            }
            if op == PrimOp::Neg {
                return Ok(Const::Int(lhs.wrapping_neg()));
            }
            let rhs = args.get(1).and_then(as_i32).ok_or("invalid Int operand")?;
            match op {
                PrimOp::Add => Ok(Const::Int(lhs.wrapping_add(rhs))),
                PrimOp::Sub => Ok(Const::Int(lhs.wrapping_sub(rhs))),
                PrimOp::Mul => Ok(Const::Int(lhs.wrapping_mul(rhs))),
                PrimOp::Div if rhs == 0 => Err(defect()),
                PrimOp::Div if lhs == i32::MIN && rhs == -1 => Ok(Const::Int(i32::MIN)),
                PrimOp::Div => Ok(Const::Int(lhs / rhs)),
                PrimOp::Rem if rhs == 0 => Err(defect()),
                PrimOp::Rem if lhs == i32::MIN && rhs == -1 => Ok(Const::Int(0)),
                PrimOp::Rem => Ok(Const::Int(lhs % rhs)),
                PrimOp::Eq => Ok(Const::Bool(lhs == rhs)),
                PrimOp::Ne => Ok(Const::Bool(lhs != rhs)),
                PrimOp::Lt => Ok(Const::Bool(lhs < rhs)),
                PrimOp::Le => Ok(Const::Bool(lhs <= rhs)),
                PrimOp::Gt => Ok(Const::Bool(lhs > rhs)),
                PrimOp::Ge => Ok(Const::Bool(lhs >= rhs)),
                PrimOp::Neg | PrimOp::Not => unreachable!(),
            }
        }
        PrimTy::I64 => {
            let lhs = args
                .first()
                .and_then(as_i64)
                .ok_or("invalid Int64 operand")?;
            if op == PrimOp::Neg {
                return Ok(Const::Int64(lhs.wrapping_neg()));
            }
            let rhs = args
                .get(1)
                .and_then(as_i64)
                .ok_or("invalid Int64 operand")?;
            match op {
                PrimOp::Add => Ok(Const::Int64(lhs.wrapping_add(rhs))),
                PrimOp::Sub => Ok(Const::Int64(lhs.wrapping_sub(rhs))),
                PrimOp::Mul => Ok(Const::Int64(lhs.wrapping_mul(rhs))),
                PrimOp::Div if rhs == 0 => Err(defect()),
                PrimOp::Div if lhs == i64::MIN && rhs == -1 => Ok(Const::Int64(i64::MIN)),
                PrimOp::Div => Ok(Const::Int64(lhs / rhs)),
                PrimOp::Rem if rhs == 0 => Err(defect()),
                PrimOp::Rem if lhs == i64::MIN && rhs == -1 => Ok(Const::Int64(0)),
                PrimOp::Rem => Ok(Const::Int64(lhs % rhs)),
                PrimOp::Eq => Ok(Const::Bool(lhs == rhs)),
                PrimOp::Ne => Ok(Const::Bool(lhs != rhs)),
                PrimOp::Lt => Ok(Const::Bool(lhs < rhs)),
                PrimOp::Le => Ok(Const::Bool(lhs <= rhs)),
                PrimOp::Gt => Ok(Const::Bool(lhs > rhs)),
                PrimOp::Ge => Ok(Const::Bool(lhs >= rhs)),
                PrimOp::Neg | PrimOp::Not => unreachable!(),
            }
        }
        PrimTy::F64 => {
            let lhs = args
                .first()
                .and_then(as_f64)
                .ok_or("invalid Float operand")?;
            if op == PrimOp::Neg {
                return Ok(Const::Float(-lhs));
            }
            let rhs = args
                .get(1)
                .and_then(as_f64)
                .ok_or("invalid Float operand")?;
            match op {
                PrimOp::Add => Ok(Const::Float(lhs + rhs)),
                PrimOp::Sub => Ok(Const::Float(lhs - rhs)),
                PrimOp::Mul => Ok(Const::Float(lhs * rhs)),
                PrimOp::Div => Ok(Const::Float(lhs / rhs)),
                PrimOp::Rem => Ok(Const::Float(lhs % rhs)),
                PrimOp::Eq => Ok(Const::Bool(lhs == rhs)),
                PrimOp::Ne => Ok(Const::Bool(lhs != rhs)),
                PrimOp::Lt => Ok(Const::Bool(lhs < rhs)),
                PrimOp::Le => Ok(Const::Bool(lhs <= rhs)),
                PrimOp::Gt => Ok(Const::Bool(lhs > rhs)),
                PrimOp::Ge => Ok(Const::Bool(lhs >= rhs)),
                PrimOp::Neg | PrimOp::Not => unreachable!(),
            }
        }
        PrimTy::String => {
            let [Const::String(lhs), Const::String(rhs)] = args.as_slice() else {
                return Err("invalid String operand".to_owned());
            };
            match op {
                PrimOp::Eq => Ok(Const::Bool(lhs == rhs)),
                PrimOp::Ne => Ok(Const::Bool(lhs != rhs)),
                _ => Err("unsupported String primitive".to_owned()),
            }
        }
    }
}

fn as_i32(value: &Const) -> Option<i32> {
    match value {
        Const::Int(value) => Some(*value),
        Const::Bool(value) => Some(i32::from(*value)),
        _ => None,
    }
}

fn as_i64(value: &Const) -> Option<i64> {
    match value {
        Const::Int64(value) => Some(*value),
        _ => None,
    }
}

fn as_f64(value: &Const) -> Option<f64> {
    match value {
        Const::Float(value) => Some(*value),
        _ => None,
    }
}

fn defect_kind(message: &str) -> &'static str {
    if message == "not yet implemented" {
        "TODO"
    } else if message.starts_with("unreachable: no match arm matched") {
        "NO_MATCH"
    } else if message.starts_with("assertion failed") {
        "ASSERT"
    } else if message == "division by zero" {
        "DIVISION_BY_ZERO"
    } else {
        "PANIC"
    }
}
