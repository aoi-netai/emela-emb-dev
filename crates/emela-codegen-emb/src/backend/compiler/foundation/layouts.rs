use super::*;
#[derive(Debug)]
pub(in crate::backend) struct AggregateLayout {
    pub(in crate::backend) ty: TypeId,
    pub(in crate::backend) kind: AggregateKind,
}

#[derive(Debug)]
pub(in crate::backend) enum AggregateKind {
    Tuple(Vec<TypeId>),
    Enum(Vec<Vec<TypeId>>),
}

pub(in crate::backend) struct TypeLayouts<'a> {
    pub(in crate::backend) discarded_results: crate::discarded::Results,
    pub(in crate::backend) core: &'a emela_core::Module,
    pub(in crate::backend) types: TypeTable,
    pub(in crate::backend) aggregates: Vec<AggregateLayout>,
    pub(in crate::backend) aggregate_ids: HashMap<TypeId, usize>,
    pub(in crate::backend) recursive_edges: HashSet<(TypeId, usize, usize)>,
    pub(in crate::backend) expanding_enums: HashSet<EnumId>,
    pub(in crate::backend) recursive_box_types: Vec<TypeId>,
    pub(in crate::backend) recursive_box_ids: HashMap<TypeId, usize>,
    pub(in crate::backend) list_types: Vec<TypeId>,
    pub(in crate::backend) list_ids: HashMap<TypeId, usize>,
    pub(in crate::backend) function_types: Vec<TypeId>,
    pub(in crate::backend) function_results: HashMap<FnId, TypeId>,
    pub(in crate::backend) closure_function_types: HashSet<TypeId>,
    pub(in crate::backend) closure_functions: HashMap<FnId, usize>,
    pub(in crate::backend) closure_environments: Vec<features::values::closure::ClosureEnvironment>,
    pub(in crate::backend) owned_string_returns: HashSet<String>,
    pub(in crate::backend) runtime_consts: HashSet<FnId>,
    pub(in crate::backend) owned_aggregate_returns: HashSet<String>,
    pub(in crate::backend) cell_types: Vec<TypeId>,
    pub(in crate::backend) cell_ids: HashMap<TypeId, usize>,
    pub(in crate::backend) function_adapters: Vec<FunctionAdapterLayout>,
    pub(in crate::backend) function_adapter_ids: HashMap<(TypeId, TypeId), usize>,
    pub(in crate::backend) handler_layouts: Vec<HandlerLayout>,
    pub(in crate::backend) handler_layout_ids: HashMap<EffectId, usize>,
    pub(in crate::backend) resume_types: Vec<TypeId>,
    pub(in crate::backend) has_resumes: bool,
    pub(in crate::backend) has_unwind: bool,
    pub(in crate::backend) has_scopes: bool,
    pub(in crate::backend) deferred_pool: bool,
    pub(in crate::backend) deferred_complete_types: Vec<TypeId>,
    pub(in crate::backend) deferred_read_types: Vec<TypeId>,
    pub(in crate::backend) deferred_poll_types: Vec<TypeId>,
    pub(in crate::backend) deferred_await_types: Vec<TypeId>,
    pub(in crate::backend) child_join_types: Vec<TypeId>,
    pub(in crate::backend) child_await_types: Vec<TypeId>,
    pub(in crate::backend) child_interrupts: bool,
    pub(in crate::backend) has_causes: bool,
    pub(in crate::backend) has_cause_raise: bool,
    pub(in crate::backend) attempt_types: Vec<(TypeId, TypeId)>,
    pub(in crate::backend) fork_types: Vec<TypeId>,
    pub(in crate::backend) evidence_nodes_required: bool,
    pub(in crate::backend) any_abi: bool,
    pub(in crate::backend) any_box_required: bool,
    pub(in crate::backend) any_box_types: Vec<TypeId>,
    pub(in crate::backend) any_box_ids: HashMap<TypeId, usize>,
    pub(in crate::backend) error_types: Vec<(EnumId, TypeId)>,
    pub(in crate::backend) error_shows: Vec<(EnumId, FnId)>,
    pub(in crate::backend) has_error_abi: bool,
    pub(in crate::backend) managed_error_payloads: bool,
}

pub(in crate::backend) struct HandlerLayout {
    pub(in crate::backend) effect: EffectId,
    pub(in crate::backend) operations: Vec<TypeId>,
}

pub(in crate::backend) struct FunctionAdapterLayout {
    pub(in crate::backend) source: TypeId,
    pub(in crate::backend) destination: TypeId,
}

