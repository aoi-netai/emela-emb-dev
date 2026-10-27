//! C spelling conventions, independent of code-generation state.
mod literals;
mod names;

pub(in crate::backend::compiler) use literals::{
    c_string_literal, float_literal, int32_literal, int64_literal,
};
pub(in crate::backend) use names::{enum_index, function_name, is_c_identifier, variable_name};
pub(in crate::backend::compiler) use names::{
    function_coerce_adapter_name, function_ref_adapter_name, function_type_name,
    owned_variable_name, type_name,
};
