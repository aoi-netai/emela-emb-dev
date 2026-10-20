//! Generate a finite-layout growing enum derivation probe.
use emela_codegen_emb::{Options, compile};
use emela_core::Roots;

#[path = "../../emela-core/tests/support/mod.rs"]
mod support;

fn main() {
    let directory = std::env::args()
        .nth(1)
        .expect("usage: growing_derive_probe OUTPUT [legacy]");
    let core = support::try_lower(
        &[("main.emel", include_str!("support/growing_derive.emel"))],
        Roots::Exported,
    )
    .unwrap();
    if std::env::args().nth(2).as_deref() == Some("legacy") {
        let _ = emela_lir::from_core(emela_core::monomorphize(&core));
        return;
    }
    let output = compile(
        &core,
        &Options::default()
            .with_heap_bytes(2048)
            .with_string_capacity(64),
    )
    .unwrap();
    std::fs::create_dir_all(&directory).unwrap();
    for file in output.files {
        std::fs::write(
            std::path::Path::new(&directory).join(file.path),
            file.contents,
        )
        .unwrap();
    }
}
