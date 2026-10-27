//! Compiler orchestration and the explicitly listed legacy feature facade.
//! Stage contracts and feature ownership are migrated in subsequent steps.

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

pub(in crate::backend) fn check_input_stage(stage: emela_lir::Stage) -> Result<(), Vec<Error>> {
    features::stages::verify(stage)
}

pub(in crate::backend) fn normalize_flow_types(
    flow: &mut emela_lir::Module,
) -> Result<(), Vec<Error>> {
    type_normalize::normalize(flow)
}

pub(crate) use fiber_normalize::normalize_infallible_cause_raise;

mod c;
mod diagnostics;
mod validation;

use c::{
    c_string_literal, float_literal, function_coerce_adapter_name, function_ref_adapter_name,
    function_type_name, int32_literal, int64_literal, owned_variable_name, type_name,
};
pub(super) use c::{enum_index, function_name, is_c_identifier, variable_name};
pub(super) use diagnostics::unsupported;
use diagnostics::{invalid_flow_ir, statement_name, term_name, value_name};
use validation::{
    directly_calls, validate_external, validate_function_refs, validate_function_value_types,
};

pub(super) use emit::signature;
use emit::{
    BlockTarget, ProtectTarget, c_type, constant_expression, emit_aggregate_field_store,
    emit_builtin, emit_call_failure_edge, emit_compound_sequence, emit_error_assignment,
    emit_failure_return, emit_owned_cleanup, emit_pending_cleanups, emit_release_if_owned,
    emit_retain_operand, emit_return, emit_runtime, emit_statement, emit_sync_interrupt_exit,
    function_ref_adapter_signature, operand_expression, operand_is_float, operand_is_int,
    operand_is_int64, operand_is_string, operand_is_string_list, operand_type, option_payload_type,
    option_variant_expression, primitive_expression,
};