impl<'a> TypeLayouts<'a> {
    pub(in crate::backend) fn new(
        flow: &'a emela_lir::Module,
        emitted: &HashSet<FnId>,
        closures: &features::values::closure::ClosureLowering,
        deferred_pool: bool,
    ) -> Result<Self, Vec<Error>> {
        let mut layouts = TypeLayouts {
            discarded_results: HashMap::new(),
            core: &flow.core,
            types: flow.core.types.clone(),
            aggregates: Vec::new(),
            aggregate_ids: HashMap::new(),
            recursive_edges: HashSet::new(),
            expanding_enums: crate::type_expansion::expanding_enums(&flow.core),
            recursive_box_types: Vec::new(),
            recursive_box_ids: HashMap::new(),
            list_types: Vec::new(),
            list_ids: HashMap::new(),
            function_types: Vec::new(),
            function_results: flow.functions.iter().map(|(id, f)| (id, f.ret)).collect(),
            closure_function_types: closures
                .environments
                .iter()
                .map(|environment| environment.function_type)
                .collect(),
            closure_functions: closures.functions.clone(),
            closure_environments: closures.environments.clone(),
            owned_aggregate_returns: HashSet::new(),
            runtime_consts: HashSet::new(),
            owned_string_returns: closures
                .functions
                .keys()
                .filter_map(|id| {
                    let f = &flow.functions[*id];
                    (!f.suspends && matches!(flow.core.types[f.ret], Type::String))
                        .then(|| f.name.clone())
                })
                .collect(),
            cell_types: Vec::new(),
            cell_ids: HashMap::new(),
            function_adapters: Vec::new(),
            function_adapter_ids: HashMap::new(),
            handler_layouts: Vec::new(),
            handler_layout_ids: HashMap::new(),
            resume_types: Vec::new(),
            has_resumes: suspend::has_operations(flow, emitted)
                || fiber_normalize::has_scopes(flow, emitted),
            has_unwind: suspend::has_operations(flow, emitted)
                || fiber_normalize::has_scopes(flow, emitted)
                || suspend::has_protects(flow, emitted)
                || (flow
                    .functions
                    .iter()
                    .any(|(id, f)| emitted.contains(&id) && f.suspends)
                    && flow.functions.iter().any(|(id, f)| {
                        emitted.contains(&id)
                            && f.stmts.iter().any(|(_, s)| {
                                matches!(
                                    s,
                                    Stmt::Let {
                                        value: Value::Fiber {
                                            op: emela_core::FiberOp::CauseRaise,
                                            ..
                                        },
                                        ..
                                    }
                                )
                            })
                    })),
            has_scopes: fiber_normalize::has_scopes(flow, emitted),
            deferred_pool,
            deferred_complete_types: Vec::new(),
            deferred_read_types: Vec::new(),
            deferred_poll_types: Vec::new(),
            deferred_await_types: Vec::new(),
            child_join_types: Vec::new(),
            child_await_types: Vec::new(),
            child_interrupts: false,
            has_causes: flow.functions.iter().any(|(id, f)| {
                emitted.contains(&id)
                    && f.stmts.iter().any(|(_, s)| {
                        matches!(
                            s,
                            Stmt::Let {
                                value: Value::Fiber {
                                    op: emela_core::FiberOp::Attempt { .. }
                                        | emela_core::FiberOp::Await { .. }
                                        | emela_core::FiberOp::CauseKind { .. }
                                        | emela_core::FiberOp::CauseToString
                                        | emela_core::FiberOp::CauseRaise,
                                    ..
                                },
                                ..
                            }
                        )
                    })
            }),
            has_cause_raise: flow.functions.iter().any(|(id, f)| {
                emitted.contains(&id)
                    && f.stmts.iter().any(|(_, s)| {
                        matches!(
                            s,
                            Stmt::Let {
                                value: Value::Fiber {
                                    op: emela_core::FiberOp::CauseRaise,
                                    ..
                                },
                                ..
                            }
                        )
                    })
            }),
            attempt_types: Vec::new(),
            fork_types: Vec::new(),
            evidence_nodes_required: false,
            any_abi: false,
            any_box_required: false,
            any_box_types: Vec::new(),
            any_box_ids: HashMap::new(),
            error_types: Vec::new(),
            error_shows: flow.error_shows.clone(),
            managed_error_payloads: false,
            has_error_abi: flow.functions.iter().any(|(id, function)| {
                emitted.contains(&id)
                    && (function.fails
                        || function
                            .seqs
                            .iter()
                            .any(|(_, sequence)| matches!(sequence.term, Term::Fail(_)))
                        || function.stmts.iter().any(|(_, statement)| match statement {
                            Stmt::Let { value, .. } => match value {
                                Value::Call { on_fail, .. } | Value::Fiber { on_fail, .. } => {
                                    on_fail.is_some()
                                }
                                Value::IsError { .. } | Value::AsError { .. } => true,
                                _ => false,
                            },
                            _ => false,
                        }))
            }),
        };
        let mut validated = HashSet::new();
        for (id, function) in flow.functions.iter() {
            if !emitted.contains(&id) {
                continue;
            }
            for ty in std::iter::once(function.ret).chain(function.vars.iter().map(|(_, v)| v.ty)) {
                layouts.validate_finite_types(
                    ty,
                    &function.name,
                    &mut HashSet::new(),
                    &mut validated,
                )?;
            }
        }
        let mut states = HashMap::new();
        for (id, function) in flow.functions.iter() {
            if !emitted.contains(&id) {
                continue;
            }
            layouts.collect(function.ret, &function.name, &mut states)?;
            for (_, variable) in function.vars.iter() {
                layouts.collect(variable.ty, &function.name, &mut states)?;
            }
            for (_, statement) in function.stmts.iter() {
                let Stmt::Let { dst, value, .. } = statement else {
                    continue;
                };
                match value {
                    Value::Builtin {
                        op: Builtin::CellNew,
                        args,
                    } => {
                        if let Some(content) =
                            args.first().and_then(|arg| operand_type(function, arg))
                        {
                            let cell = layouts.types.intern(Type::Cell(content));
                            layouts.collect(cell, &function.name, &mut states)?;
                        }
                    }
                    Value::Fiber {
                        op: emela_core::FiberOp::Attempt { exit, .. },
                        args,
                        ..
                    } => {
                        let [Operand::Var(body)] = args.as_slice() else {
                            return Err(invalid_flow_ir(&function.name, "Attempt body"));
                        };
                        let body_ty = function.vars[*body].ty;
                        let Type::Fn(ft) = layouts.types[body_ty].clone() else {
                            return Err(invalid_flow_ir(&function.name, "Attempt function type"));
                        };
                        if ft.ev || !ft.params.is_empty() {
                            return Err(unsupported(&function.name, "Attempt body ABI"));
                        }
                        let mut params = vec![TypeId::UNIT; layouts.core.enums[*exit].params];
                        if params.is_empty() {
                            return Err(invalid_flow_ir(
                                &function.name,
                                "Attempt Exit type parameters",
                            ));
                        }
                        params[0] = ft.ret;
                        let result = dst
                            .map(|v| function.vars[v].ty)
                            .unwrap_or_else(|| layouts.types.enum_type(*exit, params));
                        layouts.collect(result, &function.name, &mut states)?;
                        if child::success_type(&layouts, result) != Some(ft.ret) {
                            return Err(invalid_flow_ir(&function.name, "Attempt result layout"));
                        }
                        if !layouts.attempt_types.contains(&(body_ty, result)) {
                            layouts.attempt_types.push((body_ty, result));
                        }
                    }
                    Value::Fiber {
                        op: emela_core::FiberOp::Interrupt,
                        args,
                        ..
                    } => {
                        let [handle] = args.as_slice() else {
                            return Err(invalid_flow_ir(&function.name, "Interrupt arity"));
                        };
                        child::value_type(&layouts, function, handle)?;
                        layouts.child_interrupts = true;
                    }
                    Value::Fiber {
                        op: emela_core::FiberOp::Await { exit },
                        args,
                        ..
                    } => {
                        let [handle] = args.as_slice() else {
                            return Err(invalid_flow_ir(&function.name, "Await arity"));
                        };
                        let ty = child::value_type(&layouts, function, handle)?;
                        let mut params = vec![TypeId::UNIT; layouts.core.enums[*exit].params];
                        let Some(first) = params.first_mut() else {
                            return Err(invalid_flow_ir(
                                &function.name,
                                "Await Exit type parameters",
                            ));
                        };
                        *first = ty;
                        let result = layouts.types.enum_type(*exit, params);
                        if dst.is_some_and(|dst| {
                            layouts.types.erase_sets(function.vars[dst].ty)
                                != layouts.types.erase_sets(result)
                        }) {
                            return Err(invalid_flow_ir(&function.name, "Await result type"));
                        }
                        layouts.collect(result, &function.name, &mut states)?;
                        if child::success_type(&layouts, result) != Some(ty) {
                            return Err(invalid_flow_ir(&function.name, "Await Success payload"));
                        }
                        layouts.add_any_box_type(ty);
                        if !layouts.child_await_types.contains(&result) {
                            layouts.child_await_types.push(result);
                        }
                    }
                    Value::Fiber {
                        op: emela_core::FiberOp::Join,
                        args,
                        ..
                    } => {
                        let [handle] = args.as_slice() else {
                            return Err(invalid_flow_ir(&function.name, "Join arity"));
                        };
                        let ty = child::value_type(&layouts, function, handle)?;
                        if dst.is_some_and(|dst| function.vars[dst].ty != ty) {
                            return Err(invalid_flow_ir(&function.name, "Join result type"));
                        }
                        layouts.add_any_box_type(ty);
                        if !layouts.child_join_types.contains(&ty) {
                            layouts.child_join_types.push(ty);
                        }
                    }
                    Value::Fiber {
                        op: emela_core::FiberOp::DeferredAwait,
                        args,
                        ..
                    } => {
                        let ty = deferred::value_type(&layouts, function, &args[0])?;
                        layouts.add_any_box_type(ty);
                        if !layouts.deferred_read_types.contains(&ty) {
                            layouts.deferred_read_types.push(ty);
                        }
                        if !layouts.deferred_await_types.contains(&ty) {
                            layouts.deferred_await_types.push(ty);
                        }
                    }
                    Value::Fiber {
                        op: emela_core::FiberOp::Poll,
                        args,
                        ..
                    } => {
                        let ty = deferred::value_type(&layouts, function, &args[0])?;
                        layouts.add_any_box_type(ty);
                        if !layouts.deferred_read_types.contains(&ty) {
                            layouts.deferred_read_types.push(ty);
                        }
                        if let Some(dst) = dst {
                            let option = function.vars[*dst].ty;
                            if option_payload_type(&layouts, option) != Some(ty) {
                                return Err(invalid_flow_ir(&function.name, "Poll result type"));
                            }
                            if !layouts.deferred_poll_types.contains(&option) {
                                layouts.deferred_poll_types.push(option);
                            }
                        }
                    }
                    Value::Fiber {
                        op: emela_core::FiberOp::Complete,
                        args,
                        ..
                    } => {
                        let ty = deferred::value_type(&layouts, function, &args[0])?;
                        layouts.add_any_box_type(ty);
                        if !layouts.deferred_complete_types.contains(&ty) {
                            layouts.deferred_complete_types.push(ty);
                        }
                    }
                    Value::Fiber {
                        op: emela_core::FiberOp::Fork,
                        args,
                        ..
                    } => {
                        if let Some(Operand::Var(body)) = args.get(1) {
                            let ty = function.vars[*body].ty;
                            if !layouts.fork_types.contains(&ty) {
                                layouts.fork_types.push(ty);
                            }
                        }
                        layouts.any_abi = true;
                        layouts.any_box_required = true;
                    }
                    Value::Fiber {
                        op: emela_core::FiberOp::Deferred,
                        ..
                    } => {
                        layouts.any_abi = true;
                        layouts.any_box_required = true;
                    }
                    Value::Handler { effect, ops, .. } => {
                        let operations = ops
                            .iter()
                            .map(|&operation| function.vars[operation].ty)
                            .collect::<Vec<_>>();
                        for (index, &ty) in operations.iter().enumerate() {
                            if !flow.core.effects[*effect].ops[index].suspend {
                                continue;
                            }
                            let Type::Fn(ft) = &layouts.types[ty] else {
                                unreachable!()
                            };
                            let resume = *ft
                                .params
                                .last()
                                .expect("suspend handler has a resume parameter");
                            if !layouts.resume_types.contains(&resume) {
                                layouts.resume_types.push(resume);
                            }
                            layouts.closure_function_types.insert(resume);
                        }
                        if let Some(previous) = layouts
                            .handler_layout_ids
                            .get(effect)
                            .and_then(|index| layouts.handler_layouts.get(*index))
                        {
                            if previous.operations != operations {
                                return Err(invalid_flow_ir(
                                    &function.name,
                                    "handlers for one effect have inconsistent operation function types",
                                ));
                            }
                        } else {
                            let index = layouts.handler_layouts.len();
                            layouts.handler_layouts.push(HandlerLayout {
                                effect: *effect,
                                operations,
                            });
                            layouts.handler_layout_ids.insert(*effect, index);
                        }
                    }
                    Value::EvExtend { bindings, .. } if !bindings.is_empty() => {
                        layouts.evidence_nodes_required = true;
                    }
                    Value::EvLookup { .. } => {
                        layouts.evidence_nodes_required = true;
                    }
                    Value::Coerce(source) => {
                        if let Some(destination) = dst {
                            let source_ty = function.vars[*source].ty;
                            let destination_ty = function.vars[*destination].ty;
                            if matches!(layouts.types[source_ty], Type::Any)
                                && !matches!(layouts.types[destination_ty], Type::Any)
                            {
                                layouts.add_any_box_type(destination_ty);
                            } else if matches!(layouts.types[destination_ty], Type::Any)
                                && !matches!(layouts.types[source_ty], Type::Any)
                            {
                                layouts.add_any_box_type(source_ty);
                            }
                            if matches!(layouts.types[source_ty], Type::Fn(_))
                                && matches!(layouts.types[destination_ty], Type::Fn(_))
                            {
                                layouts.add_function_adapter(
                                    source_ty,
                                    destination_ty,
                                    &function.name,
                                )?;
                            }
                        }
                    }
                    Value::Call {
                        callee: Callee::Op { effect, op, .. },
                        args,
                        ..
                    } if flow.core.effects[*effect].ops[*op].type_params > 0 => {
                        for argument in args {
                            if let Some(ty) = operand_type(function, argument) {
                                layouts.add_any_box_type(ty);
                            }
                        }
                        if let Some(destination) = dst {
                            layouts.add_any_box_type(function.vars[*destination].ty);
                        }
                    }
                    _ => {}
                }
            }
        }
        // Signature edges do not embed their parameter/result layouts in a function record.
        // Collect them after the current inline aggregate has finished, so a callback returning
        // a tuple containing its enclosing enum does not create an artificial recursive box.
        let mut function_index = 0;
        let mut list_index = 0;
        let mut cell_index = 0;
        loop {
            if let Some(&ty) = layouts.function_types.get(function_index) {
                let Type::Fn(ft) = layouts.types[ty].clone() else {
                    unreachable!()
                };
                function_index += 1;
                for dependency in ft.params.into_iter().chain([ft.ret]) {
                    layouts.collect(dependency, "function value signature", &mut states)?;
                }
            } else if let Some(&ty) = layouts.list_types.get(list_index) {
                let Type::List(element) = layouts.types[ty] else {
                    unreachable!()
                };
                list_index += 1;
                layouts.collect(element, "List element", &mut states)?;
            } else if let Some(&ty) = layouts.cell_types.get(cell_index) {
                let Type::Cell(content) = layouts.types[ty] else {
                    unreachable!()
                };
                cell_index += 1;
                layouts.collect(content, "Cell content", &mut states)?;
            } else {
                break;
            }
        }
        layouts.has_error_abi |= layouts.function_types.iter().any(
            |&ty| matches!(&layouts.types[ty], Type::Fn(function_type) if function_type.fails),
        );
        if layouts.has_error_abi {
            let error_ids: Vec<EnumId> = flow
                .core
                .enums
                .iter()
                .filter_map(|(id, def)| {
                    (def.error
                        && states.keys().any(
                            |ty| matches!(layouts.types[*ty], Type::Enum(used, _) if used == id),
                        ))
                    .then_some(id)
                })
                .collect();
            for id in error_ids {
                let ty = layouts.types.enum_type(id, Vec::new());
                layouts.collect(ty, "EmelaError", &mut states)?;
                layouts.error_types.push((id, ty));
            }
        }
        // EmelaAny is an EmelaError record, which embeds each concrete error payload.
        // An inline Any within an error payload would therefore make its C layout infinite.
        // Reuse recursive field boxes for these edges, including ones nested in tuples.
        let error_dependencies = layouts.error_declaration_dependencies();
        for &index in &error_dependencies {
            let aggregate = &layouts.aggregates[index];
            let variants = match &aggregate.kind {
                AggregateKind::Tuple(fields) => vec![fields.as_slice()],
                AggregateKind::Enum(variants) => variants.iter().map(Vec::as_slice).collect(),
            };
            for (variant, fields) in variants.into_iter().enumerate() {
                for (field, &ty) in fields.iter().enumerate() {
                    if matches!(layouts.types[ty], Type::Any) {
                        layouts
                            .recursive_edges
                            .insert((aggregate.ty, variant, field));
                    }
                }
            }
        }
        for aggregate in &layouts.aggregates {
            let fields = match &aggregate.kind {
                AggregateKind::Tuple(fields) => vec![fields.as_slice()],
                AggregateKind::Enum(variants) => variants.iter().map(Vec::as_slice).collect(),
            };
            for (variant, fields) in fields.into_iter().enumerate() {
                for (field, &field_ty) in fields.iter().enumerate() {
                    if layouts
                        .recursive_edges
                        .contains(&(aggregate.ty, variant, field))
                        && layouts.has_materialized_payload(field_ty)
                        && !layouts.recursive_box_ids.contains_key(&field_ty)
                    {
                        let index = layouts.recursive_box_types.len();
                        layouts.recursive_box_types.push(field_ty);
                        layouts.recursive_box_ids.insert(field_ty, index);
                    }
                }
            }
        }
        layouts.managed_error_payloads = layouts
            .error_types
            .iter()
            .any(|&(_, ty)| layouts.is_managed_value(ty));
        for (id, function) in flow.functions.iter() {
            if !emitted.contains(&id) {
                continue;
            }
            let mut types = function
                .vars
                .iter()
                .map(|(_, variable)| variable.ty)
                .collect::<Vec<_>>();
            types.push(function.ret);
            types.extend(layouts.list_types.iter().copied());
            for ty in types {
                if !layouts.contains_heap_ref(ty, &mut HashSet::new()) {
                    continue;
                }
                match layouts.types[ty].clone() {
                    Type::List(_) => {}
                    Type::Cell(_) => {}
                    Type::Handler(_) | Type::Evidence | Type::FiberScope => {}
                    Type::Fn(_) => {}
                    Type::Any => {}
                    Type::Tuple(_) | Type::Enum(_, _) => {}
                    _ => {
                        return Err(unsupported(
                            &function.name,
                            "managed List reference in an unsupported value type",
                        ));
                    }
                }
            }
        }
        Ok(layouts)
    }

