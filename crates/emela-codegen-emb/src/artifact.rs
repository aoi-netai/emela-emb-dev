//! Versioned contract between C generation and a C/BSP build.
//!
//! Headers define C layouts, signatures, and ownership. This manifest only identifies
//! build inputs and interface roles; it does not duplicate declarations from the headers.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::{GeneratedFile, Output};

pub const MANIFEST_FILE: &str = "emela_artifact.json";
pub const SCHEMA_VERSION: u32 = 1;
pub const ABI_VERSION: u32 = 1;
pub const PUBLIC_HEADER: &str = "emela_embedded.h";
pub const PLATFORM_HEADER: &str = "emela_board.h";
pub const ALIASES_HEADER: &str = "emela_embedded_exports.h";

/// One ordered build input. All paths are relative to the manifest directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactFile {
    pub path: String,
    pub role: FileRole,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileRole {
    Source,
    PublicHeader,
    PlatformHeader,
    AliasesHeader,
    Header,
}

/// C11 source files and their generated interfaces, independent of a board/toolchain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema_version: u32,
    pub abi_version: u32,
    pub files: Vec<ArtifactFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactError(String);

impl std::fmt::Display for ArtifactError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid generated artifact: {}", self.0)
    }
}

impl std::error::Error for ArtifactError {}

impl Manifest {
    /// Describes an in-memory output, ignoring its previous manifest if present.
    pub fn from_files(files: &[GeneratedFile]) -> Result<Self, ArtifactError> {
        let mut entries = Vec::new();
        let mut manifests = 0;
        for file in files {
            let role = match file.path.as_str() {
                MANIFEST_FILE => {
                    manifests += 1;
                    continue;
                }
                PUBLIC_HEADER => FileRole::PublicHeader,
                PLATFORM_HEADER => FileRole::PlatformHeader,
                ALIASES_HEADER => FileRole::AliasesHeader,
                path if path.ends_with(".c") => FileRole::Source,
                path if path.ends_with(".h") => FileRole::Header,
                path => {
                    return Err(ArtifactError(format!(
                        "unsupported generated file `{path}`"
                    )));
                }
            };
            entries.push(ArtifactFile {
                path: file.path.clone(),
                role,
            });
        }
        if manifests > 1 {
            return Err(ArtifactError("duplicate manifest".into()));
        }
        let manifest = Self {
            schema_version: SCHEMA_VERSION,
            abi_version: ABI_VERSION,
            files: entries,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn from_json(json: &str) -> Result<Self, ArtifactError> {
        let manifest: Self =
            serde_json::from_str(json).map_err(|error| ArtifactError(error.to_string()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn to_json(&self) -> Result<String, ArtifactError> {
        self.validate()?;
        serde_json::to_string_pretty(self)
            .map(|json| json + "\n")
            .map_err(|error| ArtifactError(error.to_string()))
    }

    pub fn validate(&self) -> Result<(), ArtifactError> {
        if self.schema_version != SCHEMA_VERSION || self.abi_version != ABI_VERSION {
            return Err(ArtifactError(format!(
                "unsupported schema/ABI version {}/{} (expected {SCHEMA_VERSION}/{ABI_VERSION})",
                self.schema_version, self.abi_version
            )));
        }
        let mut paths = HashSet::new();
        let mut interfaces = HashSet::new();
        let mut sources = 0;
        for file in &self.files {
            // Use the same portable, normalized POSIX paths in Rust and Python. Reject
            // Windows paths too, even when generation is running on a Unix host.
            if file.path.contains(['\\', ':', '\0'])
                || file
                    .path
                    .split('/')
                    .any(|part| matches!(part, "" | "." | ".."))
            {
                return Err(ArtifactError(format!(
                    "invalid relative path `{}`",
                    file.path
                )));
            }
            if !paths.insert(&file.path) {
                return Err(ArtifactError(format!("duplicate path `{}`", file.path)));
            }
            let expected = match file.role {
                FileRole::Source => {
                    sources += 1;
                    if !file.path.ends_with(".c") {
                        return Err(ArtifactError("source must have the .c suffix".into()));
                    }
                    continue;
                }
                FileRole::Header => {
                    if !file.path.ends_with(".h")
                        || [PUBLIC_HEADER, PLATFORM_HEADER, ALIASES_HEADER]
                            .contains(&file.path.as_str())
                    {
                        return Err(ArtifactError(format!(
                            "invalid header role for `{}`",
                            file.path
                        )));
                    }
                    continue;
                }
                FileRole::PublicHeader => PUBLIC_HEADER,
                FileRole::PlatformHeader => PLATFORM_HEADER,
                FileRole::AliasesHeader => ALIASES_HEADER,
            };
            if file.path != expected || !interfaces.insert(file.role) {
                return Err(ArtifactError(format!(
                    "invalid interface role for `{}`",
                    file.path
                )));
            }
        }
        if sources == 0
            || !interfaces.contains(&FileRole::PublicHeader)
            || !interfaces.contains(&FileRole::PlatformHeader)
        {
            return Err(ArtifactError(
                "source, public_header, and platform_header are required".into(),
            ));
        }
        Ok(())
    }
}

impl Output {
    /// Rebuilds the manifest after adding generated C or headers (for example, export
    /// aliases or a native adapter). On validation failure the output is untouched.
    pub fn refresh_manifest(&mut self) -> Result<(), ArtifactError> {
        let contents = Manifest::from_files(&self.files)?.to_json()?;
        self.files.retain(|file| file.path != MANIFEST_FILE);
        self.files.push(GeneratedFile {
            path: MANIFEST_FILE.into(),
            contents,
        });
        Ok(())
    }
}

impl From<ArtifactError> for crate::Error {
    fn from(error: ArtifactError) -> Self {
        Self {
            kind: crate::ErrorKind::InvalidOutput,
            message: error.to_string(),
        }
    }
}
