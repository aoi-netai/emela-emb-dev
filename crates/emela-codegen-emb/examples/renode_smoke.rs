use std::fs;
use std::path::PathBuf;

use emela_codegen_emb::{Options, compile};
use emela_core::{External, ExternalBinding, HostRef, Module, TypeId, build};

fn main() {
    let output_dir = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/renode-f446/generated"));

    let mut core = Module::new();
    let main = core.declare("main", true);
    let signal = core.declare("renode_smoke_signal", false);
    core.define_external(
        signal,
        Vec::new(),
        TypeId::UNIT,
        External {
            bindings: vec![ExternalBinding {
                target: "c".to_owned(),
                host: HostRef::Global("renode_smoke_signal".to_owned()),
            }],
        },
    );
    core.define(main, Vec::new(), TypeId::UNIT, build::call(signal, Vec::new()));

    fs::create_dir_all(&output_dir).expect("create output directory");
    let output = compile(&core, &Options::default()).expect("compile embedded C");
    for file in output.files {
        fs::write(output_dir.join(file.path), file.contents).expect("write generated file");
    }
}
