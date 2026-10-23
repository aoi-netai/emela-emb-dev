//! Static GPIO interrupt binding and evidence-free ISR specializations.
use super::*;
use emela_lir::{FnBuilder, Module};

pub(super) struct Binding {
    pin: i32,
    callback: FnId,
}

#[derive(Default)]
pub(super) struct Plan {
    bindings: Vec<Binding>,
}

fn named(function: &Function, name: &str) -> bool {
    function.name.split('[').next() == Some(name)
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

fn fn_ref(
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
        Value::Coerce(value) => fn_ref(module, function, &(*value).into(), seen, fuel - 1),
        Value::Call {
            callee: Callee::Direct(target),
            args,
            ..
        } if module.functions[*target].constant && args.is_empty() => {
            let target = &module.functions[*target];
            let Term::Return(Some(arg)) = &target.seqs[target.body?].term else {
                return None;
            };
            fn_ref(module, target, arg, &mut HashSet::new(), fuel - 1)
        }
        _ => None,
    }
}

fn pin_id(module: &Module, function: &Function, arg: &Operand, fuel: usize) -> Option<i32> {
    if fuel == 0 {
        return None;
    }
    match arg {
        Operand::Const(Const::Int(value)) => Some(*value),
        Operand::Var(var) => match definition(function, *var)? {
            Value::Const(Const::Int(value)) => Some(*value),
            Value::Make { ctor, args }
                if module.core.enums[ctor.enum_id].name == "Std.Embedded.Gpio.GpioPin"
                    && args.len() == 1 =>
            {
                pin_id(module, function, &args[0], fuel - 1)
            }
            Value::Coerce(var) => pin_id(module, function, &(*var).into(), fuel - 1),
            Value::Prim {
                op,
                ty: PrimTy::I32,
                args,
            } => {
                let first = pin_id(module, function, args.first()?, fuel - 1)?;
                if *op == PrimOp::Neg {
                    return Some(first.wrapping_neg());
                }
                let second = pin_id(module, function, args.get(1)?, fuel - 1)?;
                match op {
                    PrimOp::Add => Some(first.wrapping_add(second)),
                    PrimOp::Sub => Some(first.wrapping_sub(second)),
                    PrimOp::Mul => Some(first.wrapping_mul(second)),
                    PrimOp::Div if second != 0 => Some(first.wrapping_div(second)),
                    PrimOp::Rem if second != 0 => Some(first.wrapping_rem(second)),
                    _ => None,
                }
            }
            Value::Call {
                callee: Callee::Direct(target),
                args,
                ..
            } if module.functions[*target].constant && args.is_empty() => {
                let f = &module.functions[*target];
                let Term::Return(Some(arg)) = &f.seqs[f.body?].term else {
                    return None;
                };
                pin_id(module, f, arg, fuel - 1)
            }
            _ => None,
        },
        _ => None,
    }
}

/// Collect before closure lifting so lambdas cannot masquerade as named callbacks.
pub(super) fn collect(module: &Module) -> Result<Vec<(i32, FnId)>, Vec<Error>> {
    let reachable = runtime_reachable(module);
    let mut bindings = Vec::new();
    let mut pins = HashSet::new();
    for (id, function) in module.functions.iter() {
        if !reachable.contains(&id) {
            continue;
        }
        // Binding declarations cannot escape into a runtime higher-order call.
        let binding_values: HashSet<_> = function
            .vars
            .iter()
            .filter_map(|(var, _)| {
                fn_ref(module, function, &var.into(), &mut HashSet::new(), 128)
                    .filter(|target| {
                        named(
                            &module.functions[*target],
                            "Std.Embedded.Gpio.bind_interrupt",
                        )
                    })
                    .map(|_| var)
            })
            .collect();
        for (_, stmt) in function.stmts.iter() {
            if let Stmt::Let { value, .. } = stmt {
                let mut used = false;
                value.for_each_var(|var| used |= binding_values.contains(&var));
                let allowed = matches!(value, Value::Coerce(_))
                    || matches!(value, Value::Call { callee: Callee::Indirect(var), args, .. } if binding_values.contains(var) && !args.iter().any(|arg| matches!(arg, Operand::Var(v) if binding_values.contains(v))));
                if used && !allowed {
                    return Err(unsupported(
                        &function.name,
                        "GPIO binding function must be used in a static call",
                    ));
                }
            }
        }
        for (_, seq) in function.seqs.iter() {
            let operands: Vec<_> = match &seq.term {
                Term::Return(_) if function.constant => vec![],
                Term::Return(value) | Term::Yield(value) | Term::Break { value, .. } => {
                    value.iter().collect()
                }
                Term::Recur(args) => args.iter().collect(),
                _ => vec![],
            };
            if operands
                .iter()
                .any(|arg| matches!(arg, Operand::Var(var) if binding_values.contains(var)))
            {
                return Err(unsupported(
                    &function.name,
                    "GPIO binding function must be used in a static call",
                ));
            }
        }
        for (_, statement) in function.stmts.iter() {
            let Stmt::Let {
                value: Value::Call { callee, args, .. },
                ..
            } = statement
            else {
                continue;
            };
            let target = match callee {
                Callee::Direct(target) => *target,
                Callee::Indirect(var) => {
                    match fn_ref(module, function, &(*var).into(), &mut HashSet::new(), 128) {
                        Some(id) => id,
                        None => continue,
                    }
                }
                _ => continue,
            };
            if !named(
                &module.functions[target],
                "Std.Embedded.Gpio.bind_interrupt",
            ) {
                continue;
            }
            let pin = args
                .first()
                .and_then(|arg| pin_id(module, function, arg, 128))
                .ok_or_else(|| {
                    unsupported(
                        &function.name,
                        "GPIO binding requires a statically resolved GpioPin constant",
                    )
                })?;
            let callback = args
                .get(1)
                .and_then(|arg| fn_ref(module, function, arg, &mut HashSet::new(), 128))
                .ok_or_else(|| {
                    unsupported(
                        &function.name,
                        "GPIO binding requires a capture-free named callback",
                    )
                })?;
            let f = &module.functions[callback];
            let params: Vec<_> = f
                .params
                .iter()
                .map(|p| f.vars[*p].ty)
                .filter(|ty| !matches!(module.core.types[*ty], Type::Evidence))
                .collect();
            if f.fails
                || f.suspends
                || f.external.is_some()
                || f.env.is_some()
                || f.ret != TypeId::UNIT
                || params.len() != 1
                || !matches!(module.core.types[params[0]], Type::Enum(event, _) if module.core.enums[event].name == "Std.Embedded.Gpio.GpioEvent")
            {
                return Err(unsupported(
                    &f.name,
                    "GPIO callback must return Unit without escaping failures or suspension",
                ));
            }
            if !pins.insert(pin) {
                return Err(unsupported(
                    &function.name,
                    &format!("duplicate GPIO interrupt binding for pin {pin}"),
                ));
            }
            bindings.push((pin, callback));
        }
    }
    Ok(bindings)
}

