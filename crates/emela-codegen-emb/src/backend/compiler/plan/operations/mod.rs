use super::*;

pub(in crate::backend::compiler::plan) mod builtins;
pub(in crate::backend::compiler::plan) mod failures;
pub(in crate::backend::compiler::plan) mod fibers;
pub(in crate::backend::compiler::plan) mod primitive_operations;
pub(in crate::backend::compiler::plan) mod runtime;
pub(in crate::backend::compiler::plan) mod statements;
pub(in crate::backend::compiler::plan) mod terms;
pub(in crate::backend::compiler::plan) mod values;

use super::suspend::{Graph, LoopTarget, Node};
pub(in crate::backend::compiler::plan) struct LoweringContext<'a> {
    pub(in crate::backend::compiler::plan) graph: &'a mut Graph,
    pub(in crate::backend::compiler::plan) function: &'a Function,
}
