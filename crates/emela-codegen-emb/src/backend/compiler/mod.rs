#[path = "foundation/traversal.rs"]
mod traversal;
pub(crate) use traversal::{function_positions, runtime_statements};
#[path = "foundation/reachability.rs"]
mod reachability;
pub(crate) use reachability::runtime_reachable;
mod emit;
pub(super) use emit::emit;
#[path = "foundation/layouts.rs"]
pub(super) mod layouts;
#[path = "foundation/type_normalize.rs"]
mod type_normalize;
pub(super) use layouts::*;
#[path = "foundation/fiber_normalize.rs"]
pub(crate) mod fiber_normalize;
use features::builtins::string_split::emit_string_split_push;
use features::values::coerce::emit_coerce;
#[path = "foundation/analysis.rs"]
mod analysis;
#[path = "features/mod.rs"]
mod features;
use analysis::*;
#[path = "foundation/heap_plan.rs"]
mod heap_plan;
#[path = "foundation/numeric_runtime.rs"]
mod numeric_runtime;
#[path = "foundation/runtime_plan.rs"]
mod runtime_plan;
#[path = "foundation/string_runtime.rs"]
mod string_runtime;
use heap_plan::function_needs_heap;
#[path = "foundation/string_buffers.rs"]
mod string_buffers;
pub(super) use std::collections::{HashMap, HashSet};
use string_buffers::{
    StringBufferPlan, count_operand, for_each_string_plan_var, functions_with_string_output,
};

pub(super) use emela_core::{Builtin, CtorRef, EffectId, EnumId, Type, TypeId, TypeTable};
pub(super) use emela_lir::{
    Callee, CaseKey, Const, FnId, Function, LabelId, OnFail, Operand, PrimOp, PrimTy, Repr,
    RuntimeOp, SeqId, Stmt, StmtId, SwitchOn, Term, Value, VarId,
};

use crate::const_eval::{self, Outcome as ConstOutcome};
pub(super) use crate::writer::Writer;
pub(super) use crate::{Error, ErrorKind, ExportedFunction, GeneratedFile, Options, Output};

#[path = "foundation/cause.rs"]
mod cause;
#[path = "foundation/child.rs"]
mod child;
#[path = "foundation/const_bridge.rs"]
mod const_bridge;
#[path = "foundation/deferred.rs"]
mod deferred;
#[path = "foundation/derive_runtime.rs"]
mod derive_runtime;
#[path = "foundation/error.rs"]
mod error_runtime;
#[path = "foundation/external.rs"]
pub(super) mod external;
#[path = "foundation/owned.rs"]
mod owned;
#[path = "suspend.rs"]
mod suspend;
#[path = "foundation/task_values.rs"]
mod task_values;

pub(super) const HEADER_FILE: &str = "emela_embedded.h";
const SOURCE_FILE: &str = "emela_embedded.c";
mod platform;
mod target;
pub(super) use target::{LinkContext, Target};

/// Owned, target-independent lowered program. Not a finalized C string or a stable file format.
#[derive(Clone)]
pub(super) struct CompiledModule {
    lowered: emela_lir::Module,
    closures: features::values::closure::ClosureLowering,
    const_groups: Vec<crate::const_origins::Group>,
    runtime_consts: HashSet<FnId>,
    deferred_pool: bool,
}

impl CompiledModule {
    #[cfg(test)]
    pub(in crate::backend) fn lowered(&self) -> &emela_lir::Module {
        &self.lowered
    }
}

/// Compile Flow IR once; no peripheral recognition or target selection happens here.
pub(super) fn compile_flow(flow: &emela_lir::Module) -> Result<CompiledModule, Vec<Error>> {
    check_input_stage(flow.stage)?;
    let mut source = flow.clone();
    normalize_flow_types(&mut source)?;
    crate::verify_flow(&source)?;
    let flow = &source;
    let const_groups = crate::const_origins::groups(flow)?;
    crate::const_origins::function_groups(flow)?;
    let mut prepared_flow = flow.clone();
    external::normalize_exports(&mut prepared_flow);
    const_bridge::prepare(&mut prepared_flow, &const_groups);
    let mut runtime_consts = crate::const_runtime::prepare(&mut prepared_flow, runtime_reachable);
    runtime_consts.extend(
        const_groups
            .iter()
            .flat_map(|group| group.specializations.iter().copied()),
    );

    crate::scope_abi::normalize(&mut prepared_flow)?;
    fiber_normalize::normalize_scope_results(&mut prepared_flow);
    let mut closures = features::values::closure::lower(&prepared_flow)?;
    let mut lowered_flow = closures
        .module
        .take()
        .expect("closure lowering returns its Flow IR module");
    fiber_normalize::erase_inert_failures(&mut lowered_flow, &mut closures);
    let closure_functions: HashSet<_> = closures.functions.keys().copied().collect();
    features::values::coerce::lower(&mut lowered_flow, &closure_functions);
    let deferred_pool = fiber_normalize::needs_deferred_pool(&lowered_flow, &closures);
    features::values::handler::lower(&mut lowered_flow);
    Ok(CompiledModule {
        lowered: lowered_flow,
        closures,
        const_groups,
        runtime_consts,
        deferred_pool,
    })
}

fn emit_local_declarations(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    string_buffers: &StringBufferPlan,
    writer: &mut Writer,
) {
    for (id, variable) in function.vars.iter() {
        if !function.params.contains(&id) {
            if layouts.is_managed_value(variable.ty) {
                let initial = if layouts.is_managed_list(variable.ty) {
                    "NULL"
                } else {
                    "{0}"
                };
                writer.decl(
                    &layouts.c_type(variable.ty),
                    &variable_name(id),
                    Some(initial),
                );
                writer.decl("int", &owned_variable_name(id), Some("INT32_C(0)"));
            } else {
                writer.decl(&layouts.c_type(variable.ty), &variable_name(id), None);
            }
        }
    }
    for (name, capacity) in &string_buffers.declarations {
        writer.line(&format!("char {name}[{capacity}];"));
    }
    for (variable, _) in function.vars.iter() {
        writer.line(&format!("(void)&{};", variable_name(variable)));
    }
}

fn emit_closure_environment_declarations(
    layouts: &TypeLayouts<'_>,
    closures: &features::values::closure::ClosureLowering,
    writer: &mut Writer,
) {
    for (index, environment) in closures.environments.iter().enumerate() {
        writer.line("typedef struct {");
        let mut represented = false;
        for (capture_index, capture) in environment.captures.iter().enumerate() {
            if layouts.type_repr(capture.ty) == Repr::Void {
                continue;
            }
            represented = true;
            writer.line(&format!(
                "  {} c{capture_index};",
                layouts.c_type(capture.ty)
            ));
        }
        if !represented {
            writer.line("  uint8_t _unit;");
        }
        writer.line(&format!("}} emela_closure_env_{index};"));
        writer.blank();
    }
}