pub(super) fn prepare(module: &mut Module, bindings: Vec<(i32, FnId)>) -> Result<Plan, Vec<Error>> {
    let mut plan = Plan::default();
    let mut specialized = HashMap::new();
    let mut operations = HashMap::new();
    for (pin, callback) in bindings {
        let callback = specialize(module, callback, &mut specialized, &mut operations)?;
        plan.bindings.push(Binding { pin, callback });
    }
    Ok(plan)
}

fn isr_operation(
    module: &mut Module,
    effect: EffectId,
    op: usize,
    operations: &mut HashMap<usize, FnId>,
) -> Result<FnId, Vec<Error>> {
    if let Some(id) = operations.get(&op) {
        return Ok(*id);
    }
    let operation = module.core.effects[effect].ops[op].clone();
    let tuple = module
        .core
        .types
        .intern(Type::Tuple(vec![TypeId::INT, operation.ret]));
    let mut raw = FnBuilder::new(
        &module.core,
        &format!("Std.Embedded.Board.gpio_isr_{}", operation.name),
    );
    raw.set_ret(tuple);
    for (i, &ty) in operation.params.iter().enumerate() {
        raw.param(&format!("arg{i}"), ty);
    }
    let raw = module.add(raw.finish_external(emela_core::External {
        bindings: vec![emela_core::ExternalBinding {
            target: "c".into(),
            host: emela_core::HostRef::Global(format!(
                "__emela_peripheral_gpio_isr_{}",
                operation.name
            )),
        }],
    }));
    let check = module
        .functions
        .iter()
        .find(|(_, f)| named(f, "Std.Embedded.Gpio.check_status"))
        .map(|(id, _)| id)
        .expect("binding retains GPIO status conversion");
    let mut b = FnBuilder::new(
        &module.core,
        &format!("$emb_gpio_isr$v1$op_{}", operation.name),
    );
    b.set_ret(operation.ret).set_fails();
    let args = operation
        .params
        .iter()
        .enumerate()
        .map(|(i, &ty)| b.param(&format!("arg{i}"), ty).into())
        .collect();
    let result = b.assign(
        "result",
        tuple,
        Value::Call {
            callee: Callee::Direct(raw),
            args,
            suspends: false,
            on_fail: None,
        },
    );
    let status = b.assign(
        "status",
        TypeId::INT,
        Value::TupleGet {
            base: result,
            index: 0,
        },
    );
    b.let_(
        None,
        Value::Call {
            callee: Callee::Direct(check),
            args: vec![status.into()],
            suspends: false,
            on_fail: Some(OnFail::Propagate),
        },
    );
    if operation.ret == TypeId::UNIT {
        b.term(Term::Return(None));
    } else {
        let value = b.assign(
            "level",
            operation.ret,
            Value::TupleGet {
                base: result,
                index: 1,
            },
        );
        b.term(Term::Return(Some(value.into())));
    }
    let id = module.add(b.finish());
    operations.insert(op, id);
    Ok(id)
}

