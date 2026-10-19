//! Embedded C backend foundations.
//!
//! This crate starts from Core IR, obtains and verifies Flow IR, and will emit embedded C without
//! running the Heap IR passes. C emission is added in later implementation phases.

mod const_eval;
mod emit;
mod float_parse;
mod heap;
mod unicode_grapheme;
mod writer;

/// Configuration for embedded C generation.
///
/// Configuration for board-independent generation and fixed embedded resource limits.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Options {
    string_capacity: u16,
    heap_bytes: u32,
    isr_entries: Vec<String>,
    isr_safe_externals: Vec<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            string_capacity: 256,
            heap_bytes: 0,
            isr_entries: Vec::new(),
            isr_safe_externals: Vec::new(),
        }
    }
}

impl Options {
    /// Sets the maximum size in bytes for a dynamically generated String buffer.
    ///
    /// Values from 1 through 512 are accepted. The generated code reports a String capacity
    /// defect if an operation exceeds this limit.
    pub fn with_string_capacity(mut self, bytes: u16) -> Self {
        self.string_capacity = bytes;
        self
    }

    /// Returns the maximum configured size in bytes for a dynamically generated String buffer.
    pub fn string_capacity(&self) -> u16 {
        self.string_capacity
    }

    /// Sets the total byte capacity reserved for the generated fixed heap.
    ///
    /// A capacity of zero disables heap-backed values. The default is zero.
    pub fn with_heap_bytes(mut self, bytes: u32) -> Self {
        self.heap_bytes = bytes;
        self
    }

    /// Returns the configured byte capacity for the generated fixed heap.
    pub fn heap_bytes(&self) -> u32 {
        self.heap_bytes
    }

    /// Marks an exported Emela function as an interrupt-service entry point.
    pub fn with_isr_entry(mut self, function_name: impl Into<String>) -> Self {
        self.isr_entries.push(function_name.into());
        self
    }

    /// Marks a C symbol as safe to call from a configured interrupt-service entry point.
    pub fn with_isr_safe_external(mut self, symbol: impl Into<String>) -> Self {
        self.isr_safe_externals.push(symbol.into());
        self
    }
}

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

/// The stage that failed while compiling Flow IR to embedded C.
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
    /// Backend options are outside their documented range.
    InvalidOptions,
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
