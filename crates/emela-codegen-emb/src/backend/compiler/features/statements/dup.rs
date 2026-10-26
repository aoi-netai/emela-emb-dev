//! emela_lir::Stmt::Dup
use super::super::*;
pub(in crate::backend) fn emit(
    context: &mut Context<'_, '_>,
    statement: &Stmt,
) -> Result<(), Vec<Error>> {
    let function = context.function;
    Err(unsupported(&function.name, statement_name(statement)))
}

pub(in crate::backend) fn lower(
    context: &mut LoweringContext<'_>,
    _id: StmtId,
    _statement: &Stmt,
    next: usize,
) -> Result<usize, Vec<Error>> {
    let _graph = &mut *context.graph;
    let function = context.function;
    let _entry = next;
    Err(unsupported(
        &function.name,
        "unsupported Stmt at Flow stage",
    ))
}
