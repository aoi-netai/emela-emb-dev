//! Typed result delivery for structured child fibers.

use super::*;

impl TypeLayouts<'_> {
    pub(super) fn has_child_waits(&self) -> bool {
        !self.child_join_types.is_empty()
            || !self.child_await_types.is_empty()
            || self.child_interrupts
    }
}

pub(super) fn success_type(layouts: &TypeLayouts<'_>, exit: TypeId) -> Option<TypeId> {
    let fields = layouts.enum_fields(exit)?.get(emela_core::EXIT_SUCCESS)?;
    if fields.len() == 1 {
        Some(fields[0])
    } else {
        None
    }
}

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
    for &exit in &layouts.child_await_types {
        let ty = success_type(layouts, exit).expect("validated Await Success payload");
        let represented = layouts.type_repr(ty) != Repr::Void;
        writer.begin_block(&format!(
            "static void emela_child_await_{}(emela_child *child, void *result)",
            exit.index()
        ));
        // Explicit Interrupt and Cause delivery are connected with the unwind runtime in 7.8.
        writer.line("if (child->status != 1) emela_scheduler_invalid();");
        if represented {
            writer.line("EmelaAny emela_v_0 = child->result;");
            writer.line(&format!("{} emela_v_1 = {{0}};", layouts.c_type(ty)));
            let f = deferred::coerce_function(layouts, TypeId::ANY, ty);
            let vars: Vec<_> = f.vars.iter().map(|(id, _)| id).collect();
            emit_coerce(layouts, &f, Some(vars[1]), vars[0], writer)?;
        }
        let field = if represented {
            "f0 = emela_v_1"
        } else {
            "_unit = UINT8_C(0)"
        };
        writer.line(&format!(
            "*({cty} *)result = ({cty}){{.tag = INT32_C({tag}), .payload.v{tag}.{field}}};",
            cty = layouts.c_type(exit),
            tag = emela_core::EXIT_SUCCESS
        ));
        writer.end_block();
    }
    Ok(())
}

pub(super) fn emit_wait(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    op: emela_core::FiberOp,
    args: &[Operand],
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let [handle] = args else {
        return Err(invalid_flow_ir(&function.name, "Join arity"));
    };
    let ty = value_type(layouts, function, handle)?;
    if op == emela_core::FiberOp::Interrupt {
        let handle = operand_expression(handle).map_err(|why| unsupported(&function.name, why))?;
        writer.line(&format!("emela_child_interrupt(fiber, {handle});"));
        return Ok(());
    }
    let result = dst
        .map(|v| format!("&{}", variable_name(v)))
        .unwrap_or_else(|| "NULL".into());
    let (awaiting, deliver) = match op {
        emela_core::FiberOp::Join => (
            0,
            if layouts.type_repr(ty) == Repr::Void {
                "NULL".into()
            } else {
                format!("emela_child_join_{}", ty.index())
            },
        ),
        emela_core::FiberOp::Await { exit } => {
            // Discarding the Exit still waits. No value or Cause needs to be materialized.
            let result_ty = layouts
                .child_await_types
                .iter()
                .copied()
                .find(|&result| {
                    matches!(layouts.types[result], Type::Enum(id, _) if id == exit)
                        && success_type(layouts, result) == Some(ty)
                })
                .ok_or_else(|| invalid_flow_ir(&function.name, "Await result layout"))?;
            (1, format!("emela_child_await_{}", result_ty.index()))
        }
        _ => return Err(invalid_flow_ir(&function.name, "child wait operation")),
    };
    let handle = operand_expression(handle).map_err(|why| unsupported(&function.name, why))?;
    writer.line(&format!(
        "emela_child_wait(fiber, {handle}, {awaiting}, {result}, {deliver});"
    ));
    Ok(())
}
