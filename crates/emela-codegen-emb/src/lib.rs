//! Embedded C backend foundations.
//!
//! This crate starts from Core IR, obtains and verifies Flow IR, and will emit embedded C without
//! running the Heap IR passes. C emission is added in later implementation phases.

mod emit;
#[allow(dead_code)]
mod writer;

/// Configuration for embedded C generation.
///
/// The initial Flow IR handoff does not need target-specific settings yet. This type is kept
/// non-exhaustive so board and resource options can be added without requiring struct literals.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct Options {}

/// Generated files returned by the backend.
///
/// File emission is not implemented in this phase; this type establishes the public result shape
/// for the later C emission phases.
#[derive(Debug, Clone, Default)]
pub struct Output {
    /// Files keyed by their path relative to the generated output directory.
    pub files: Vec<GeneratedFile>,
}

/// One file produced by the backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedFile {
    /// Path relative to the generated output directory.
    pub path: String,
    /// File contents.
    pub contents: String,
}

/// The stage that failed while preparing Flow IR for embedded code generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// The Core module could not be verified after monomorphization.
    InvalidCore,
    /// Core IR could not be converted to Flow IR.
    FlowLowering,
    /// The Flow IR module was not valid for the Flow stage.
    InvalidFlowIr,
    /// A valid Flow IR construct is not implemented by this backend yet.
    Unsupported,
}

/// An error encountered while preparing a module for embedded code generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub kind: ErrorKind,
    pub message: String,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

/// Converts Core IR to Flow IR and emits embedded C files.
pub fn compile(core: &emela_core::Module, options: &Options) -> Result<Output, Vec<Error>> {
    let flow = flow_from_core(core)?;
    compile_flow(&flow, options)
}

/// Verifies and emits a Flow IR module without running Heap IR passes.
pub fn compile_flow(flow: &emela_lir::Module, options: &Options) -> Result<Output, Vec<Error>> {
    verify_flow(flow)?;
    emit::emit(flow, options)
}

/// Monomorphizes Core IR, lowers it to Flow IR, and verifies the Flow-stage invariants.
///
/// This path deliberately stops at Flow IR. It does not call `emela_lir::to_heap`.
pub fn flow_from_core(core: &emela_core::Module) -> Result<emela_lir::Module, Vec<Error>> {
    let mono = emela_core::monomorphize(core);
    if let Err(errors) = emela_core::verify_monomorphic(&mono) {
        return Err(errors
            .into_iter()
            .map(|error| Error {
                kind: ErrorKind::InvalidCore,
                message: format!("invalid Core IR after monomorphization: {error}"),
            })
            .collect());
    }

    let flow = emela_lir::from_core(mono).map_err(|errors| {
        errors
            .into_iter()
            .map(|error| Error {
                kind: ErrorKind::FlowLowering,
                message: format!("failed to lower Core IR to Flow IR: {error}"),
            })
            .collect::<Vec<_>>()
    })?;

    verify_flow(&flow)?;
    Ok(flow)
}

/// Verifies that a LIR module is still in the Flow stage and satisfies its invariants.
pub fn verify_flow(flow: &emela_lir::Module) -> Result<(), Vec<Error>> {
    if flow.stage != emela_lir::Stage::Flow {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "expected Flow IR, found {:?}; embedded codegen does not run Heap IR passes",
                flow.stage
            ),
        }]);
    }

    emela_lir::verify(flow).map_err(|errors| {
        errors
            .into_iter()
            .map(|error| Error {
                kind: ErrorKind::InvalidFlowIr,
                message: format!("invalid Flow IR: {error}"),
            })
            .collect()
    })
}
