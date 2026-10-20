//! Backend-owned resumable control flow. Input remains verified Flow IR.

use super::*;

fn scalar(types: &TypeTable, ty: TypeId) -> bool {
    matches!(
        types[ty],
        Type::Int | Type::Int64 | Type::Float | Type::Bool | Type::Unit | Type::Never
    )
}

pub(super) fn validate(
    flow: &emela_lir::Module,
    emitted: &HashSet<FnId>,
    options: &Options,
) -> Result<bool, Vec<Error>> {
    let mut enabled = false;
    for (id, function) in flow.functions.iter().filter(|(id, _)| emitted.contains(id)) {
        if !function.suspends {
            continue;
        }
        enabled = true;
        if function.fails || function.constant {
            return Err(unsupported(
                &function.name,
                "failing or const suspend function (phase 7 follow-up)",
            ));
        }
        if function.external.is_some()
            && (!scalar(&flow.core.types, function.ret)
                || function
                    .params
                    .iter()
                    .any(|p| !scalar(&flow.core.types, function.vars[*p].ty)))
        {
            return Err(unsupported(
                &function.name,
                "non-scalar external task ABI (phase 7 follow-up)",
            ));
        }
        for (_, stmt) in function.stmts.iter() {
            if matches!(stmt, Stmt::Protect { .. }) {
                return Err(unsupported(
                    &function.name,
                    "Protect cleanup in a suspend function (phase 7 follow-up)",
                ));
            }
            if let Stmt::Let { value, .. } = stmt {
                match value {
                    Value::Call {
                        callee: Callee::Direct(target),
                        on_fail: None,
                        ..
                    } => {
                        let _ = target;
                    }
                    Value::Fiber { .. } => {
                        return Err(unsupported(
                            &function.name,
                            &format!(
                                "{} in a suspend function (phase 7 follow-up)",
                                value_name(value)
                            ),
                        ));
                    }
                    _ => {}
                }
            }
        }
        // Suspended entry points have an asynchronous start ABI, not a synchronous adapter.
        for (caller_id, caller) in flow.functions.iter().filter(|(id, _)| emitted.contains(id)) {
            if caller_id == id {
                continue;
            }
            if !caller.suspends && directly_calls(caller, id) {
                return Err(unsupported(
                    &caller.name,
                    "suspend call from a synchronous function",
                ));
            }
        }
    }
    if enabled
        && (options.fiber_capacity == 0 || options.frame_capacity == 0 || options.frame_bytes == 0)
    {
        return Err(vec![Error { kind: ErrorKind::InvalidOptions, message: "fiber_capacity, frame_capacity, and frame_bytes must be positive for suspend functions".into() }]);
    }
    if has_tasks(flow, emitted) && (options.task_capacity == 0 || options.event_capacity == 0) {
        return Err(vec![Error {
            kind: ErrorKind::InvalidOptions,
            message: "task_capacity and event_capacity must be positive for suspend external calls"
                .into(),
        }]);
    }
    Ok(enabled)
}

pub(super) fn has_tasks(flow: &emela_lir::Module, emitted: &HashSet<FnId>) -> bool {
    flow.functions
        .iter()
        .any(|(id, f)| emitted.contains(&id) && f.suspends && f.external.is_some())
}

pub(super) fn external_symbol(
    function: &Function,
    options: &Options,
) -> Result<String, Vec<Error>> {
    let binding = function
        .external
        .as_ref()
        .and_then(|external| external.binding("c"))
        .ok_or_else(|| unsupported(&function.name, "suspend external without C binding"))?;
    let original = match &binding.host {
        emela_core::HostRef::Global(name) | emela_core::HostRef::Import { name, .. } => name,
    };
    let (_, symbol) = options
        .task_adapters
        .iter()
        .find(|(name, _)| name == original)
        .ok_or_else(|| {
            unsupported(
                &function.name,
                "suspend external needs an explicit Options::with_task_adapter mapping",
            )
        })?;
    if !is_c_identifier(symbol) {
        return Err(unsupported(&function.name, "invalid C task adapter symbol"));
    }
    Ok(symbol.clone())
}

