//! Flow IR backend with explicit feature owners and checked connection passes.
mod board;
#[cfg(test)]
mod cnative;
pub(crate) mod compiler;

pub(crate) fn compile_flow(
    flow: &emela_lir::Module,
    options: &crate::Options,
) -> Result<crate::Output, Vec<crate::Error>> {
    let compiled = compiler::compile_flow(flow)?;
    board::link(&compiled, options)
}

#[cfg(test)]
use compiler::*;
#[cfg(test)]
#[path = "tests/separation.rs"]
mod separation_tests;
#[cfg(test)]
#[path = "../../../emela-core/tests/support/mod.rs"]
pub(super) mod support;
