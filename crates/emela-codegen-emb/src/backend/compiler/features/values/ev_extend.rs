//! emela_lir::Value::EvExtend
use super::super::*;
pub(in crate::backend) fn emit(
    context: &mut Context<'_, '_>,
    dst: &Option<VarId>,
    value: &Value,
) -> Result<(), Vec<Error>> {
    let layouts = context.layouts;
    let function = context.function;
    let writer = &mut *context.writer;
    match value {
        Value::EvExtend { base, bindings } => {
            emit_evidence_extend(layouts, function, *dst, *base, bindings, writer)?;
        }
        other => return Err(unsupported(&function.name, value_name(other))),
    };
    Ok(())
}

pub(in crate::backend) fn emit_evidence_extend(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    base: Option<VarId>,
    bindings: &[(EffectId, VarId)],
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let Some(dst) = dst else {
        return Err(invalid_flow_ir(
            &function.name,
            "EvExtend has no destination",
        ));
    };
    if !matches!(layouts.types[function.vars[dst].ty], Type::Evidence) {
        return Err(invalid_flow_ir(
            &function.name,
            "EvExtend destination is not Evidence",
        ));
    }
    if let Some(base) = base
        && !matches!(layouts.types[function.vars[base].ty], Type::Evidence)
    {
        return Err(invalid_flow_ir(
            &function.name,
            "EvExtend base is not Evidence",
        ));
    }
    if bindings.is_empty() {
        writer.assign(
            &variable_name(dst),
            &base.map_or_else(|| "NULL".to_owned(), variable_name),
        );
        return Ok(());
    }
    let suffix = u32::from(dst.into_raw());
    let chain = format!("emela_evidence_chain_{suffix}");
    let base_expression = base.map_or_else(|| "NULL".to_owned(), variable_name);
    writer.decl("void *", &chain, Some(&base_expression));
    if base.is_some() {
        writer.line(&format!("emela_heap_retain({chain});"));
    }
    let node_kind = layouts.evidence_node_kind();
    for (index, (effect, handler)) in bindings.iter().enumerate() {
        if layouts.types[function.vars[*handler].ty] != Type::Handler(*effect) {
            return Err(invalid_flow_ir(
                &function.name,
                "EvExtend binding does not match its handler effect",
            ));
        }
        let node = format!("emela_evidence_node_{suffix}_{index}");
        writer.decl(
            "emela_evidence_node *",
            &node,
            Some(&format!(
                "(emela_evidence_node *)emela_heap_allocate(sizeof(emela_evidence_node), UINT32_C({node_kind}))"
            )),
        );
        writer.assign(
            &format!("{node}->effect"),
            &format!("UINT32_C({})", u32::from(effect.into_raw())),
        );
        writer.assign(&format!("{node}->handler"), &variable_name(*handler));
        writer.line(&format!("emela_heap_retain({node}->handler);"));
        writer.assign(&format!("{node}->parent"), &chain);
        writer.assign(&chain, &node);
    }
    writer.assign(&variable_name(dst), &chain);
    Ok(())
}

pub(super) fn result_ownership(is_empty: bool) -> ResultOwnership {
    if is_empty {
        ResultOwnership::Borrowed
    } else {
        ResultOwnership::Owned
    }
}

pub(super) fn needs_heap(is_empty: bool) -> bool {
    !is_empty
}