    pub(in crate::backend) fn add_any_box_type(&mut self, ty: TypeId) {
        self.any_abi = true;
        if self.type_repr(ty) == Repr::Void || matches!(self.types[ty], Type::Any) {
            return;
        }
        self.any_box_required = true;
        if self.is_runtime_handle(ty) {
            return;
        }
        if !self.any_box_ids.contains_key(&ty) {
            let index = self.any_box_types.len();
            self.any_box_types.push(ty);
            self.any_box_ids.insert(ty, index);
        }
    }

    pub(in crate::backend) fn add_function_adapter(
        &mut self,
        source: TypeId,
        destination: TypeId,
        function: &str,
    ) -> Result<(), Vec<Error>> {
        let (Type::Fn(source_fn), Type::Fn(destination_fn)) =
            (self.types[source].clone(), self.types[destination].clone())
        else {
            return Err(invalid_flow_ir(
                function,
                "function Coerce has non-function types",
            ));
        };
        if self.closure_function_types.contains(&source) {
            self.closure_function_types.insert(destination);
        }
        if source_fn.params == destination_fn.params
            && source_fn.ret == destination_fn.ret
            && source_fn.fails == destination_fn.fails
            && source_fn.suspends == destination_fn.suspends
            && source_fn.ev == destination_fn.ev
        {
            return Ok(());
        }
        let compatible = source_fn.params == destination_fn.params
            && source_fn.ret == destination_fn.ret
            && (!source_fn.suspends || destination_fn.suspends)
            && source_fn.sets.is_none()
            && destination_fn.sets.is_none()
            && (!source_fn.ev || destination_fn.ev);
        if !compatible {
            return Err(unsupported(
                function,
                &format!(
                    "function-value coercion with incompatible parameters, result, or effect flags ({source_fn:?} -> {destination_fn:?})"
                ),
            ));
        }
        if self
            .function_adapter_ids
            .contains_key(&(source, destination))
        {
            return Ok(());
        }
        let index = self.function_adapters.len();
        self.function_adapters.push(FunctionAdapterLayout {
            source,
            destination,
        });
        self.function_adapter_ids
            .insert((source, destination), index);
        self.closure_function_types.insert(destination);
        Ok(())
    }

    pub(in crate::backend) fn validate_finite_types(
        &mut self,
        ty: TypeId,
        _function: &str,
        active: &mut HashSet<TypeId>,
        validated: &mut HashSet<TypeId>,
    ) -> Result<(), Vec<Error>> {
        if validated.contains(&ty) || !active.insert(ty) {
            return Ok(());
        }
        let dependencies = match self.types[ty].clone() {
            Type::Fn(ft) => ft.params.into_iter().chain([ft.ret]).collect(),
            Type::Tuple(fields) => fields,
            Type::List(element) | Type::Cell(element) => vec![element],
            Type::Enum(id, args) if self.enum_layout_required(id) => self.core.enums[id]
                .variants
                .iter()
                .flat_map(|variant| &variant.tys)
                .map(|&field| self.types.subst(field, &args))
                .collect(),
            _ => vec![],
        };
        let lazy_fields =
            matches!(self.types[ty], Type::Enum(id, _) if self.expanding_enums.contains(&id));
        for dependency in dependencies {
            if lazy_fields && self.type_repr(dependency) == Repr::Ref {
                continue;
            }
            self.validate_finite_types(dependency, _function, active, validated)?;
        }
        active.remove(&ty);
        validated.insert(ty);
        Ok(())
    }

