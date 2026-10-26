//! Core type representation decisions; allocation and ABI planning share TypeLayouts.
use super::*;
pub(in crate::backend) fn int_repr(_layouts: &TypeLayouts<'_>, _ty: &Type) -> Repr {
    Repr::I32
}
pub(in crate::backend) fn int64_repr(_layouts: &TypeLayouts<'_>, _ty: &Type) -> Repr {
    Repr::I64
}
pub(in crate::backend) fn float_repr(_layouts: &TypeLayouts<'_>, _ty: &Type) -> Repr {
    Repr::F64
}
pub(in crate::backend) fn bool_repr(_layouts: &TypeLayouts<'_>, _ty: &Type) -> Repr {
    Repr::I32
}
pub(in crate::backend) fn string_repr(_layouts: &TypeLayouts<'_>, _ty: &Type) -> Repr {
    Repr::Ref
}
pub(in crate::backend) fn unit_repr(_layouts: &TypeLayouts<'_>, _ty: &Type) -> Repr {
    Repr::Void
}
pub(in crate::backend) fn never_repr(_layouts: &TypeLayouts<'_>, _ty: &Type) -> Repr {
    Repr::Void
}
pub(in crate::backend) fn tuple_repr(_layouts: &TypeLayouts<'_>, _ty: &Type) -> Repr {
    Repr::Ref
}
pub(in crate::backend) fn list_repr(_layouts: &TypeLayouts<'_>, _ty: &Type) -> Repr {
    Repr::Ref
}
pub(in crate::backend) fn enum_repr(layouts: &TypeLayouts<'_>, ty: &Type) -> Repr {
    let Type::Enum(id, ..) = ty else {
        unreachable!("enum representation")
    };
    if layouts.enum_is_i32(*id) {
        Repr::I32
    } else {
        Repr::Ref
    }
}
pub(in crate::backend) fn param_repr(_layouts: &TypeLayouts<'_>, _ty: &Type) -> Repr {
    Repr::Ref
}
pub(in crate::backend) fn fn_repr(_layouts: &TypeLayouts<'_>, _ty: &Type) -> Repr {
    Repr::Ref
}
pub(in crate::backend) fn handler_repr(_layouts: &TypeLayouts<'_>, _ty: &Type) -> Repr {
    Repr::Ref
}
pub(in crate::backend) fn evidence_repr(_layouts: &TypeLayouts<'_>, _ty: &Type) -> Repr {
    Repr::Ref
}
pub(in crate::backend) fn cell_repr(_layouts: &TypeLayouts<'_>, _ty: &Type) -> Repr {
    Repr::Ref
}
pub(in crate::backend) fn fiber_scope_repr(_layouts: &TypeLayouts<'_>, _ty: &Type) -> Repr {
    Repr::Ref
}
pub(in crate::backend) fn any_repr(_layouts: &TypeLayouts<'_>, _ty: &Type) -> Repr {
    Repr::Ref
}
pub(in crate::backend) fn set_repr(_layouts: &TypeLayouts<'_>, _ty: &Type) -> Repr {
    Repr::Void
}
pub(in crate::backend) fn repr(layouts: &TypeLayouts<'_>, ty: &Type) -> Repr {
    match ty {
        Type::Int => int_repr(layouts, ty),
        Type::Int64 => int64_repr(layouts, ty),
        Type::Float => float_repr(layouts, ty),
        Type::Bool => bool_repr(layouts, ty),
        Type::String => string_repr(layouts, ty),
        Type::Unit => unit_repr(layouts, ty),
        Type::Never => never_repr(layouts, ty),
        Type::Tuple(..) => tuple_repr(layouts, ty),
        Type::List(..) => list_repr(layouts, ty),
        Type::Enum(..) => enum_repr(layouts, ty),
        Type::Param(..) => param_repr(layouts, ty),
        Type::Fn(..) => fn_repr(layouts, ty),
        Type::Handler(..) => handler_repr(layouts, ty),
        Type::Evidence => evidence_repr(layouts, ty),
        Type::Cell(..) => cell_repr(layouts, ty),
        Type::FiberScope => fiber_scope_repr(layouts, ty),
        Type::Any => any_repr(layouts, ty),
        Type::Set(..) => set_repr(layouts, ty),
    }
}
