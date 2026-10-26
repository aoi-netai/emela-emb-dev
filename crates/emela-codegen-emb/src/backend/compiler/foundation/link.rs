//! Restricted connection edits on an isolated copy of a compiled program.
use super::*;
use emela_lir::Module;
#[cfg(test)]
#[path = "link_tests.rs"]
mod tests;
#[path = "link_validate.rs"]
mod validation;

impl CompiledModule {
    pub(in crate::backend) fn source(&self) -> &emela_lir::Module {
        &self.source
    }

    #[cfg(test)]
    pub(in crate::backend) fn lowered(&self) -> &emela_lir::Module {
        &self.lowered
    }

    pub(in crate::backend) fn prepare_link(&self) -> LinkPreparation {
        LinkPreparation {
            compiled: self.clone(),
            original_functions: self.lowered.functions.len(),
            erased_abis: HashSet::new(),
        }
    }
}

/// Existing definitions and side tables are inaccessible for arbitrary mutation.
pub(in crate::backend) struct LinkPreparation {
    compiled: CompiledModule,
    original_functions: usize,
    erased_abis: HashSet<FnId>,
}

impl LinkPreparation {
    pub fn module(&self) -> &emela_lir::Module {
        &self.compiled.lowered
    }

    pub fn tuple_type(&mut self, elements: Vec<TypeId>) -> Result<TypeId, Vec<Error>> {
        if elements
            .iter()
            .any(|id| id.index() >= self.module().core.types.len())
        {
            return Err(invalid_flow_ir(
                "connection",
                "tuple refers to an unknown type",
            ));
        }
        Ok(self
            .compiled
            .lowered
            .core
            .types
            .intern(Type::Tuple(elements)))
    }

    pub fn enum_type(&mut self, id: EnumId) -> Result<TypeId, Vec<Error>> {
        let Some((_, definition)) = self.module().core.enums.iter().find(|(key, _)| *key == id)
        else {
            return Err(invalid_flow_ir("connection", "unknown enum"));
        };
        if definition.params != 0 {
            return Err(invalid_flow_ir(
                "connection",
                "connection enum must be monomorphic",
            ));
        }
        Ok(self
            .compiled
            .lowered
            .core
            .types
            .intern(Type::Enum(id, vec![])))
    }

    /// IDs are append-only. Bodies can be defined later to support recursive specializations.
    pub fn add_function(&mut self, function: Function) -> Result<FnId, Vec<Error>> {
        if self
            .module()
            .functions
            .iter()
            .any(|(_, existing)| existing.name == function.name)
        {
            return Err(invalid_flow_ir(
                &function.name,
                "duplicate connection function name",
            ));
        }
        check_added_header(&function)?;
        validation::references(self.module(), &function)?;
        Ok(self.compiled.lowered.add(function))
    }

    pub fn define_added_function(
        &mut self,
        id: FnId,
        function: Function,
    ) -> Result<(), Vec<Error>> {
        let index = u32::from(id.into_raw()) as usize;
        if index < self.original_functions || index >= self.module().functions.len() {
            return Err(invalid_flow_ir(
                &function.name,
                "only newly added functions can be defined",
            ));
        }
        if self.module().functions[id].name != function.name {
            return Err(invalid_flow_ir(
                &function.name,
                "connection function identity cannot change",
            ));
        }
        check_added_header(&function)?;
        validation::references(self.module(), &function)?;
        self.compiled.lowered.functions[id] = function;
        Ok(())
    }

