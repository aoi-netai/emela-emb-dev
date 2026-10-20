//! Typed producers and consumers for the fixed Deferred runtime.

use super::*;

pub(super) fn value_type(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    handle: &Operand,
) -> Result<TypeId, Vec<Error>> {
    let ty = operand_type(function, handle)
        .ok_or_else(|| invalid_flow_ir(&function.name, "Deferred handle without type"))?;
    match &layouts.types[ty] {
        Type::Enum(_, args)
            if layouts.is_named_external(ty, "Std.Deferred.Deferred") && args.len() == 1 =>
        {
            Ok(args[0])
        }
        _ => Err(unsupported(
            &function.name,
            "Deferred operation without a concrete Deferred[A] handle",
        )),
    }
}

fn coerce_function(layouts: &TypeLayouts<'_>, source: TypeId, destination: TypeId) -> Function {
    let mut f = Function::new("Deferred value");
    f.vars.alloc(emela_lir::VarData {
        hint: "source".into(),
        ty: source,
        repr: layouts.type_repr(source),
    });
    f.vars.alloc(emela_lir::VarData {
        hint: "destination".into(),
        ty: destination,
        repr: layouts.type_repr(destination),
    });
    f
}

pub(super) fn emit_runtime(
    layouts: &TypeLayouts<'_>,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    for &ty in &layouts.deferred_read_types {
        if layouts.type_repr(ty) == Repr::Void {
            continue;
        }
        writer.begin_block(&format!(
            "static inline {} emela_deferred_read_{}(EmelaAny emela_v_0)",
            layouts.c_type(ty),
            ty.index()
        ));
        writer.line(&format!("{} emela_v_1 = {{0}};", layouts.c_type(ty)));
        let f = coerce_function(layouts, TypeId::ANY, ty);
        let vars: Vec<_> = f.vars.iter().map(|(id, _)| id).collect();
        emit_coerce(layouts, &f, Some(vars[1]), vars[0], writer)?;
        writer.line("return emela_v_1;");
        writer.end_block();
    }
    for &ty in &layouts.deferred_complete_types {
        if !matches!(layouts.types[ty], Type::String | Type::Any)
            && type_contains_string(layouts, ty, &mut HashSet::new())
        {
            return Err(unsupported(
                "Deferred.complete",
                "aggregate or List containing a borrowed String",
            ));
        }
        let represented = layouts.type_repr(ty) != Repr::Void;
        writer.begin_block(&format!(
            "static inline int32_t emela_deferred_complete_{}(emela_deferred *deferred{})",
            ty.index(),
            if represented {
                format!(", {} emela_v_0", layouts.c_type(ty))
            } else {
                String::new()
            }
        ));
        writer.line("emela_deferred_slot *slot = emela_deferred_prepare_complete(deferred);");
        writer.line("if (slot == NULL) return INT32_C(0);");
        writer.line("EmelaAny emela_v_1 = {0};");
        if represented {
            if matches!(layouts.types[ty], Type::String) {
                writer.line("emela_v_0 = emela_heap_own_string(emela_v_0);");
            }
            let f = coerce_function(layouts, ty, TypeId::ANY);
            let vars: Vec<_> = f.vars.iter().map(|(id, _)| id).collect();
            emit_coerce(layouts, &f, Some(vars[1]), vars[0], writer)?;
            if matches!(layouts.types[ty], Type::String) {
                writer.line("emela_heap_release(emela_v_0.owner);");
            }
        }
        writer.line("slot->value = emela_v_1; slot->full = 1;");
        writer.line("emela_deferred_publish(slot);");
        writer.line("return INT32_C(1);");
        writer.end_block();
    }
    for &option in &layouts.deferred_poll_types {
        let ty = option_payload_type(layouts, option).expect("validated Poll payload");
        let f = Function::new("Deferred.poll");
        let none = option_variant_expression(layouts, &f, option, "None", None)?;
        let represented = layouts.type_repr(ty) != Repr::Void;
        let some =
            option_variant_expression(layouts, &f, option, "Some", represented.then_some("value"))?;
        writer.begin_block(&format!(
            "static inline {} emela_deferred_poll_{}(emela_deferred *deferred)",
            layouts.c_type(option),
            option.index()
        ));
        writer.line("emela_deferred_slot *slot = emela_deferred_get_poll(deferred);");
        writer.line(&format!("if (!slot->full) return {none};"));
        if represented {
            writer.line(&format!(
                "{} value = emela_deferred_read_{}(slot->value);",
                layouts.c_type(ty),
                ty.index()
            ));
        }
        writer.line(&format!("return {some};"));
        writer.end_block();
    }
    Ok(())
}

pub(super) fn emit_poll(
    _layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    args: &[Operand],
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let [handle] = args else {
        return Err(invalid_flow_ir(&function.name, "Poll arity"));
    };
    let handle = operand_expression(handle).map_err(|why| unsupported(&function.name, why))?;
    if let Some(dst) = dst {
        writer.assign(
            &variable_name(dst),
            &format!(
                "emela_deferred_poll_{}({handle})",
                function.vars[dst].ty.index()
            ),
        );
    } else {
        writer.line(&format!("(void)emela_deferred_get_poll({handle});"));
    }
    Ok(())
}

pub(super) fn emit_complete(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    args: &[Operand],
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let [handle, value] = args else {
        return Err(invalid_flow_ir(&function.name, "Complete arity"));
    };
    let ty = value_type(layouts, function, handle)?;
    if operand_type(function, value) != Some(ty) {
        return Err(invalid_flow_ir(&function.name, "Complete value type"));
    }
    let mut call =
        vec![operand_expression(handle).map_err(|why| unsupported(&function.name, why))?];
    if layouts.type_repr(ty) != Repr::Void {
        call.push(operand_expression(value).map_err(|why| unsupported(&function.name, why))?);
    }
    let expression = format!(
        "emela_deferred_complete_{}({})",
        ty.index(),
        call.join(", ")
    );
    if let Some(dst) = dst {
        writer.assign(&variable_name(dst), &expression);
    } else {
        writer.line(&format!("(void){expression};"));
    }
    Ok(())
}