    pub(in crate::backend) fn collect(
        &mut self,
        ty: TypeId,
        function: &str,
        states: &mut HashMap<TypeId, u8>,
    ) -> Result<(), Vec<Error>> {
        if let Type::Enum(id, args) = self.types[ty].clone()
            && self.core.enums[id].name == "Std.Fiber.Exit"
            && args
                .iter()
                .any(|&arg| matches!(self.types[arg], Type::Set(_)))
        {
            // Flow erases the error set of Await to Unit. A declared Exit[A, {}] uses the
            // same runtime layout, including across direct function result boundaries.
            let args = args
                .into_iter()
                .map(|arg| {
                    if matches!(self.types[arg], Type::Set(_)) {
                        TypeId::UNIT
                    } else {
                        arg
                    }
                })
                .collect();
            let canonical = self.types.enum_type(id, args);
            self.collect(canonical, function, states)?;
            let index = self.aggregate_ids[&canonical];
            self.aggregate_ids.insert(ty, index);
            states.insert(ty, 2);
            return Ok(());
        }
        if matches!(self.types[ty], Type::Cell(_)) {
            if !self.cell_ids.contains_key(&ty) {
                self.cell_ids.insert(ty, self.cell_types.len());
                self.cell_types.push(ty);
            }
            return Ok(());
        }
        if matches!(self.types[ty], Type::Fn(_)) {
            if self.function_types.contains(&ty) {
                return Ok(());
            }
            self.function_types.push(ty);
            return Ok(());
        }
        if matches!(self.types[ty], Type::Any) {
            self.any_abi = true;
            return Ok(());
        }
        if matches!(self.types[ty], Type::List(_)) {
            if !self.list_ids.contains_key(&ty) {
                let index = self.list_types.len();
                self.list_types.push(ty);
                self.list_ids.insert(ty, index);
            }
            return Ok(());
        }
        let kind = match self.types[ty].clone() {
            Type::Tuple(fields) => Some(AggregateKind::Tuple(fields)),
            Type::Enum(id, args) if self.enum_layout_required(id) => {
                let raw_fields: Vec<Vec<TypeId>> = self.core.enums[id]
                    .variants
                    .iter()
                    .map(features::variant_fields::types)
                    .collect();
                let fields = raw_fields
                    .iter()
                    .map(|variant| {
                        variant
                            .iter()
                            .map(|&field| self.types.subst(field, &args))
                            .collect()
                    })
                    .collect();
                Some(AggregateKind::Enum(fields))
            }
            _ => None,
        };
        let Some(kind) = kind else {
            return Ok(());
        };

        match states.get(&ty) {
            Some(2) => return Ok(()),
            Some(1) => {
                return Err(unsupported(
                    function,
                    &format!("recursive inline aggregate type {}", type_name(ty)),
                ));
            }
            _ => {}
        }
        states.insert(ty, 1);

        let variants = match &kind {
            AggregateKind::Tuple(fields) => vec![fields.as_slice()],
            AggregateKind::Enum(variants) => variants.iter().map(Vec::as_slice).collect(),
        };
        for (variant, fields) in variants.into_iter().enumerate() {
            for (index, &field) in fields.iter().enumerate() {
                if self.type_repr(field) == Repr::Void {
                    continue;
                }
                let lazy_field = matches!(self.types[ty], Type::Enum(id, _) if self.expanding_enums.contains(&id))
                    && self.type_repr(field) == Repr::Ref;
                if lazy_field || matches!(states.get(&field), Some(1)) {
                    self.recursive_edges.insert((ty, variant, index));
                } else {
                    self.collect(field, function, states)?;
                }
            }
        }

        let index = self.aggregates.len();
        self.aggregate_ids.insert(ty, index);
        self.aggregates.push(AggregateLayout { ty, kind });
        states.insert(ty, 2);
        Ok(())
    }

    pub(in crate::backend) fn enum_is_i32(&self, id: emela_core::EnumId) -> bool {
        let def = &self.core.enums[id];
        !def.error
            && !def.external
            && !def.variants.is_empty()
            && def.variants.iter().all(|variant| variant.fields.is_empty())
    }

    pub(in crate::backend) fn enum_layout_required(&self, id: emela_core::EnumId) -> bool {
        let def = &self.core.enums[id];
        // Cause's source field describes its error set; it is never a runtime closure.
        def.name != "Std.Cause.Cause"
            && !def.external
            && !def.variants.is_empty()
            && !self.enum_is_i32(id)
    }

    pub(in crate::backend) fn type_repr(&self, ty: TypeId) -> Repr {
        features::types::repr(self, &self.types[ty])
    }

    pub(in crate::backend) fn c_type(&self, ty: TypeId) -> String {
        match self.types[ty] {
            Type::Int | Type::Bool => "int32_t".to_owned(),
            Type::Int64 => "int64_t".to_owned(),
            Type::Float => "double".to_owned(),
            Type::Unit | Type::Never | Type::Set(_) => "void".to_owned(),
            Type::String => "EmelaString".to_owned(),
            Type::Tuple(_) => type_name(ty),
            Type::Fn(_) => function_type_name(ty),
            Type::Enum(id, _) if self.enum_is_i32(id) => "int32_t".to_owned(),
            Type::Enum(id, _) if self.enum_layout_required(id) => {
                type_name(self.aggregates[self.aggregate_ids[&ty]].ty)
            }
            Type::Any => "EmelaAny".to_owned(),
            _ => c_type(self.type_repr(ty)).to_owned(),
        }
    }

    pub(in crate::backend) fn is_named_external(&self, ty: TypeId, name: &str) -> bool {
        matches!(&self.types[ty], Type::Enum(id, _) if self.core.enums[*id].external && self.core.enums[*id].name == name)
    }

    pub(in crate::backend) fn is_runtime_handle(&self, ty: TypeId) -> bool {
        self.is_named_external(ty, "Std.Fiber.Fiber")
            || self.is_named_external(ty, "Std.Deferred.Deferred")
            || self.is_cause(ty)
    }

    pub(in crate::backend) fn is_cause(&self, ty: TypeId) -> bool {
        matches!(&self.types[ty], Type::Enum(id, _) if self.core.enums[*id].name == "Std.Cause.Cause")
    }

    pub(in crate::backend) fn error_enum(&self, ty: TypeId) -> Option<EnumId> {
        match self.types[ty] {
            Type::Enum(id, _) if self.core.enums[id].error => Some(id),
            _ => None,
        }
    }

    pub(in crate::backend) fn error_type(&self, enum_id: EnumId) -> Option<TypeId> {
        self.error_types
            .iter()
            .find_map(|(id, ty)| (*id == enum_id).then_some(*ty))
    }

    pub(in crate::backend) fn enum_fields(&self, ty: TypeId) -> Option<&[Vec<TypeId>]> {
        let index = *self.aggregate_ids.get(&ty)?;
        match &self.aggregates[index].kind {
            AggregateKind::Enum(variants) => Some(variants),
            AggregateKind::Tuple(_) => None,
        }
    }

    pub(in crate::backend) fn has_cell_cycles(&self) -> bool {
        self.cell_types.iter().any(|&ty| {
            let Type::Cell(content) = self.types[ty] else {
                unreachable!()
            };
            self.cell_content_can_cycle(content, &mut HashSet::new())
        })
    }

    pub(in crate::backend) fn cell_content_can_cycle(
        &self,
        ty: TypeId,
        seen: &mut HashSet<TypeId>,
    ) -> bool {
        if !seen.insert(ty) {
            return false;
        }
        if self.is_runtime_handle(ty) {
            return true;
        }
        match self.types[ty].clone() {
            Type::Any
            | Type::Cell(_)
            | Type::Fn(_)
            | Type::Handler(_)
            | Type::Evidence
            | Type::FiberScope => true,
            Type::List(element) => self.cell_content_can_cycle(element, seen),
            _ => self.aggregate_ids.get(&ty).is_some_and(|&index| {
                let fields = match &self.aggregates[index].kind {
                    AggregateKind::Tuple(fields) => vec![fields.as_slice()],
                    AggregateKind::Enum(variants) => variants.iter().map(Vec::as_slice).collect(),
                };
                fields
                    .into_iter()
                    .flatten()
                    .any(|&field| self.cell_content_can_cycle(field, seen))
            }),
        }
    }

    pub(in crate::backend) fn cell_content_c_type(&self, content: TypeId) -> String {
        if self.type_repr(content) == Repr::Void {
            "uint8_t".into()
        } else {
            self.c_type(content)
        }
    }

    pub(in crate::backend) fn cell_kind(&self, cell: TypeId) -> usize {
        self.list_types.len() + self.recursive_box_types.len() + self.cell_ids[&cell]
    }

    pub(in crate::backend) fn list_node_name(&self, ty: TypeId) -> String {
        format!("emela_list_node_{}", self.list_ids[&ty])
    }

    pub(in crate::backend) fn list_index(&self, ty: TypeId) -> u32 {
        u32::try_from(self.list_ids[&ty]).expect("too many concrete List layouts")
    }

    pub(in crate::backend) fn is_managed_list(&self, ty: TypeId) -> bool {
        matches!(self.types[ty], Type::List(_))
    }

    pub(in crate::backend) fn is_managed_value(&self, ty: TypeId) -> bool {
        self.contains_owned_ref(ty, &mut HashSet::new())
    }