fn emit_closure_string_copy_helper(layouts: &TypeLayouts<'_>, writer: &mut Writer) {
    let kind = layouts.string_owner_kind();
    writer.begin_block("static inline EmelaString emela_heap_copy_string(EmelaString value)");
    writer.line("if (value.length == UINT16_C(0)) return (EmelaString){ .data = \"\" };");
    writer.line(&format!(
        "char *bytes = (char *)emela_heap_allocate((size_t)value.length, UINT32_C({kind}));"
    ));
    writer.line("for (uint32_t index = UINT32_C(0); index < (uint32_t)value.length; ++index)");
    writer.begin_block("");
    writer.line("bytes[index] = value.data[index];");
    writer.end_block();
    writer.line("EmelaString copy = { bytes, value.length, bytes };");
    writer.ret(Some("copy"));
    writer.end_block();
    writer.blank();
    writer.begin_block("static inline EmelaString emela_heap_own_string(EmelaString value)");
    writer.line("if (value.length == UINT16_C(0)) return (EmelaString){ .data = \"\" };");
    writer.line("if (value.owner == NULL) return emela_heap_copy_string(value);");
    writer.line("emela_heap_retain(value.owner);");
    writer.line("return value;");
    writer.end_block();
}

fn emit_closure_capture_loads(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    environment: usize,
    writer: &mut Writer,
) {
    let captures = &layouts.closure_environments[environment].captures;
    writer.begin_block("if (emela_env == NULL)");
    writer.line(
        "emela_board_defect(EMELA_DEFECT_PANIC, \"null closure environment\", UINT16_C(24));",
    );
    writer.end_block();
    for (capture_index, capture) in captures.iter().enumerate() {
        if layouts.type_repr(capture.ty) == Repr::Void {
            continue;
        }
        let source = format!("((emela_closure_env_{environment} *)emela_env)->c{capture_index}");
        if layouts.is_managed_value(capture.ty) {
            layouts.emit_retain_value_expression(capture.ty, &source, writer);
        }
        writer.assign(&variable_name(capture.local), &source);
        if layouts.is_managed_value(function.vars[capture.local].ty) {
            writer.assign(&owned_variable_name(capture.local), "INT32_C(1)");
        }
    }
}

fn emit_release_if_owned(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    variable: VarId,
    writer: &mut Writer,
) {
    let ty = function.vars[variable].ty;
    if !layouts.is_managed_value(ty) {
        return;
    }
    writer.begin_block(&format!(
        "if ({} != INT32_C(0))",
        owned_variable_name(variable)
    ));
    layouts.emit_release_value_expression(ty, &variable_name(variable), writer);
    writer.end_block();
    writer.assign(&owned_variable_name(variable), "INT32_C(0)");
}

fn emit_retain_operand(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    operand: &Operand,
    writer: &mut Writer,
) {
    if let Operand::Var(variable) = operand
        && layouts.is_managed_value(function.vars[*variable].ty)
    {
        layouts.emit_retain_value_expression(
            function.vars[*variable].ty,
            &variable_name(*variable),
            writer,
        );
    }
}

fn emit_owned_cleanup(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    except: Option<VarId>,
    writer: &mut Writer,
) {
    for (variable, data) in function.vars.iter() {
        if except == Some(variable)
            || function.params.contains(&variable)
            || !layouts.is_managed_value(data.ty)
        {
            continue;
        }
        emit_release_if_owned(layouts, function, variable, writer);
    }
    if layouts.has_cause_raise && layouts.has_unwind && !function.suspends && !function.constant {
        writer.line(
            "if (emela_current_fiber != NULL) emela_current_fiber->holds = emela_entry_holds;",
        );
    }
}

fn operand_type(function: &Function, operand: &Operand) -> Option<TypeId> {
    match operand {
        Operand::Var(variable) => Some(function.vars[*variable].ty),
        Operand::Const(Const::Int(_)) => Some(TypeId::INT),
        Operand::Const(Const::Bool(_)) => Some(TypeId::BOOL),
        Operand::Const(Const::Int64(_)) => Some(TypeId::INT64),
        Operand::Const(Const::Float(_)) => Some(TypeId::FLOAT),
        Operand::Const(Const::String(_)) => Some(TypeId::STRING),
        Operand::Const(Const::Unit) => Some(TypeId::UNIT),
    }
}

fn validate_function_value_types(
    layouts: &TypeLayouts<'_>,
    emitted: &HashSet<FnId>,
    flow: &emela_lir::Module,
) -> Result<(), Vec<Error>> {
    for &ty in &layouts.function_types {
        let Type::Fn(function_type) = layouts.types[ty].clone() else {
            unreachable!("function type registry contains a non-function")
        };
        if function_type.sets.is_some()
            || !function_value_abi_type_supported(layouts, function_type.ret, &mut HashSet::new())
            || function_type.params.iter().any(|&parameter| {
                !function_value_abi_type_supported(layouts, parameter, &mut HashSet::new())
            })
        {
            let owner = flow
                .functions
                .iter()
                .find(|(id, function)| {
                    emitted.contains(id)
                        && (function.ret == ty
                            || function.vars.iter().any(|(_, variable)| variable.ty == ty))
                })
                .map_or("function value", |(_, function)| function.name.as_str());
            return Err(unsupported(
                owner,
                "function values with unsupported parameter, result, or suspend types",
            ));
        }
    }
    Ok(())
}

fn function_value_abi_type_supported(
    layouts: &TypeLayouts<'_>,
    ty: TypeId,
    seen: &mut HashSet<TypeId>,
) -> bool {
    match layouts.types[ty].clone() {
        Type::Int
        | Type::Int64
        | Type::Float
        | Type::Bool
        | Type::Unit
        | Type::Never
        | Type::String
        | Type::Any
        | Type::List(_)
        | Type::Cell(_)
        | Type::Handler(_)
        | Type::Evidence
        | Type::FiberScope => true,
        Type::Enum(enum_id, _) => {
            layouts.enum_is_i32(enum_id)
                || layouts.core.enums[enum_id].external
                || layouts.is_cause(ty)
                || aggregate_function_abi_supported(layouts, ty, seen)
        }
        Type::Tuple(_) => aggregate_function_abi_supported(layouts, ty, seen),
        Type::Fn(function_type) => {
            if !seen.insert(ty) {
                // Function signatures may refer back through an aggregate field.
                return true;
            }
            let supported = function_type.sets.is_none()
                && function_type
                    .params
                    .iter()
                    .all(|&parameter| function_value_abi_type_supported(layouts, parameter, seen))
                && function_value_abi_type_supported(layouts, function_type.ret, seen);
            seen.remove(&ty);
            supported
        }
        _ => false,
    }
}

fn aggregate_function_abi_supported(
    layouts: &TypeLayouts<'_>,
    ty: TypeId,
    seen: &mut HashSet<TypeId>,
) -> bool {
    let Some(&index) = layouts.aggregate_ids.get(&ty) else {
        return false;
    };
    if !seen.insert(ty) {
        return true;
    }
    let fields = match &layouts.aggregates[index].kind {
        AggregateKind::Tuple(fields) => vec![fields.as_slice()],
        AggregateKind::Enum(variants) => variants.iter().map(Vec::as_slice).collect(),
    };
    let supported = fields.into_iter().enumerate().all(|(variant, fields)| {
        fields.iter().enumerate().all(|(index, &field)| {
            // Opaque owned edges use the heap header ABI; materialized payload roots are checked separately.
            layouts.is_recursive_box_field(ty, variant, index)
                || layouts.type_repr(field) == Repr::Void
                || function_value_abi_type_supported(layouts, field, seen)
        })
    });
    seen.remove(&ty);
    supported
}

