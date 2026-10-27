//! Embedded C backend foundations.
//!
//! This crate verifies Flow IR and emits C with fixed-capacity memory and feature-specific
//! runtimes without running Heap IR passes. A convenience entry also lowers ordinary Core IR.

pub mod artifact;
pub mod boards;
mod const_eval;
mod const_origins;
mod const_runtime;
mod derived;
pub use derived::{DerivedOperation, derived_binding};
mod backend;
mod discarded;
mod float_parse;
mod float_to_string;
mod heap;
mod scope_abi;
mod type_expansion;
mod unicode_grapheme;
mod writer;

mod contract;
mod input;

pub use contract::{
    Error, ErrorKind, ExportedFunction, ExternalAdapter, ExternalResult, GeneratedFile, Options,
    Output,
};
pub use input::{flow_from_core, verify_flow};

/// Converts Core IR to Flow IR and emits embedded C files.
pub fn compile(core: &emela_core::Module, options: &Options) -> Result<Output, Vec<Error>> {
    let flow = flow_from_core(core)?;
    compile_flow(&flow, options)
}

/// Verifies and emits a Flow IR module without running Heap IR passes.
pub fn compile_flow(flow: &emela_lir::Module, options: &Options) -> Result<Output, Vec<Error>> {
    backend::compile_flow(flow, options)
}
