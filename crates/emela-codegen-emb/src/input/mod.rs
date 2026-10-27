//! Upstream Core/Flow adaptation and validation.

use crate::{Error, ErrorKind, backend, const_origins, derived};

/// Monomorphizes Core IR, lowers it to Flow IR, and verifies the Flow-stage invariants.
///
/// The backend normalizes the conservative failure edge of an infallible CauseRaise before
/// verifying Flow IR. This path does not call `emela_lir::to_heap`.
pub fn flow_from_core(core: &emela_core::Module) -> Result<emela_lir::Module, Vec<Error>> {
    derived::check_core_input(core)?;
    let prepared_consts = const_origins::monomorphize(core);
    let mono = prepared_consts.module;
    if let Err(errors) = emela_core::verify_monomorphic(&mono) {
        return Err(errors
            .into_iter()
            .map(|error| Error {
                kind: ErrorKind::InvalidCore,
                message: format!("invalid Core IR after monomorphization: {error}"),
            })
            .collect());
    }

    derived::check_core_input(&mono)?;
    let mut flow = emela_lir::from_core(mono).map_err(|errors| {
        errors
            .into_iter()
            .map(|error| Error {
                kind: ErrorKind::FlowLowering,
                message: format!("failed to lower Core IR to Flow IR: {error}"),
            })
            .collect::<Vec<_>>()
    })?;

    const_origins::attach(&mut flow, &prepared_consts.groups);
    const_origins::attach_function_groups(&mut flow, &prepared_consts.function_groups);
    backend::compiler::normalize_infallible_cause_raise(&mut flow);
    verify_flow(&flow)?;
    Ok(flow)
}

/// Verifies that a LIR module is still in the Flow stage and satisfies its invariants.
pub fn verify_flow(flow: &emela_lir::Module) -> Result<(), Vec<Error>> {
    if flow.stage != emela_lir::Stage::Flow {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "expected Flow IR, found {:?}; embedded codegen does not run Heap IR passes",
                flow.stage
            ),
        }]);
    }

    emela_lir::verify(flow).map_err(|errors| {
        errors
            .into_iter()
            .map(|error| Error {
                kind: ErrorKind::InvalidFlowIr,
                message: format!("invalid Flow IR: {error}"),
            })
            .collect::<Vec<_>>()
    })?;

    // The shared verifier checks representations but does not reject every operation that
    // belongs to a later stage or that from_core must already have lowered into Flow control.
    let mut errors = Vec::new();
    for (_, function) in flow.functions.iter() {
        for (_, statement) in function.stmts.iter() {
            let emela_lir::Stmt::Let { value, .. } = statement else {
                continue;
            };
            let reason = match value {
                emela_lir::Value::Runtime {
                    op:
                        op @ (emela_lir::RuntimeOp::Defect
                        | emela_lir::RuntimeOp::DefectTake
                        | emela_lir::RuntimeOp::DefectResume
                        | emela_lir::RuntimeOp::Yield
                        | emela_lir::RuntimeOp::YieldPerform),
                    ..
                } => Some(format!("Heap IR runtime `{}` at stage Flow", op.name())),
                emela_lir::Value::Builtin {
                    op:
                        op @ (emela_core::Builtin::Panic
                        | emela_core::Builtin::Todo
                        | emela_core::Builtin::Dbg
                        | emela_core::Builtin::Inspect
                        | emela_core::Builtin::ShowNested
                        | emela_core::Builtin::ShowString
                        | emela_core::Builtin::DebugPrint),
                    ..
                } => Some(format!(
                    "unlowered builtin `{}` at stage Flow",
                    op.info().path()
                )),
                _ => None,
            };
            if let Some(reason) = reason {
                errors.push(Error {
                    kind: ErrorKind::InvalidFlowIr,
                    message: format!("invalid Flow IR in function `{}`: {reason}", function.name),
                });
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}
