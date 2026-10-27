//! Standalone connection: reads only generated C/headers/manifest and package inputs.
use emela_codegen_emb::platform::{self, ConnectOptions, Optimization, PlatformPackage};
use std::path::Path;

pub struct ArtifactConnection<'a> {
    pub artifact: &'a Path,
    pub output: &'a Path,
    pub platform: &'a str,
    pub package: Option<&'a Path>,
    pub compiler: &'a Path,
    pub optimization: Optimization,
    pub main: bool,
}
pub fn connect_artifact(request: ArtifactConnection<'_>) -> Result<std::path::PathBuf, String> {
    let output = platform::execution::load_artifact(request.artifact)?;
    let package = if let Some(path) = request.package {
        serde_json::from_slice::<PlatformPackage>(
            &std::fs::read(path).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?
    } else if request.platform == "native" {
        platform::native()
    } else {
        platform::board(request.platform)?
    };
    let toolchain = platform::execution::inspect_toolchain(request.compiler)?;
    let options = ConnectOptions {
        optimization: request.optimization,
        executable: if request.platform == "native" {
            "program"
        } else {
            "firmware.elf"
        }
        .into(),
        main: request.main && (request.package.is_some() || request.platform == "native"),
    };
    let plan = platform::prepare_connection(&output, &package, &options, &toolchain)?;
    platform::execution::execute(&plan, request.output)
}
pub use emela_codegen_emb::platform::Optimization as ConnectionOptimization;
