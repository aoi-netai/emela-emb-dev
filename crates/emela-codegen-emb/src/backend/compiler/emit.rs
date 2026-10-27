//! Final layout, runtime assembly, and C output for a selected connection.
use super::*;

/// Each connection emits from its own copy, leaving the shared compiled artifact unchanged.
pub(in crate::backend) fn emit(
    compiled: &CompiledModule,
    target: &impl Target,
    options: &Options,
) -> Result<Output, Vec<Error>> {
    if !(1..=512).contains(&options.string_capacity()) {
        return Err(vec![Error {
            kind: ErrorKind::InvalidOptions,
            message: format!(
                "string capacity must be between 1 and 512 bytes, found {}",
                options.string_capacity()
            ),
        }]);
    }
    let CompiledModule {
        lowered: mut lowered_flow,
        closures,
        const_groups,
        runtime_consts,
        deferred_pool,
        ..
    } = compiled.clone();
    if deferred_pool && options.deferred_capacity == 0 {
        return Err(vec![Error {
            kind: ErrorKind::InvalidOptions,
            message: "deferred_capacity must be positive for Deferred operations".into(),
        }]);
    }
    let discarded_results = crate::discarded::normalize(&mut lowered_flow);
    let flow = &lowered_flow;
    let emitted_functions = runtime_reachable(flow);
    let has_scheduler = suspend::validate(flow, &emitted_functions, options)?;
    let has_tasks = suspend::has_tasks(flow, &emitted_functions);
    let task_value_types = task_values::result_types(flow, &emitted_functions);
    let mut layouts = TypeLayouts::new(flow, &emitted_functions, &closures, deferred_pool)?;
    layouts.discarded_results = discarded_results;
    layouts.runtime_consts = runtime_consts.clone();
    let const_bridges = const_bridge::Bridges::new(flow, &const_groups, &closures, &mut layouts)?;
    for &id in &runtime_consts {
        if matches!(layouts.types[flow.functions[id].ret], Type::String) {
            layouts
                .owned_string_returns
                .insert(flow.functions[id].name.clone());
        }
    }
    let external_bindings =
        external::resolve(flow, &emitted_functions, &mut layouts, options, target)?;
    let target_headers = target.headers(flow, &emitted_functions, options)?;
    let derived_plan = derive_runtime::Plan::new(flow, &emitted_functions, &layouts)?;
    task_values::validate_types(flow, &emitted_functions, &layouts)?;
    validate_function_value_types(&layouts, &emitted_functions, flow)?;
    validate_function_refs(flow, &emitted_functions, &layouts)?;
    let mut string_output_functions =
        functions_with_string_output(flow, &emitted_functions, &layouts);
    external::extend_string_output(
        flow,
        &emitted_functions,
        &external_bindings,
        &layouts,
        &mut string_output_functions,
    );
    layouts.owned_aggregate_returns = flow
        .functions
        .iter()
        .filter(|(id, f)| {
            emitted_functions.contains(id)
                && !f.suspends
                && !f.constant
                && f.external.is_none()
                && !matches!(layouts.types[f.ret], Type::String)
                && layouts.has_inline_string(f.ret)
        })
        .filter(|(id, f)| {
            runtime_consts.contains(id)
                || !StringBufferPlan::new(
                    &layouts,
                    *id,
                    f,
                    options.string_capacity(),
                    &string_output_functions,
                    false,
                )
                .declarations
                .is_empty()
        })
        .map(|(_, f)| f.name.clone())
        .collect();
    let empty_list_functions = crate::const_runtime::empty_list_functions(flow);
    let heap_functions: HashSet<_> = flow
        .functions
        .iter()
        .filter_map(|(id, function)| {
            (emitted_functions.contains(&id)
                && (function_needs_heap(
                    flow,
                    id,
                    function,
                    &layouts,
                    &closures,
                    &string_output_functions,
                    &empty_list_functions,
                ) || external::needs_heap(
                    function,
                    external_bindings.get(&id),
                    &layouts,
                    options,
                )))
            .then_some(id)
        })
        .collect();
    let heap_required = !heap_functions.is_empty();
    if options.heap_bytes() == 0 && heap_required {
        let (_, function) = flow
            .functions
            .iter()
            .find(|(id, _)| heap_functions.contains(id))
            .expect("heap requirement came from an emitted function");
        return Err(unsupported(
            &function.name,
            "heap-backed List, recursive value, shared Cell, closure environment, handler, or evidence table with heap_bytes = 0; configure Options::with_heap_bytes",
        ));
    }
    let mut const_outcomes = HashMap::new();
    for (id, function) in flow
        .functions
        .iter()
        .filter(|(id, function)| emitted_functions.contains(id) && function.constant)
    {
        match const_eval::evaluate(flow, id) {
            Ok(outcome) => {
                const_outcomes.insert(id, outcome);
            }
            Err(reason) => {
                let caller = flow.functions.iter().find_map(|(caller_id, caller)| {
                    (emitted_functions.contains(&caller_id)
                        && !caller.constant
                        && directly_calls(caller, id))
                    .then_some(caller)
                });
                let location = caller.map_or(function.name.as_str(), |caller| caller.name.as_str());
                return Err(unsupported(
                    location,
                    &format!(
                        "const `{}` cannot be evaluated statically: {reason}",
                        function.name
                    ),
                ));
            }
        }
    }

    let mut names = HashMap::new();
    let mut public_names = HashMap::new();
    for (id, function) in flow
        .functions
        .iter()
        .filter(|(id, _)| emitted_functions.contains(id))
    {
        let name = if function.external.is_some() {
            if function.exported {
                return Err(unsupported(&function.name, "exported external functions"));
            }
            if function.suspends {
                suspend::external_symbol(function, options, target)?
            } else {
                let binding = &external_bindings[&id];
                if binding.adapted {
                    function_name(id, function)
                } else {
                    binding.symbol.clone()
                }
            }
        } else {
            function_name(id, function)
        };
        public_names.insert(id, name.clone());
        let implementation_name = if function.exported && function.fails && !function.suspends {
            format!("{name}_impl")
        } else {
            name
        };
        names.insert(id, implementation_name);
    }
    let connection = LinkContext {
        flow,
        layouts: &layouts,
        names: &names,
        emitted: &emitted_functions,
        heap_functions: &heap_functions,
        options,
    };
    target.validate(&connection)?;
    let main_entry = flow.functions.iter().find(|(id, function)| {
        emitted_functions.contains(id) && function.exported && function.name == "main"
    });
    if let Some((_, function)) = main_entry
        && !function.params.is_empty()
    {
        return Err(unsupported(
            &function.name,
            "an embedded main function with parameters",
        ));
    }

    let mut header = Writer::new();
    header.line("#ifndef EMELA_EMBEDDED_H");
    header.line("#define EMELA_EMBEDDED_H");
    header.blank();
    header.line("#include <stdint.h>");
    header.blank();
    layouts.emit_string_declaration(&mut header);
    if has_scheduler
        || layouts
            .function_types
            .iter()
            .any(|&ty| matches!(&layouts.types[ty], Type::Fn(ft) if ft.suspends))
    {
        header.line("typedef struct emela_fiber emela_fiber;");
    }
    if layouts.has_error_abi || layouts.any_abi {
        header.line("typedef struct EmelaError EmelaError;");
        if layouts.any_abi {
            header.line("typedef struct EmelaError EmelaAny;");
        }
        header.blank();
    }
    layouts.emit_declarations(&mut header);
    external::emit_header(
        flow,
        &external_bindings,
        &layouts,
        heap_required,
        &mut header,
    );
    if has_scheduler {
        header.line(include_str!("../../runtime/scheduler.h"));
    }
    if has_tasks {
        header.line("#define EMELA_HAS_TASK_RUNTIME 1");
        header.line(include_str!("../../runtime/task.h"));
        if !task_value_types.is_empty() || suspend::has_failing_tasks(flow, &emitted_functions) {
            header.line(include_str!("../../runtime/task_context.h"));
        }
        if suspend::has_failing_tasks(flow, &emitted_functions) {
            header.line(include_str!("../../runtime/task_failure.h"));
        }
        task_values::emit_header(&layouts, &task_value_types, &mut header);
        for (id, function) in flow
            .functions
            .iter()
            .filter(|(id, f)| emitted_functions.contains(id) && f.suspends && f.external.is_some())
        {
            if !suspend::scalar(&layouts.types, function.ret) {
                header.line(&format!("/* Success: emela_task_complete_value_{}. Arguments are borrowed only during start. */", function.ret.index()));
            }
            header.line(&format!(
                "{};",
                suspend::task_signature(&layouts, function, &names[&id])
            ));
        }
    }
    if !layouts.aggregates.is_empty() {
        header.blank();
    }

    for (id, function) in flow
        .functions
        .iter()
        .filter(|(id, f)| emitted_functions.contains(id) && f.exported)
    {
        if function.suspends {
            header.line(&format!(
                "{};",
                suspend::start_signature(&layouts, function, &public_names[&id])
            ));
            continue;
        }
        let name = &public_names[&id];
        let takes_string_output = string_output_functions.contains(&id);
        if takes_string_output {
            header.line("/* Caller provides this output buffer; the returned EmelaString points into it. */");
        }
        header.line(&format!(
            "{};",
            signature(&layouts, function, name, false, false, takes_string_output)
        ));
    }
    if main_entry.is_some() {
        header.line(target.entry_declaration());
    }
    if heap_required {
        header.line("/* Retains a value allocated from the fixed heap. */");
        header.line("void emela_embedded_retain(void *value);");
        header.line("/* Releases a value allocated from the fixed heap. */");
        header.line("void emela_embedded_release(void *value);");
        header.line("/* Retains the backing owner of an EmelaString value. */");
        header.line("void emela_embedded_retain_string(const EmelaString *value);");
        header.line("/* Releases the backing owner of an EmelaString value. */");
        header.line("void emela_embedded_release_string(EmelaString *value);");
        layouts.emit_public_release_declarations(&mut header);
        layouts.emit_public_function_ownership_declarations(&mut header);
    }

    header.blank();
    header.line("#endif");

    let mut runtime = runtime_plan::Requirements::collect(flow, &emitted_functions);
    derived_plan.require_runtime(&layouts, &mut runtime);
    if !string_output_functions.is_empty() {
        runtime.require(runtime_plan::Helper::StringOutput);
        runtime.defects = true;
    }
    runtime.defects |= !runtime_consts.is_empty()
        || has_scheduler
        || heap_required
        || external_bindings
            .iter()
            .any(|(&id, binding)| binding.adapted && flow.functions[id].fails)
        || external_bindings
            .values()
            .any(|binding| binding.result == crate::ExternalResult::StringBuffer)
        || const_outcomes.values().any(|outcome| {
            matches!(
                outcome,
                ConstOutcome::Defect { .. } | ConstOutcome::SelfReference { .. }
            )
        });
    let platform_header = platform::header(platform::Requirements {
        defects: runtime.defects,
        const_defects: !runtime_consts.is_empty(),
        lifecycle: main_entry.is_some(),
        debug: runtime.debug,
        event_lock: has_tasks || target.needs_event_lock(&target_headers),
    });

    let mut source = Writer::new();
    source.line(&format!("#include \"{HEADER_FILE}\""));
    source.line(&format!("#include \"{}\"", platform_header.path));
    source.line("#include <math.h>");
    source.line("#include <stddef.h>");
    for file in &target_headers {
        source.line(&format!("#include \"{}\"", file.path));
    }
    source.blank();
    if !runtime_consts.is_empty() {
        crate::const_runtime::emit_runtime(flow, &runtime_consts, options, &mut source)?;
    }
    if has_scheduler {
        if !task_value_types.is_empty() || suspend::has_failing_tasks(flow, &emitted_functions) {
            source.line("#define EMELA_TASK_CONTEXT_RUNTIME 1");
        }
        if !task_value_types.is_empty() {
            task_values::emit_union(&task_value_types, &mut source);
        }
        if suspend::has_failing_tasks(flow, &emitted_functions) {
            source.line("#define EMELA_TASK_FAILURE_RUNTIME 1");
        }
        suspend::emit_runtime(
            &mut source,
            options,
            has_tasks,
            layouts.has_resumes,
            layouts.has_unwind,
            layouts.has_scopes,
            layouts.deferred_pool,
            layouts.has_error_abi,
        );
    }
    if layouts.has_scopes {
        if layouts.has_child_waits() {
            source.line("#define EMELA_CHILD_WAIT_RUNTIME");
        }
        source.line(include_str!("../../runtime/scope_types.c"));
    }
    if layouts.deferred_pool {
        source.line(include_str!("../../runtime/deferred_types.c"));
    }
    cause::emit_types(&layouts, &mut source);
    error_runtime::emit_types(&layouts, &mut source);
    emit_closure_environment_declarations(&layouts, &closures, &mut source);
    layouts.emit_runtime_value_declarations(&mut source);
    if heap_required {
        layouts.emit_list_node_declarations(&mut source);
        crate::heap::emit_runtime(&mut source, options.heap_bytes(), layouts.has_cell_cycles());
        emit_closure_string_copy_helper(&layouts, &mut source);
        error_runtime::emit_ownership_declarations(&layouts, &mut source);
        emit_any_ownership_declarations(&layouts, &mut source);
        layouts.emit_heap_ownership_helpers(&mut source);
        owned::emit_helpers(&layouts, &mut source);
        error_runtime::emit_ownership(&layouts, true, &mut source);
        emit_any_ownership(&layouts, true, &mut source);
        if layouts.deferred_pool {
            source.line(&format!(
                "#define EMELA_DEFERRED_KIND UINT32_C({})",
                layouts.deferred_kind()
            ));
            source.line(include_str!("../../runtime/deferred.c"));
        }
        layouts.emit_heap_destroy_switch(&closures.environments, &mut source);
        layouts.emit_public_release_helpers(&mut source);
        layouts.emit_public_function_ownership_helpers(&mut source);
        crate::heap::emit_public_retain(&mut source);
        crate::heap::emit_public_release(&mut source);
        emit_public_string_ownership(&mut source);
        external::emit_ownership_helpers(flow, &external_bindings, &layouts, &mut source);
    } else {
        source.line("static inline void emela_heap_retain(void *value) { (void)value; }");
        source.line("static inline void emela_heap_release(void *value) { (void)value; }");
        error_runtime::emit_ownership(&layouts, false, &mut source);
        emit_any_ownership(&layouts, false, &mut source);
        layouts.emit_noheap_ownership_stubs(&mut source);
        source.blank();
    }
    if suspend::has_failing_tasks(flow, &emitted_functions) {
        suspend::emit_task_failure_ownership(&layouts, &mut source);
    }
    task_values::emit_dispatch(&layouts, &task_value_types, &mut source);
    cause::emit_runtime(&layouts, &mut source);
    if layouts.has_resumes {
        suspend::emit_resume_runtime(&layouts, &mut source);
    }
    if layouts.has_unwind {
        source.line(include_str!("../../runtime/unwind.c"));
    }
    if layouts.has_scopes {
        if layouts.has_child_waits() {
            source.line(include_str!("../../runtime/child_wait.c"));
            child::emit_runtime(&layouts, &mut source)?;
        }
        suspend::emit_scope_runtime(&layouts, &mut source)?;
    }
    cause::emit_attempts(&layouts, &mut source)?;
    cause::emit_infallible_raise(&layouts, &mut source);
    if layouts.deferred_pool {
        deferred::emit_runtime(&layouts, &mut source)?;
    }
    runtime.emit_helpers(&mut source);
    derived_plan.emit(&layouts, &mut source);

    for (id, function) in flow
        .functions
        .iter()
        .filter(|(id, _)| emitted_functions.contains(id))
    {
        let name = &names[&id];
        let takes_string_output = string_output_functions.contains(&id);
        if function.suspends {
            suspend::emit_declarations(
                &layouts,
                function,
                name,
                closures.functions.get(&id).copied(),
                &mut source,
            )?;
        } else if function.external.is_some() {
            let prefix = if external_bindings[&id].adapted {
                ""
            } else {
                "extern "
            };
            source.line(&format!(
                "{prefix}{};",
                signature(
                    &layouts,
                    function,
                    name,
                    external_bindings[&id].adapted,
                    function.fails && external_bindings[&id].adapted,
                    string_output_functions.contains(&id)
                )
            ));
        } else if !function.exported || function.fails {
            source.line(&format!(
                "{};",
                if closures.functions.contains_key(&id) {
                    closure_signature(&layouts, function, name, true)
                } else {
                    signature(
                        &layouts,
                        function,
                        name,
                        true,
                        function.fails,
                        takes_string_output,
                    )
                }
            ));
        }
    }
    target.emit_declarations(&target_headers, &mut source);
    let function_ref_adapters = function_ref_adapters(flow, &emitted_functions, &layouts);
    for &(target, function_type) in &function_ref_adapters {
        source.line(&format!(
            "{};",
            function_ref_adapter_signature(
                &layouts,
                function_type,
                &function_ref_adapter_name(target, function_type),
            )
        ));
    }
    for (index, adapter) in layouts.function_adapters.iter().enumerate() {
        source.line(&format!(
            "{};",
            function_ref_adapter_signature(
                &layouts,
                adapter.destination,
                &function_coerce_adapter_name(index),
            )
        ));
    }
    source.blank();
    const_bridges.emit(flow, &layouts, &names, heap_required, &mut source)?;
    error_runtime::emit_display(
        &layouts,
        &names,
        &string_output_functions,
        options.string_capacity(),
        &mut source,
    )?;

    for (id, function) in flow
        .functions
        .iter()
        .filter(|(id, _)| emitted_functions.contains(id))
    {
        if function.suspends {
            suspend::emit_function(
                flow,
                &layouts,
                id,
                function,
                &names[&id],
                &names,
                &string_output_functions,
                closures.functions.get(&id).copied(),
                options,
                &mut source,
            )?;
            continue;
        }

        if function.external.is_some() {
            if function.body.is_some() {
                return Err(vec![Error {
                    kind: ErrorKind::InvalidFlowIr,
                    message: format!(
                        "invalid Flow IR in function `{}`: external function has a body",
                        function.name
                    ),
                }]);
            }
            if crate::derived::kind(function).is_some() {
                derive_runtime::emit_adapter(&layouts, function, &names[&id], &mut source);
                continue;
            }
            if target.emit_adapter(&connection, function, &names[&id], &mut source)? {
                continue;
            }
            external::emit_adapter(
                &layouts,
                function,
                &external_bindings[&id],
                &names[&id],
                &mut source,
            );
            continue;
        }
        let Some(body) = function.body else {
            return Err(unsupported(&function.name, "function without a body"));
        };
        let sequence = &function.seqs[body];
        if const_bridges.getters.contains_key(&id) {
            const_bridges.emit_getter(id, flow, &layouts, &names, &mut source);
            continue;
        }
        let initializer_name = format!("{}_const_init", names[&id]);
        let is_runtime_const = runtime_consts.contains(&id);
        let name = if is_runtime_const {
            &initializer_name
        } else {
            &names[&id]
        };
        let is_static = is_runtime_const || !function.exported || function.fails;
        let takes_string_output = string_output_functions.contains(&id);
        let function_signature = if closures.functions.contains_key(&id) {
            closure_signature(&layouts, function, name, is_static)
        } else {
            signature(
                &layouts,
                function,
                name,
                is_static,
                function.fails,
                takes_string_output,
            )
        };
        source.begin_block(&function_signature);
        if function.constant {
            match &const_outcomes[&id] {
                ConstOutcome::Value(emela_lir::Const::Unit) => source.ret(None),
                ConstOutcome::Value(value) => {
                    let expression = constant_expression(value).map_err(|kind| {
                        unsupported(&function.name, &format!("{kind} in a const value"))
                    })?;
                    let Some(expression) = expression else {
                        return Err(invalid_flow_ir(
                            &function.name,
                            "non-Unit const value has no C expression",
                        ));
                    };
                    source.ret(Some(&expression));
                }
                ConstOutcome::Defect { kind, message } => {
                    let length = u16::try_from(message.len()).map_err(|_| {
                        unsupported(
                            &function.name,
                            "a const defect message longer than the embedded C ABI supports",
                        )
                    })?;
                    source.line(&format!(
                        "emela_board_defect(EMELA_DEFECT_{kind}, {}, UINT16_C({length}));",
                        c_string_literal(message.as_bytes())
                    ));
                }
                ConstOutcome::SelfReference { message } => {
                    let length = u16::try_from(message.len()).map_err(|_| {
                        unsupported(
                            &function.name,
                            "a const defect message longer than the embedded C ABI supports",
                        )
                    })?;
                    source.line(&format!(
                        "{}(EMELA_DEFECT_PANIC, {}, UINT16_C({length}));",
                        if runtime_consts.is_empty() {
                            "emela_board_defect"
                        } else {
                            "emela_const_self_reference"
                        },
                        c_string_literal(message.as_bytes())
                    ));
                }
            }
            source.end_block();
            source.blank();
            continue;
        }
        if layouts.has_cause_raise && layouts.has_unwind {
            source.line("unsigned emela_entry_holds = emela_current_fiber == NULL ? 0 : emela_current_fiber->holds;");
            source.line("(void)emela_entry_holds;");
        }
        if function.fails {
            source.line("*emela_error_out = (EmelaError){0};");
        }
        if takes_string_output {
            source.decl(
                "uint16_t",
                "emela_emb_string_output_capacity",
                Some(&format!(
                    "emela_string_capacity < UINT16_C({}) ? emela_string_capacity : UINT16_C({})",
                    options.string_capacity(),
                    options.string_capacity()
                )),
            );
        }
        let string_buffers = StringBufferPlan::new(
            &layouts,
            id,
            function,
            options.string_capacity(),
            &string_output_functions,
            takes_string_output,
        );
        if !string_buffers.declarations.is_empty()
            && layouts.has_inline_string(function.ret)
            && !layouts.owned_string_returns.contains(&function.name)
            && !layouts.owned_aggregate_returns.contains(&function.name)
            && !takes_string_output
        {
            return Err(unsupported(
                &function.name,
                "a String scratch buffer that could outlive its function stack frame",
            ));
        }
        emit_local_declarations(&layouts, function, &string_buffers, &mut source);
        if let Some(&environment) = layouts.closure_functions.get(&id) {
            emit_closure_capture_loads(&layouts, function, environment, &mut source);
        }
        let mut loops = Vec::new();
        let mut blocks = Vec::new();
        let mut protects = Vec::new();
        let mut next_temp = 0;
        let mut next_label = 0;
        for &statement in &sequence.stmts {
            emit_statement(
                &layouts,
                function,
                &function.stmts[statement],
                &mut source,
                &names,
                &string_buffers,
                &mut loops,
                &mut blocks,
                &mut protects,
                &mut next_temp,
                &mut next_label,
            )?;
        }
        emit_return(
            &layouts,
            function,
            &sequence.term,
            &string_buffers,
            &mut source,
        )?;
        source.end_block();
        source.blank();
        if is_runtime_const {
            emit_const_getter(
                &layouts,
                id,
                function,
                &names[&id],
                &initializer_name,
                &mut source,
            );
        }
    }
    for &(target, function_type) in &function_ref_adapters {
        let target_function = &flow.functions[target];
        let takes_output = string_output_functions.contains(&target);
        let adapter_capacity = if takes_output {
            let plan = StringBufferPlan::new(
                &layouts,
                target,
                target_function,
                options.string_capacity(),
                &string_output_functions,
                true,
            );
            target_function
                .seqs
                .iter()
                .filter_map(|(_, seq)| match &seq.term {
                    Term::Return(Some(Operand::Var(var))) => Some(
                        plan.buffers
                            .get(var)
                            .map_or(options.string_capacity(), |(_, size)| *size),
                    ),
                    Term::Return(Some(Operand::Const(Const::String(value)))) => Some(
                        u16::try_from(value.len())
                            .unwrap_or(u16::MAX)
                            .min(options.string_capacity()),
                    ),
                    _ => None,
                })
                .max()
                .unwrap_or(options.string_capacity())
                .max(1)
        } else {
            options.string_capacity()
        };
        emit_function_ref_adapter(
            &layouts,
            function_type,
            &names[&target],
            &function_ref_adapter_name(target, function_type),
            takes_output,
            adapter_capacity,
            &mut source,
        );
        source.blank();
    }
    for (index, adapter) in layouts.function_adapters.iter().enumerate() {
        emit_function_coerce_adapter(&layouts, adapter, index, &mut source);
        source.blank();
    }
    for (id, function) in flow
        .functions
        .iter()
        .filter(|(id, f)| emitted_functions.contains(id) && f.exported && f.fails && !f.suspends)
    {
        emit_fallible_wrapper(
            &layouts,
            function,
            &public_names[&id],
            &names[&id],
            string_output_functions.contains(&id),
            &mut source,
        );
        source.blank();
    }
    if let Some((id, function)) = main_entry {
        target.emit_entry(
            &connection,
            function,
            &public_names[&id],
            string_output_functions.contains(&id),
            &mut source,
        );
    }

    target.emit_helpers(&connection, &target_headers, &mut source);
    let mut output = Output {
        exports: flow
            .functions
            .iter()
            .filter(|(id, function)| emitted_functions.contains(id) && function.exported)
            .map(|(id, function)| ExportedFunction {
                source_name: function.name.clone(),
                c_symbol: if function.suspends {
                    format!("{}_start", public_names[&id])
                } else {
                    public_names[&id].clone()
                },
                suspends: function.suspends,
            })
            .collect(),
        files: vec![
            GeneratedFile {
                path: HEADER_FILE.to_owned(),
                contents: header.finish(),
            },
            platform_header,
            GeneratedFile {
                path: SOURCE_FILE.to_owned(),
                contents: source.finish(),
            },
        ]
        .into_iter()
        .chain(target_headers)
        .collect(),
    };
    output
        .refresh_manifest()
        .map_err(|error| vec![Error::from(error)])?;
    Ok(output)
}
