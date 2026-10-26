//! emela_lir::Term::Unwind
use super::super::*;
pub(in crate::backend) fn emit(
    context: &mut Context<'_, '_>,
    term: &Term,
    _dst: Option<VarId>,
) -> Result<(), Vec<Error>> {
    let function = context.function;
    Err(unsupported(&function.name, term_name(term)))
}

pub(in crate::backend) fn lower(
    context: &mut LoweringContext<'_>,
    _term: &Term,
    _dst: Option<VarId>,
    next: usize,
) -> Result<usize, Vec<Error>> {
    let _graph = &mut *context.graph;
    let function = context.function;
    let _entry = next;
    Err(unsupported(
        &function.name,
        "unsupported Term at Flow stage",
    ))
}

pub(in crate::backend) fn emit_function(
    _layouts: &TypeLayouts<'_>,
    function: &Function,
    term: &Term,
    _string_buffers: &StringBufferPlan,
    _writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    Err(unsupported(&function.name, term_name(term)))
}
