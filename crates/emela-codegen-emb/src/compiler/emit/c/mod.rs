//! C spelling conventions, independent of code-generation state.
mod literals;

pub(in crate::compiler::emit) use crate::semantics::c::{enum_index, variable_name};
pub(in crate::compiler::emit) use crate::semantics::c::{
    function_coerce_adapter_name, function_ref_adapter_name, function_type_name,
    owned_variable_name, type_name,
};
pub(in crate::compiler::emit) use literals::{
    c_string_literal, float_literal, int32_literal, int64_literal,
};
