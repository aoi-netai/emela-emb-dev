use super::super::*;
pub(in crate::backend) fn string_contains(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let op = Builtin::StringContains;
    let (dst, expressions) = builtin_utilities::prepare(
        context,
        dst,
        op,
        args,
        2,
        ResultType::Exact(Type::Bool),
        |layouts, function, args| {
            args.iter()
                .all(|operand| operand_is_string(layouts, function, operand))
        },
    )?;
    let writer = &mut *context.writer;
    let expression = format!(
        "emela_emb_string_contains({}, {})",
        expressions[0], expressions[1]
    );
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

pub(in crate::backend) fn emit_string_contains_helper(writer: &mut Writer) {
    writer.begin_block(
        "static inline int32_t emela_emb_string_contains(EmelaString value, EmelaString needle)",
    );
    writer.begin_block("if (needle.length > value.length)");
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.line("uint32_t last = (uint32_t)value.length - needle.length;");
    writer.begin_block("for (uint32_t offset = 0; offset <= last; ++offset)");
    writer.begin_block("if (emela_emb_string_matches_at(value, needle, offset))");
    writer.line("return INT32_C(1);");
    writer.end_block();
    writer.end_block();
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.blank();
}

pub(super) fn require_runtime(_args: &[Operand], requirements: &mut runtime_plan::Requirements) {
    requirements.require(runtime_plan::Helper::StringContains);
}
