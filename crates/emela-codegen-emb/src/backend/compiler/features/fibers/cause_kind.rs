//! emela_core::FiberOp::CauseKind
use super::super::*;
pub(in crate::backend) fn emit(
    context: &mut Context<'_, '_>,
    dst: &Option<VarId>,
    value: &Value,
) -> Result<(), Vec<Error>> {
    let function = context.function;
    let writer = &mut *context.writer;
    let Value::Fiber { args, .. } = value else {
        unreachable!("Fiber entry")
    };
    let dst = *dst;
    cause::emit_kind(function, dst, args, writer)
}
