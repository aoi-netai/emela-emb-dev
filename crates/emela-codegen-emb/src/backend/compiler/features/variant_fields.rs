use emela_core::{TypeId, VariantDef, VariantFields};

pub(super) fn unit() -> usize {
    0
}
pub(super) fn positional(count: usize) -> usize {
    count
}
pub(super) fn named(names: &[String]) -> usize {
    names.len()
}

pub(super) fn arity(fields: &VariantFields) -> usize {
    match fields {
        VariantFields::Unit => unit(),
        VariantFields::Positional(count) => positional(*count),
        VariantFields::Named(names) => named(names),
    }
}

pub(in crate::backend) fn types(variant: &VariantDef) -> Vec<TypeId> {
    let mut result = Vec::with_capacity(arity(&variant.fields));
    result.extend_from_slice(&variant.tys);
    result
}