pub(super) fn task_signature(layouts: &TypeLayouts<'_>, function: &Function, name: &str) -> String {
    let mut params = vec!["EmelaTask task".to_owned()];
    params.extend(function.params.iter().map(|id| {
        format!(
            "{} {}",
            layouts.c_type(function.vars[*id].ty),
            variable_name(*id)
        )
    }));
    format!("void {name}({})", params.join(", "))
}

pub(super) fn emit_runtime(writer: &mut Writer, options: &Options, tasks: bool) {
    writer.line(&format!(
        "#define EMELA_FIBER_CAPACITY {}",
        options.fiber_capacity
    ));
    writer.line(&format!(
        "#define EMELA_FRAME_CAPACITY {}",
        options.frame_capacity
    ));
    writer.line(&format!(
        "#define EMELA_FRAME_BYTES {}",
        options.frame_bytes
    ));
    if tasks {
        writer.line(&format!(
            "#define EMELA_TASK_CAPACITY {}",
            options.task_capacity
        ));
        writer.line(&format!(
            "#define EMELA_EVENT_CAPACITY {}",
            options.event_capacity
        ));
    }
    writer.line(include_str!("runtime/scheduler.c"));
    if tasks {
        writer.line(include_str!("runtime/task.c"));
    }
}

fn parameters(layouts: &TypeLayouts<'_>, function: &Function) -> Vec<String> {
    let mut params = function
        .params
        .iter()
        .map(|id| {
            format!(
                "{} {}",
                layouts.c_type(function.vars[*id].ty),
                variable_name(*id)
            )
        })
        .collect::<Vec<_>>();
    if layouts.type_repr(function.ret) != Repr::Void {
        params.push(format!("{} *result", layouts.c_type(function.ret)));
    }
    params
}

pub(super) fn start_signature(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    name: &str,
) -> String {
    let params = parameters(layouts, function);
    format!(
        "EmelaFiber {name}_start({})",
        if params.is_empty() {
            "void".to_owned()
        } else {
            params.join(", ")
        }
    )
}

fn push_signature(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    name: &str,
    environment: Option<usize>,
) -> String {
    let mut params = vec!["emela_fiber *fiber".to_owned()];
    if environment.is_some() {
        params.push("void *emela_env".to_owned());
    }
    params.extend(parameters(layouts, function));
    format!("static void {name}_push({})", params.join(", "))
}

pub(super) fn emit_declarations(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    name: &str,
    environment: Option<usize>,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    writer.line(&format!(
        "{};",
        push_signature(layouts, function, name, environment)
    ));
    Ok(())
}

enum Node {
    Statement(StmtId, usize),
    Assign(Vec<(VarId, Operand)>, usize),
    Branch(Operand, usize, usize),
    Switch(SwitchOn, Vec<(CaseKey, usize)>, Option<usize>),
    Return(Option<Operand>),
    Defect(Operand),
    Unreachable,
}

#[derive(Clone)]
struct LoopTarget {
    vars: Vec<VarId>,
    entry: usize,
}

struct Graph {
    nodes: Vec<Node>,
    blocks: HashMap<LabelId, (Option<VarId>, usize)>,
    loops: Vec<LoopTarget>,
}

impl Graph {
    fn add(&mut self, node: Node) -> usize {
        let id = self.nodes.len();
        self.nodes.push(node);
        id
    }

    fn assign(&mut self, dst: Option<VarId>, value: &Option<Operand>, next: usize) -> usize {
        let values = dst.zip(value.clone()).into_iter().collect();
        self.add(Node::Assign(values, next))
    }

