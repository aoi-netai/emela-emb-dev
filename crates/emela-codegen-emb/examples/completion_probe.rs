//! Generate the combined const, recursive type, external ABI and suspension probe.
use emela_codegen_emb::{ExternalAdapter, ExternalResult, Options, compile_flow, flow_from_core};
use emela_core::Roots;
#[path = "support/gc_anchor.rs"]
mod gc_anchor;
#[path = "../../emela-core/tests/support/mod.rs"]
mod support;
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let directory = std::path::Path::new(args.get(1).expect("output directory"));
    let cycles = args.get(2).is_some_and(|arg| arg == "gc");
    let core = support::try_lower(
        &[("main.emel", include_str!("support/completion_2_4.emel"))],
        Roots::Exported,
    )
    .unwrap();
    let mut flow = flow_from_core(&core).unwrap();
    if cycles {
        gc_anchor::add(&mut flow);
    }
    let options = Options::default()
        .with_heap_bytes(8192)
        .with_frame_bytes(512)
        .with_const_diagnostic_bytes(512)
        .with_external_adapter(
            "host_text",
            ExternalAdapter::new("board_text", ExternalResult::Borrowed),
        )
        .with_task_adapter("host_wait", "board_wait_start");
    let output = compile_flow(&flow, &options).unwrap();
    std::fs::create_dir_all(directory).unwrap();
    for file in output.files {
        std::fs::write(directory.join(file.path), file.contents).unwrap();
    }
}
