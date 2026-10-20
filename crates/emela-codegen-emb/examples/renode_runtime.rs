//! Generate the scheduler and real-ISR integration probe for STM32F446/Renode.
use emela_codegen_emb::{Options, compile_flow};
use emela_core::{External, ExternalBinding, HostRef, Roots};
use std::{fs, path::PathBuf};

#[path = "support/gc_anchor.rs"]
mod gc_anchor;
#[path = "../../emela-core/tests/support/mod.rs"]
mod support;

fn main() {
    let mut args = std::env::args_os().skip(1);
    let directory = PathBuf::from(args.next().expect("output directory"));
    let mode = args.next().expect("scalar/gc/string/gc-string");
    let mode = mode.to_str().unwrap();
    assert!(["scalar", "gc", "string", "gc-string"].contains(&mode));
    let collector = mode == "gc" || mode == "gc-string";
    let managed = mode == "string" || mode == "gc-string";
    let mut source = include_str!("support/phase7_integration.emel").to_owned();
    source.push_str(include_str!("support/renode_runtime_cleanup.emel"));
    if managed {
        source = source
            .replace(
                "fn request(n: Int) -> Int fails",
                "fn request(n: Int) -> String fails",
            )
            .replace("state.set(Int.to_string(value))", "state.set(value)");
    }
    let core = support::try_lower(&[("main.emel", &source)], Roots::Exported).unwrap();
    let mut flow = emela_codegen_emb::flow_from_core(&core).unwrap();
    // Source external declarations cannot fail: preserve the verified-Flow adapter.
    let (_, request) = flow
        .functions
        .iter_mut()
        .find(|(_, f)| f.name == "request")
        .unwrap();
    assert!(request.suspends && request.fails);
    request.body = None;
    request.external = Some(External {
        bindings: vec![ExternalBinding {
            target: "c".into(),
            host: HostRef::Global("host_request".into()),
        }],
    });
    if collector {
        gc_anchor::add(&mut flow);
    }
    let mut output = compile_flow(
        &flow,
        &Options::default()
            .with_heap_bytes(4096)
            .with_string_capacity(32)
            .with_fiber_capacity(8)
            .with_frame_capacity(48)
            .with_frame_bytes(288)
            .with_task_capacity(2)
            .with_event_capacity(2)
            .with_deferred_capacity(2)
            .with_task_adapter("host_request", "board_request_start")
            .with_task_adapter("host_wait", "board_wait_start"),
    )
    .unwrap();
    let header = &output
        .files
        .iter()
        .find(|f| f.path == "emela_embedded.h")
        .unwrap()
        .contents;
    let public_name = |name: &str| {
        let line = header
            .lines()
            .find(|line| line.contains(&format!("emela_export_{name}_")))
            .unwrap();
        line.split('(')
            .next()
            .unwrap()
            .split_whitespace()
            .last()
            .unwrap()
            .to_owned()
    };
    let error = header
        .lines()
        .find(|line| {
            line.trim_start().starts_with("emela_type_")
                && line.trim_end().ends_with(';')
                && line.contains(" e")
        })
        .unwrap();
    let mut fields = error.split_whitespace();
    let ty = fields.next().unwrap();
    let tag: usize = fields
        .next()
        .unwrap()
        .trim_start_matches('e')
        .trim_end_matches(';')
        .parse()
        .unwrap();
    let anchor = if collector {
        format!(
            "EmelaAny value = {}((EmelaAny){{0}}); return value.tag == 0 && value.payload.any_box == NULL;",
            public_name("gc_anchor")
        )
    } else {
        "return 1;".to_owned()
    };
    let probe = format!(
        r#"#ifndef EMELA_RUNTIME_PROBE_H
#define EMELA_RUNTIME_PROBE_H
#include <stddef.h>
#include <stdint.h>
#include "emela_embedded.h"
#define EMELA_RUNTIME_MANAGED {managed}
#define EMELA_RUNTIME_COLLECTOR {collector}
#define emela_runtime_work_start {work}
#define emela_runtime_cleanup_start {cleanup}
#define emela_runtime_parts {parts}
#define emela_runtime_float_roundtrip {float_roundtrip}
#define emela_runtime_unicode_signature {unicode_signature}
static inline int emela_runtime_anchor(void) {{ {anchor} }}
static inline void emela_runtime_error(EmelaError *error, char *message, void *parts) {{
  *error = (EmelaError){{.tag = {error_tag}, .payload.e{tag} = ({ty}){{.tag = 0,
    .payload.v0 = {{.f0 = {{.data = message, .length = 3}}, .f1 = parts}}}}}};
}}
void emela_runtime_cancel(EmelaFiber);
void emela_runtime_sample(void);
size_t emela_runtime_heap(void);
unsigned emela_runtime_children(void);
int emela_runtime_pools_empty(void);
#endif
"#,
        managed = u8::from(managed),
        collector = u8::from(collector),
        work = public_name("work"),
        cleanup = public_name("cancel_cleanup"),
        parts = public_name("parts"),
        float_roundtrip = public_name("float_roundtrip"),
        unicode_signature = public_name("unicode_signature"),
        error_tag = tag + 1
    );
    let c = &mut output
        .files
        .iter_mut()
        .find(|f| f.path == "emela_embedded.c")
        .unwrap()
        .contents;
    let allocation = "return (unsigned char *)block + header_size;";
    assert_eq!(c.matches(allocation).count(), 1);
    *c = format!(
        "static void emela_runtime_heap_sample(void);\n{}",
        c.replace(
            allocation,
            "emela_runtime_heap_sample();\nreturn (unsigned char *)block + header_size;"
        )
    );
    c.push_str(include_str!("support/renode_runtime_inspect.c"));
    fs::create_dir_all(&directory).unwrap();
    for file in output.files {
        fs::write(directory.join(file.path), file.contents).unwrap();
    }
    fs::write(directory.join("runtime_probe.h"), probe).unwrap();
}