fn validate_function_refs(
    flow: &emela_lir::Module,
    emitted: &HashSet<FnId>,
    layouts: &TypeLayouts<'_>,
) -> Result<(), Vec<Error>> {
    for (function_id, function) in flow.functions.iter() {
        if !emitted.contains(&function_id) {
            continue;
        }
        for (_, statement) in function.stmts.iter() {
            let Stmt::Let {
                dst,
                value: Value::FnRef(target),
                ..
            } = statement
            else {
                continue;
            };
            let Some(dst) = dst else {
                continue;
            };
            let Type::Fn(function_type) = layouts.types[function.vars[*dst].ty].clone() else {
                return Err(invalid_flow_ir(
                    &function.name,
                    "FnRef destination is not a function value",
                ));
            };
            let target_function = &flow.functions[*target];
            let mut expected_params: Vec<TypeId> = function_type
                .params
                .into_iter()
                .filter(|&parameter| layouts.type_repr(parameter) != Repr::Void)
                .collect();
            if function_type.ev {
                expected_params.push(TypeId::EVIDENCE);
            }
            let target_params: Vec<TypeId> = target_function
                .params
                .iter()
                .skip(usize::from(target_function.env.is_some()))
                .map(|&parameter| target_function.vars[parameter].ty)
                .collect();
            if expected_params != target_params
                || function_type.ret != target_function.ret
                || function_type.fails != target_function.fails
                || function_type.suspends != target_function.suspends
            {
                return Err(invalid_flow_ir(
                    &function.name,
                    "FnRef type does not match the referenced function ABI",
                ));
            }
        }
    }
    Ok(())
}

fn emit_const_getter(
    layouts: &TypeLayouts<'_>,
    id: FnId,
    function: &Function,
    name: &str,
    initializer: &str,
    writer: &mut Writer,
) {
    let index = u32::from(id.into_raw());
    let state = format!("emela_const_state_{index}");
    let cache = format!("emela_const_value_{index}");
    writer.line(&format!("static EmelaConstState {state};"));
    let is_void = matches!(layouts.types[function.ret], Type::Unit | Type::Never);
    if !is_void {
        writer.line(&format!("static {} {cache};", layouts.c_type(function.ret)));
    }
    writer.begin_block(&signature(
        layouts,
        function,
        name,
        !function.exported,
        false,
        false,
    ));
    writer.begin_block(&format!("if ({state}.status == 3)"));
    writer.line(&format!(
        "emela_embedded_report_defect({state}.kind, {state}.message, {state}.length);"
    ));
    writer.end_block();
    writer.begin_block(&format!("if ({state}.status == 1)"));
    let message = format!(
        "const {} refers to itself while being evaluated",
        function.name
    );
    writer.line(&format!(
        "emela_const_self_reference(EMELA_DEFECT_PANIC, {}, UINT16_C({}));",
        c_string_literal(message.as_bytes()),
        message.len()
    ));
    writer.end_block();
    writer.begin_block(&format!("if ({state}.status == 0)"));
    let prefix = format!("while evaluating const {}: ", function.name);
    writer.line(&format!(
        "EmelaConstContext context = {{ emela_const_current, &{state}, {}, UINT16_C({}) }};",
        c_string_literal(prefix.as_bytes()),
        prefix.len()
    ));
    writer.line(&format!("{state}.status = 1;"));
    writer.line("emela_const_current = &context;");
    if is_void {
        writer.line(&format!("{initializer}();"));
    } else {
        writer.line(&format!("{cache} = {initializer}();"));
        if layouts.has_inline_string(function.ret) {
            layouts.emit_freeze_value_in_place(function.ret, &cache, writer);
        }
    }
    writer.line("emela_const_current = context.previous;");
    writer.line(&format!("{state}.status = 2;"));
    writer.end_block();
    if is_void {
        if matches!(layouts.types[function.ret], Type::Never) {
            writer.line(
                "emela_board_defect(EMELA_DEFECT_PANIC, \"Never const returned\", UINT16_C(20));",
            );
        } else {
            writer.ret(None);
        }
    } else {
        layouts.emit_retain_value_expression(function.ret, &cache, writer);
        writer.ret(Some(&cache));
    }
    writer.end_block();
    writer.blank();
}

fn directly_calls(caller: &Function, target: FnId) -> bool {
    let (positions, _) = function_positions(caller);
    caller.stmts.iter().any(|(id, statement)| {
        positions.contains_key(&id)
            && matches!(
                statement,
                Stmt::Let {
                    value: Value::Call {
                        callee: Callee::Direct(id),
                        ..
                    },
                    ..
                } if *id == target
            )
    })
}

fn operand_is_string(layouts: &TypeLayouts<'_>, function: &Function, operand: &Operand) -> bool {
    match operand {
        Operand::Const(Const::String(_)) => true,
        Operand::Var(variable) => {
            matches!(layouts.types[function.vars[*variable].ty], Type::String)
        }
        _ => false,
    }
}

fn operand_is_string_list(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    operand: &Operand,
) -> bool {
    match operand {
        Operand::Var(variable) => {
            matches!(
                layouts.types[function.vars[*variable].ty],
                Type::List(TypeId::STRING)
            )
        }
        _ => false,
    }
}

fn operand_is_int(layouts: &TypeLayouts<'_>, function: &Function, operand: &Operand) -> bool {
    match operand {
        Operand::Const(Const::Int(_)) => true,
        Operand::Var(variable) => matches!(layouts.types[function.vars[*variable].ty], Type::Int),
        _ => false,
    }
}

fn operand_is_int64(layouts: &TypeLayouts<'_>, function: &Function, operand: &Operand) -> bool {
    match operand {
        Operand::Const(Const::Int64(_)) => true,
        Operand::Var(variable) => {
            matches!(layouts.types[function.vars[*variable].ty], Type::Int64)
        }
        _ => false,
    }
}

fn operand_is_float(layouts: &TypeLayouts<'_>, function: &Function, operand: &Operand) -> bool {
    match operand {
        Operand::Const(Const::Float(_)) => true,
        Operand::Var(variable) => matches!(layouts.types[function.vars[*variable].ty], Type::Float),
        _ => false,
    }
}

fn option_payload_type(layouts: &TypeLayouts<'_>, ty: TypeId) -> Option<TypeId> {
    let Type::Enum(enum_id, _) = layouts.types[ty] else {
        return None;
    };
    let definition = &layouts.core.enums[enum_id];
    if definition.name != "Option" {
        return None;
    }
    let some_index = definition
        .variants
        .iter()
        .position(|variant| variant.name == "Some")?;
    let fields = layouts.enum_fields(ty)?.get(some_index)?;
    (fields.len() == 1).then_some(fields[0])
}

