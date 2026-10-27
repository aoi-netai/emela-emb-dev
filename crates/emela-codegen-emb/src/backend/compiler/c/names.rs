//! Stable C identifiers for generated types, functions, and local values.
use emela_core::{EnumId, TypeId};
use emela_lir::{FnId, Function, VarId};

pub(in crate::backend) fn is_c_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first.is_ascii_alphabetic())
        || !chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
    {
        return false;
    }
    !matches!(
        value,
        "auto"
            | "break"
            | "case"
            | "char"
            | "const"
            | "continue"
            | "default"
            | "do"
            | "double"
            | "else"
            | "enum"
            | "extern"
            | "float"
            | "for"
            | "goto"
            | "if"
            | "inline"
            | "int"
            | "long"
            | "register"
            | "restrict"
            | "return"
            | "short"
            | "signed"
            | "sizeof"
            | "static"
            | "struct"
            | "switch"
            | "typedef"
            | "union"
            | "unsigned"
            | "void"
            | "volatile"
            | "while"
            | "_Alignas"
            | "_Alignof"
            | "_Atomic"
            | "_Bool"
            | "_Complex"
            | "_Generic"
            | "_Imaginary"
            | "_Noreturn"
            | "_Static_assert"
            | "_Thread_local"
    )
}

pub(in crate::backend::compiler) fn type_name(ty: TypeId) -> String {
    format!("emela_type_{}", ty.index())
}

pub(in crate::backend::compiler) fn function_type_name(ty: TypeId) -> String {
    format!("emela_function_type_{}", ty.index())
}

pub(in crate::backend) fn function_name(id: FnId, function: &Function) -> String {
    let index = u32::from(id.into_raw());
    if function.exported {
        format!("emela_export_{}_{}", c_identifier(&function.name), index)
    } else {
        format!("emela_fn_{index}")
    }
}

pub(in crate::backend) fn enum_index(id: EnumId) -> u32 {
    u32::from(id.into_raw())
}

pub(in crate::backend::compiler) fn c_identifier(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "function".to_owned()
    } else {
        sanitized
    }
}

pub(in crate::backend) fn variable_name(id: VarId) -> String {
    format!("emela_v_{}", u32::from(id.into_raw()))
}

pub(in crate::backend::compiler) fn owned_variable_name(id: VarId) -> String {
    format!("emela_owned_v_{}", u32::from(id.into_raw()))
}

pub(in crate::backend::compiler) fn function_ref_adapter_name(
    target: FnId,
    function_type: TypeId,
) -> String {
    format!(
        "emela_fn_ref_adapter_{}_{}",
        u32::from(target.into_raw()),
        function_type.index()
    )
}

pub(in crate::backend::compiler) fn function_coerce_adapter_name(index: usize) -> String {
    format!("emela_fn_coerce_adapter_{index}")
}
