//! emela_core::FiberOp::Join
use super::super::*;
pub(in crate::backend) fn emit(
    context: &mut Context<'_, '_>,
    _dst: &Option<VarId>,
    _value: &Value,
) -> Result<(), Vec<Error>> {
    let function = context.function;
    Err(unsupported(
        &function.name,
        "Fiber::Join (phase 7 follow-up)",
    ))
}

pub(in crate::backend) fn emit_suspend(
    context: &mut SuspendContext<'_, '_>,
    statement: &Stmt,
) -> Result<(), Vec<Error>> {
    let layouts = context.layouts;
    let function = context.function;
    let writer = &mut *context.writer;
    let next = &context.next;
    if let Stmt::Let {
        dst,
        value:
            Value::Fiber {
                op:
                    op @ (emela_core::FiberOp::Join
                    | emela_core::FiberOp::Await { .. }
                    | emela_core::FiberOp::Interrupt),
                args,
                on_fail: None,
            },
        ..
    } = statement
    {
        prepare_result(layouts, function, *dst, writer);
        writer.line(&format!("frame->base.pc = {next};"));
        child::emit_wait(layouts, function, *dst, *op, args, writer)?;
        writer.line("return;");
    } else {
        unreachable!("suspend operation entry")
    };
    Ok(())
}
