use std::collections::HashMap;

use emela_core::Builtin;
use emela_lir::{
    Callee, Const, FnId, Function, Module, Operand, PrimOp, PrimTy, Stmt, Term, Value, VarId,
};

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Outcome {
    Value(Const),
    Defect { kind: &'static str, message: String },
    SelfReference { message: String },
}

pub(crate) fn evaluate(module: &Module, function: FnId) -> Result<Outcome, &'static str> {
    match evaluate_function(module, function, Vec::new(), &mut Vec::new())? {
        Outcome::SelfReference { message } => Ok(Outcome::Defect {
            kind: "PANIC",
            message,
        }),
        outcome => Ok(outcome),
    }
}

fn evaluate_function(
    module: &Module,
    id: FnId,
    arguments: Vec<Const>,
    active: &mut Vec<FnId>,
) -> Result<Outcome, &'static str> {
    let function = &module.functions[id];
    if active.contains(&id) {
        if function.constant {
            return Ok(Outcome::SelfReference {
                message: format!(
                    "const {} refers to itself while being evaluated",
                    function.name
                ),
            });
        }
        return Err("const evaluation reached a recursive helper function");
    }
    if arguments.len() != function.params.len() {
        return Err("direct call has an unsupported argument count");
    }
    if function.constant && !arguments.is_empty() {
        return Err("const function has parameters");
    }
    if function.fails || function.suspends || function.external.is_some() {
        return Err("called function has a failure, suspension, or external body");
    }
    let body = function.body.ok_or("called function has no body")?;

    active.push(id);
    let result = evaluate_body(module, function, body, arguments, active);
    active.pop();
    let outcome = result?;

    if function.constant {
        Ok(match outcome {
            Outcome::Defect { kind, message } => Outcome::Defect {
                kind,
                message: format!("while evaluating const {}: {message}", function.name),
            },
            other => other,
        })
    } else {
        Ok(outcome)
    }
}

fn evaluate_body(
    module: &Module,
    function: &Function,
    body: emela_lir::SeqId,
    arguments: Vec<Const>,
    active: &mut Vec<FnId>,
) -> Result<Outcome, &'static str> {
    let sequence = &function.seqs[body];
    let mut values = HashMap::new();
    for (&param, value) in function.params.iter().zip(arguments) {
        values.insert(param, value);
    }

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

        match evaluate_value(module, value, &values, active)? {
            Outcome::Value(value) => {
                if let Some(dst) = dst {
                    values.insert(*dst, value);
                }
            }
            outcome => return Ok(outcome),
        }
    }

    match &sequence.term {
        Term::Return(None) => Ok(Outcome::Value(Const::Unit)),
        Term::Return(Some(operand)) => Ok(Outcome::Value(operand_value(operand, &values)?)),
        Term::Defect(operand) => {
            let Const::String(message) = operand_value(operand, &values)? else {
                return Err("const defect message is not a String");
            };
            Ok(Outcome::Defect {
                kind: defect_kind(&message),
                message,
            })
        }
        _ => Err("const body has an unsupported terminator"),
    }
}

fn evaluate_value(
    module: &Module,
    value: &Value,
    values: &HashMap<VarId, Const>,
    active: &mut Vec<FnId>,
) -> Result<Outcome, &'static str> {
    match value {
        Value::Const(value) => Ok(Outcome::Value(value.clone())),
        Value::Prim { op, ty, args } => {
            let args = args
                .iter()
                .map(|arg| operand_value(arg, values))
                .collect::<Result<Vec<_>, _>>()?;
            match evaluate_primitive(*op, *ty, args) {
                Ok(value) => Ok(Outcome::Value(value)),
                Err(message) if message == "division by zero" => Ok(Outcome::Defect {
                    kind: defect_kind(&message),
                    message,
                }),
                Err(_) => Err("const body contains an unsupported primitive"),
            }
        }
        Value::Call {
            callee: Callee::Direct(id),
            args,
            suspends: false,
            on_fail: None,
        } => {
            let arguments = args
                .iter()
                .map(|arg| operand_value(arg, values))
                .collect::<Result<Vec<_>, _>>()?;
            evaluate_function(module, *id, arguments, active)
        }
        Value::Call { .. } => Err("const body contains an indirect, fallible, or suspending call"),
        Value::Builtin { op, args } => {
            let args = args
                .iter()
                .map(|arg| operand_value(arg, values))
                .collect::<Result<Vec<_>, _>>()?;
            evaluate_builtin(*op, args).map(Outcome::Value)
        }
        _ => Err("const body contains a value that is not statically evaluable"),
    }
}

fn evaluate_builtin(op: Builtin, args: Vec<Const>) -> Result<Const, &'static str> {
    match (op, args.as_slice()) {
        (Builtin::StringByteSize, [Const::String(value)]) => i32::try_from(value.len())
            .map(Const::Int)
            .map_err(|_| "String byte size exceeds Int range"),
        (Builtin::StringContains, [Const::String(value), Const::String(needle)]) => {
            Ok(Const::Bool(value.contains(needle.as_str())))
        }
        (Builtin::StringStartsWith, [Const::String(value), Const::String(prefix)]) => {
            Ok(Const::Bool(value.starts_with(prefix.as_str())))
        }
        (Builtin::StringEndsWith, [Const::String(value), Const::String(suffix)]) => {
            Ok(Const::Bool(value.ends_with(suffix.as_str())))
        }
        _ => Err("const body contains an unsupported builtin or builtin operand"),
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
