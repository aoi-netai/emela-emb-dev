use super::super::*;
pub(in crate::backend) fn int_to_string(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let op = Builtin::IntToString;
    let (dst, expressions) = builtin_utilities::prepare(
        context,
        dst,
        op,
        args,
        1,
        ResultType::Exact(Type::String),
        |layouts, function, args| {
            args.iter()
                .all(|operand| operand_is_int(layouts, function, operand))
        },
    )?;
    let function = context.function;
    let writer = &mut *context.writer;
    let string_buffers = context.string_buffers;
    let expression = match &args[0] {
        Operand::Const(Const::Int(value)) => constant_expression(&Const::String(value.to_string()))
            .map_err(|variant| unsupported(&function.name, variant))?
            .expect("String constant has an expression"),
        _ => {
            let Some((buffer, capacity)) = string_buffers.buffer(dst) else {
                return Err(invalid_flow_ir(
                    &function.name,
                    "dynamic Int.to_string has no planned output buffer",
                ));
            };
            format!(
                "emela_emb_int_to_string((int64_t)({}), {buffer}, {capacity})",
                expressions[0]
            )
        }
    };
    writer.assign(&variable_name(dst), &expression);
    Ok(())
}

pub(super) fn string_behavior(args: &[Operand]) -> StringBehavior {
    StringBehavior {
        output: if matches!(args, [Operand::Var(_)]) {
            StringOutput::Buffer
        } else {
            StringOutput::None
        },
        ..StringBehavior::default()
    }
}
pub(super) fn string_bound(args: &[Operand], bounds: &StringBounds<'_>) -> Option<usize> {
    let [arg] = args else { return None };
    Some(match arg {
        Operand::Const(Const::Int(value)) => value.to_string().len(),
        Operand::Const(Const::Int64(value)) => value.to_string().len(),
        _ => 11.min(usize::from(bounds.capacity)),
    })
}

pub(super) fn require_runtime(args: &[Operand], requirements: &mut runtime_plan::Requirements) {
    if matches!(args, [Operand::Var(_)]) {
        requirements.require(runtime_plan::Helper::IntToString);
        requirements.defects = true;
    }
}