fn specialize(
    module: &mut Module,
    target: FnId,
    specialized: &mut HashMap<FnId, FnId>,
    operations: &mut HashMap<usize, FnId>,
) -> Result<FnId, Vec<Error>> {
    if let Some(id) = specialized.get(&target) {
        return Ok(*id);
    }
    let original = module.functions[target].clone();
    if original.external.is_some() {
        return Ok(target);
    }
    if original
        .stmts
        .iter()
        .any(|(_, stmt)| matches!(stmt, Stmt::Loop { .. }))
    {
        return Err(unsupported(
            &original.name,
            "recursive call reachable from ISR (including optimized tail recursion)",
        ));
    }
    let mut function = original.clone();
    function.name = format!(
        "$emb_gpio_isr$v1${}_{}",
        u32::from(target.into_raw()),
        original.name
    );
    function.exported = false;
    // ISR constants are evaluated from their pure fixed-value definitions, without
    // entering main's lazy initialization cells. Heap and recursion checks still apply.
    function.constant = false;
    function
        .params
        .retain(|p| !matches!(module.core.types[function.vars[*p].ty], Type::Evidence));
    let id = module.add(function.clone());
    specialized.insert(target, id);
    for (_, stmt) in function.stmts.iter_mut() {
        let Stmt::Let { dst, value, .. } = stmt else {
            continue;
        };
        match value {
            Value::EvLookup { effect, .. }
                if module.core.effects[*effect].name == "Std.Embedded.Gpio.Gpio" =>
            {
                if let Some(dst) = dst {
                    function.vars[*dst].ty = TypeId::INT;
                    function.vars[*dst].repr = Repr::I32;
                }
                *value = Value::Const(Const::Int(0));
            }
            Value::Call { callee, args, .. } => {
                let next = match callee {
                    Callee::Op { effect, op, .. } => {
                        let def = &module.core.effects[*effect];
                        if def.name != "Std.Embedded.Gpio.Gpio"
                            || !matches!(def.ops[*op].name.as_str(), "read" | "write")
                        {
                            return Err(unsupported(
                                &original.name,
                                &format!(
                                    "operation {}.{} is forbidden in a GPIO ISR",
                                    def.name, def.ops[*op].name
                                ),
                            ));
                        }
                        isr_operation(module, *effect, *op, operations)?
                    }
                    Callee::Direct(next) => {
                        let params = module.functions[*next].params.clone();
                        *args = args
                            .iter()
                            .zip(params)
                            .filter(|(_, p)| {
                                !matches!(
                                    module.core.types[module.functions[*next].vars[*p].ty],
                                    Type::Evidence
                                )
                            })
                            .map(|(arg, _)| arg.clone())
                            .collect();
                        specialize(module, *next, specialized, operations)?
                    }
                    Callee::Indirect(_) => continue,
                };
                *callee = Callee::Direct(next);
            }
            _ => {}
        }
    }
    // Removed evidence parameters are not runtime values in the specialization.
    for (_, var) in function.vars.iter_mut() {
        if matches!(module.core.types[var.ty], Type::Evidence) {
            var.ty = TypeId::INT;
            var.repr = Repr::I32;
        }
    }
    module.functions[id] = function;
    Ok(id)
}

impl Plan {
    pub(super) fn validate(
        &self,
        flow: &Module,
        heap: &HashSet<FnId>,
        layouts: &TypeLayouts<'_>,
        names: &HashMap<FnId, String>,
        options: &Options,
    ) -> Result<(), Vec<Error>> {
        for binding in &self.bindings {
            validate_isr_function(
                flow,
                binding.callback,
                &flow.functions[binding.callback].name,
                heap,
                layouts,
                names,
                options,
                &mut HashSet::new(),
                &mut HashSet::new(),
            )?;
        }
        Ok(())
    }

    pub(super) fn emit(
        &self,
        flow: &Module,
        layouts: &TypeLayouts<'_>,
        names: &HashMap<FnId, String>,
        writer: &mut Writer,
    ) {
        writer.begin_block("int32_t emela_gpio_has_binding(int32_t pin)");
        writer.line("(void)pin;");
        for binding in &self.bindings {
            writer.line(&format!("if (pin == INT32_C({})) return 1;", binding.pin));
        }
        writer.ret(Some("0"));
        writer.end_block();
        writer.begin_block("void emela_gpio_dispatch(int32_t pin, int32_t level)");
        writer.line("(void)pin; (void)level;");
        writer.line("if (level != 0 && level != 1) return;");
        for binding in &self.bindings {
            let function = &flow.functions[binding.callback];
            let event = function.vars[function.params[0]].ty;
            let pin = layouts.enum_fields(event).expect("GPIO event fields")[0][0];
            writer.line(&format!("if (pin == INT32_C({})) {{ {}(({}){{ .tag = 0, .payload.v0 = {{ .f0 = ({}){{ .tag = 0, .payload.v0.f0 = INT32_C({}) }}, .f1 = level }} }}); return; }}", binding.pin, names[&binding.callback], layouts.c_type(event), layouts.c_type(pin), binding.pin));
        }
        writer.end_block();
    }
}