    fn sequence(
        &mut self,
        function: &Function,
        seq: SeqId,
        dst: Option<VarId>,
        next: usize,
    ) -> Result<usize, Vec<Error>> {
        let sequence = &function.seqs[seq];
        let mut entry = match &sequence.term {
            Term::Yield(value) => self.assign(dst, value, next),
            Term::Return(value) => self.add(Node::Return(value.clone())),
            Term::Defect(value) => self.add(Node::Defect(value.clone())),
            Term::Unreachable => self.add(Node::Unreachable),
            Term::Break { label, value } => {
                let &(dst, next) = self.blocks.get(label).ok_or_else(|| {
                    invalid_flow_ir(&function.name, "suspend Break without enclosing Block")
                })?;
                self.assign(dst, value, next)
            }
            Term::Recur(values) => {
                let target = self
                    .loops
                    .last()
                    .ok_or_else(|| {
                        invalid_flow_ir(&function.name, "suspend Recur without enclosing Loop")
                    })?
                    .clone();
                self.add(Node::Assign(
                    target.vars.into_iter().zip(values.clone()).collect(),
                    target.entry,
                ))
            }
            _ => {
                return Err(unsupported(
                    &function.name,
                    "failure terminator in suspend function (phase 7 follow-up)",
                ));
            }
        };
        for &id in sequence.stmts.iter().rev() {
            entry = match &function.stmts[id] {
                Stmt::Let { .. } => self.add(Node::Statement(id, entry)),
                Stmt::If {
                    dst,
                    cond,
                    then,
                    else_,
                } => {
                    let yes = self.sequence(function, *then, *dst, entry)?;
                    let no = self.sequence(function, *else_, *dst, entry)?;
                    self.add(Node::Branch(cond.clone(), yes, no))
                }
                Stmt::Switch {
                    dst,
                    on,
                    cases,
                    default,
                } => {
                    let cases = cases
                        .iter()
                        .map(|case| {
                            Ok((
                                case.key.clone(),
                                self.sequence(function, case.body, *dst, entry)?,
                            ))
                        })
                        .collect::<Result<_, Vec<Error>>>()?;
                    let default = default
                        .map(|seq| self.sequence(function, seq, *dst, entry))
                        .transpose()?;
                    self.add(Node::Switch(*on, cases, default))
                }
                Stmt::Block { dst, label, body } => {
                    self.blocks.insert(*label, (*dst, entry));
                    let body = self.sequence(function, *body, *dst, entry)?;
                    self.blocks.remove(label);
                    body
                }
                Stmt::Loop {
                    dst,
                    vars,
                    inits,
                    body,
                } => {
                    let head = self.add(Node::Unreachable);
                    self.loops.push(LoopTarget {
                        vars: vars.clone(),
                        entry: head,
                    });
                    let body = self.sequence(function, *body, *dst, entry)?;
                    self.loops.pop();
                    self.nodes[head] = Node::Assign(vec![], body);
                    self.add(Node::Assign(
                        vars.iter().copied().zip(inits.clone()).collect(),
                        head,
                    ))
                }
                _ => {
                    return Err(unsupported(
                        &function.name,
                        "unsupported compound suspend statement",
                    ));
                }
            };
        }
        Ok(entry)
    }
}