    pub(in crate::backend) fn contains_owned_ref(
        &self,
        ty: TypeId,
        seen: &mut HashSet<TypeId>,
    ) -> bool {
        if self.is_runtime_handle(ty) {
            return true;
        }
        if (self.any_box_required || self.has_error_ownership())
            && matches!(self.types[ty], Type::Any)
        {
            return true;
        }
        if matches!(
            self.types[ty],
            Type::String
                | Type::List(_)
                | Type::Cell(_)
                | Type::Handler(_)
                | Type::Evidence
                | Type::FiberScope
        ) {
            return true;
        }
        if matches!(self.types[ty], Type::Fn(_)) {
            return self.closure_function_types.contains(&ty);
        }
        if !seen.insert(ty) {
            return false;
        }
        let Some(&aggregate_index) = self.aggregate_ids.get(&ty) else {
            return false;
        };
        let aggregate = &self.aggregates[aggregate_index];
        match &aggregate.kind {
            AggregateKind::Tuple(fields) => fields.iter().enumerate().any(|(index, &field)| {
                self.recursive_edges.contains(&(ty, 0, index))
                    || self.contains_owned_ref(field, seen)
            }),
            AggregateKind::Enum(variants) => {
                variants.iter().enumerate().any(|(variant, fields)| {
                    fields.iter().enumerate().any(|(index, &field)| {
                        self.recursive_edges.contains(&(ty, variant, index))
                            || self.contains_owned_ref(field, seen)
                    })
                })
            }
        }
    }

    pub(in crate::backend) fn contains_heap_ref(
        &self,
        ty: TypeId,
        seen: &mut HashSet<TypeId>,
    ) -> bool {
        if self.is_runtime_handle(ty) {
            return true;
        }
        if (self.any_box_required || self.has_error_ownership())
            && matches!(self.types[ty], Type::Any)
        {
            return true;
        }
        if matches!(
            self.types[ty],
            Type::List(_) | Type::Cell(_) | Type::Handler(_) | Type::Evidence | Type::FiberScope
        ) {
            return true;
        }
        if matches!(self.types[ty], Type::Fn(_)) {
            return self.closure_function_types.contains(&ty);
        }
        if !seen.insert(ty) {
            return false;
        }
        let Some(&aggregate_index) = self.aggregate_ids.get(&ty) else {
            return false;
        };
        let aggregate = &self.aggregates[aggregate_index];
        match &aggregate.kind {
            AggregateKind::Tuple(fields) => fields.iter().enumerate().any(|(index, &field)| {
                self.recursive_edges.contains(&(ty, 0, index))
                    || self.contains_heap_ref(field, seen)
            }),
            AggregateKind::Enum(variants) => {
                variants.iter().enumerate().any(|(variant, fields)| {
                    fields.iter().enumerate().any(|(index, &field)| {
                        self.recursive_edges.contains(&(ty, variant, index))
                            || self.contains_heap_ref(field, seen)
                    })
                })
            }
        }
    }

    pub(in crate::backend) fn is_recursive_box_field(
        &self,
        owner: TypeId,
        variant: usize,
        field: usize,
    ) -> bool {
        self.recursive_edges.contains(&(owner, variant, field))
    }

    pub(in crate::backend) fn has_materialized_payload(&self, ty: TypeId) -> bool {
        match self.types[ty] {
            Type::Enum(id, _) if self.enum_layout_required(id) => {
                self.aggregate_ids.contains_key(&ty)
            }
            Type::Tuple(_) => self.aggregate_ids.contains_key(&ty),
            Type::Fn(_) => self.function_types.contains(&ty),
            Type::List(_) => self.list_ids.contains_key(&ty),
            Type::Cell(_) => self.cell_ids.contains_key(&ty),
            Type::Any => self.any_abi,
            _ => true,
        }
    }

    pub(in crate::backend) fn recursive_box_kind(&self, ty: TypeId) -> Option<u32> {
        let index = *self.recursive_box_ids.get(&ty)?;
        u32::try_from(self.list_types.len() + index).ok()
    }

    pub(in crate::backend) fn string_owner_kind(&self) -> u32 {
        u32::try_from(
            self.list_types.len() + self.recursive_box_types.len() + self.cell_types.len(),
        )
        .expect("too many fixed-heap object kinds")
    }

    pub(in crate::backend) fn any_box_kind(&self) -> u32 {
        self.runtime_value_kind_base()
            + u32::try_from(self.function_adapters.len()).expect("too many function adapters")
            + u32::from(self.evidence_nodes_required)
            + u32::try_from(self.handler_layouts.len()).expect("too many handler effects")
    }

    pub(in crate::backend) fn closure_environment_kind(&self, index: usize) -> u32 {
        u32::try_from(self.string_owner_kind() as usize + 1 + index)
            .expect("too many fixed-heap object kinds")
    }

    pub(in crate::backend) fn aggregate_helper_index(&self, ty: TypeId) -> Option<usize> {
        let index = *self.aggregate_ids.get(&ty)?;
        self.is_managed_value(ty).then_some(index)
    }

    pub(in crate::backend) fn emit_retain_value_expression(
        &self,
        ty: TypeId,
        expression: &str,
        writer: &mut Writer,
    ) {
        if self.is_managed_list(ty)
            || self.is_runtime_handle(ty)
            || matches!(
                self.types[ty],
                Type::Cell(_) | Type::Handler(_) | Type::Evidence | Type::FiberScope
            )
        {
            writer.line(&format!("emela_heap_retain({expression});"));
        } else if matches!(self.types[ty], Type::Any) {
            writer.line(&format!("emela_any_retain({expression});"));
        } else if matches!(self.types[ty], Type::String) {
            writer.line(&format!("emela_heap_retain(({expression}).owner);"));
        } else if matches!(self.types[ty], Type::Fn(_)) {
            writer.line(&format!("emela_heap_retain(({expression}).env);"));
        } else if let Some(index) = self.aggregate_helper_index(ty) {
            writer.line(&format!(
                "emela_heap_retain_value_{index}(&({expression}));"
            ));
        }
    }

    pub(in crate::backend) fn emit_enqueue_value_expression(
        &self,
        ty: TypeId,
        expression: &str,
        pending: &str,
        tail: &str,
        writer: &mut Writer,
    ) {
        if self.is_managed_list(ty)
            || self.is_runtime_handle(ty)
            || matches!(
                self.types[ty],
                Type::Cell(_) | Type::Handler(_) | Type::Evidence | Type::FiberScope
            )
        {
            writer.line(&format!(
                "emela_heap_enqueue({expression}, {pending}, {tail});"
            ));
        } else if matches!(self.types[ty], Type::Any) {
            writer.line(&format!(
                "emela_any_enqueue({expression}, {pending}, {tail});"
            ));
        } else if matches!(self.types[ty], Type::String) {
            writer.line(&format!(
                "emela_heap_enqueue(({expression}).owner, {pending}, {tail});"
            ));
        } else if matches!(self.types[ty], Type::Fn(_)) {
            writer.line(&format!(
                "emela_heap_enqueue(({expression}).env, {pending}, {tail});"
            ));
        } else if let Some(index) = self.aggregate_helper_index(ty) {
            writer.line(&format!(
                "emela_heap_enqueue_value_{index}(&({expression}), {pending}, {tail});"
            ));
        }
    }

    pub(in crate::backend) fn emit_release_value_expression(
        &self,
        ty: TypeId,
        expression: &str,
        writer: &mut Writer,
    ) {
        if self.is_managed_list(ty)
            || self.is_runtime_handle(ty)
            || matches!(
                self.types[ty],
                Type::Cell(_) | Type::Handler(_) | Type::Evidence | Type::FiberScope
            )
        {
            writer.line(&format!("emela_heap_release({expression});"));
        } else if matches!(self.types[ty], Type::Any) {
            writer.line(&format!("emela_any_release({expression});"));
        } else if matches!(self.types[ty], Type::String) {
            writer.line(&format!("emela_heap_release(({expression}).owner);"));
        } else if matches!(self.types[ty], Type::Fn(_)) {
            writer.line(&format!("emela_heap_release(({expression}).env);"));
        } else if let Some(index) = self.aggregate_helper_index(ty) {
            writer.line(&format!(
                "emela_heap_release_value_{index}(&({expression}));"
            ));
        }
    }

    pub(in crate::backend) fn emit_noheap_ownership_stubs(&self, writer: &mut Writer) {
        for (index, aggregate) in self.aggregates.iter().enumerate() {
            if !self.is_managed_value(aggregate.ty)
                || self.contains_heap_ref(aggregate.ty, &mut HashSet::new())
            {
                continue;
            }
            let aggregate_type = self.c_type(aggregate.ty);
            writer.begin_block(&format!(
                "static inline void emela_heap_retain_value_{index}(const {aggregate_type} *value)"
            ));
            writer.line("(void)value;");
            writer.end_block();
            writer.begin_block(&format!(
                "static inline void emela_heap_release_value_{index}({aggregate_type} *value)"
            ));
            writer.line("(void)value;");
            writer.end_block();
            writer.blank();
        }
    }

