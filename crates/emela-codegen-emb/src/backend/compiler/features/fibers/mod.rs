use super::*;
pub(in crate::backend) mod attempt;
pub(in crate::backend) mod r#await;
pub(in crate::backend) mod cause_kind;
pub(in crate::backend) mod cause_raise;
pub(in crate::backend) mod cause_to_string;
pub(in crate::backend) mod complete;
pub(in crate::backend) mod deferred;
pub(in crate::backend) mod deferred_await;
pub(in crate::backend) mod fork;
pub(in crate::backend) mod interrupt;
pub(in crate::backend) mod join;
pub(in crate::backend) mod poll;
pub(in crate::backend) mod scope;
pub(in crate::backend) fn emit(
    context: &mut Context<'_, '_>,
    dst: &Option<VarId>,
    value: &Value,
) -> Result<(), Vec<Error>> {
    let Value::Fiber { op, .. } = value else {
        unreachable!("Fiber dispatch")
    };
    match op {
        emela_core::FiberOp::Scope { .. } => scope::emit(context, dst, value),
        emela_core::FiberOp::Fork => fork::emit(context, dst, value),
        emela_core::FiberOp::Deferred => deferred::emit(context, dst, value),
        emela_core::FiberOp::Join => join::emit(context, dst, value),
        emela_core::FiberOp::Await { .. } => r#await::emit(context, dst, value),
        emela_core::FiberOp::Interrupt => interrupt::emit(context, dst, value),
        emela_core::FiberOp::Attempt { .. } => attempt::emit(context, dst, value),
        emela_core::FiberOp::Complete => complete::emit(context, dst, value),
        emela_core::FiberOp::DeferredAwait => deferred_await::emit(context, dst, value),
        emela_core::FiberOp::Poll => poll::emit(context, dst, value),
        emela_core::FiberOp::CauseKind { .. } => cause_kind::emit(context, dst, value),
        emela_core::FiberOp::CauseToString => cause_to_string::emit(context, dst, value),
        emela_core::FiberOp::CauseRaise => cause_raise::emit(context, dst, value),
    }
}

pub(in crate::backend) fn emit_waiting(
    context: &mut SuspendContext<'_, '_>,
    statement: &Stmt,
    op: emela_core::FiberOp,
) -> Result<(), Vec<Error>> {
    match op {
        emela_core::FiberOp::Join => join::emit_suspend(context, statement),
        emela_core::FiberOp::Await { .. } => r#await::emit_suspend(context, statement),
        emela_core::FiberOp::Interrupt => interrupt::emit_suspend(context, statement),
        _ => unreachable!("waiting Fiber selector"),
    }
}

pub(super) fn string_behavior(op: emela_core::FiberOp) -> StringBehavior {
    match op {
        emela_core::FiberOp::CauseToString => cause_to_string::string_behavior(),
        emela_core::FiberOp::Scope { .. }
        | emela_core::FiberOp::Fork
        | emela_core::FiberOp::Deferred
        | emela_core::FiberOp::Join
        | emela_core::FiberOp::Await { .. }
        | emela_core::FiberOp::Interrupt
        | emela_core::FiberOp::Attempt { .. }
        | emela_core::FiberOp::Complete
        | emela_core::FiberOp::DeferredAwait
        | emela_core::FiberOp::Poll
        | emela_core::FiberOp::CauseKind { .. }
        | emela_core::FiberOp::CauseRaise => StringBehavior::default(),
    }
}