fn option_variant_expression(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    ty: TypeId,
    variant_name: &str,
    value: Option<&str>,
) -> Result<String, Vec<Error>> {
    let Type::Enum(enum_id, _) = layouts.types[ty] else {
        return Err(invalid_flow_ir(
            &function.name,
            "Option builtin result is not an enum",
        ));
    };
    let definition = &layouts.core.enums[enum_id];
    if definition.name != "Option" {
        return Err(invalid_flow_ir(
            &function.name,
            "Option builtin result has the wrong enum type",
        ));
    }
    let Some(variant_index) = definition
        .variants
        .iter()
        .position(|variant| variant.name == variant_name)
    else {
        return Err(invalid_flow_ir(
            &function.name,
            "Option enum is missing a required variant",
        ));
    };
    let Some(fields) = layouts
        .enum_fields(ty)
        .and_then(|variants| variants.get(variant_index))
    else {
        return Err(invalid_flow_ir(
            &function.name,
            "Option result has no inline enum layout",
        ));
    };
    let mut initializers = vec![format!(".tag = {}", int32_literal(variant_index as i32))];
    match value {
        Some(value) if fields.len() == 1 && layouts.type_repr(fields[0]) != Repr::Void => {
            initializers.push(format!(".payload.v{variant_index}.f0 = {value}"));
        }
        Some(_) => {
            return Err(invalid_flow_ir(
                &function.name,
                "Option Some payload does not match its runtime representation",
            ));
        }
        None if fields
            .iter()
            .all(|field| layouts.type_repr(*field) == Repr::Void) =>
        {
            initializers.push(format!(".payload.v{variant_index}._unit = UINT8_C(0)"));
        }
        None => {
            return Err(invalid_flow_ir(
                &function.name,
                "Option None variant unexpectedly carries a runtime payload",
            ));
        }
    }
    Ok(format!(
        "({}){{ {} }}",
        layouts.c_type(ty),
        initializers.join(", ")
    ))
}

fn emit_builtin(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    op: Builtin,
    args: &[Operand],
    string_buffers: &StringBufferPlan,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    features::builtins::emit(
        &mut features::BuiltinContext {
            layouts,
            function,
            string_buffers,
            writer,
        },
        dst,
        op,
        args,
    )
}

// Keep the lowering context explicit across this emission boundary.

fn emit_any_ownership_declarations(layouts: &TypeLayouts<'_>, writer: &mut Writer) {
    if !layouts.any_abi {
        return;
    }
    writer.line("static inline void emela_any_retain(EmelaAny value);");
    writer.line("static inline void emela_any_own(EmelaAny *value);");
    writer.line("static inline void emela_any_enqueue(EmelaAny value, emela_heap_block **pending, emela_heap_block **tail);");
    writer.line("static inline void emela_any_release(EmelaAny value);");
}

fn emit_any_ownership(layouts: &TypeLayouts<'_>, heap: bool, writer: &mut Writer) {
    if !layouts.any_abi {
        return;
    }
    writer.begin_block("static inline void emela_any_retain(EmelaAny value)");
    writer.line("(void)value;");
    if heap && layouts.any_box_required {
        writer.line("if (value.tag == 0) emela_heap_retain(value.payload.any_box);");
    }
    if layouts.has_error_ownership() {
        writer.line("if (value.tag != 0) emela_error_retain(value);");
    }
    writer.end_block();
    writer.begin_block("static inline void emela_any_own(EmelaAny *value)");
    writer.line("(void)value;");
    if heap && layouts.any_box_required {
        writer.line("if (value->tag == 0) emela_heap_retain(value->payload.any_box);");
    }
    if layouts.has_error_ownership() {
        writer.line("if (value->tag != 0) emela_error_own(value);");
    }
    writer.end_block();
    if heap {
        writer.begin_block("static inline void emela_any_enqueue(EmelaAny value, emela_heap_block **pending, emela_heap_block **tail)");
        writer.line("(void)value; (void)pending; (void)tail;");
        if layouts.any_box_required {
            writer.line(
                "if (value.tag == 0) emela_heap_enqueue(value.payload.any_box, pending, tail);",
            );
        }
        if layouts.has_error_ownership() {
            writer.line("if (value.tag != 0) emela_error_enqueue(value, pending, tail);");
        }
        writer.end_block();
    }
    writer.begin_block("static inline void emela_any_release(EmelaAny value)");
    if heap {
        writer.line("emela_heap_block *pending = NULL, *tail = NULL;");
        writer.line("emela_any_enqueue(value, &pending, &tail); emela_heap_drain(pending, tail);");
    } else {
        writer.line("(void)value;");
    }
    writer.end_block();
}

fn emit_public_string_ownership(writer: &mut Writer) {
    writer.begin_block("void emela_embedded_retain_string(const EmelaString *value)");
    writer.begin_block("if (value != NULL)");
    writer.line("emela_heap_retain(value->owner);");
    writer.end_block();
    writer.end_block();
    writer.blank();

    writer.begin_block("void emela_embedded_release_string(EmelaString *value)");
    writer.begin_block("if (value != NULL)");
    writer.line("emela_heap_release(value->owner);");
    writer.line("*value = (EmelaString){0};");
    writer.end_block();
    writer.end_block();
    writer.blank();
}

fn emit_runtime(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    op: RuntimeOp,
    args: &[Operand],
    string_buffers: &StringBufferPlan,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    features::runtime::emit(
        &mut features::BuiltinContext {
            layouts,
            function,
            string_buffers,
            writer,
        },
        dst,
        op,
        args,
    )
}

// Keep the lowering context explicit across this emission boundary.
#[allow(clippy::too_many_arguments)]
fn emit_statement(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    statement: &Stmt,
    writer: &mut Writer,
    names: &HashMap<FnId, String>,
    string_buffers: &StringBufferPlan,
    loops: &mut Vec<Vec<VarId>>,
    blocks: &mut Vec<BlockTarget>,
    protects: &mut Vec<ProtectTarget>,
    next_temp: &mut usize,
    next_label: &mut usize,
) -> Result<(), Vec<Error>> {
    let managed_dst = statement
        .dst()
        .filter(|&dst| layouts.is_managed_value(function.vars[dst].ty));
    if let Some(dst) = managed_dst {
        emit_release_if_owned(layouts, function, dst, writer);
    }
    let retain_result = matches!(statement, Stmt::Let { value, .. }
        if features::values::result_ownership(value) == ResultOwnership::Borrowed);
    emit_statement_inner(
        layouts,
        function,
        statement,
        writer,
        names,
        string_buffers,
        loops,
        blocks,
        protects,
        next_temp,
        next_label,
    )?;
    if let Some(dst) = managed_dst {
        if retain_result {
            layouts.emit_retain_value_expression(
                function.vars[dst].ty,
                &variable_name(dst),
                writer,
            );
        }
        writer.assign(&owned_variable_name(dst), "INT32_C(1)");
        if string_buffers.discarded_results.contains(&dst) {
            emit_release_if_owned(layouts, function, dst, writer);
        }
    }
    if layouts.has_cause_raise
        && layouts.has_unwind
        && !function.suspends
        && matches!(
            statement,
            Stmt::Let {
                value: Value::Call { .. }
                    | Value::Fiber {
                        op: emela_core::FiberOp::Attempt { .. },
                        ..
                    },
                ..
            }
        )
    {
        emit_sync_interrupt_exit(
            layouts,
            function,
            writer,
            names,
            string_buffers,
            loops,
            blocks,
            protects,
            next_temp,
            next_label,
        )?;
    }
    Ok(())
}

