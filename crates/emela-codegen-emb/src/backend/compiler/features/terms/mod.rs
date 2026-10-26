use super::*;
pub(in crate::backend) mod r#break;
pub(in crate::backend) mod defect;
pub(in crate::backend) mod fail;
pub(in crate::backend) mod recur;
pub(in crate::backend) mod return_;
pub(in crate::backend) mod unreachable;
pub(in crate::backend) mod unwind;
pub(in crate::backend) mod r#yield;
pub(in crate::backend) fn emit(
    context: &mut Context<'_, '_>,
    term: &Term,
    dst: Option<VarId>,
) -> Result<(), Vec<Error>> {
    match term {
        Term::Yield(..) => r#yield::emit(context, term, dst),
        Term::Return(..) => return_::emit(context, term, dst),
        Term::Fail(..) => fail::emit(context, term, dst),
        Term::Break { .. } => r#break::emit(context, term, dst),
        Term::Recur(..) => recur::emit(context, term, dst),
        Term::Defect(..) => defect::emit(context, term, dst),
        Term::Unwind => unwind::emit(context, term, dst),
        Term::Unreachable => unreachable::emit(context, term, dst),
    }
}

pub(in crate::backend) fn lower(
    context: &mut LoweringContext<'_>,
    term: &Term,
    dst: Option<VarId>,
    next: usize,
) -> Result<usize, Vec<Error>> {
    match term {
        Term::Yield(..) => r#yield::lower(context, term, dst, next),
        Term::Return(..) => return_::lower(context, term, dst, next),
        Term::Fail(..) => fail::lower(context, term, dst, next),
        Term::Break { .. } => r#break::lower(context, term, dst, next),
        Term::Recur(..) => recur::lower(context, term, dst, next),
        Term::Defect(..) => defect::lower(context, term, dst, next),
        Term::Unwind => unwind::lower(context, term, dst, next),
        Term::Unreachable => unreachable::lower(context, term, dst, next),
    }
}

pub(in crate::backend) fn emit_function(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    term: &Term,
    string_buffers: &StringBufferPlan,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    match term {
        Term::Yield(..) => r#yield::emit_function(layouts, function, term, string_buffers, writer),
        Term::Return(..) => return_::emit_function(layouts, function, term, string_buffers, writer),
        Term::Fail(..) => fail::emit_function(layouts, function, term, string_buffers, writer),
        Term::Break { .. } => {
            r#break::emit_function(layouts, function, term, string_buffers, writer)
        }
        Term::Recur(..) => recur::emit_function(layouts, function, term, string_buffers, writer),
        Term::Defect(..) => defect::emit_function(layouts, function, term, string_buffers, writer),
        Term::Unwind => unwind::emit_function(layouts, function, term, string_buffers, writer),
        Term::Unreachable => {
            unreachable::emit_function(layouts, function, term, string_buffers, writer)
        }
    }
}

pub(in crate::backend) fn require_runtime(
    flow: &emela_lir::Module,
    function: &Function,
    term: &Term,
    requirements: &mut runtime_plan::Requirements,
) {
    match term {
        Term::Defect(value) => defect::require_runtime(flow, function, value, requirements),
        Term::Unreachable => unreachable::require_runtime(requirements),
        Term::Yield(_)
        | Term::Return(_)
        | Term::Fail(_)
        | Term::Break { .. }
        | Term::Recur(_)
        | Term::Unwind => {}
    }
}