pub(super) fn emit_function(
    flow: &emela_lir::Module,
    layouts: &TypeLayouts<'_>,
    function: &Function,
    name: &str,
    names: &HashMap<FnId, String>,
    output_calls: &HashSet<FnId>,
    environment: Option<usize>,
    options: &Options,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    if function.external.is_some() {
        writer.begin_block(&push_signature(layouts, function, name, None));
        let kind = task_kind(layouts, function.ret, &function.name)?;
        let result = if layouts.type_repr(function.ret) == Repr::Void {
            "NULL"
        } else {
            "result"
        };
        writer.line(&format!(
            "EmelaTask task = emela_task_begin(fiber, {result}, EMELA_TASK_{kind});"
        ));
        let mut args = vec!["task".to_owned()];
        args.extend(function.params.iter().copied().map(variable_name));
        writer.line(&format!("{name}({});", args.join(", ")));
        writer.end_block();
        return Ok(());
    }
    let mut graph = Graph {
        nodes: vec![],
        blocks: HashMap::new(),
        loops: vec![],
    };
    let body = function
        .body
        .ok_or_else(|| invalid_flow_ir(&function.name, "suspend function without body"))?;
    let entry = graph.sequence(function, body, None, usize::MAX)?;
    let strings = StringBufferPlan::new(
        layouts,
        function,
        options.string_capacity(),
        output_calls,
        false,
    );
    let has_result = layouts.type_repr(function.ret) != Repr::Void;
    writer.line(&format!("typedef struct {name}_frame {{"));
    writer.line("  emela_frame base;");
    if has_result {
        writer.line(&format!("  {} *result;", layouts.c_type(function.ret)));
    }
    for (id, var) in function.vars.iter() {
        writer.line(&format!(
            "  {} local_{};",
            layouts.c_type(var.ty),
            u32::from(id.into_raw())
        ));
        if layouts.is_managed_value(var.ty) {
            writer.line(&format!("  int owned_{};", u32::from(id.into_raw())));
        }
        if let Type::Cell(content) = layouts.types[var.ty].clone()
            && !layouts.is_closure_cell_capture(function, id)
        {
            let storage_type = if layouts.type_repr(content) == Repr::Void {
                "uint8_t".to_owned()
            } else {
                layouts.c_type(content)
            };
            writer.line(&format!("  {storage_type} {};", cell_storage_name(id)));
            if layouts.is_managed_value(content) {
                writer.line(&format!("  int32_t {};", cell_initialized_name(id)));
            }
        }
    }
    for (name, capacity) in &strings.declarations {
        writer.line(&format!("  char {name}[{capacity}];"));
    }
    writer.line(&format!("}} {name}_frame;"));
    writer.line(&format!("_Static_assert(sizeof({name}_frame) <= EMELA_FRAME_BYTES, \"increase Options::with_frame_bytes for {name}\");"));
    writer.begin_block(&format!(
        "static void {name}_step(emela_fiber *fiber, emela_frame *base)"
    ));
    writer.line(&format!("{name}_frame *frame = ({name}_frame *)base;"));
    // Aliases let the shared emitter address persistent fields without stack copies.
    for (id, _) in function.vars.iter() {
        writer.line(&format!(
            "#define {} (frame->local_{})",
            variable_name(id),
            u32::from(id.into_raw())
        ));
        let ty = function.vars[id].ty;
        if layouts.is_managed_value(ty) {
            writer.line(&format!(
                "#define {} (frame->owned_{})",
                owned_variable_name(id),
                u32::from(id.into_raw())
            ));
        }
        if matches!(layouts.types[ty], Type::Cell(_))
            && !layouts.is_closure_cell_capture(function, id)
        {
            for field in [cell_storage_name(id), cell_initialized_name(id)] {
                writer.line(&format!("#define {field} (frame->{field})"));
            }
        }
    }
    for (name, _) in &strings.declarations {
        writer.line(&format!("#define {name} (frame->{name})"));
    }
    writer.begin_block("for (;;)");
    writer.begin_block("switch (frame->base.pc)");
    let mut next_temp = 0;
    for (index, node) in graph.nodes.iter().enumerate() {
        writer.begin_block(&format!("case {index}:"));
        match node {
            Node::Statement(id, next) => {
                let statement = &function.stmts[*id];
                if let Stmt::Let {
                    dst,
                    value:
                        Value::Call {
                            callee: Callee::Indirect(callee),
                            args,
                            on_fail: None,
                            ..
                        },
                    ..
                } = statement
                    && matches!(&layouts.types[function.vars[*callee].ty], Type::Fn(ft) if ft.suspends)
                {
                    let Type::Fn(ft) = &layouts.types[function.vars[*callee].ty] else {
                        unreachable!()
                    };
                    prepare_result(layouts, function, *dst, writer);
                    let callee = variable_name(*callee);
                    writer.line(&format!(
                        "if ({callee}.call == NULL) emela_scheduler_invalid();"
                    ));
                    let mut arguments = vec!["fiber".to_owned(), format!("{callee}.env")];
                    arguments.extend(
                        args.iter()
                            .map(operand_expression)
                            .collect::<Result<Vec<_>, _>>()
                            .map_err(|why| unsupported(&function.name, why))?,
                    );
                    if layouts.type_repr(ft.ret) != Repr::Void {
                        arguments.push(
                            dst.map(|dst| format!("&{}", variable_name(dst)))
                                .unwrap_or_else(|| "NULL".to_owned()),
                        );
                    }
                    writer.line(&format!("frame->base.pc = {next};"));
                    writer.line(&format!("{callee}.call({}); return;", arguments.join(", ")));
                } else if let Stmt::Let {
                    dst,
                    value:
                        Value::Call {
                            callee: Callee::Direct(target),
                            args,
                            suspends: true,
                            ..
                        },
                    ..
                } = statement
                {
                    let target_fn = &flow.functions[*target];
                    if target_fn.suspends
                        && let Some(dst) = dst
                    {
                        emit_release_if_owned(layouts, function, *dst, writer);
                        if layouts.is_managed_value(function.vars[*dst].ty) {
                            writer.assign(
                                &variable_name(*dst),
                                &format!("({}){{0}}", layouts.c_type(function.vars[*dst].ty)),
                            );
                            writer.assign(&owned_variable_name(*dst), "INT32_C(1)");
                        }
                    }
                    if target_fn.suspends {
                        let mut arguments = vec!["fiber".to_owned()];
                        arguments.extend(
                            args.iter()
                                .map(operand_expression)
                                .collect::<Result<Vec<_>, _>>()
                                .map_err(|why| unsupported(&function.name, why))?,
                        );
                        if layouts.type_repr(target_fn.ret) != Repr::Void {
                            arguments.push(
                                dst.map(|dst| format!("&{}", variable_name(dst)))
                                    .unwrap_or_else(|| "NULL".to_owned()),
                            );
                        }
                        writer.line(&format!("frame->base.pc = {next};"));
                        writer.line(&format!(
                            "{}_push({});",
                            names[target],
                            arguments.join(", ")
                        ));
                    } else {
                        let mut statement = statement.clone();
                        if let Stmt::Let {
                            value: Value::Call { suspends, .. },
                            ..
                        } = &mut statement
                        {
                            *suspends = false;
                        }
                        emit_statement(
                            layouts,
                            function,
                            &statement,
                            writer,
                            names,
                            &strings,
                            &mut vec![],
                            &mut vec![],
                            &mut vec![],
                            &mut next_temp,
                            &mut 0,
                        )?;
                        writer.line(&format!("frame->base.pc = {next};"));
                    }
                    writer.line("return;");
                } else {
                    emit_statement(
                        layouts,
                        function,
                        statement,
                        writer,
                        names,
                        &strings,
                        &mut vec![],
                        &mut vec![],
                        &mut vec![],
                        &mut next_temp,
                        &mut 0,
                    )?;
                    freeze_statement_result(layouts, function, statement, writer);
                    writer.line(&format!("frame->base.pc = {next}; continue;"));
                }
            }
            Node::Assign(values, next) => {
                // Recur is a parallel assignment: read every old value before overwriting any.
                for (index, (dst, value)) in values.iter().enumerate() {
                    writer.decl(
                        &layouts.c_type(function.vars[*dst].ty),
                        &format!("next_{index}"),
                        Some(
                            &operand_expression(value)
                                .map_err(|why| unsupported(&function.name, why))?,
                        ),
                    );
                    retain_frame_value(
                        layouts,
                        function.vars[*dst].ty,
                        &format!("next_{index}"),
                        writer,
                    );
                }
                for (index, (dst, _)) in values.iter().enumerate() {
                    emit_release_if_owned(layouts, function, *dst, writer);
                    writer.assign(&variable_name(*dst), &format!("next_{index}"));
                    if layouts.is_managed_value(function.vars[*dst].ty) {
                        writer.assign(&owned_variable_name(*dst), "INT32_C(1)");
                    }
                }
                writer.line(&format!("frame->base.pc = {next}; continue;"));
            }
            Node::Branch(cond, yes, no) => {
                writer.line(&format!(
                    "frame->base.pc = {} ? {yes} : {no}; continue;",
                    operand_expression(cond).map_err(|why| unsupported(&function.name, why))?
                ));
            }
            Node::Switch(on, cases, default) => {
                let SwitchOn::Lit(var) = on else {
                    return Err(unsupported(
                        &function.name,
                        "aggregate switch in suspend function",
                    ));
                };
                writer.begin_block(&format!("switch ({})", variable_name(*var)));
                for (key, target) in cases {
                    let CaseKey::Lit(value) = key else {
                        return Err(invalid_flow_ir(&function.name, "nonliteral scalar switch"));
                    };
                    writer.line(&format!(
                        "case {}: frame->base.pc = {target}; break;",
                        constant_expression(value)
                            .map_err(|why| unsupported(&function.name, why))?
                            .unwrap()
                    ));
                }
                if let Some(target) = default {
                    writer.line(&format!("default: frame->base.pc = {target}; break;"));
                } else {
                    writer.line("default: emela_scheduler_invalid();");
                }
                writer.end_block();
                writer.line("continue;");
            }
            Node::Return(value) => {
                if let Some(value) = value {
                    writer.begin_block("if (frame->result != NULL)");
                    let expression = operand_expression(value)
                        .map_err(|why| unsupported(&function.name, why))?;
                    writer.decl(&layouts.c_type(function.ret), "returned", Some(&expression));
                    retain_frame_value(layouts, function.ret, "returned", writer);
                    writer.line(&format!("*frame->result = returned;"));
                    writer.end_block();
                }
                writer.line("emela_frame_return(fiber); return;");
            }
            Node::Defect(value) => {
                emit_return(
                    layouts,
                    function,
                    &Term::Defect(value.clone()),
                    &strings,
                    writer,
                )?;
            }
            Node::Unreachable => writer.line("emela_scheduler_invalid();"),
        }
        writer.end_block();
    }
    writer.line("default: emela_scheduler_invalid();");
    writer.end_block();
    writer.end_block();
    writer.end_block();
    writer.begin_block(&format!("static void {name}_drop(emela_frame *base)"));
    writer.line(&format!("{name}_frame *frame = ({name}_frame *)base;"));
    writer.line("(void)frame;");
    for (id, _) in function.vars.iter() {
        emit_release_if_owned(layouts, function, id, writer);
    }
    emit_cell_cleanup(layouts, function, writer);
    writer.end_block();
    for (id, _) in function.vars.iter() {
        writer.line(&format!("#undef {}", variable_name(id)));
        writer.line(&format!("#undef {}", owned_variable_name(id)));
        writer.line(&format!("#undef {}", cell_storage_name(id)));
        writer.line(&format!("#undef {}", cell_initialized_name(id)));
    }
    for (name, _) in &strings.declarations {
        writer.line(&format!("#undef {name}"));
    }
    writer.begin_block(&push_signature(layouts, function, name, environment));
    writer.line(&format!(
        "emela_frame *base = emela_frame_push(fiber, sizeof({name}_frame), {name}_step);"
    ));
    writer.line(&format!("{name}_frame *frame = ({name}_frame *)base;"));
    writer.line(&format!("*frame = ({name}_frame){{.base = *base}};"));
    writer.line(&format!("frame->base.pc = {entry};"));
    writer.line(&format!("frame->base.drop = {name}_drop;"));
    if has_result {
        writer.line("frame->result = result;");
    }
    for &id in &function.params {
        writer.line(&format!(
            "frame->local_{} = {};",
            u32::from(id.into_raw()),
            variable_name(id)
        ));
        let field = format!("frame->local_{}", u32::from(id.into_raw()));
        retain_frame_value(layouts, function.vars[id].ty, &field, writer);
        if layouts.is_managed_value(function.vars[id].ty) {
            writer.line(&format!(
                "frame->owned_{} = INT32_C(1);",
                u32::from(id.into_raw())
            ));
        }
    }
    if let Some(environment) = environment {
        writer.line("if (emela_env == NULL) emela_scheduler_invalid();");
        for (index, capture) in layouts.closure_environments[environment]
            .captures
            .iter()
            .enumerate()
        {
            if layouts.type_repr(capture.ty) == Repr::Void {
                continue;
            }
            let id = u32::from(capture.local.into_raw());
            writer.line(&format!(
                "frame->local_{id} = ((emela_closure_env_{environment} *)emela_env)->c{index};"
            ));
            retain_frame_value(layouts, capture.ty, &format!("frame->local_{id}"), writer);
            if layouts.is_managed_value(capture.ty) {
                writer.line(&format!("frame->owned_{id} = INT32_C(1);"));
            }
        }
    }
    writer.end_block();
    if function.exported {
        writer.begin_block(&start_signature(layouts, function, name));
        writer.line("EmelaFiber handle = emela_fiber_create();");
        writer.line("emela_fiber *fiber = emela_fiber_get(handle);");
        let mut args = vec!["fiber".to_owned()];
        args.extend(function.params.iter().map(|id| variable_name(*id)));
        if has_result {
            args.push("result".to_owned());
        }
        writer.line(&format!("{name}_push({});", args.join(", ")));
        writer.line("emela_ready_push(fiber);");
        writer.line("return handle;");
        writer.end_block();
    }
    Ok(())
}

