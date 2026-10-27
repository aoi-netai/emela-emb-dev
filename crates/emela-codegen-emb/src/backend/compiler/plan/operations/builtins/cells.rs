use super::super::*;

pub(in crate::backend::compiler::plan) fn needs_heap(op: Builtin) -> bool {
    matches!(op, Builtin::CellNew)
}
