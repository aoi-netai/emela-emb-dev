//! 組み込みCソースの出力段階。リンクはボード側のビルドで行う。

use emela_codegen_c_embedded::compile;
use std::path::PathBuf;

use crate::diagnostic::Diagnostic;
use crate::output::{BuildOutput, OutputFile};
use crate::pipeline::{Analysis, Backend};

/// GPIO のボード境界を持つ組み込みC出力。
#[derive(Debug, Default, Clone, Copy)]
pub struct EmbeddedC;

impl Backend<emela_core::Module> for EmbeddedC {
    fn emit(
        &mut self,
        module: &emela_core::Module,
        _: &Analysis,
    ) -> Result<BuildOutput, Vec<Diagnostic>> {
        let output = compile(module).map_err(|errors| {
            errors
                .into_iter()
                .map(|error| Diagnostic::error(error.message))
                .collect::<Vec<_>>()
        })?;
        Ok(BuildOutput {
            files: vec![
                OutputFile::new("main.c", output.main),
                OutputFile::new("emela_gpio.h", output.gpio_header),
            ],
            entry: PathBuf::from("main.c"),
        })
    }

    fn runner(&self) -> crate::pipeline::Runner {
        crate::pipeline::Runner::None
    }
}