fn retain_frame_value(layouts: &TypeLayouts<'_>, ty: TypeId, value: &str, writer: &mut Writer) {
    if matches!(layouts.types[ty], Type::String) {
        writer.line(&format!("{value} = emela_heap_own_string({value});"));
    } else if layouts.is_managed_value(ty) {
        layouts.emit_retain_value_expression(ty, value, writer);
    }
}

fn freeze_statement_result(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    statement: &Stmt,
    writer: &mut Writer,
) {
    if let Some(dst) = statement.dst()
        && matches!(layouts.types[function.vars[dst].ty], Type::String)
    {
        let value = variable_name(dst);
        writer.begin_block(&format!("if ({value}.owner == NULL)"));
        writer.assign(&value, &format!("emela_heap_copy_string({value})"));
        writer.end_block();
    }
}

fn task_kind(
    layouts: &TypeLayouts<'_>,
    ty: TypeId,
    function: &str,
) -> Result<&'static str, Vec<Error>> {
    match layouts.types[ty] {
        Type::Int | Type::Bool => Ok("INT"),
        Type::Int64 => Ok("INT64"),
        Type::Float => Ok("FLOAT"),
        Type::Unit | Type::Never => Ok("UNIT"),
        _ => Err(unsupported(function, "non-scalar external task result")),
    }
}

fn prepare_result(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    writer: &mut Writer,
) {
    if let Some(dst) = dst {
        emit_release_if_owned(layouts, function, dst, writer);
        if layouts.is_managed_value(function.vars[dst].ty) {
            writer.assign(
                &variable_name(dst),
                &format!("({}){{0}}", layouts.c_type(function.vars[dst].ty)),
            );
            writer.assign(&owned_variable_name(dst), "INT32_C(1)");
        }
    }
}
