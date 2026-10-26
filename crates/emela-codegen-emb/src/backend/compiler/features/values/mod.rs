// Keep analysis dispatch exhaustive: every new variant needs an explicit planning decision.
use super::*;
pub(in crate::backend) mod as_error;
pub(in crate::backend) mod builtin;
pub(in crate::backend) mod call;
pub(in crate::backend) mod closure;
pub(in crate::backend) mod coerce;
pub(in crate::backend) mod const_;
pub(in crate::backend) mod env_get;
pub(in crate::backend) mod ev_extend;
pub(in crate::backend) mod ev_lookup;
pub(in crate::backend) mod fiber;
pub(in crate::backend) mod field;
pub(in crate::backend) mod fn_ref;
pub(in crate::backend) mod handler;
pub(in crate::backend) mod is_error;
pub(in crate::backend) mod list;
pub(in crate::backend) mod list_head;
pub(in crate::backend) mod list_is_empty;
pub(in crate::backend) mod list_tail;
pub(in crate::backend) mod make;
pub(in crate::backend) mod make_closure;
pub(in crate::backend) mod prim;
pub(in crate::backend) mod runtime;
pub(in crate::backend) mod tag;
pub(in crate::backend) mod tuple;
pub(in crate::backend) mod tuple_get;
pub(in crate::backend) fn emit(
    context: &mut Context<'_, '_>,
    dst: &Option<VarId>,
    value: &Value,
) -> Result<(), Vec<Error>> {
    match value {
        Value::Const(_) => const_::emit(context, dst, value),
        Value::Prim { .. } => prim::emit(context, dst, value),
        Value::Call { .. } => call::emit(context, dst, value),
        Value::Builtin { .. } => builtin::emit(context, dst, value),
        Value::Make { .. } => make::emit(context, dst, value),
        Value::Field { .. } => field::emit(context, dst, value),
        Value::Tag(_) => tag::emit(context, dst, value),
        Value::Tuple(_) => tuple::emit(context, dst, value),
        Value::TupleGet { .. } => tuple_get::emit(context, dst, value),
        Value::List { .. } => list::emit(context, dst, value),
        Value::ListIsEmpty(_) => list_is_empty::emit(context, dst, value),
        Value::ListHead(_) => list_head::emit(context, dst, value),
        Value::ListTail(_) => list_tail::emit(context, dst, value),
        Value::FnRef(_) => fn_ref::emit(context, dst, value),
        Value::Closure(_) => closure::emit(context, dst, value),
        Value::MakeClosure { .. } => make_closure::emit(context, dst, value),
        Value::EnvGet { .. } => env_get::emit(context, dst, value),
        Value::EvLookup { .. } => ev_lookup::emit(context, dst, value),
        Value::EvExtend { .. } => ev_extend::emit(context, dst, value),
        Value::Handler { .. } => handler::emit(context, dst, value),
        Value::Fiber { .. } => fiber::emit(context, dst, value),
        Value::IsError { .. } => is_error::emit(context, dst, value),
        Value::AsError { .. } => as_error::emit(context, dst, value),
        Value::Runtime { .. } => runtime::emit(context, dst, value),
        Value::Coerce(_) => coerce::emit(context, dst, value),
    }
}

pub(in crate::backend) fn string_behavior(value: &Value) -> StringBehavior {
    match value {
        Value::Builtin { op, args } => builtins::string_behavior(*op, args),
        Value::Runtime { op, args } => super::runtime::string_behavior(*op, args),
        Value::Fiber { op, .. } => fibers::string_behavior(*op),
        Value::Call { callee, .. } => call::string_behavior(callee),
        Value::Const(_)
        | Value::Prim { .. }
        | Value::Make { .. }
        | Value::Field { .. }
        | Value::Tag(_)
        | Value::Tuple(_)
        | Value::TupleGet { .. }
        | Value::List { .. }
        | Value::ListIsEmpty(_)
        | Value::ListHead(_)
        | Value::ListTail(_)
        | Value::FnRef(_)
        | Value::Closure(_)
        | Value::MakeClosure { .. }
        | Value::EnvGet { .. }
        | Value::EvLookup { .. }
        | Value::EvExtend { .. }
        | Value::Handler { .. }
        | Value::IsError { .. }
        | Value::AsError { .. }
        | Value::Coerce(_) => StringBehavior::default(),
    }
}