    pub(in crate::backend) fn emit_heap_ownership_helpers(&self, writer: &mut Writer) {
        for (index, aggregate) in self.aggregates.iter().enumerate() {
            if !self.is_managed_value(aggregate.ty) {
                continue;
            }
            let aggregate_type = self.c_type(aggregate.ty);
            writer.begin_block(&format!(
                "static void emela_heap_retain_value_{index}(const {aggregate_type} *value)"
            ));
            self.emit_aggregate_retain_body(aggregate, writer);
            writer.end_block();

            writer.begin_block(&format!(
                "static void emela_heap_enqueue_value_{index}(const {aggregate_type} *value, emela_heap_block **pending, emela_heap_block **tail)"
            ));
            self.emit_aggregate_enqueue_body(aggregate, writer);
            writer.end_block();

            writer.begin_block(&format!(
                "static void emela_heap_release_value_{index}(const {aggregate_type} *value)"
            ));
            writer.line("emela_heap_block *pending = NULL;");
            writer.line("emela_heap_block *tail = NULL;");
            writer.line(&format!(
                "emela_heap_enqueue_value_{index}(value, &pending, &tail);"
            ));
            writer.line("emela_heap_drain(pending, tail);");
            writer.end_block();
            writer.blank();
        }
    }

    pub(in crate::backend) fn emit_aggregate_retain_body(
        &self,
        aggregate: &AggregateLayout,
        writer: &mut Writer,
    ) {
        match &aggregate.kind {
            AggregateKind::Tuple(fields) => {
                for (index, &field_ty) in fields.iter().enumerate() {
                    if self.is_recursive_box_field(aggregate.ty, 0, index) {
                        writer.line(&format!("emela_heap_retain(value->f{index});"));
                        continue;
                    }
                    if self.is_managed_value(field_ty) {
                        self.emit_retain_value_expression(
                            field_ty,
                            &format!("value->f{index}"),
                            writer,
                        );
                    }
                }
            }
            AggregateKind::Enum(variants) => {
                writer.begin_block("switch (value->tag)");
                for (variant, fields) in variants.iter().enumerate() {
                    writer.line(&format!("case INT32_C({variant}):"));
                    writer.begin_block("");
                    for (index, &field_ty) in fields.iter().enumerate() {
                        if self.is_recursive_box_field(aggregate.ty, variant, index) {
                            writer.line(&format!(
                                "emela_heap_retain(value->payload.v{variant}.f{index});"
                            ));
                            continue;
                        }
                        if self.is_managed_value(field_ty) {
                            self.emit_retain_value_expression(
                                field_ty,
                                &format!("value->payload.v{variant}.f{index}"),
                                writer,
                            );
                        }
                    }
                    writer.line("break;");
                    writer.end_block();
                }
                writer.line("default:");
                writer.begin_block("");
                writer.line("break;");
                writer.end_block();
                writer.end_block();
            }
        }
    }

    pub(in crate::backend) fn emit_aggregate_enqueue_body(
        &self,
        aggregate: &AggregateLayout,
        writer: &mut Writer,
    ) {
        match &aggregate.kind {
            AggregateKind::Tuple(fields) => {
                for (index, &field_ty) in fields.iter().enumerate() {
                    if self.is_recursive_box_field(aggregate.ty, 0, index) {
                        writer.line(&format!(
                            "emela_heap_enqueue(value->f{index}, pending, tail);"
                        ));
                        continue;
                    }
                    if self.is_managed_value(field_ty) {
                        self.emit_enqueue_value_expression(
                            field_ty,
                            &format!("value->f{index}"),
                            "pending",
                            "tail",
                            writer,
                        );
                    }
                }
            }
            AggregateKind::Enum(variants) => {
                writer.begin_block("switch (value->tag)");
                for (variant, fields) in variants.iter().enumerate() {
                    writer.line(&format!("case INT32_C({variant}):"));
                    writer.begin_block("");
                    for (index, &field_ty) in fields.iter().enumerate() {
                        if self.is_recursive_box_field(aggregate.ty, variant, index) {
                            writer.line(&format!(
                                "emela_heap_enqueue(value->payload.v{variant}.f{index}, pending, tail);"
                            ));
                            continue;
                        }
                        if self.is_managed_value(field_ty) {
                            self.emit_enqueue_value_expression(
                                field_ty,
                                &format!("value->payload.v{variant}.f{index}"),
                                "pending",
                                "tail",
                                writer,
                            );
                        }
                    }
                    writer.line("break;");
                    writer.end_block();
                }
                writer.line("default:");
                writer.begin_block("");
                writer.line("break;");
                writer.end_block();
                writer.end_block();
            }
        }
    }

    pub(in crate::backend) fn emit_heap_destroy_switch(
        &self,
        environments: &[features::values::closure::ClosureEnvironment],
        writer: &mut Writer,
    ) {
        if self.has_resumes && self.has_error_ownership() {
            writer.line("static void emela_resume_enqueue_error(void *value, emela_heap_block **pending, emela_heap_block **tail);");
        }
        writer.begin_block(
            "static void emela_heap_destroy(uint32_t kind, void *value, emela_heap_block **pending, emela_heap_block **tail)",
        );
        writer.line("(void)value; (void)pending; (void)tail;");
        writer.begin_block("switch (kind)");
        for &list_ty in &self.list_types {
            let Type::List(element) = self.types[list_ty] else {
                unreachable!("list layout has a non-List type")
            };
            let node_type = self.list_node_name(list_ty);
            let kind = self.list_index(list_ty);
            writer.line(&format!("case UINT32_C({kind}):"));
            writer.begin_block("");
            writer.line(&format!("{node_type} *node = ({node_type} *)value;"));
            if self.type_repr(element) != Repr::Void && self.is_managed_value(element) {
                self.emit_enqueue_value_expression(
                    element,
                    "node->head",
                    "pending",
                    "tail",
                    writer,
                );
            }
            writer.line("emela_heap_enqueue(node->tail, pending, tail);");
            writer.line("break;");
            writer.end_block();
        }
        for &box_ty in &self.recursive_box_types {
            let box_index = self.recursive_box_ids[&box_ty];
            let kind = self.list_types.len() + box_index;
            writer.line(&format!("case UINT32_C({kind}):"));
            writer.begin_block("");
            self.emit_enqueue_value_expression(
                box_ty,
                &format!("(*(({} const *)value))", self.c_type(box_ty)),
                "pending",
                "tail",
                writer,
            );
            writer.line("break;");
            writer.end_block();
        }
        for (index, &cell_ty) in self.cell_types.iter().enumerate() {
            let Type::Cell(content) = self.types[cell_ty] else {
                unreachable!()
            };
            let kind = self.list_types.len() + self.recursive_box_types.len() + index;
            writer.line(&format!("case UINT32_C({kind}):"));
            writer.begin_block("");
            self.emit_enqueue_value_expression(
                content,
                &format!("(*(({} *)value))", self.cell_content_c_type(content)),
                "pending",
                "tail",
                writer,
            );
            writer.line("break;");
            writer.end_block();
        }
        let string_owner_kind = self.string_owner_kind();
        writer.line(&format!("case UINT32_C({string_owner_kind}):"));
        writer.begin_block("");
        writer.line("break;");
        writer.end_block();
        if self.has_resumes {
            if self.has_error_ownership() {
                writer.line(&format!(
                    "case UINT32_C({}): emela_resume_enqueue_error(value, pending, tail); break;",
                    self.resume_kind()
                ));
            } else {
                writer.line(&format!("case UINT32_C({}): break;", self.resume_kind()));
            }
        }
        if self.has_causes {
            writer.line(&format!("case UINT32_C({}): {{", self.cause_kind()));
            if self.has_error_ownership() {
                writer.line("emela_cause *cause = (emela_cause *)value;");
                writer.line(
                    "if (cause->kind == 0) emela_error_enqueue(cause->error, pending, tail);",
                );
            }
            writer.line("break; }");
        }
        if self.has_error_notes() {
            writer.line(&format!("case UINT32_C({}): emela_heap_enqueue(((emela_error_notes *)value)->head, pending, tail); break;", self.error_notes_kind()));
            writer.line(&format!("case UINT32_C({}): emela_heap_enqueue(((emela_error_note *)value)->next, pending, tail); break;", self.error_note_kind()));
        }
        if self.has_scopes {
            writer.line(&format!("case UINT32_C({}): {{", self.scope_kind()));
            if self.has_error_ownership() {
                writer.line(
                    "emela_error_enqueue(((emela_scope *)value)->first_error, pending, tail);",
                );
            }
            writer.line("break; }");
            writer.line(&format!("case UINT32_C({}): {{", self.child_kind()));
            writer.line("emela_child *child = (emela_child *)value;");
            if self.has_cell_cycles() {
                writer.line("if (emela_heap_trace_mode == 0 || emela_heap_trace_mode == 3) emela_child_unlink(child);");
            } else {
                writer.line("emela_child_unlink(child);");
            }
            writer.line("emela_heap_enqueue(child->scope, pending, tail);");
            if self.has_error_ownership() {
                writer.line("emela_error_enqueue(child->error, pending, tail);");
            }
            self.emit_enqueue_value_expression(
                TypeId::ANY,
                "child->result",
                "pending",
                "tail",
                writer,
            );
            writer.line("break; }");
            writer.line(&format!("case UINT32_C({}): {{", self.deferred_kind()));
            if self.deferred_pool {
                writer.line("emela_deferred *deferred = (emela_deferred *)value;");
                writer.begin_block("if (deferred->slot != EMELA_SLOT_NONE)");
                if !self.has_cell_cycles() {
                    writer.line("EmelaAny stored = emela_deferred_recycle(deferred);");
                } else {
                    writer.line("EmelaAny stored = emela_heap_trace_mode == 1 || emela_heap_trace_mode == 2 ? emela_deferred_slot_get(deferred)->value : emela_deferred_recycle(deferred);");
                }
                self.emit_enqueue_value_expression(
                    TypeId::ANY,
                    "stored",
                    "pending",
                    "tail",
                    writer,
                );
                writer.end_block();
            }
            writer.line("emela_heap_enqueue(((emela_deferred *)value)->scope, pending, tail);");
            writer.line("break; }");
        }
        for (index, environment) in environments.iter().enumerate() {
            let kind = self.closure_environment_kind(index);
            writer.line(&format!("case UINT32_C({kind}):"));
            writer.begin_block("");
            writer.line(&format!(
                "emela_closure_env_{index} *environment = (emela_closure_env_{index} *)value;"
            ));
            writer.line("(void)environment;");
            for (capture_index, capture) in environment.captures.iter().enumerate() {
                if self.type_repr(capture.ty) != Repr::Void && self.is_managed_value(capture.ty) {
                    self.emit_enqueue_value_expression(
                        capture.ty,
                        &format!("environment->c{capture_index}"),
                        "pending",
                        "tail",
                        writer,
                    );
                }
            }
            writer.line("break;");
            writer.end_block();
        }
        for (index, adapter) in self.function_adapters.iter().enumerate() {
            let kind = self.function_adapter_kind(index);
            writer.line(&format!("case UINT32_C({kind}):"));
            writer.begin_block("");
            writer.line(&format!(
                "emela_fn_coerce_env_{index} *environment = (emela_fn_coerce_env_{index} *)value;"
            ));
            writer.line("(void)environment;");
            if self.is_managed_value(adapter.source) {
                self.emit_enqueue_value_expression(
                    adapter.source,
                    "environment->source",
                    "pending",
                    "tail",
                    writer,
                );
            }
            writer.line("break;");
            writer.end_block();
        }
        if self.evidence_nodes_required {
            let kind = self.evidence_node_kind();
            writer.line(&format!("case UINT32_C({kind}):"));
            writer.begin_block("");
            writer.line("emela_evidence_node *entry = (emela_evidence_node *)value;");
            writer.line("emela_heap_enqueue(entry->handler, pending, tail);");
            writer.line("emela_heap_enqueue(entry->parent, pending, tail);");
            writer.line("break;");
            writer.end_block();
        }
        for layout in &self.handler_layouts {
            let kind = self.handler_kind(layout.effect);
            let handler_type = Self::handler_type_name(layout.effect);
            writer.line(&format!("case UINT32_C({kind}):"));
            writer.begin_block("");
            writer.line(&format!(
                "{handler_type} *handler = ({handler_type} *)value;"
            ));
            writer.line("(void)handler;");
            for (index, &operation) in layout.operations.iter().enumerate() {
                if self.is_managed_value(operation) {
                    self.emit_enqueue_value_expression(
                        operation,
                        &format!("handler->op_{index}"),
                        "pending",
                        "tail",
                        writer,
                    );
                }
            }
            writer.line("break;");
            writer.end_block();
        }
        if self.any_box_required {
            let kind = self.any_box_kind();
            writer.line(&format!("case UINT32_C({kind}):"));
            writer.begin_block("");
            writer.line("emela_any_box *box = (emela_any_box *)value;");
            writer.begin_block("switch (box->type)");
            for (index, &ty) in self.any_box_types.iter().enumerate() {
                writer.line(&format!("case UINT32_C({index}):"));
                writer.begin_block("");
                if self.is_managed_value(ty) {
                    self.emit_enqueue_value_expression(
                        ty,
                        &format!("box->payload.v{index}"),
                        "pending",
                        "tail",
                        writer,
                    );
                }
                writer.line("break;");
                writer.end_block();
            }
            writer.line("default:");
            writer.begin_block("");
            writer.line(
                "emela_board_defect(EMELA_DEFECT_PANIC, \"invalid Any box type\", UINT16_C(20));",
            );
            writer.line("break;");
            writer.end_block();
            writer.end_block();
            writer.line("break;");
            writer.end_block();
        }
        writer.line("default:");
        writer.begin_block("");
        writer.line(&format!(
            "emela_board_defect(EMELA_DEFECT_PANIC, {}, UINT16_C({}));",
            c_string_literal(b"invalid fixed heap reference"),
            b"invalid fixed heap reference".len()
        ));
        writer.line("break;");
        writer.end_block();
        writer.end_block();
        writer.end_block();
        writer.blank();
    }