    /// Erase one function-valued compile-time parameter and its matching call arguments.
    /// Capture sources and locals remain live even when IR operands do not mention them.
    pub fn erase_callback_parameter(
        &mut self,
        targets: &[FnId],
        parameter: usize,
    ) -> Result<(), Vec<Error>> {
        let targets: HashSet<_> = targets.iter().copied().collect();
        for &id in &targets {
            let Some((_, function)) = self.module().functions.iter().find(|(key, _)| *key == id)
            else {
                return Err(invalid_flow_ir(
                    "connection",
                    "unknown callback binding function",
                ));
            };
            let Some(&variable) = function.params.get(parameter) else {
                return Err(invalid_flow_ir(
                    &function.name,
                    "callback parameter is absent",
                ));
            };
            if function.exported || function.constant {
                return Err(invalid_flow_ir(
                    &function.name,
                    "exported and const function ABIs cannot be changed by a connection",
                ));
            }
            if !matches!(
                self.module().core.types[function.vars[variable].ty],
                Type::Fn(_)
            ) {
                return Err(invalid_flow_ir(
                    &function.name,
                    "only function-valued callback parameters can be erased",
                ));
            }
            let closures = &self.compiled.closures;
            if closures.functions.contains_key(&id)
                || closures.families.contains_key(&id)
                || closures
                    .environments
                    .iter()
                    .any(|environment| environment.parent == id)
                || self.compiled.runtime_consts.contains(&id)
                || self
                    .compiled
                    .const_groups
                    .iter()
                    .any(|group| group.canonical == id || group.specializations.contains(&id))
            {
                return Err(invalid_flow_ir(
                    &function.name,
                    "callback ABI is referenced by compiler metadata",
                ));
            }
        }
        let protected = capture_variables(&self.compiled);
        erase_callback_parameter(&mut self.compiled.lowered, &targets, parameter, &protected)?;
        self.erased_abis.extend(targets);
        Ok(())
    }

    /// Freeze only after backend-aware validation; no partially prepared module can be emitted.
    pub fn finish(mut self, roots: Vec<FnId>) -> Result<LinkedModule, Vec<Error>> {
        validate_linked(
            &self.compiled,
            self.original_functions,
            &roots,
            &self.erased_abis,
        )?;
        let reachable = runtime_reachable_from(self.module(), &roots);
        self.compiled.deferred_pool |= fiber_normalize::needs_deferred_pool_for(
            self.module(),
            &self.compiled.closures,
            &reachable,
        );
        Ok(LinkedModule {
            compiled: self.compiled,
            roots,
        })
    }
}

fn check_added_header(function: &Function) -> Result<(), Vec<Error>> {
    if function.exported || function.constant || function.env.is_some() {
        return Err(invalid_flow_ir(
            &function.name,
            "connection functions must be private runtime functions without an environment",
        ));
    }
    Ok(())
}

fn capture_variables(compiled: &CompiledModule) -> HashSet<(FnId, VarId)> {
    let mut variables = HashSet::new();
    for environment in &compiled.closures.environments {
        for capture in &environment.captures {
            variables.insert((environment.parent, capture.source));
        }
    }
    for (&function, &index) in &compiled.closures.functions {
        for capture in &compiled.closures.environments[index].captures {
            variables.insert((function, capture.local));
        }
    }
    variables
}

fn validate_linked(
    compiled: &CompiledModule,
    original_functions: usize,
    roots: &[FnId],
    erased_abis: &HashSet<FnId>,
) -> Result<(), Vec<Error>> {
    let module = &compiled.lowered;
    validation::metadata(compiled)?;
    for &root in roots {
        if u32::from(root.into_raw()) as usize >= module.functions.len() {
            return Err(invalid_flow_ir("connection", "unknown connection root"));
        }
    }
    let reachable = runtime_reachable_from(module, roots);
    for (id, function) in module.functions.iter() {
        let mut checked = function.clone();
        if let Some(&environment) = compiled.closures.functions.get(&id) {
            // Captures are implicit definitions in this backend, not ABI parameters.
            checked.params.extend(
                compiled.closures.environments[environment]
                    .captures
                    .iter()
                    .map(|capture| capture.local),
            );
        }
        let added = u32::from(id.into_raw()) as usize >= original_functions;
        if added {
            check_added_header(function)?;
        }
        if added || reachable.contains(&id) {
            let active = runtime_statements(function);
            for (_, statement) in function.stmts.iter().filter(|(id, _)| active.contains(id)) {
                if let Stmt::Let { value, .. } = statement {
                    match value {
                        Value::FnRef(target)
                            if reachable.contains(&id) && erased_abis.contains(target) =>
                        {
                            return Err(invalid_flow_ir(
                                &function.name,
                                "erased callback ABI still escapes as a function value",
                            ));
                        }
                        Value::Closure(_) | Value::MakeClosure { .. } | Value::EnvGet { .. } => {
                            return Err(invalid_flow_ir(
                                &function.name,
                                "connection function requires new closure metadata",
                            ));
                        }
                        Value::FnRef(target)
                            if added && compiled.closures.functions.contains_key(target) =>
                        {
                            return Err(invalid_flow_ir(
                                &function.name,
                                "connection function cannot capture an existing closure environment",
                            ));
                        }
                        Value::Call {
                            callee: Callee::Direct(target),
                            ..
                        } if added && compiled.closures.functions.contains_key(target) => {
                            return Err(invalid_flow_ir(
                                &function.name,
                                "connection function cannot directly call a closure requiring an environment",
                            ));
                        }
                        _ => {}
                    }
                }
            }
        }
        let errors = emela_lir::verify_function(module, &checked);
        if !errors.is_empty() {
            return Err(errors
                .into_iter()
                .map(|error| Error {
                    kind: ErrorKind::InvalidFlowIr,
                    message: format!("invalid linked IR: {error}"),
                })
                .collect());
        }
    }
    Ok(())
}

