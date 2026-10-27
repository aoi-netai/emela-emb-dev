//! Keep the extracted data contract independent of upstream IR and implementations.
use std::{fs, path::Path};
use syn::visit::{self, Visit};

#[derive(Default)]
struct ContractDependencies {
    violations: Vec<String>,
}

impl ContractDependencies {
    fn path(&mut self, segments: &[String]) {
        let names: Vec<_> = segments.iter().map(String::as_str).collect();
        let forbidden = match names.as_slice() {
            ["crate", module, ..] => *module != "contract",
            ["super", ..] => true,
            ["std", module, ..] => !["collections", "error", "fmt"].contains(module),
            _ => names.iter().any(|name| {
                matches!(
                    *name,
                    "emela_core" | "emela_lir" | "backend" | "input" | "writer"
                )
            }),
        };
        if forbidden {
            self.violations.push(segments.join("::"));
        }
    }

    fn imports(&mut self, prefix: &mut Vec<String>, tree: &syn::UseTree) {
        match tree {
            syn::UseTree::Path(path) => {
                prefix.push(path.ident.to_string());
                self.imports(prefix, &path.tree);
                prefix.pop();
            }
            syn::UseTree::Group(group) => {
                for tree in &group.items {
                    self.imports(prefix, tree);
                }
            }
            syn::UseTree::Name(name) => {
                prefix.push(name.ident.to_string());
                self.path(prefix);
                prefix.pop();
            }
            syn::UseTree::Rename(rename) => {
                prefix.push(rename.ident.to_string());
                self.path(prefix);
                prefix.pop();
            }
            syn::UseTree::Glob(_) => self.violations.push("glob import".into()),
        }
    }
}

impl<'ast> Visit<'ast> for ContractDependencies {
    fn visit_path(&mut self, path: &'ast syn::Path) {
        self.path(
            &path
                .segments
                .iter()
                .map(|segment| segment.ident.to_string())
                .collect::<Vec<_>>(),
        );
        visit::visit_path(self, path);
    }

    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        self.imports(&mut Vec::new(), &item.tree);
    }

    fn visit_macro(&mut self, invocation: &'ast syn::Macro) {
        if invocation.path.segments.last().is_some_and(|segment| {
            matches!(
                segment.ident.to_string().as_str(),
                "include" | "include_str" | "include_bytes"
            )
        }) {
            self.violations
                .push("external source or asset include".into());
        }
        visit::visit_macro(self, invocation);
    }

    fn visit_attribute(&mut self, attribute: &'ast syn::Attribute) {
        if attribute.path().is_ident("path") {
            self.violations.push("module path override".into());
        }
        visit::visit_attribute(self, attribute);
    }
}

#[test]
fn contracts_do_not_depend_on_upstream_ir_or_compiler_implementations() {
    let mut pending = vec![Path::new(env!("CARGO_MANIFEST_DIR")).join("src/contract")];
    let mut checked = 0;
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let mut dependencies = ContractDependencies::default();
                dependencies
                    .visit_file(&syn::parse_file(&fs::read_to_string(&path).unwrap()).unwrap());
                assert!(
                    dependencies.violations.is_empty(),
                    "{}: {:?}",
                    path.display(),
                    dependencies.violations
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 0, "the contract source directory must be checked");
}

#[test]
fn contract_guard_rejects_aliases_grouped_imports_and_indirect_source_includes() {
    for source in [
        "use emela_lir::Module as Program;",
        "use std::{collections::HashMap, fs as storage};",
        "pub use crate::{backend::compiler as implementation};",
        "type Layout = crate::backend::compiler::SomeLayout;",
        "use super::implementation;",
        "use crate::contract::*;",
        "include!(\"../backend/implementation.rs\");",
        "#[path = \"../backend/implementation.rs\"] mod implementation;",
    ] {
        let mut dependencies = ContractDependencies::default();
        dependencies.visit_file(&syn::parse_file(source).unwrap());
        assert!(!dependencies.violations.is_empty(), "accepted {source}");
    }
}