    pub(in crate::backend) fn emit_public_release_declarations(&self, writer: &mut Writer) {
        for (index, aggregate) in self.aggregates.iter().enumerate() {
            if !self.is_managed_value(aggregate.ty) {
                continue;
            }
            let aggregate_type = self.c_type(aggregate.ty);
            writer.line("/* Retains heap references before copying this aggregate value. */");
            writer.line(&format!(
                "void emela_embedded_retain_value_{index}(const {aggregate_type} *value);"
            ));
            writer.line("/* Releases the heap references owned by this aggregate value. */");
            writer.line(&format!(
                "void emela_embedded_release_value_{index}({aggregate_type} *value);"
            ));
        }
    }

    pub(in crate::backend) fn emit_public_release_helpers(&self, writer: &mut Writer) {
        for (index, aggregate) in self.aggregates.iter().enumerate() {
            if !self.is_managed_value(aggregate.ty) {
                continue;
            }
            let aggregate_type = self.c_type(aggregate.ty);
            writer.begin_block(&format!(
                "void emela_embedded_retain_value_{index}(const {aggregate_type} *value)"
            ));
            writer.begin_block("if (value != NULL)");
            writer.line(&format!("emela_heap_retain_value_{index}(value);"));
            writer.end_block();
            writer.end_block();
            writer.begin_block(&format!(
                "void emela_embedded_release_value_{index}({aggregate_type} *value)"
            ));
            writer.begin_block("if (value != NULL)");
            writer.line(&format!("emela_heap_release_value_{index}(value);"));
            writer.line(&format!("*value = ({aggregate_type}){{0}};"));
            writer.end_block();
            writer.end_block();
            writer.blank();
        }
    }

    pub(in crate::backend) fn emit_public_function_ownership_declarations(
        &self,
        writer: &mut Writer,
    ) {
        for &ty in &self.function_types {
            if !self.closure_function_types.contains(&ty) {
                continue;
            }
            let function_type = self.c_type(ty);
            writer.line("/* Retains the environment owned by this function value. */");
            writer.line(&format!(
                "void emela_embedded_retain_function_{}(const {function_type} *value);",
                ty.index()
            ));
            writer.line("/* Releases the environment owned by this function value. */");
            writer.line(&format!(
                "void emela_embedded_release_function_{}({function_type} *value);",
                ty.index()
            ));
        }
    }

    pub(in crate::backend) fn emit_public_function_ownership_helpers(&self, writer: &mut Writer) {
        for &ty in &self.function_types {
            if !self.closure_function_types.contains(&ty) {
                continue;
            }
            let function_type = self.c_type(ty);
            writer.begin_block(&format!(
                "void emela_embedded_retain_function_{}(const {function_type} *value)",
                ty.index()
            ));
            writer.begin_block("if (value != NULL)");
            writer.line("emela_heap_retain(value->env);");
            writer.end_block();
            writer.end_block();
            writer.blank();
            writer.begin_block(&format!(
                "void emela_embedded_release_function_{}({function_type} *value)",
                ty.index()
            ));
            writer.begin_block("if (value != NULL)");
            writer.line("emela_heap_release(value->env);");
            writer.end_block();
            writer.end_block();
            writer.blank();
        }
    }

    pub(in crate::backend) fn emit_list_node_declarations(&self, writer: &mut Writer) {
        for &ty in &self.list_types {
            let Type::List(element) = self.types[ty].clone() else {
                unreachable!("list layout has a non-List type")
            };
            writer.line(&format!("typedef struct {} {{", self.list_node_name(ty)));
            if self.type_repr(element) == Repr::Void {
                writer.line("  uint8_t _unit;");
            } else {
                writer.line(&format!("  {} head;", self.c_type(element)));
            }
            writer.line("  void *tail;");
            writer.line(&format!("}} {};", self.list_node_name(ty)));
            writer.blank();
        }
    }