fn definition(function: &Function, var: VarId) -> Option<&Value> {
    function.stmts.iter().find_map(|(_, stmt)| match stmt {
        Stmt::Let {
            dst: Some(dst),
            value,
            ..
        } if *dst == var => Some(value),
        _ => None,
    })
}

pub(in crate::backend) fn pure_function_value(module: &Module, id: FnId, fuel: usize) -> bool {
    if fuel == 0 {
        return false;
    }
    let f = &module.functions[id];
    f.params.is_empty()
        && !f.fails
        && !f.suspends
        && f.external.is_none()
        && matches!(module.core.types[f.ret], Type::Fn(_))
        && f.body
            .is_some_and(|body| matches!(f.seqs[body].term, Term::Return(Some(_))))
        && f.stmts.iter().all(|(_, stmt)| match stmt {
            Stmt::Let {
                value: Value::FnRef(_) | Value::Coerce(_) | Value::Const(_),
                ..
            } => true,
            Stmt::Let {
                value:
                    Value::Call {
                        callee: Callee::Direct(target),
                        ..
                    },
                ..
            } => pure_function_value(module, *target, fuel - 1),
            _ => false,
        })
}

pub(in crate::backend) fn resolve_function_value(
    module: &Module,
    function: &Function,
    arg: &Operand,
    seen: &mut HashSet<VarId>,
    fuel: usize,
) -> Option<FnId> {
    if fuel == 0 {
        return None;
    }
    let Operand::Var(var) = arg else { return None };
    if !seen.insert(*var) {
        return None;
    }
    match definition(function, *var)? {
        Value::FnRef(target) => Some(*target),
        Value::Coerce(value) => {
            resolve_function_value(module, function, &(*value).into(), seen, fuel - 1)
        }
        Value::Call {
            callee: Callee::Direct(target),
            args,
            ..
        } if (module.functions[*target].constant || pure_function_value(module, *target, fuel))
            && args.is_empty() =>
        {
            let target = &module.functions[*target];
            let Term::Return(Some(arg)) = &target.seqs[target.body?].term else {
                return None;
            };
            resolve_function_value(module, target, arg, &mut HashSet::new(), fuel - 1)
        }
        _ => None,
    }
}

