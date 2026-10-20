//! Generate the completion integration probe for STM32F446/Renode.
use emela_codegen_emb::{ExternalAdapter, ExternalResult, Options, compile};
use emela_core::Roots;
use std::{fs, path::PathBuf};

#[path = "../../emela-core/tests/support/mod.rs"]
mod support;

fn main() {
    let directory = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .expect("output directory");
    let core = support::try_lower(
        &[("main.emel", include_str!("support/renode_completion.emel"))],
        Roots::Exported,
    )
    .unwrap();
    let mut output = compile(
        &core,
        &Options::default()
            .with_heap_bytes(8192)
            .with_external_adapter(
                "host_completion_text",
                ExternalAdapter::new("board_completion_text", ExternalResult::Borrowed),
            ),
    )
    .unwrap();
    let header = &output
        .files
        .iter()
        .find(|f| f.path == "emela_embedded.h")
        .unwrap()
        .contents;
    let public_name = |name: &str| {
        let start = header.find(&format!("emela_export_{name}_")).unwrap();
        header[start..].split('(').next().unwrap().to_owned()
    };
    let check = public_name("check");
    let text = public_name("text_value");
    let derived = public_name("derived_text");
    let probe = format!(
        "#ifndef EMELA_COMPLETION_PROBE_H\n#define EMELA_COMPLETION_PROBE_H\n#include <stddef.h>\n#include \"emela_embedded.h\"\n#define emela_completion_check {check}\n#define emela_completion_text {text}\n#define emela_completion_derived {derived}\nsize_t emela_completion_heap(void);\n#endif\n"
    );
    output
        .files
        .iter_mut()
        .find(|f| f.path == "emela_embedded.c")
        .unwrap()
        .contents
        .push_str(
            r#"
size_t emela_completion_heap(void) {
  size_t used = 0;
  if (!emela_heap_initialized) return 0;
  for (unsigned char *p=emela_heap_storage; p<emela_heap_end();) {
    emela_heap_block *block=(emela_heap_block *)(void *)p;
    if (!block->is_free) used += block->size;
    p += block->size;
  }
  return used;
}
"#,
        );
    fs::create_dir_all(&directory).unwrap();
    for file in output.files {
        fs::write(directory.join(file.path), file.contents).unwrap();
    }
    fs::write(directory.join("completion_probe.h"), probe).unwrap();
}