pub(in crate::backend) fn string_bound(value: &Value, bounds: &StringBounds<'_>) -> Option<usize> {
    match value {
        Value::Const(value) => const_::string_bound(value),
        Value::Builtin { op, args } => builtins::string_bound(*op, args, bounds),
        Value::Runtime { op, args } => super::runtime::string_bound(*op, args, bounds),
        Value::Prim { .. }
        | Value::Call { .. }
        | Value::Make { .. }
        | Value::Field { .. }
        | Value::Tag(_)
        | Value::Tuple(_)
        | Value::TupleGet { .. }
        | Value::List { .. }
        | Value::ListIsEmpty(_)
        | Value::ListHead(_)
        | Value::ListTail(_)
        | Value::FnRef(_)
        | Value::Closure(_)
        | Value::MakeClosure { .. }
        | Value::EnvGet { .. }
        | Value::EvLookup { .. }
        | Value::EvExtend { .. }
        | Value::Handler { .. }
        | Value::Fiber { .. }
        | Value::IsError { .. }
        | Value::AsError { .. }
        | Value::Coerce(_) => None,
    }
}

pub(in crate::backend) fn result_ownership(value: &Value) -> ResultOwnership {
    match value {
        Value::Builtin { op, args } => builtins::result_ownership(*op, args),
        Value::ListHead(_) => list_head::result_ownership(),
        Value::ListTail(_) => list_tail::result_ownership(),
        Value::Field { .. } => field::result_ownership(),
        Value::TupleGet { .. } => tuple_get::result_ownership(),
        Value::AsError { .. } => as_error::result_ownership(),
        Value::EvLookup { .. } => ev_lookup::result_ownership(),
        Value::EvExtend { bindings, .. } => ev_extend::result_ownership(bindings.is_empty()),
        Value::List { elems, tail } => list::result_ownership(elems, tail),
        Value::Const(_)
        | Value::Prim { .. }
        | Value::Call { .. }
        | Value::Make { .. }
        | Value::Tag(_)
        | Value::Tuple(_)
        | Value::ListIsEmpty(_)
        | Value::FnRef(_)
        | Value::Closure(_)
        | Value::MakeClosure { .. }
        | Value::EnvGet { .. }
        | Value::Handler { .. }
        | Value::Fiber { .. }
        | Value::IsError { .. }
        | Value::Runtime { .. }
        | Value::Coerce(_) => ResultOwnership::Owned,
    }
}

pub(in crate::backend) fn require_runtime(
    value: &Value,
    active: bool,
    requirements: &mut runtime_plan::Requirements,
) {
    match value {
        Value::Prim { op, ty, .. } => {
            primitive_operations::require_runtime(*op, *ty, active, requirements)
        }
        Value::Builtin { op, args } => builtins::require_runtime(*op, args, requirements),
        Value::Runtime { op, args } => super::runtime::require_runtime(*op, args, requirements),
        Value::Call {
            callee,
            suspends,
            on_fail,
            ..
        } => call::require_runtime(callee, *suspends, on_fail, requirements),
        Value::Const(_)
        | Value::Make { .. }
        | Value::Field { .. }
        | Value::Tag(_)
        | Value::Tuple(_)
        | Value::TupleGet { .. }
        | Value::List { .. }
        | Value::ListIsEmpty(_)
        | Value::ListHead(_)
        | Value::ListTail(_)
        | Value::FnRef(_)
        | Value::Closure(_)
        | Value::MakeClosure { .. }
        | Value::EnvGet { .. }
        | Value::EvLookup { .. }
        | Value::EvExtend { .. }
        | Value::Handler { .. }
        | Value::Fiber { .. }
        | Value::IsError { .. }
        | Value::AsError { .. }
        | Value::Coerce(_) => {}
    }
}