/// Compiler-owned ABI rewrite; no mutable module escapes to a connection.
fn erase_callback_parameter(
    module: &mut emela_lir::Module,
    targets: &HashSet<FnId>,
    parameter: usize,
    protected: &HashSet<(FnId, VarId)>,
) -> Result<(), Vec<Error>> {
    if targets.is_empty() {
        return Ok(());
    }
    let mut redirects = Vec::new();
    let mut discard_candidates: HashMap<FnId, HashSet<VarId>> = HashMap::new();
    for (id, function) in module.functions.iter() {
        for (statement, stmt) in function.stmts.iter() {
            if let Stmt::Let {
                value:
                    Value::Call {
                        callee: Callee::Indirect(var),
                        ..
                    },
                ..
            } = stmt
                && let Some(target) = resolve_function_value(
                    module,
                    function,
                    &(*var).into(),
                    &mut HashSet::new(),
                    128,
                )
                && targets.contains(&target)
            {
                redirects.push((id, statement, target));
                discard_candidates.entry(id).or_default().insert(*var);
            }
        }
    }
    for (id, statement, target) in redirects {
        if let Stmt::Let {
            value: Value::Call { callee, .. },
            ..
        } = &mut module.functions[id].stmts[statement]
        {
            *callee = Callee::Direct(target);
        }
    }
    for &id in targets {
        if module.functions[id].params.len() > parameter {
            module.functions[id].params.remove(parameter);
        }
    }
    let constants: HashSet<_> = module
        .functions
        .iter()
        .filter_map(|(id, _)| pure_function_value(module, id, 128).then_some(id))
        .collect();
    for (function_id, function) in module.functions.iter_mut() {
        if function.name.starts_with("$emb_") {
            continue;
        }
        for (_, statement) in function.stmts.iter_mut() {
            if let Stmt::Let {
                value:
                    Value::Call {
                        callee: Callee::Direct(id),
                        args,
                        ..
                    },
                ..
            } = statement
                && targets.contains(id)
                && args.len() > parameter
                && let Operand::Var(variable) = args.remove(parameter)
            {
                discard_candidates
                    .entry(function_id)
                    .or_default()
                    .insert(variable);
            }
        }
        loop {
            let mut used = HashSet::new();
            fn operand(used: &mut HashSet<VarId>, arg: &Operand) {
                if let Operand::Var(v) = arg {
                    used.insert(*v);
                }
            }
            for (_, statement) in function.stmts.iter() {
                match statement {
                    Stmt::Let { value, .. } => value.for_each_var(|v| {
                        used.insert(v);
                    }),
                    Stmt::If { cond, .. } => operand(&mut used, cond),
                    Stmt::Switch {
                        on: SwitchOn::Tag(v) | SwitchOn::Lit(v),
                        ..
                    }
                    | Stmt::Dup(v)
                    | Stmt::Drop(v) => {
                        used.insert(*v);
                    }
                    Stmt::Loop { inits, .. } => {
                        for arg in inits {
                            operand(&mut used, arg);
                        }
                    }
                    _ => {}
                }
            }
            for (_, seq) in function.seqs.iter() {
                match &seq.term {
                    Term::Return(arg) | Term::Yield(arg) | Term::Break { value: arg, .. } => {
                        if let Some(arg) = arg {
                            operand(&mut used, arg);
                        }
                    }
                    Term::Fail(arg) | Term::Defect(arg) => operand(&mut used, arg),
                    Term::Recur(args) => {
                        for arg in args {
                            operand(&mut used, arg);
                        }
                    }
                    _ => {}
                }
            }
            let mut changed = false;
            for (_, statement) in function.stmts.iter_mut() {
                if let Stmt::Let {
                    dst: Some(dst),
                    value,
                    ..
                } = statement
                    && !used.contains(dst)
                    && !protected.contains(&(function_id, *dst))
                    && discard_candidates
                        .get(&function_id)
                        .is_some_and(|variables| variables.contains(dst))
                    && matches!(module.core.types[function.vars[*dst].ty], Type::Fn(_))
                    && (matches!(value, Value::FnRef(_) | Value::Coerce(_))
                        || matches!(value, Value::Call { callee: Callee::Direct(id), .. } if constants.contains(id)))
                    && !matches!(value, Value::Const(_))
                {
                    value.for_each_var(|variable| {
                        discard_candidates
                            .entry(function_id)
                            .or_default()
                            .insert(variable);
                    });
                    *value = Value::Const(Const::Int(0));
                    function.vars[*dst].ty = TypeId::INT;
                    function.vars[*dst].repr = Repr::I32;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
    }
    Ok(())
}
