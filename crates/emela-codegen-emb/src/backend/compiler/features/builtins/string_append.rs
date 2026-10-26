use super::super::*;
pub(in crate::backend) fn string_append(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    let op = Builtin::StringAppend;
    let (dst, expressions) = builtin_utilities::prepare(
        context,
        dst,
        op,
        args,
        2,
        ResultType::Exact(Type::String),
        |layouts, function, args| {
            args.iter()
                .all(|operand| operand_is_string(layouts, function, operand))
        },
    )?;
    let function = context.function;
    let writer = &mut *context.writer;
    let string_buffers = context.string_buffers;
    let expression = match mode(args) {
        AppendMode::Constant => {
            let [
                Operand::Const(Const::String(left)),
                Operand::Const(Const::String(right)),
            ] = args
            else {
                unreachable!("constant append")
            };
            constant_expression(&Const::String(format!("{left}{right}")))
                .map_err(|variant| unsupported(&function.name, variant))?
                .ok_or_else(|| {
                    invalid_flow_ir(&function.name, "String append has no C expression")
                })?
        }
        AppendMode::AliasInput => {
            let [left, right] = args else {
                unreachable!("binary append")
            };
            let other = if matches!(left, Operand::Const(Const::String(value)) if value.is_empty())
            {
                right
            } else {
                left
            };
            operand_expression(other).map_err(|variant| unsupported(&function.name, variant))?
        }
        AppendMode::Buffered => {
            let Some((buffer, capacity)) = string_buffers.buffer(dst) else {
                return Err(invalid_flow_ir(
                    &function.name,
                    "dynamic String append has no planned output buffer",
                ));
            };
            let copy_left = if matches!(args[0], Operand::Const(Const::String(_))) {
                "INT32_C(1)".to_owned()
            } else {
                format!("(({}).data != {buffer})", expressions[0])
            };
            format!(
                "emela_emb_string_append({}, {}, {buffer}, {capacity}, {copy_left})",
                expressions[0], expressions[1]
            )
        }
    };
    writer.assign(&variable_name(dst), &expression);
    Ok(())
}

fn is_static(args: &[Operand]) -> bool {
    matches!(
        args,
        [
            Operand::Const(Const::String(_)),
            Operand::Const(Const::String(_))
        ]
    )
}

fn has_empty_constant(args: &[Operand]) -> bool {
    matches!(args.first(), Some(Operand::Const(Const::String(value))) if value.is_empty())
        || matches!(args.get(1), Some(Operand::Const(Const::String(value))) if value.is_empty())
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum AppendMode {
    Constant,
    AliasInput,
    Buffered,
}

pub(super) fn mode(args: &[Operand]) -> AppendMode {
    if is_static(args) {
        AppendMode::Constant
    } else if has_empty_constant(args) {
        AppendMode::AliasInput
    } else {
        AppendMode::Buffered
    }
}

pub(super) fn string_behavior(args: &[Operand]) -> StringBehavior {
    let buffered = args.len() == 2 && mode(args) == AppendMode::Buffered;
    StringBehavior {
        output: if buffered {
            StringOutput::Buffer
        } else {
            StringOutput::None
        },
        reusable_input: if buffered {
            match args.first() {
                Some(Operand::Var(var)) => Some(*var),
                _ => None,
            }
        } else {
            None
        },
        input_use: if has_empty_constant(args) {
            InputUse::Escapes
        } else {
            InputUse::ReadOnly
        },
        ..StringBehavior::default()
    }
}

pub(super) fn string_bound(args: &[Operand], bounds: &StringBounds<'_>) -> Option<usize> {
    let [left, right] = args else { return None };
    let (left, right) = (bounds.operand(left), bounds.operand(right));
    Some(match mode(args) {
        AppendMode::Constant => left.saturating_add(right),
        AppendMode::AliasInput => left.max(right),
        AppendMode::Buffered => left.saturating_add(right).min(usize::from(bounds.capacity)),
    })
}

pub(super) fn result_ownership(args: &[Operand]) -> ResultOwnership {
    if has_empty_constant(args) {
        ResultOwnership::Borrowed
    } else {
        ResultOwnership::Owned
    }
}

pub(in crate::backend) fn emit_string_append_helper(writer: &mut Writer) {
    writer.begin_block(
        "static inline EmelaString emela_emb_string_append(EmelaString left, EmelaString right, char *buffer, uint16_t capacity, int copy_left)",
    );
    writer.begin_block(
        "if (left.length > capacity || right.length > (uint16_t)(capacity - left.length))",
    );
    writer.line(
        "emela_board_defect(EMELA_DEFECT_STRING_CAPACITY, \"string capacity exceeded\", UINT16_C(24));",
    );
    writer.end_block();
    writer.begin_block("if (copy_left)");
    writer.begin_block("for (uint32_t i = UINT32_C(0); i < (uint32_t)left.length; ++i)");
    writer.line("buffer[i] = left.data[i];");
    writer.end_block();
    writer.end_block();
    writer.begin_block("for (uint32_t i = UINT32_C(0); i < (uint32_t)right.length; ++i)");
    writer.line("buffer[(uint32_t)left.length + i] = right.data[i];");
    writer.end_block();
    writer.line(
        "return (EmelaString){ .data = buffer, .length = (uint16_t)(left.length + right.length) };",
    );
    writer.end_block();
    writer.blank();
}

pub(super) fn require_runtime(args: &[Operand], requirements: &mut runtime_plan::Requirements) {
    if args.len() == 2 && mode(args) == AppendMode::Buffered {
        requirements.require(runtime_plan::Helper::StringAppend);
        requirements.defects = true;
    }
}
