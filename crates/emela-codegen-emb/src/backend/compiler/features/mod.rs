use super::*;
pub(in crate::backend) mod builtin_utilities;
pub(in crate::backend) mod builtins;
pub(in crate::backend) mod statements;
pub(in crate::backend) mod values;
pub(in crate::backend) struct Context<'a, 'ir> {
    pub(in crate::backend) layouts: &'a TypeLayouts<'ir>,
    pub(in crate::backend) function: &'a Function,
    pub(in crate::backend) writer: &'a mut Writer,
    pub(in crate::backend) names: &'a HashMap<FnId, String>,
    pub(in crate::backend) string_buffers: &'a StringBufferPlan,
    pub(in crate::backend) loops: &'a mut Vec<Vec<VarId>>,
    pub(in crate::backend) blocks: &'a mut Vec<BlockTarget>,
    pub(in crate::backend) protects: &'a mut Vec<ProtectTarget>,
    pub(in crate::backend) next_temp: &'a mut usize,
    pub(in crate::backend) next_label: &'a mut usize,
}
pub(in crate::backend) struct BuiltinContext<'a, 'ir> {
    pub(in crate::backend) layouts: &'a TypeLayouts<'ir>,
    pub(in crate::backend) function: &'a Function,
    pub(in crate::backend) string_buffers: &'a StringBufferPlan,
    pub(in crate::backend) writer: &'a mut Writer,
}
use builtin_utilities::ResultType;

type Emission = Result<(), Vec<Error>>;

pub(in crate::backend) mod constants;

pub(in crate::backend) mod operands;

pub(in crate::backend) mod representations;

pub(in crate::backend) mod primitive_operations;

pub(in crate::backend) mod primitive_types;

pub(in crate::backend) mod terms;

pub(in crate::backend) mod failures;

pub(in crate::backend) mod runtime;

pub(in crate::backend) mod stages;

pub(in crate::backend) mod case_keys;
pub(in crate::backend) mod fibers;
pub(in crate::backend) mod switches;
use super::suspend::{Graph, LoopTarget, Node};
pub(in crate::backend) struct LoweringContext<'a> {
    pub(in crate::backend) graph: &'a mut Graph,
    pub(in crate::backend) function: &'a Function,
}

pub(in crate::backend) struct SuspendContext<'a, 'ir> {
    pub(in crate::backend) flow: &'a emela_lir::Module,
    pub(in crate::backend) layouts: &'a TypeLayouts<'ir>,
    pub(in crate::backend) function: &'a Function,
    pub(in crate::backend) writer: &'a mut Writer,
    pub(in crate::backend) names: &'a HashMap<FnId, String>,
    pub(in crate::backend) strings: &'a StringBufferPlan,
    pub(in crate::backend) next_temp: &'a mut usize,
    pub(in crate::backend) next: usize,
    pub(in crate::backend) index: usize,
    pub(in crate::backend) failing: bool,
    pub(in crate::backend) call_error_var: VarId,
    pub(in crate::backend) call_error_label: LabelId,
}

use super::suspend::{emit_operation, emit_sync_statement, prepare_result};

pub(in crate::backend) mod types;

pub(in crate::backend) mod host_modules;
pub(in crate::backend) mod host_refs;
pub(in crate::backend) mod variant_fields;

pub(in crate::backend) mod control_utilities;

use super::suspend::retain_frame_value;
