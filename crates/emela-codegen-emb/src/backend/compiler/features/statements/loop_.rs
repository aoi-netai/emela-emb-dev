//! emela_lir::Stmt::Loop
use super::super::*;
pub(in crate::backend) fn emit(
    context: &mut Context<'_, '_>,
    statement: &Stmt,
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
    match statement {
        Stmt::Loop {
            dst,
            vars,
            inits,
            body,
        } => emit_loop(
            layouts,
            function,
            *dst,
            vars,
            inits,
            *body,
            writer,
            names,
            string_buffers,
            loops,
            blocks,
            protects,
            next_temp,
            next_label,
        ),
        other => Err(unsupported(&function.name, statement_name(other))),
    }
}

pub(in crate::backend) fn lower(
    context: &mut LoweringContext<'_>,
    _id: StmtId,
    statement: &Stmt,
    next: usize,
) -> Result<usize, Vec<Error>> {
    let graph = &mut *context.graph;
    let function = context.function;
    let entry = next;
    let Stmt::Loop {
        dst,
        vars,
        inits,
        body,
    } = statement
    else {
        unreachable!("graph dispatch")
    };
    Ok({
        let head = graph.add(Node::Unreachable);
        graph.loops.push(LoopTarget {
            vars: vars.clone(),
            entry: head,
            depth: graph.depth,
        });
        let body = graph.sequence(function, *body, *dst, entry)?;
        graph.loops.pop();
        graph.nodes[head] = Node::Assign(vec![], body);
        graph.add(Node::Assign(
            vars.iter().copied().zip(inits.clone()).collect(),
            head,
        ))
    })
}

#[allow(clippy::too_many_arguments)]
pub(in crate::backend) fn emit_loop(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    vars: &[VarId],
    inits: &[Operand],
    body: emela_lir::SeqId,
    writer: &mut Writer,
    names: &HashMap<FnId, String>,
    string_buffers: &StringBufferPlan,
    loops: &mut Vec<Vec<VarId>>,
    blocks: &mut Vec<BlockTarget>,
    protects: &mut Vec<ProtectTarget>,
    next_temp: &mut usize,
    next_label: &mut usize,
) -> Result<(), Vec<Error>> {
    if vars.len() != inits.len() {
        return Err(invalid_flow_ir(
            &function.name,
            "loop variable and initializer counts differ",
        ));
    }
    for (&variable, initial) in vars.iter().zip(inits) {
        let expression =
            operand_expression(initial).map_err(|variant| unsupported(&function.name, variant))?;
        emit_retain_operand(layouts, function, initial, writer);
        emit_release_if_owned(layouts, function, variable, writer);
        writer.assign(&variable_name(variable), &expression);
        if layouts.is_managed_value(function.vars[variable].ty) {
            writer.assign(&owned_variable_name(variable), "INT32_C(1)");
        }
    }
    writer.begin_block("for (;;)");
    loops.push(vars.to_vec());
    let result = emit_compound_sequence(
        layouts,
        function,
        body,
        dst,
        writer,
        names,
        string_buffers,
        loops,
        blocks,
        protects,
        next_temp,
        next_label,
    );
    loops.pop();
    result?;
    writer.line("break;");
    writer.end_block();
    Ok(())
}
