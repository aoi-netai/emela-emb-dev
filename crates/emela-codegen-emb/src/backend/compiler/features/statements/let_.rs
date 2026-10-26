//! emela_lir::Stmt::Let
use super::super::*;
pub(in crate::backend) fn emit(
    context: &mut Context<'_, '_>,
    statement: &Stmt,
) -> Result<(), Vec<Error>> {
    let function = context.function;
    match statement {
        Stmt::Let {
            dst,
            value,
            on_defect,
            on_suspend,
        } => {
            if on_defect.is_some() || on_suspend.is_some() {
                return Err(vec![Error {
                    kind: ErrorKind::InvalidFlowIr,
                    message: format!(
                        "invalid Flow IR in function `{}`: Flow-stage Let has a landing edge",
                        function.name
                    ),
                }]);
            }
            values::emit(context, dst, value)?;
            Ok(())
        }
        other => Err(unsupported(&function.name, statement_name(other))),
    }
}

pub(in crate::backend) fn lower(
    context: &mut LoweringContext<'_>,
    id: StmtId,
    statement: &Stmt,
    next: usize,
) -> Result<usize, Vec<Error>> {
    let Stmt::Let { value, .. } = statement else {
        unreachable!("Let entry")
    };
    match value {
        Value::Call { .. } => values::call::lower(context, id, value, next),
        Value::Fiber { .. } => values::fiber::lower(context, id, value, next),
        _ => Ok(context.graph.add(Node::Statement(id, next))),
    }
}
