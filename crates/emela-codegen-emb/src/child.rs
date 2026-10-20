//! Typed result delivery for structured child fibers.

use super::*;

pub(super) fn value_type(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    handle: &Operand,
) -> Result<TypeId, Vec<Error>> {
    let ty = operand_type(function, handle)
        .ok_or_else(|| invalid_flow_ir(&function.name, "Fiber handle without type"))?;
    match &layouts.types[ty] {
        Type::Enum(_, args)
            if layouts.is_named_external(ty, "Std.Fiber.Fiber") && args.len() == 1 =>
        {
            Ok(args[0])
        }
        _ => Err(unsupported(
            &function.name,
            "Fiber operation without a concrete Fiber[A] handle",
        )),
    }
}

pub(super) fn emit_runtime(
    layouts: &TypeLayouts<'_>,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    for &ty in &layouts.child_join_types {
        if layouts.type_repr(ty) == Repr::Void {
            continue;
        }
        writer.begin_block(&format!(
            "static void emela_child_join_{}(emela_child *child, void *result)",
            ty.index()
        ));
        writer.line("if (child->status != 1) emela_scheduler_invalid();");
        writer.line("EmelaAny emela_v_0 = child->result;");
        writer.line(&format!("{} emela_v_1 = {{0}};", layouts.c_type(ty)));
        let f = deferred::coerce_function(layouts, TypeId::ANY, ty);
        let vars: Vec<_> = f.vars.iter().map(|(id, _)| id).collect();
        emit_coerce(layouts, &f, Some(vars[1]), vars[0], writer)?;
        writer.line(&format!("*({} *)result = emela_v_1;", layouts.c_type(ty)));
        writer.end_block();
    }
    Ok(())
}

pub(super) fn emit_join(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    args: &[Operand],
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let [handle] = args else {
        return Err(invalid_flow_ir(&function.name, "Join arity"));
    };
    let ty = value_type(layouts, function, handle)?;
    let result = dst
        .map(|v| format!("&{}", variable_name(v)))
        .unwrap_or_else(|| "NULL".into());
    let deliver = if layouts.type_repr(ty) == Repr::Void {
        "NULL".into()
    } else {
        format!("emela_child_join_{}", ty.index())
    };
    let handle = operand_expression(handle).map_err(|why| unsupported(&function.name, why))?;
    writer.line(&format!(
        "emela_child_wait(fiber, {handle}, 0, {result}, {deliver});"
    ));
    Ok(())
}
