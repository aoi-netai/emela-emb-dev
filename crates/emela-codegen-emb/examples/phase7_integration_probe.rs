use emela_codegen_emb::{Options, compile_flow};
use std::{fs, path::PathBuf};

#[path = "support/phase7_integration_flow.rs"]
mod fixture;
#[path = "../../emela-core/tests/support/mod.rs"]
mod support;

fn main() {
    let mut args = std::env::args_os().skip(1);
    let directory = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/phase7-integration-probe/generated"));
    let with_collector = args.next().is_some_and(|mode| mode == "gc");
    let output = compile_flow(
        &fixture::lower(with_collector),
        &Options::default()
            .with_heap_bytes(4096)
            .with_string_capacity(32)
            .with_fiber_capacity(8)
            .with_frame_capacity(48)
            .with_frame_bytes(240)
            .with_task_capacity(2)
            .with_event_capacity(2)
            .with_deferred_capacity(2)
            .with_task_adapter("host_request", "board_request_start"),
    )
    .expect("compile phase 7 integration fixture");
    fs::create_dir_all(&directory).expect("create output directory");
    for file in output.files {
        fs::write(directory.join(file.path), file.contents).expect("write generated file");
    }
}
