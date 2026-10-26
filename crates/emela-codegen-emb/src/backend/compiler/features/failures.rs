use super::*;
pub(in crate::backend) fn break_edge(
    context: &mut Context<'_, '_>,
    edge: OnFail,
    error_name: &str,
) -> Result<(), Vec<Error>> {
    let layouts = context.layouts;
    let function = context.function;
    let writer = &mut *context.writer;
    let names = context.names;
    let string_buffers = context.string_buffers;
    let loops = &mut *context.loops;
    let blocks = &mut *context.blocks;
    let protects = &mut *context.protects;
    let next_temp = &mut *context.next_temp;
    let next_label = &mut *context.next_label;
    let OnFail::Break(label) = edge else {
        unreachable!("failure edge dispatch")
    };
    {
        let Some(target_index) = blocks.iter().rposition(|target| target.label == label) else {
            return Err(invalid_flow_ir(
                &function.name,
                "failing call target is not an enclosing emitted block",
            ));
        };
        let target_dst = blocks[target_index].dst;
        let protect_depth = blocks[target_index].protect_depth;
        let Some(target_dst) = target_dst else {
            return Err(invalid_flow_ir(
                &function.name,
                "failing call target block has no error destination",
            ));
        };
        if !matches!(layouts.types[function.vars[target_dst].ty], Type::Any) {
            return Err(invalid_flow_ir(
                &function.name,
                "failing call target does not use the EmelaError representation",
            ));
        }
        emit_release_if_owned(layouts, function, target_dst, writer);
        writer.assign(&variable_name(target_dst), error_name);
        writer.assign(error_name, "(EmelaError){0}");
        if layouts.is_managed_value(function.vars[target_dst].ty) {
            writer.assign(&owned_variable_name(target_dst), "INT32_C(1)");
        }
        emit_pending_cleanups(
            layouts,
            function,
            protect_depth,
            writer,
            names,
            string_buffers,
            loops,
            blocks,
            protects,
            next_temp,
            next_label,
        )?;
        blocks[target_index].used = true;
        writer.line(&format!("goto {};", blocks[target_index].exit));
        Ok(())
    }
}
pub(in crate::backend) fn propagate_edge(
    context: &mut Context<'_, '_>,
    edge: OnFail,
    error_name: &str,
) -> Result<(), Vec<Error>> {
    let layouts = context.layouts;
    let function = context.function;
    let writer = &mut *context.writer;
    let names = context.names;
    let string_buffers = context.string_buffers;
    let loops = &mut *context.loops;
    let blocks = &mut *context.blocks;
    let protects = &mut *context.protects;
    let next_temp = &mut *context.next_temp;
    let next_label = &mut *context.next_label;
    let OnFail::Propagate = edge else {
        unreachable!("failure edge dispatch")
    };
    {
        if !function.fails {
            return Err(invalid_flow_ir(
                &function.name,
                "failure propagates through a function without a failure channel",
            ));
        }
        writer.assign("*emela_error_out", error_name);
        writer.assign(error_name, "(EmelaError){0}");
        emit_pending_cleanups(
            layouts,
            function,
            0,
            writer,
            names,
            string_buffers,
            loops,
            blocks,
            protects,
            next_temp,
            next_label,
        )?;
        emit_failure_return(layouts, function, writer);
        Ok(())
    }
}
pub(in crate::backend) fn handle_edge(
    context: &mut Context<'_, '_>,
    edge: OnFail,
    _error_name: &str,
) -> Result<(), Vec<Error>> {
    let function = context.function;
    let OnFail::Handle { .. } = edge else {
        unreachable!("failure edge dispatch")
    };
    Err(invalid_flow_ir(
        &function.name,
        "Heap IR failure landing appears in Flow IR",
    ))
}
pub(in crate::backend) fn emit(
    context: &mut Context<'_, '_>,
    edge: OnFail,
    error_name: &str,
) -> Result<(), Vec<Error>> {
    match edge {
        OnFail::Break(..) => break_edge(context, edge, error_name),
        OnFail::Propagate => propagate_edge(context, edge, error_name),
        OnFail::Handle { .. } => handle_edge(context, edge, error_name),
    }
}