// Keep the lowering context explicit across this emission boundary.
#[allow(clippy::too_many_arguments)]
fn emit_statement_inner(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    statement: &Stmt,
    writer: &mut Writer,
    names: &HashMap<FnId, String>,
    string_buffers: &StringBufferPlan,
    loops: &mut Vec<Vec<VarId>>,
    blocks: &mut Vec<BlockTarget>,
    protects: &mut Vec<ProtectTarget>,
    next_temp: &mut usize,
    next_label: &mut usize,
) -> Result<(), Vec<Error>> {
    features::statements::emit(
        &mut features::Context {
            layouts,
            function,
            writer,
            names,
            string_buffers,
            loops,
            blocks,
            protects,
            next_temp,
            next_label,
        },
        statement,
    )
}

// Keep the lowering context explicit across this emission boundary.

fn invalid_flow_ir(function: &str, reason: &str) -> Vec<Error> {
    vec![Error {
        kind: ErrorKind::InvalidFlowIr,
        message: format!("invalid Flow IR in function `{function}`: {reason}"),
    }]
}

// Keep the lowering context explicit across this emission boundary.

// Keep the lowering context explicit across this emission boundary.

// Keep the lowering context explicit across this emission boundary.

// Keep the lowering context explicit across this emission boundary.
#[allow(clippy::too_many_arguments)]
fn emit_compound_sequence(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    sequence: emela_lir::SeqId,
    dst: Option<VarId>,
    writer: &mut Writer,
    names: &HashMap<FnId, String>,
    string_buffers: &StringBufferPlan,
    loops: &mut Vec<Vec<VarId>>,
    blocks: &mut Vec<BlockTarget>,
    protects: &mut Vec<ProtectTarget>,
    next_temp: &mut usize,
    next_label: &mut usize,
) -> Result<(), Vec<Error>> {
    let sequence = &function.seqs[sequence];
    for &statement in &sequence.stmts {
        emit_statement(
            layouts,
            function,
            &function.stmts[statement],
            writer,
            names,
            string_buffers,
            loops,
            blocks,
            protects,
            next_temp,
            next_label,
        )?;
    }
    features::terms::emit(
        &mut features::Context {
            layouts,
            function,
            writer,
            names,
            string_buffers,
            loops,
            blocks,
            protects,
            next_temp,
            next_label,
        },
        &sequence.term,
        dst,
    )?;
    Ok(())
}

struct BlockTarget {
    label: LabelId,
    dst: Option<VarId>,
    exit: String,
    used: bool,
    protect_depth: usize,
}

#[derive(Clone, Copy)]
struct ProtectTarget {
    cleanup: emela_lir::SeqId,
}

// Keep the lowering context explicit across this emission boundary.
#[allow(clippy::too_many_arguments)]
fn emit_call_failure_edge(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    edge: OnFail,
    error_name: &str,
    writer: &mut Writer,
    names: &HashMap<FnId, String>,
    string_buffers: &StringBufferPlan,
    loops: &mut Vec<Vec<VarId>>,
    blocks: &mut Vec<BlockTarget>,
    protects: &mut Vec<ProtectTarget>,
    next_temp: &mut usize,
    next_label: &mut usize,
) -> Result<(), Vec<Error>> {
    features::failures::emit(
        &mut features::Context {
            layouts,
            function,
            writer,
            names,
            string_buffers,
            loops,
            blocks,
            protects,
            next_temp,
            next_label,
        },
        edge,
        error_name,
    )
}

fn emit_error_assignment(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    error: &Operand,
    target: &str,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let Operand::Var(variable) = error else {
        return Err(invalid_flow_ir(
            &function.name,
            "Fail operand is not an error variable",
        ));
    };
    let ty = function.vars[*variable].ty;
    if matches!(layouts.types[ty], Type::Any) {
        writer.assign(target, &variable_name(*variable));
        writer.line(&format!("emela_error_retain({target});"));
        return Ok(());
    }
    let Some(enum_id) = layouts.error_enum(ty) else {
        return Err(invalid_flow_ir(
            &function.name,
            "Fail operand is not an error enum or propagated EmelaError",
        ));
    };
    if layouts.error_type(enum_id).is_none() {
        return Err(invalid_flow_ir(
            &function.name,
            "Fail operand error enum is missing from EmelaError layout",
        ));
    }
    let object = format!("({target})");
    writer.assign(target, "(EmelaError){0}");
    writer.assign(
        &format!("{object}.tag"),
        &format!("UINT32_C({})", enum_index(enum_id) + 1),
    );
    layouts.emit_owned_value_store(
        ty,
        &variable_name(*variable),
        &format!("{object}.payload.e{}", enum_index(enum_id)),
        writer,
    );
    Ok(())
}

fn emit_failure_return(layouts: &TypeLayouts<'_>, function: &Function, writer: &mut Writer) {
    emit_owned_cleanup(layouts, function, None, writer);
    if layouts.type_repr(function.ret) == Repr::Void {
        writer.ret(None);
    } else {
        writer.ret(Some(&format!("({}){{0}}", layouts.c_type(function.ret))));
    }
}

// Keep the lowering context explicit across this emission boundary.

// Keep the lowering context explicit across this emission boundary.
#[allow(clippy::too_many_arguments)]
fn emit_pending_cleanups(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    keep_depth: usize,
    writer: &mut Writer,
    names: &HashMap<FnId, String>,
    string_buffers: &StringBufferPlan,
    loops: &mut Vec<Vec<VarId>>,
    blocks: &mut Vec<BlockTarget>,
    protects: &mut Vec<ProtectTarget>,
    next_temp: &mut usize,
    next_label: &mut usize,
) -> Result<(), Vec<Error>> {
    if keep_depth > protects.len() {
        return Err(invalid_flow_ir(
            &function.name,
            "control transfer escapes a block outside the active protect stack",
        ));
    }
    let saved = protects.clone();
    for index in (keep_depth..saved.len()).rev() {
        protects.truncate(index);
        let result = emit_compound_sequence(
            layouts,
            function,
            saved[index].cleanup,
            None,
            writer,
            names,
            string_buffers,
            loops,
            blocks,
            protects,
            next_temp,
            next_label,
        );
        *protects = saved.clone();
        result?;
    }
    Ok(())
}

// Keep the lowering context explicit across this emission boundary.
#[allow(clippy::too_many_arguments)]
fn emit_aggregate_field_store(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    owner_ty: TypeId,
    variant: usize,
    field: usize,
    field_ty: TypeId,
    value: &Operand,
    target: &str,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let expression = operand_expression(value).map_err(|kind| unsupported(&function.name, kind))?;
    if layouts.is_recursive_box_field(owner_ty, variant, field) {
        let box_kind = layouts.recursive_box_kind(field_ty).ok_or_else(|| {
            invalid_flow_ir(
                &function.name,
                "recursive aggregate field has no heap box layout",
            )
        })?;
        let field_type = layouts.c_type(field_ty);
        writer.assign(
            target,
            &format!("emela_heap_allocate(sizeof({field_type}), UINT32_C({box_kind}))"),
        );
        layouts.emit_owned_value_store(
            field_ty,
            &expression,
            &format!("*(({field_type} *){target})"),
            writer,
        );
    } else {
        if layouts.is_managed_value(field_ty) {
            emit_retain_operand(layouts, function, value, writer);
        }
        writer.assign(target, &expression);
    }
    Ok(())
}

