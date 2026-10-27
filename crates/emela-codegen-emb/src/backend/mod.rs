//! Target independent C compilation.
pub(crate) mod compiler;
pub(crate) use compiler::compile_flow;

#[cfg(test)]
#[path = "tests/separation.rs"]
mod separation_tests;
#[cfg(test)]
#[path = "../../../emela-core/tests/support/mod.rs"]
pub(super) mod support;
