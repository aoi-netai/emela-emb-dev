//! Data contracts. These modules must not depend on compiler or platform implementations.

mod error;
mod options;
mod output;

pub use error::{Error, ErrorKind};
pub use options::{ExternalAdapter, ExternalResult, Options};
pub use output::{ExportedFunction, GeneratedFile, Output};
