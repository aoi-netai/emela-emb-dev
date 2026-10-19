use std::fs;
use std::path::PathBuf;

use emela_codegen_emb::{Options, compile};
use emela_core::{BinOp, External, ExternalBinding, HostRef, Module, OpTy, TypeId, build};

fn main() {
    let output_dir = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/renode-f446/generated"));

    let mut core = Module::new();
    let main = core.declare("main", true);
    let compute = core.declare("renode_smoke_compute", false);
    let input = core.local("input", TypeId::INT);
    let is_large = core.local("is_large", TypeId::BOOL);
    core.define(
        compute,
        vec![input],
        TypeId::INT,
        build::let_(
            is_large,
            build::bin(BinOp::Gt, OpTy::Int, build::var(input), build::int(20)),
            build::if_(
                build::var(is_large),
                build::bin(BinOp::Mul, OpTy::Int, build::var(input), build::int(2)),
                build::bin(BinOp::Add, OpTy::Int, build::var(input), build::int(24)),
            ),
        ),
    );
    let signal = core.declare("renode_smoke_signal", false);
    let signal_value = core.local("value", TypeId::INT);
    core.define_external(
        signal,
        vec![signal_value],
        TypeId::UNIT,
        External {
            bindings: vec![ExternalBinding {
                target: "c".to_owned(),
                host: HostRef::Global("renode_smoke_signal".to_owned()),
            }],
        },
    );
    let first_result = core.local("first_result", TypeId::INT);
    let first_signal = core.local("first_signal", TypeId::UNIT);
    let second_result = core.local("second_result", TypeId::INT);
    let second_signal = core.local("second_signal", TypeId::UNIT);
    core.define(
        main,
        Vec::new(),
        TypeId::UNIT,
        build::let_(
            first_result,
            build::call(compute, vec![build::int(21)]),
            build::let_(
                first_signal,
                build::call(signal, vec![build::var(first_result)]),
                build::let_(
                    second_result,
                    build::call(compute, vec![build::int(10)]),
                    build::let_(
                        second_signal,
                        build::call(signal, vec![build::var(second_result)]),
                        build::unit(),
                    ),
                ),
            ),
        ),
    );

    fs::create_dir_all(&output_dir).expect("create output directory");
    let output = compile(&core, &Options::default()).expect("compile embedded C");
    for file in output.files {
        fs::write(output_dir.join(file.path), file.contents).expect("write generated file");
    }
}