pub(in crate::backend) fn lower_break(
    context: &mut LoweringContext<'_>,
    edge: OnFail,
) -> Result<usize, Vec<Error>> {
    let graph = &mut *context.graph;
    let function = context.function;
    let OnFail::Break(label) = edge else {
        unreachable!("failure graph entry")
    };
    {
        let &(dst, next, depth) = graph.blocks.get(&label).ok_or_else(|| {
            invalid_flow_ir(&function.name, "suspend failure without enclosing Block")
        })?;
        let dst = dst.ok_or_else(|| {
            invalid_flow_ir(
                &function.name,
                "suspend failure Block without error destination",
            )
        })?;
        let catch = graph.add(Node::Catch(dst, next, graph.error_depth));
        Ok(graph.leave(depth, catch))
    }
}
pub(in crate::backend) fn lower_propagate(
    context: &mut LoweringContext<'_>,
    edge: OnFail,
) -> Result<usize, Vec<Error>> {
    let graph = &mut *context.graph;
    let function = context.function;
    let OnFail::Propagate = edge else {
        unreachable!("failure graph entry")
    };
    {
        if !function.fails {
            return Err(invalid_flow_ir(
                &function.name,
                "suspend failure without a channel",
            ));
        }
        let fail = graph.add(Node::Fail(graph.error_depth));
        Ok(graph.leave(0, fail))
    }
}
pub(in crate::backend) fn lower_handle(
    context: &mut LoweringContext<'_>,
    edge: OnFail,
) -> Result<usize, Vec<Error>> {
    let _graph = &mut *context.graph;
    let function = context.function;
    let OnFail::Handle { .. } = edge else {
        unreachable!("failure graph entry")
    };
    Err(invalid_flow_ir(
        &function.name,
        "Heap IR failure edge in Flow IR",
    ))
}
pub(in crate::backend) fn lower(
    context: &mut LoweringContext<'_>,
    edge: OnFail,
) -> Result<usize, Vec<Error>> {
    match edge {
        OnFail::Break(..) => lower_break(context, edge),
        OnFail::Propagate => lower_propagate(context, edge),
        OnFail::Handle { .. } => lower_handle(context, edge),
    }
}

pub(in crate::backend) fn emit_check(
    context: &mut SuspendContext<'_, '_>,
    node: &Node,
) -> Result<(), Vec<Error>> {
    let writer = &mut *context.writer;
    let Node::CheckError {
        normal,
        failed,
        slot,
    } = node
    else {
        unreachable!("frame state dispatch")
    };
    {
        writer . line (& format ! ("if (frame->call_error.tag == 0) base->pc = {normal}; else {{ emela_error_clear(&frame->pending_error[{slot}]); frame->pending_error[{slot}] = frame->call_error; frame->call_error = (EmelaError){{0}}; base->pc = {failed}; }} continue;")) ;
    };
    Ok(())
}

pub(in crate::backend) fn emit_catch(
    context: &mut SuspendContext<'_, '_>,
    node: &Node,
) -> Result<(), Vec<Error>> {
    let layouts = context.layouts;
    let function = context.function;
    let writer = &mut *context.writer;
    let Node::Catch(dst, next, slot) = node else {
        unreachable!("frame state dispatch")
    };
    {
        if !matches!(layouts.types[function.vars[*dst].ty], Type::Any) {
            return Err(invalid_flow_ir(
                &function.name,
                "failure Block destination is not Any",
            ));
        }
        emit_release_if_owned(layouts, function, *dst, writer);
        writer.assign(
            &variable_name(*dst),
            &format!("frame->pending_error[{slot}]"),
        );
        writer.line(&format!(
            "frame->pending_error[{slot}] = (EmelaError){{0}};"
        ));
        if layouts.is_managed_value(function.vars[*dst].ty) {
            writer.assign(&owned_variable_name(*dst), "1");
        }
        writer.line(&format!("base->pc = {next}; continue;"));
    };
    Ok(())
}