fn validate_external(flow: &emela_lir::Module, function: &Function) -> Result<String, Vec<Error>> {
    if fiber_normalize::is_infallible_cause_fail(function) {
        if function.params.len() != 1
            || function.vars[function.params[0]].ty != TypeId::ANY
            || function.ret != TypeId::NEVER
            || function.fails
            || function.suspends
        {
            return Err(invalid_flow_ir(
                &function.name,
                "invalid backend Cause landing binding",
            ));
        }
        return Ok(fiber_normalize::INFALLIBLE_CAUSE_FAIL.into());
    }
    let Some(external) = &function.external else {
        return Err(unsupported(&function.name, "missing external binding"));
    };
    let Some(binding) = external.binding("c") else {
        return Err(unsupported(
            &function.name,
            "external function without a C binding",
        ));
    };
    let symbol = features::host_refs::symbol(&binding.host).to_owned();
    if !is_c_identifier(&symbol) {
        return Err(unsupported(
            &function.name,
            "external function with an invalid C symbol",
        ));
    }

    let supported_type = |ty| {
        matches!(
            flow.core.ty(ty),
            emela_core::Type::Int
                | emela_core::Type::Int64
                | emela_core::Type::Float
                | emela_core::Type::Bool
                | emela_core::Type::Unit
                | emela_core::Type::Never
        )
    };
    if function
        .params
        .iter()
        .any(|&param| !supported_type(function.vars[param].ty))
        || !supported_type(function.ret)
    {
        return Err(unsupported(
            &function.name,
            "external function with a non-scalar signature",
        ));
    }
    Ok(symbol)
}

pub(super) fn is_c_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first.is_ascii_alphabetic())
        || !chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
    {
        return false;
    }
    !matches!(
        value,
        "auto"
            | "break"
            | "case"
            | "char"
            | "const"
            | "continue"
            | "default"
            | "do"
            | "double"
            | "else"
            | "enum"
            | "extern"
            | "float"
            | "for"
            | "goto"
            | "if"
            | "inline"
            | "int"
            | "long"
            | "register"
            | "restrict"
            | "return"
            | "short"
            | "signed"
            | "sizeof"
            | "static"
            | "struct"
            | "switch"
            | "typedef"
            | "union"
            | "unsigned"
            | "void"
            | "volatile"
            | "while"
            | "_Alignas"
            | "_Alignof"
            | "_Atomic"
            | "_Bool"
            | "_Complex"
            | "_Generic"
            | "_Imaginary"
            | "_Noreturn"
            | "_Static_assert"
            | "_Thread_local"
    )
}

fn primitive_expression(op: PrimOp, ty: PrimTy, args: &[Operand]) -> Result<String, &'static str> {
    features::primitive_types::emit(op, ty, args)
}

fn emit_return(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    term: &Term,
    string_buffers: &StringBufferPlan,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    features::terms::emit_function(layouts, function, term, string_buffers, writer)
}

fn operand_expression(operand: &Operand) -> Result<String, &'static str> {
    features::operands::dispatch(operand)
}

fn constant_expression(value: &emela_lir::Const) -> Result<Option<String>, &'static str> {
    features::constants::dispatch(value)
}

fn c_string_literal(bytes: &[u8]) -> String {
    let escaped = bytes
        .iter()
        .map(|byte| format!("\\x{byte:02x}"))
        .collect::<String>();
    format!("\"{escaped}\"")
}

fn int32_literal(value: i32) -> String {
    if value == i32::MIN {
        "(-INT32_C(2147483647) - INT32_C(1))".to_owned()
    } else if value < 0 {
        format!("(-INT32_C({}))", -value)
    } else {
        format!("INT32_C({value})")
    }
}

fn int64_literal(value: i64) -> String {
    if value == i64::MIN {
        "(-INT64_C(9223372036854775807) - INT64_C(1))".to_owned()
    } else {
        format!("INT64_C({value})")
    }
}

fn float_literal(value: f64) -> String {
    if value.is_nan() {
        "NAN".to_owned()
    } else if value == f64::INFINITY {
        "INFINITY".to_owned()
    } else if value == f64::NEG_INFINITY {
        "(-INFINITY)".to_owned()
    } else {
        let literal = format!("{value:?}");
        if literal.contains(['.', 'e', 'E']) {
            literal
        } else {
            format!("{literal}.0")
        }
    }
}

pub(super) fn signature(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    name: &str,
    is_static: bool,
    takes_error_out: bool,
    takes_string_output: bool,
) -> String {
    let return_type = layouts.c_type(function.ret);
    let mut params: Vec<String> = function
        .params
        .iter()
        .map(|&variable| {
            let data = &function.vars[variable];
            format!("{} {}", layouts.c_type(data.ty), variable_name(variable))
        })
        .collect::<Vec<_>>();
    if takes_error_out {
        params.push("EmelaError *emela_error_out".to_owned());
    }
    if takes_string_output {
        params.push("char *emela_string_output".to_owned());
        params.push("uint16_t emela_string_capacity".to_owned());
    }
    let params = if params.is_empty() {
        "void".to_owned()
    } else {
        params.join(", ")
    };
    let linkage = if is_static { "static " } else { "" };
    format!("{linkage}{return_type} {name}({params})")
}

fn closure_signature(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    name: &str,
    is_static: bool,
) -> String {
    let signature = signature(layouts, function, name, is_static, function.fails, false);
    let open = signature
        .find('(')
        .expect("function signature contains an opening parenthesis");
    let params = &signature[open + 1..signature.len() - 1];
    if params == "void" {
        format!("{}(void *emela_env)", &signature[..open])
    } else {
        format!("{}(void *emela_env, {params})", &signature[..open])
    }
}

fn function_ref_adapters(
    flow: &emela_lir::Module,
    emitted: &HashSet<FnId>,
    layouts: &TypeLayouts<'_>,
) -> Vec<(FnId, TypeId)> {
    let mut adapters = HashSet::new();
    for (function_id, function) in flow.functions.iter() {
        if !emitted.contains(&function_id) {
            continue;
        }
        for (_, statement) in function.stmts.iter() {
            let Stmt::Let {
                dst: Some(dst),
                value: Value::FnRef(target),
                ..
            } = statement
            else {
                continue;
            };
            if !layouts.closure_functions.contains_key(target) {
                adapters.insert((*target, function.vars[*dst].ty));
            }
        }
    }
    let mut adapters: Vec<_> = adapters.into_iter().collect();
    adapters.sort_by_key(|(target, function_type)| {
        (u32::from(target.into_raw()), function_type.index())
    });
    adapters
}

fn function_ref_adapter_name(target: FnId, function_type: TypeId) -> String {
    format!(
        "emela_fn_ref_adapter_{}_{}",
        u32::from(target.into_raw()),
        function_type.index()
    )
}

fn function_coerce_adapter_name(index: usize) -> String {
    format!("emela_fn_coerce_adapter_{index}")
}