pub(in crate::backend) fn is_known_empty(
    value: &Value,
    known_empty: &HashSet<VarId>,
    empty_list_functions: &HashSet<FnId>,
) -> bool {
    match value {
        Value::Call { callee, .. } => call::is_known_empty(callee, empty_list_functions),
        Value::List { elems, tail } => list::is_known_empty(elems, tail, known_empty),
        Value::Const(_)
        | Value::Prim { .. }
        | Value::Builtin { .. }
        | Value::Make { .. }
        | Value::Field { .. }
        | Value::Tag(_)
        | Value::Tuple(_)
        | Value::TupleGet { .. }
        | Value::ListIsEmpty(_)
        | Value::ListHead(_)
        | Value::ListTail(_)
        | Value::FnRef(_)
        | Value::Closure(_)
        | Value::MakeClosure { .. }
        | Value::EnvGet { .. }
        | Value::EvLookup { .. }
        | Value::EvExtend { .. }
        | Value::Handler { .. }
        | Value::Fiber { .. }
        | Value::IsError { .. }
        | Value::AsError { .. }
        | Value::Runtime { .. }
        | Value::Coerce(_) => false,
    }
}

pub(in crate::backend) fn needs_heap(
    value: &Value,
    dst: Option<VarId>,
    context: &HeapContext<'_, '_>,
) -> bool {
    match value {
        Value::FnRef(target) => fn_ref::needs_heap(*target, context),
        Value::List { elems, tail } => list::needs_heap(elems, tail, context),
        Value::ListHead(_) => list_head::needs_heap(),
        Value::ListTail(_) => list_tail::needs_heap(),
        Value::Fiber { .. } => fiber::needs_heap(),
        Value::Builtin { op, .. } => builtins::needs_heap(*op),
        Value::Handler { .. } => handler::needs_heap(),
        Value::EvLookup { .. } => ev_lookup::needs_heap(),
        Value::EvExtend { bindings, .. } => ev_extend::needs_heap(bindings.is_empty()),
        Value::Coerce(source) => coerce::needs_heap(dst, *source, context),
        Value::Call { callee, .. } => call::needs_heap(callee),
        Value::Runtime { op, .. } => super::runtime::needs_heap(*op),
        Value::Const(_)
        | Value::Prim { .. }
        | Value::Make { .. }
        | Value::Field { .. }
        | Value::Tag(_)
        | Value::Tuple(_)
        | Value::TupleGet { .. }
        | Value::ListIsEmpty(_)
        | Value::Closure(_)
        | Value::MakeClosure { .. }
        | Value::EnvGet { .. }
        | Value::IsError { .. }
        | Value::AsError { .. } => false,
    }
}

pub(in crate::backend) fn for_each_plan_var(
    layouts: &TypeLayouts<'_>,
    value: &Value,
    mut visit: impl FnMut(VarId),
) {
    value.for_each_var(&mut visit);
    match value {
        Value::FnRef(target) => fn_ref::for_each_capture(layouts, *target, visit),
        Value::Const(_)
        | Value::Prim { .. }
        | Value::Call { .. }
        | Value::Builtin { .. }
        | Value::Make { .. }
        | Value::Field { .. }
        | Value::Tag(_)
        | Value::Tuple(_)
        | Value::TupleGet { .. }
        | Value::List { .. }
        | Value::ListIsEmpty(_)
        | Value::ListHead(_)
        | Value::ListTail(_)
        | Value::Closure(_)
        | Value::MakeClosure { .. }
        | Value::EnvGet { .. }
        | Value::EvLookup { .. }
        | Value::EvExtend { .. }
        | Value::Handler { .. }
        | Value::Fiber { .. }
        | Value::IsError { .. }
        | Value::AsError { .. }
        | Value::Runtime { .. }
        | Value::Coerce(_) => {}
    }
}
