use super::super::*;
pub(in crate::backend) fn string_byte_size(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let op = Builtin::StringByteSize;
    let (dst, expressions) = builtin_utilities::prepare(
        context,
        dst,
        op,
        args,
        1,
        ResultType::Exact(Type::Int),
        |layouts, function, args| {
            args.iter()
                .all(|operand| operand_is_string(layouts, function, operand))
        },
    )?;
    let writer = &mut *context.writer;
    let expression = format!("(int32_t)(({}).length)", expressions[0]);
    writer.assign(&variable_name(dst), &expression);
    Ok(())
}

pub(super) fn string_behavior(args: &[Operand]) -> StringBehavior {
    // Preserve the conservative treatment of empty literal inputs.
    let empty = args
        .iter()
        .take(2)
        .any(|arg| matches!(arg, Operand::Const(Const::String(value)) if value.is_empty()));
    StringBehavior {
        input_use: if empty {
            InputUse::Escapes
        } else {
            InputUse::ReadOnly
        },
        ..StringBehavior::default()
    }
}