fn function_ref_adapter_signature(
    layouts: &TypeLayouts<'_>,
    function_type: TypeId,
    name: &str,
) -> String {
    let Type::Fn(function_type) = layouts.types[function_type].clone() else {
        unreachable!("function reference adapter has a non-function type")
    };
    let mut params = vec!["void *emela_env".to_owned()];
    if function_type.suspends {
        params.insert(0, "emela_fiber *fiber".to_owned());
    }
    for (index, &parameter) in function_type.params.iter().enumerate() {
        if layouts.type_repr(parameter) != Repr::Void {
            params.push(format!("{} emela_arg_{index}", layouts.c_type(parameter)));
        }
    }
    if function_type.ev {
        params.push("void *emela_evidence".to_owned());
    }
    if function_type.fails {
        params.push("EmelaError *emela_error_out".to_owned());
    }
    if function_type.suspends && layouts.type_repr(function_type.ret) != Repr::Void {
        params.push(format!("{} *result", layouts.c_type(function_type.ret)));
    }
    format!(
        "static {} {name}({})",
        if function_type.suspends {
            "void".to_owned()
        } else {
            layouts.c_type(function_type.ret)
        },
        params.join(", ")
    )
}

fn emit_function_ref_adapter(
    layouts: &TypeLayouts<'_>,
    function_type: TypeId,
    target_name: &str,
    adapter_name: &str,
    takes_string_output: bool,
    string_capacity: u16,
    writer: &mut Writer,
) {
    let Type::Fn(function_type_data) = layouts.types[function_type].clone() else {
        unreachable!("function reference adapter has a non-function type")
    };
    writer.begin_block(&function_ref_adapter_signature(
        layouts,
        function_type,
        adapter_name,
    ));
    writer.line("(void)emela_env;");
    let mut args = Vec::new();
    if function_type_data.suspends {
        args.push("fiber".to_owned());
    }
    for (index, &parameter) in function_type_data.params.iter().enumerate() {
        if layouts.type_repr(parameter) != Repr::Void {
            args.push(format!("emela_arg_{index}"));
        }
    }
    if function_type_data.ev {
        args.push("emela_evidence".to_owned());
    }
    if function_type_data.fails {
        args.push("emela_error_out".to_owned());
    }
    if function_type_data.suspends {
        if layouts.type_repr(function_type_data.ret) != Repr::Void {
            args.push("result".to_owned());
        }
        writer.line(&format!("{target_name}_push({});", args.join(", ")));
        writer.end_block();
        return;
    }
    if takes_string_output {
        writer.line(&format!("char buffer[{string_capacity}];"));
        args.push("buffer".to_owned());
        args.push(format!("UINT16_C({string_capacity})"));
    }
    let call = format!("{target_name}({})", args.join(", "));
    if takes_string_output {
        writer.line(&format!("EmelaString returned = {call};"));
        writer.ret(Some("emela_heap_own_string(returned)"));
        writer.end_block();
        return;
    }
    if layouts.type_repr(function_type_data.ret) == Repr::Void {
        writer.line(&format!("{call};"));
        writer.ret(None);
    } else {
        writer.ret(Some(&call));
    }
    writer.end_block();
}

fn emit_function_coerce_adapter(
    layouts: &TypeLayouts<'_>,
    adapter: &FunctionAdapterLayout,
    index: usize,
    writer: &mut Writer,
) {
    let Type::Fn(source_type) = layouts.types[adapter.source].clone() else {
        unreachable!("function adapter source has non-function type")
    };
    let Type::Fn(destination_type) = layouts.types[adapter.destination].clone() else {
        unreachable!("function adapter destination has non-function type")
    };
    let traps_failure = source_type.fails && !destination_type.fails;
    if traps_failure && source_type.suspends {
        writer.line(&format!(
            "typedef struct {{ emela_frame base; EmelaError error; }} emela_coerce_check_{index};"
        ));
        writer.line(&format!("_Static_assert(sizeof(emela_coerce_check_{index}) <= EMELA_FRAME_BYTES, \"increase frame_bytes for function coercion\");"));
        writer.begin_block(&format!(
            "static void emela_coerce_check_step_{index}(emela_fiber *fiber, emela_frame *base)"
        ));
        writer.line(&format!(
            "emela_coerce_check_{index} *frame = (emela_coerce_check_{index} *)base;"
        ));
        writer.line("if (frame->error.tag != 0) emela_board_defect(EMELA_DEFECT_ASSERT, \"failure in infallible coercion\", UINT16_C(30));");
        writer.line("emela_frame_return(fiber);");
        writer.end_block();
        writer.begin_block(&format!(
            "static void emela_coerce_check_drop_{index}(emela_frame *base)"
        ));
        writer.line(&format!(
            "emela_error_clear(&((emela_coerce_check_{index} *)base)->error);"
        ));
        writer.end_block();
    }
    writer.begin_block(&function_ref_adapter_signature(
        layouts,
        adapter.destination,
        &function_coerce_adapter_name(index),
    ));
    writer.line(&format!(
        "emela_fn_coerce_env_{index} *environment = (emela_fn_coerce_env_{index} *)emela_env;"
    ));
    writer.begin_block("if (environment == NULL || environment->source.call == NULL)");
    writer.line(
        "emela_board_defect(EMELA_DEFECT_PANIC, \"invalid coerced function value\", UINT16_C(28));",
    );
    writer.end_block();

    let mut args = vec!["environment->source.env".to_owned()];
    if source_type.suspends {
        args.insert(0, "fiber".to_owned());
    }
    for (parameter_index, &parameter) in source_type.params.iter().enumerate() {
        if layouts.type_repr(parameter) != Repr::Void {
            args.push(format!("emela_arg_{parameter_index}"));
        }
    }
    if source_type.ev {
        args.push("emela_evidence".to_owned());
    } else if destination_type.ev {
        writer.line("(void)emela_evidence;");
    }
    if traps_failure {
        if source_type.suspends {
            writer.line(&format!("emela_coerce_check_{index} *check = (emela_coerce_check_{index} *)emela_frame_push(fiber, sizeof(*check), emela_coerce_check_step_{index});"));
            writer.line("check->error = (EmelaError){0};");
            writer.line(&format!(
                "check->base.drop = emela_coerce_check_drop_{index};"
            ));
            args.push("&check->error".into());
        } else {
            writer.line("EmelaError checked_error = {0};");
            args.push("&checked_error".into());
        }
    } else if source_type.fails {
        args.push("emela_error_out".to_owned());
    } else if destination_type.fails {
        writer.assign("*emela_error_out", "(EmelaError){0}");
    }
    if source_type.suspends && layouts.type_repr(source_type.ret) != Repr::Void {
        args.push("result".to_owned());
    }
    let call = format!("environment->source.call({})", args.join(", "));
    let trap = "if (checked_error.tag != 0) emela_board_defect(EMELA_DEFECT_ASSERT, \"failure in infallible coercion\", UINT16_C(30));";
    if destination_type.suspends {
        if source_type.suspends || layouts.type_repr(source_type.ret) == Repr::Void {
            writer.line(&format!("{call};"));
            if traps_failure && !source_type.suspends {
                writer.line(trap);
            }
        } else {
            writer.decl(&layouts.c_type(source_type.ret), "returned", Some(&call));
            if traps_failure {
                writer.line(trap);
            }
            writer.begin_block("if (result != NULL)");
            writer.assign("*result", "returned");
            writer.end_block();
            if layouts.is_managed_value(source_type.ret) {
                writer.begin_block("else");
                layouts.emit_release_value_expression(source_type.ret, "returned", writer);
                writer.end_block();
            }
        }
        writer.line("(void)fiber;");
        writer.end_block();
        return;
    }
    if layouts.type_repr(source_type.ret) == Repr::Void {
        writer.line(&format!("{call};"));
        if traps_failure {
            writer.line(trap);
        }
        writer.ret(None);
    } else if traps_failure {
        writer.decl(&layouts.c_type(source_type.ret), "returned", Some(&call));
        writer.line(trap);
        writer.ret(Some("returned"));
    } else {
        writer.ret(Some(&call));
    }
    writer.end_block();
}

