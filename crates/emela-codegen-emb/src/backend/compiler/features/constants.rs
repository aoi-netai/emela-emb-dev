use super::*;
pub(in crate::backend) fn int_value(
    value: &emela_lir::Const,
) -> Result<Option<String>, &'static str> {
    let emela_lir::Const::Int(value) = value else {
        unreachable!("variant dispatch")
    };
    Ok(Some(int32_literal(*value)))
}
pub(in crate::backend) fn int64_value(
    value: &emela_lir::Const,
) -> Result<Option<String>, &'static str> {
    let emela_lir::Const::Int64(value) = value else {
        unreachable!("variant dispatch")
    };
    Ok(Some(int64_literal(*value)))
}
pub(in crate::backend) fn float_value(
    value: &emela_lir::Const,
) -> Result<Option<String>, &'static str> {
    let emela_lir::Const::Float(value) = value else {
        unreachable!("variant dispatch")
    };
    Ok(Some(float_literal(*value)))
}
pub(in crate::backend) fn bool_value(
    value: &emela_lir::Const,
) -> Result<Option<String>, &'static str> {
    let emela_lir::Const::Bool(value) = value else {
        unreachable!("variant dispatch")
    };
    Ok(Some(if *value {
        "INT32_C(1)".to_owned()
    } else {
        "INT32_C(0)".to_owned()
    }))
}
pub(in crate::backend) fn string_value(
    value: &emela_lir::Const,
) -> Result<Option<String>, &'static str> {
    let emela_lir::Const::String(value) = value else {
        unreachable!("variant dispatch")
    };
    {
        let length = u16::try_from(value.len()).map_err(|_| "String constant is too long")?;
        Ok(Some(format!(
            "(EmelaString){{ .data = {}, .length = UINT16_C({length}) }}",
            c_string_literal(value.as_bytes())
        )))
    }
}
pub(in crate::backend) fn unit_value(
    value: &emela_lir::Const,
) -> Result<Option<String>, &'static str> {
    let emela_lir::Const::Unit = value else {
        unreachable!("variant dispatch")
    };
    Ok(None)
}
pub(in crate::backend) fn dispatch(
    value: &emela_lir::Const,
) -> Result<Option<String>, &'static str> {
    match value {
        Const::Int(..) => int_value(value),
        Const::Int64(..) => int64_value(value),
        Const::Float(..) => float_value(value),
        Const::Bool(..) => bool_value(value),
        Const::String(..) => string_value(value),
        Const::Unit => unit_value(value),
    }
}
