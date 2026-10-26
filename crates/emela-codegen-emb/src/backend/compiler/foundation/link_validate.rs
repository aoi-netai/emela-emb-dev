//! Check arena references before the shared verifier or a connection can index them.
use super::*;

pub(super) fn metadata(compiled: &CompiledModule) -> Result<(), Vec<Error>> {
    let module = &compiled.lowered;
    let known = |id: FnId| (u32::from(id.into_raw()) as usize) < module.functions.len();
    let closures = &compiled.closures;
    let bad = || {
        invalid_flow_ir(
            "connection",
            "closure or const metadata is inconsistent with linked IR",
        )
    };
    if closures.module.is_some()
        || closures
            .families
            .iter()
            .any(|(id, (parent, _))| !known(*id) || !known(*parent))
        || compiled.runtime_consts.iter().any(|id| !known(*id))
        || compiled.const_groups.iter().any(|group| {
            !known(group.canonical) || group.specializations.iter().any(|id| !known(*id))
        })
    {
        return Err(bad());
    }
    for environment in &closures.environments {
        if !known(environment.parent)
            || environment.function_type.index() >= module.core.types.len()
            || !matches!(module.core.types[environment.function_type], Type::Fn(_))
        {
            return Err(bad());
        }
        let parent = &module.functions[environment.parent];
        for capture in &environment.captures {
            if (u32::from(capture.source.into_raw()) as usize) >= parent.vars.len()
                || parent.vars[capture.source].ty != capture.ty
            {
                return Err(bad());
            }
        }
    }
    for (&id, &index) in &closures.functions {
        let Some(environment) = closures.environments.get(index) else {
            return Err(bad());
        };
        if !known(id) {
            return Err(bad());
        }
        let function = &module.functions[id];
        for capture in &environment.captures {
            if (u32::from(capture.local.into_raw()) as usize) >= function.vars.len()
                || function.vars[capture.local].ty != capture.ty
                || function.params.contains(&capture.local)
            {
                return Err(bad());
            }
        }
    }
    Ok(())
}

pub(super) fn references(module: &Module, function: &Function) -> Result<(), Vec<Error>> {
    let variable = |id: VarId| (u32::from(id.into_raw()) as usize) < function.vars.len();
    let sequence = |id: SeqId| (u32::from(id.into_raw()) as usize) < function.seqs.len();
    let label = |id: LabelId| (u32::from(id.into_raw()) as usize) < function.labels.len();
    let target = |id: FnId| (u32::from(id.into_raw()) as usize) < module.functions.len();
    let enum_exists = |id: EnumId| module.core.enums.iter().any(|(key, _)| key == id);
    let effect_exists = |id: EffectId| module.core.effects.iter().any(|(key, _)| key == id);
    let operand = |value: &Operand| match value {
        Operand::Var(id) => variable(*id),
        Operand::Const(_) => true,
    };
    let failure = |edge: &Option<OnFail>| match edge {
        Some(OnFail::Break(id)) => label(*id),
        Some(OnFail::Handle { error, body }) => variable(*error) && sequence(*body),
        Some(OnFail::Propagate) | None => true,
    };
    let constructor =
        |ctor: &CtorRef| {
            module.core.enums.iter().any(|(id, definition)| {
                id == ctor.enum_id && ctor.variant < definition.variants.len()
            })
        };
    let bad = || {
        invalid_flow_ir(
            &function.name,
            "connection function contains an invalid arena reference",
        )
    };
    if function.ret.index() >= module.core.types.len()
        || function
            .vars
            .iter()
            .any(|(_, data)| data.ty.index() >= module.core.types.len())
        || function.params.iter().any(|&id| !variable(id))
        || function.body.is_some_and(|id| !sequence(id))
    {
        return Err(bad());
    }
    for (_, seq) in function.seqs.iter() {
        if seq
            .stmts
            .iter()
            .any(|id| (u32::from(id.into_raw()) as usize) >= function.stmts.len())
        {
            return Err(bad());
        }
        let valid = match &seq.term {
            Term::Return(value) | Term::Yield(value) => value.as_ref().is_none_or(operand),
            Term::Fail(value) | Term::Defect(value) => operand(value),
            Term::Break { label: id, value } => label(*id) && value.as_ref().is_none_or(operand),
            Term::Recur(args) => args.iter().all(&operand),
            Term::Unwind | Term::Unreachable => true,
        };
        if !valid {
            return Err(bad());
        }
    }
    for (_, statement) in function.stmts.iter() {
        if statement.dst().is_some_and(|id| !variable(id)) {
            return Err(bad());
        }
        let valid = match statement {
            Stmt::Let {
                value,
                on_defect,
                on_suspend,
                ..
            } => {
                let mut valid = true;
                value.for_each_var(|id| valid &= variable(id));
                if !valid || on_defect.is_some() || on_suspend.is_some() {
                    return Err(bad());
                }
                match value {
                    Value::Call {
                        callee, on_fail, ..
                    } => {
                        failure(on_fail)
                            && match callee {
                                Callee::Direct(id) => target(*id),
                                Callee::Indirect(_) => true,
                                Callee::Op { effect, op, .. } => module
                                    .core
                                    .effects
                                    .iter()
                                    .any(|(id, def)| id == *effect && *op < def.ops.len()),
                            }
                    }
                    Value::FnRef(id) => target(*id),
                    Value::Make { ctor, .. } | Value::Field { ctor, .. } => constructor(ctor),
                    Value::EvLookup { effect, .. } | Value::Handler { effect, .. } => {
                        effect_exists(*effect)
                    }
                    Value::EvExtend { bindings, .. } => {
                        bindings.iter().all(|(id, _)| effect_exists(*id))
                    }
                    Value::IsError { enum_id, .. } | Value::AsError { enum_id, .. } => {
                        enum_exists(*enum_id)
                    }
                    Value::Fiber { on_fail, .. } => failure(on_fail),
                    Value::Closure(_) | Value::MakeClosure { .. } | Value::EnvGet { .. } => {
                        return Err(invalid_flow_ir(
                            &function.name,
                            "connection function requires new closure metadata",
                        ));
                    }
                    _ => true,
                }
            }
            Stmt::If {
                cond, then, else_, ..
            } => operand(cond) && sequence(*then) && sequence(*else_),
            Stmt::Switch {
                on, cases, default, ..
            } => {
                let var = match on {
                    SwitchOn::Tag(id) | SwitchOn::Lit(id) => *id,
                };
                variable(var)
                    && cases.iter().all(|case| sequence(case.body))
                    && default.is_none_or(sequence)
            }
            Stmt::Loop {
                vars, inits, body, ..
            } => {
                vars.iter().all(|&id| variable(id)) && inits.iter().all(&operand) && sequence(*body)
            }
            Stmt::Block {
                label: id, body, ..
            } => label(*id) && sequence(*body),
            Stmt::Protect { body, cleanup, .. } => sequence(*body) && sequence(*cleanup),
            Stmt::Dup(_) | Stmt::Drop(_) => false,
        };
        if !valid {
            return Err(bad());
        }
    }
    Ok(())
}
