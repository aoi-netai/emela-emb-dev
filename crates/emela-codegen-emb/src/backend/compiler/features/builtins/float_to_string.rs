use super::super::*;
pub(in crate::backend) fn float_to_string(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let op = Builtin::FloatToString;
    let (dst, expressions) = builtin_utilities::prepare(
        context,
        dst,
        op,
        args,
        1,
        ResultType::Exact(Type::String),
        |layouts, function, args| operand_is_float(layouts, function, &args[0]),
    )?;
    let function = context.function;
    let writer = &mut *context.writer;
    let string_buffers = context.string_buffers;
    let expression = match &args[0] {
        Operand::Const(Const::Float(value)) => {
            constant_expression(&Const::String(const_eval::format_float(*value)))
                .map_err(|variant| unsupported(&function.name, variant))?
                .expect("String constant has an expression")
        }
        _ => {
            let Some((buffer, capacity)) = string_buffers.buffer(dst) else {
                return Err(invalid_flow_ir(
                    &function.name,
                    "dynamic Float.to_string has no planned output buffer",
                ));
            };
            format!(
                "emela_emb_float_to_string({}, {buffer}, {capacity})",
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
        Operand::Const(Const::Float(value)) => const_eval::format_float(*value).len(),
        _ => crate::float_to_string::MAX_OUTPUT_BYTES.min(usize::from(bounds.capacity)),
    })
}

pub(super) fn require_runtime(args: &[Operand], requirements: &mut runtime_plan::Requirements) {
    if matches!(args, [Operand::Var(_)]) {
        requirements.require(runtime_plan::Helper::FloatToString);
        requirements.defects = true;
    }
}