    pub(in crate::backend) fn emit_runtime_value_declarations(&self, writer: &mut Writer) {
        if self.evidence_nodes_required {
            writer.line("typedef struct emela_evidence_node {");
            writer.line("  uint32_t effect;");
            writer.line("  void *handler;");
            writer.line("  void *parent;");
            writer.line("} emela_evidence_node;");
            writer.blank();
        }
        for (index, adapter) in self.function_adapters.iter().enumerate() {
            writer.line(&format!("typedef struct emela_fn_coerce_env_{index} {{"));
            writer.line(&format!("  {} source;", self.c_type(adapter.source)));
            writer.line(&format!("}} emela_fn_coerce_env_{index};"));
            writer.blank();
        }
        if self.any_box_required {
            writer.line("typedef struct emela_any_box {");
            writer.line("  uint32_t type;");
            writer.line("  union {");
            writer.line("    uint8_t _empty;");
            for (index, &ty) in self.any_box_types.iter().enumerate() {
                writer.line(&format!("    {} v{index};", self.c_type(ty)));
            }
            writer.line("  } payload;");
            writer.line("} emela_any_box;");
            writer.blank();
        }
        for layout in &self.handler_layouts {
            writer.line(&format!(
                "typedef struct {} {{",
                Self::handler_type_name(layout.effect)
            ));
            writer.line("  int32_t active;");
            writer.line("  const char *released_message;");
            writer.line("  uint16_t released_message_length;");
            for (index, _) in layout
                .operations
                .iter()
                .enumerate()
                .filter(|(i, _)| self.core.effects[layout.effect].ops[*i].suspend)
            {
                writer.line(&format!("  const char *resume_error_{index};"));
                writer.line(&format!("  uint16_t resume_error_length_{index};"));
            }
            if layout.operations.is_empty() {
                writer.line("  uint8_t _empty;");
            } else {
                for (index, &operation) in layout.operations.iter().enumerate() {
                    writer.line(&format!("  {} op_{index};", self.c_type(operation)));
                }
            }
            writer.line(&format!("}} {};", Self::handler_type_name(layout.effect)));
            writer.blank();
        }
    }

    pub(in crate::backend) fn handler_type_name(effect: EffectId) -> String {
        format!("emela_handler_{}", u32::from(effect.into_raw()))
    }

    pub(in crate::backend) fn runtime_value_kind_base(&self) -> u32 {
        if self.closure_environments.is_empty() {
            self.string_owner_kind() + 1
        } else {
            self.closure_environment_kind(self.closure_environments.len() - 1) + 1
        }
    }

    pub(in crate::backend) fn function_adapter_kind(&self, index: usize) -> u32 {
        self.runtime_value_kind_base() + u32::try_from(index).expect("too many function adapters")
    }

    pub(in crate::backend) fn resume_kind(&self) -> u32 {
        self.any_box_kind() + 1
    }

    pub(in crate::backend) fn scope_kind(&self) -> u32 {
        self.resume_kind() + 1
    }
    pub(in crate::backend) fn child_kind(&self) -> u32 {
        self.resume_kind() + 2
    }
    pub(in crate::backend) fn deferred_kind(&self) -> u32 {
        self.resume_kind() + 3
    }
    pub(in crate::backend) fn cause_kind(&self) -> u32 {
        self.resume_kind() + 4
    }

    pub(in crate::backend) fn evidence_node_kind(&self) -> u32 {
        self.runtime_value_kind_base()
            + u32::try_from(self.function_adapters.len()).expect("too many function adapters")
    }

    pub(in crate::backend) fn handler_kind(&self, effect: EffectId) -> u32 {
        let handler_index = self.handler_layout_ids[&effect];
        self.runtime_value_kind_base()
            + u32::try_from(self.function_adapters.len()).expect("too many function adapters")
            + u32::from(self.evidence_nodes_required)
            + u32::try_from(handler_index).expect("too many handler effects")
    }

    pub(in crate::backend) fn error_declaration_dependencies(&self) -> HashSet<usize> {
        let mut pending: Vec<_> = self.error_types.iter().map(|&(_, ty)| ty).collect();
        let mut visited = HashSet::new();
        while let Some(ty) = pending.pop() {
            let Some(&index) = self.aggregate_ids.get(&ty) else {
                continue;
            };
            if !visited.insert(index) {
                continue;
            }
            let aggregate = &self.aggregates[index];
            let variants = match &aggregate.kind {
                AggregateKind::Tuple(fields) => vec![fields.as_slice()],
                AggregateKind::Enum(variants) => variants.iter().map(Vec::as_slice).collect(),
            };
            for (variant, fields) in variants.into_iter().enumerate() {
                for (field, &ty) in fields.iter().enumerate() {
                    if !self.is_recursive_box_field(aggregate.ty, variant, field) {
                        pending.push(ty);
                    }
                }
            }
        }
        visited
    }

    pub(in crate::backend) fn emit_declarations(&self, writer: &mut Writer) {
        // All named value types are visible in prototypes, even when an aggregate contains
        // a callback whose signature returns that aggregate. Only inline fields require a
        // complete definition, so function records can precede the ordered aggregates.
        for &ty in &self.function_types {
            let name = function_type_name(ty);
            writer.line(&format!("typedef struct {name} {name};"));
        }
        for aggregate in &self.aggregates {
            let name = type_name(aggregate.ty);
            writer.line(&format!("typedef struct {name} {name};"));
        }
        writer.blank();
        for &ty in &self.function_types {
            let Type::Fn(function_type) = self.types[ty].clone() else {
                unreachable!("function type registry contains a non-function")
            };
            let mut parameters = vec!["void *emela_env".to_owned()];
            if function_type.suspends {
                parameters.insert(0, "emela_fiber *fiber".to_owned());
            }
            parameters.extend(
                function_type
                    .params
                    .iter()
                    .filter(|&&parameter| self.type_repr(parameter) != Repr::Void)
                    .map(|&parameter| self.c_type(parameter)),
            );
            if function_type.ev {
                parameters.push("void *emela_evidence".to_owned());
            }
            if function_type.fails {
                parameters.push("EmelaError *emela_error_out".to_owned());
            }
            if function_type.suspends && self.type_repr(function_type.ret) != Repr::Void {
                parameters.push(format!("{} *result", self.c_type(function_type.ret)));
            }
            writer.line(&format!("struct {} {{", function_type_name(ty)));
            writer.line(&format!(
                "  {} (*call)({});",
                if function_type.suspends {
                    "void".to_owned()
                } else {
                    self.c_type(function_type.ret)
                },
                parameters.join(", ")
            ));
            writer.line("  void *env;");
            writer.line("};");
        }
        if !self.function_types.is_empty() {
            writer.blank();
        }
        let error_dependencies = self.error_declaration_dependencies();
        let declarations = self
            .aggregates
            .iter()
            .enumerate()
            .filter(|(index, _)| error_dependencies.contains(index))
            .map(|(_, aggregate)| Some(aggregate))
            .chain(std::iter::once(None))
            .chain(
                self.aggregates
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| !error_dependencies.contains(index))
                    .map(|(_, aggregate)| Some(aggregate)),
            );
        for aggregate in declarations {
            let Some(aggregate) = aggregate else {
                self.emit_error_declaration(writer);
                continue;
            };
            writer.line(&format!("struct {} {{", type_name(aggregate.ty)));
            match &aggregate.kind {
                AggregateKind::Tuple(fields) => {
                    let mut represented = false;
                    for (index, &field) in fields.iter().enumerate() {
                        if self.type_repr(field) == Repr::Void {
                            continue;
                        }
                        represented = true;
                        let field_type = if self.is_recursive_box_field(aggregate.ty, 0, index) {
                            "void *".to_owned()
                        } else {
                            self.c_type(field)
                        };
                        writer.line(&format!("  {field_type} f{index};"));
                    }
                    if !represented {
                        writer.line("  uint8_t _unit;");
                    }
                }
                AggregateKind::Enum(variants) => {
                    writer.line("  int32_t tag;");
                    writer.line("  union {");
                    for (variant_index, fields) in variants.iter().enumerate() {
                        let represented_fields = fields
                            .iter()
                            .enumerate()
                            .filter(|(_, field)| self.type_repr(**field) != Repr::Void)
                            .map(|(field_index, field)| {
                                let field_type = if self.is_recursive_box_field(
                                    aggregate.ty,
                                    variant_index,
                                    field_index,
                                ) {
                                    "void *".to_owned()
                                } else {
                                    self.c_type(*field)
                                };
                                format!("{field_type} f{field_index};")
                            })
                            .collect::<Vec<_>>();
                        let body = if represented_fields.is_empty() {
                            "uint8_t _unit;".to_owned()
                        } else {
                            represented_fields.join(" ")
                        };
                        writer.line(&format!("    struct {{ {body} }} v{variant_index};"));
                    }
                    writer.line("  } payload;");
                }
            }
            writer.line("};");
            writer.blank();
        }
    }

    pub(in crate::backend) fn emit_string_declaration(&self, writer: &mut Writer) {
        writer.line(
            "typedef struct { const char *data; uint16_t length; void *owner; } EmelaString;",
        );
        writer.blank();
    }

    pub(in crate::backend) fn emit_error_declaration(&self, writer: &mut Writer) {
        if !self.has_error_abi && !self.any_abi {
            return;
        }
        writer.line("struct EmelaError {");
        writer.line("  uint32_t tag;");
        if self.has_error_notes() {
            writer.line("  void *notes;");
        }
        writer.line("  union {");
        if self.any_abi {
            writer.line("    void *any_box;");
        }
        if self.error_types.is_empty() {
            if !self.any_abi {
                writer.line("    uint8_t _empty;");
            }
        } else {
            for (enum_id, ty) in &self.error_types {
                writer.line(&format!(
                    "    {} e{};",
                    self.c_type(*ty),
                    enum_index(*enum_id)
                ));
            }
        }
        writer.line("  } payload;");
        writer.line("};");
        writer.blank();
    }
}