fn emit_fallible_wrapper(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    public_name: &str,
    implementation_name: &str,
    takes_string_output: bool,
    writer: &mut Writer,
) {
    writer.begin_block(&signature(
        layouts,
        function,
        public_name,
        false,
        false,
        takes_string_output,
    ));
    writer.decl("EmelaError", "emela_error", Some("{0}"));
    let mut args: Vec<String> = function
        .params
        .iter()
        .map(|&variable| variable_name(variable))
        .collect();
    args.push("&emela_error".to_owned());
    if takes_string_output {
        args.push("emela_string_output".to_owned());
        args.push("emela_string_capacity".to_owned());
    }
    let call = format!("{implementation_name}({})", args.join(", "));
    if layouts.type_repr(function.ret) == Repr::Void {
        writer.line(&format!("{call};"));
    } else {
        writer.decl(&layouts.c_type(function.ret), "emela_result", Some(&call));
    }
    writer.begin_block("if (emela_error.tag != UINT32_C(0))");
    writer.line(
        "emela_board_defect(EMELA_DEFECT_UNHANDLED_ERROR, \"unhandled error\", UINT16_C(15));",
    );
    writer.end_block();
    if layouts.type_repr(function.ret) == Repr::Void {
        writer.ret(None);
    } else {
        writer.ret(Some("emela_result"));
    }
    writer.end_block();
}

fn type_name(ty: TypeId) -> String {
    format!("emela_type_{}", ty.index())
}

fn function_type_name(ty: TypeId) -> String {
    format!("emela_function_type_{}", ty.index())
}

fn c_type(repr: Repr) -> &'static str {
    features::representations::dispatch(repr)
}

pub(super) fn function_name(id: FnId, function: &Function) -> String {
    let index = u32::from(id.into_raw());
    if function.exported {
        format!("emela_export_{}_{}", c_identifier(&function.name), index)
    } else {
        format!("emela_fn_{index}")
    }
}

pub(super) fn enum_index(id: EnumId) -> u32 {
    u32::from(id.into_raw())
}

fn c_identifier(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "function".to_owned()
    } else {
        sanitized
    }
}

pub(super) fn variable_name(id: VarId) -> String {
    format!("emela_v_{}", u32::from(id.into_raw()))
}

fn owned_variable_name(id: VarId) -> String {
    format!("emela_owned_v_{}", u32::from(id.into_raw()))
}

fn statement_name(statement: &emela_lir::Stmt) -> &'static str {
    match statement {
        emela_lir::Stmt::Let { .. } => "Stmt::Let",
        emela_lir::Stmt::If { .. } => "Stmt::If",
        emela_lir::Stmt::Switch { .. } => "Stmt::Switch",
        emela_lir::Stmt::Loop { .. } => "Stmt::Loop",
        emela_lir::Stmt::Block { .. } => "Stmt::Block",
        emela_lir::Stmt::Protect { .. } => "Stmt::Protect",
        emela_lir::Stmt::Dup(_) => "Stmt::Dup",
        emela_lir::Stmt::Drop(_) => "Stmt::Drop",
    }
}

fn value_name(value: &Value) -> &'static str {
    match value {
        Value::Const(_) => "Value::Const",
        Value::Prim { .. } => "Value::Prim",
        Value::Call { .. } => "Value::Call",
        Value::Builtin { .. } => "Value::Builtin",
        Value::Make { .. } => "Value::Make",
        Value::Field { .. } => "Value::Field",
        Value::Tag(_) => "Value::Tag",
        Value::Tuple(_) => "Value::Tuple",
        Value::TupleGet { .. } => "Value::TupleGet",
        Value::List { .. } => "Value::List",
        Value::ListIsEmpty(_) => "Value::ListIsEmpty",
        Value::ListHead(_) => "Value::ListHead",
        Value::ListTail(_) => "Value::ListTail",
        Value::FnRef(_) => "Value::FnRef",
        Value::Closure(_) => "Value::Closure",
        Value::MakeClosure { .. } => "Value::MakeClosure",
        Value::EnvGet { .. } => "Value::EnvGet",
        Value::EvLookup { .. } => "Value::EvLookup",
        Value::EvExtend { .. } => "Value::EvExtend",
        Value::Handler { .. } => "Value::Handler",
        Value::Fiber { .. } => "Value::Fiber",
        Value::IsError { .. } => "Value::IsError",
        Value::AsError { .. } => "Value::AsError",
        Value::Runtime { .. } => "Value::Runtime",
        Value::Coerce { .. } => "Value::Coerce",
    }
}

fn term_name(term: &Term) -> &'static str {
    match term {
        Term::Yield(_) => "Term::Yield",
        Term::Return(_) => "Term::Return",
        Term::Fail(_) => "Term::Fail",
        Term::Break { .. } => "Term::Break",
        Term::Recur(_) => "Term::Recur",
        Term::Defect(_) => "Term::Defect",
        Term::Unwind => "Term::Unwind",
        Term::Unreachable => "Term::Unreachable",
    }
}

pub(super) fn unsupported(function: &str, construct: &str) -> Vec<Error> {
    vec![Error {
        kind: ErrorKind::Unsupported,
        message: format!(
            "embedded C codegen does not support {construct} in function `{function}` yet"
        ),
    }]
}

#[allow(clippy::too_many_arguments)]
fn emit_sync_interrupt_exit(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    writer: &mut Writer,
    names: &HashMap<FnId, String>,
    strings: &StringBufferPlan,
    loops: &mut Vec<Vec<VarId>>,
    blocks: &mut Vec<BlockTarget>,
    protects: &mut Vec<ProtectTarget>,
    next_temp: &mut usize,
    next_label: &mut usize,
) -> Result<(), Vec<Error>> {
    writer.begin_block("if (emela_current_fiber != NULL && emela_current_fiber->forced)");
    writer.line("emela_current_fiber->forced = 0;");
    writer.line("emela_interrupt_hold(emela_current_fiber);");
    emit_pending_cleanups(
        layouts, function, 0, writer, names, strings, loops, blocks, protects, next_temp,
        next_label,
    )?;
    writer.line("emela_current_fiber->forced = 1;");
    if function.fails {
        writer.line("emela_error_clear(emela_error_out);");
    }
    emit_failure_return(layouts, function, writer);
    writer.end_block();
    Ok(())
}

pub(in crate::backend) fn check_input_stage(stage: emela_lir::Stage) -> Result<(), Vec<Error>> {
    features::stages::verify(stage)
}

pub(in crate::backend) fn normalize_flow_types(
    flow: &mut emela_lir::Module,
) -> Result<(), Vec<Error>> {
    type_normalize::normalize(flow)
}

pub(crate) use fiber_normalize::normalize_infallible_cause_raise;
