use emela_codegen_emb::{Options, compile_flow};
use std::{fs, path::PathBuf};
#[path = "support/suspend_flow.rs"]
mod suspend_flow;

fn main() {
    let dir = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/suspend-probe/generated"));
    let options = Options::default()
        .with_fiber_capacity(2)
        .with_frame_capacity(64)
        .with_frame_bytes(128);
    let output =
        compile_flow(&suspend_flow::recursive_flow(), &options).expect("compile suspend probe");
    fs::create_dir_all(&dir).expect("create output directory");
    for file in output.files {
        fs::write(dir.join(file.path), file.contents).expect("write generated file");
    }
}
