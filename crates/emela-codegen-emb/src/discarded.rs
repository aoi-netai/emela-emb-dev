//! Materialize discarded runtime results without retaining them until function return.
use emela_core::{Builtin, Sig, Type, TypeId};
use emela_lir::{FnId, Module, RuntimeOp, Stmt, Value, VarData, VarId};
use std::collections::{HashMap, HashSet};

// Function names need not be unique in a caller-supplied verified Flow module.
pub(crate) type Results = HashMap<FnId, HashSet<VarId>>;

pub(crate) fn normalize(module: &mut Module) -> Results {
    let mut discarded = Results::new();
    for (function_id, function) in module.functions.iter_mut() {
        for (_, statement) in function.stmts.iter_mut() {
            let Stmt::Let {
                dst: None, value, ..
            } = statement
            else {
                continue;
            };
            let Some(ty) = result_type(&mut module.core, value) else {
                continue;
            };
            let repr = match module.core.types[ty] {
                Type::Int | Type::Bool => emela_lir::Repr::I32,
                Type::Int64 => emela_lir::Repr::I64,
                Type::Float => emela_lir::Repr::F64,
                Type::Unit | Type::Never => emela_lir::Repr::Void,
                _ => emela_lir::Repr::Ref,
            };
            if repr == emela_lir::Repr::Void {
                continue;
            }
            let variable = function.vars.alloc(VarData {
                hint: "discarded_result".into(),
                ty,
                repr,
            });
            let Stmt::Let { dst, .. } = statement else {
                unreachable!()
            };
            *dst = Some(variable);
            discarded.entry(function_id).or_default().insert(variable);
        }
    }
    discarded
}

fn result_type(core: &mut emela_core::Module, value: &Value) -> Option<TypeId> {
    match value {
        Value::Builtin {
            op:
                Builtin::CellNew
                | Builtin::CellGet
                | Builtin::CellSet
                | Builtin::Panic
                | Builtin::Todo
                | Builtin::Dbg,
            ..
        } => None,
        Value::Builtin { op, .. } => signature_type(core, op.info().ret),
        Value::Runtime {
            op: RuntimeOp::ShowString,
            ..
        } => Some(TypeId::STRING),
        _ => None,
    }
}

fn signature_type(core: &mut emela_core::Module, sig: Sig) -> Option<TypeId> {
    Some(match sig {
        Sig::Int => TypeId::INT,
        Sig::Int64 => TypeId::INT64,
        Sig::Float => TypeId::FLOAT,
        Sig::Bool => TypeId::BOOL,
        Sig::String => TypeId::STRING,
        Sig::Unit => TypeId::UNIT,
        Sig::Never => TypeId::NEVER,
        Sig::List(element) => {
            let element = signature_type(core, *element)?;
            core.types.intern(Type::List(element))
        }
        Sig::Option(payload) => {
            let payload = signature_type(core, *payload)?;
            let option = core.option_enum();
            core.enum_type(option, vec![payload])
        }
        Sig::Param(_) => return None,
    })
}
