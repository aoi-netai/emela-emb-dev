//! Generate deliberate capacity defects for STM32F446 acceptance.
use emela_codegen_emb::{Options, compile_flow};
use emela_core::{Builtin, External, ExternalBinding, HostRef, TypeId};
use emela_lir::{Callee, Const, FnBuilder, Module, Term, Value};

fn binding(name: &str) -> External {
    External {
        bindings: vec![ExternalBinding {
            target: "c".into(),
            host: HostRef::Global(name.into()),
        }],
    }
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let directory = std::path::Path::new(args.get(1).expect("output directory"));
    let mode = args
        .get(2)
        .expect("string, heap, fiber, frame, task, or event")
        .as_str();
    let mut flow = Module::new(emela_core::Module::new());
    let mut options = Options::default();
    let scheduler = matches!(mode, "fiber" | "frame" | "task" | "event");
    let mut b = FnBuilder::new(&flow.core, "work");
    match mode {
        "string" => {
            let mut seed = FnBuilder::new(&flow.core, "seed");
            seed.set_ret(TypeId::INT);
            let seed = flow.add(seed.finish_external(binding("negative_seed")));
            b = FnBuilder::new(&flow.core, "work");
            b.set_exported().set_ret(TypeId::STRING);
            let n = b.assign(
                "n",
                TypeId::INT,
                Value::Call {
                    callee: Callee::Direct(seed),
                    args: vec![],
                    suspends: false,
                    on_fail: None,
                },
            );
            let s = b.assign(
                "s",
                TypeId::STRING,
                Value::Builtin {
                    op: Builtin::IntToString,
                    args: vec![n.into()],
                },
            );
            b.term(Term::Return(Some(s.into())));
            options = options.with_string_capacity(1);
        }
        "heap" => {
            let list = flow.core.list(TypeId::INT);
            b = FnBuilder::new(&flow.core, "work");
            b.set_exported();
            b.assign(
                "list",
                list,
                Value::List {
                    elems: (0..8).map(|n| Const::Int(n).into()).collect(),
                    tail: None,
                },
            );
            b.term(Term::Return(None));
            options = options.with_heap_bytes(64);
        }
        "fiber" => {
            b.set_exported().set_suspends().set_ret(TypeId::INT);
            b.term(Term::Return(Some(Const::Int(42).into())));
            options = options.with_fiber_capacity(1);
        }
        "frame" => {
            let mut leaf = FnBuilder::new(&flow.core, "leaf");
            leaf.set_suspends().set_ret(TypeId::INT);
            leaf.term(Term::Return(Some(Const::Int(42).into())));
            let leaf = flow.add(leaf.finish());
            b = FnBuilder::new(&flow.core, "work");
            b.set_exported().set_suspends().set_ret(TypeId::INT);
            let n = b.assign(
                "n",
                TypeId::INT,
                Value::Call {
                    callee: Callee::Direct(leaf),
                    args: vec![],
                    suspends: true,
                    on_fail: None,
                },
            );
            b.term(Term::Return(Some(n.into())));
            options = options.with_frame_capacity(1);
        }
        "task" | "event" => {
            let mut wait = FnBuilder::new(&flow.core, "wait");
            wait.set_suspends().set_ret(TypeId::INT);
            let wait = flow.add(wait.finish_external(binding("host_negative_wait")));
            b = FnBuilder::new(&flow.core, "work");
            b.set_exported().set_suspends().set_ret(TypeId::INT);
            let n = b.assign(
                "n",
                TypeId::INT,
                Value::Call {
                    callee: Callee::Direct(wait),
                    args: vec![],
                    suspends: true,
                    on_fail: None,
                },
            );
            b.term(Term::Return(Some(n.into())));
            options = options
                .with_fiber_capacity(2)
                .with_event_capacity(1)
                .with_task_capacity(if mode == "event" { 2 } else { 1 })
                .with_task_adapter("host_negative_wait", "negative_task_start");
        }
        _ => panic!("unknown negative mode"),
    }
    flow.add(b.finish());
    let output = compile_flow(&flow, &options).unwrap();
    let header = &output
        .files
        .iter()
        .find(|f| f.path == "emela_embedded.h")
        .unwrap()
        .contents;
    let start = header.find("emela_export_work_").unwrap();
    let name = header[start..].split('(').next().unwrap();
    let run = match mode {
        "string" => format!("(void){name}(emela_negative_bytes+1,1);"),
        "heap" => format!("{name}();"),
        "fiber" => format!("(void){name}(NULL);(void){name}(NULL);"),
        "frame" => format!("(void){name}(NULL);while(emela_scheduler_run(1)){{}}"),
        "event" => format!(
            "(void){name}(NULL);while(emela_scheduler_run(1)){{}}(void){name}(NULL);while(emela_scheduler_run(1)){{}}emela_negative_first_accepted=(uint32_t)emela_task_complete_int(emela_negative_tasks[0],42);(void)emela_task_complete_int(emela_negative_tasks[1],42);"
        ),
        "task" => format!(
            "(void){name}(NULL);while(emela_scheduler_run(1)){{}}(void){name}(NULL);while(emela_scheduler_run(1)){{}}"
        ),
        _ => unreachable!(),
    };
    let kind = if mode == "string" {
        "STRING_CAPACITY"
    } else {
        "HEAP_EXHAUSTED"
    };
    let message = match mode {
        "string" => "string capacity exceeded",
        "heap" => "fixed heap exhausted",
        "event" => "fixed event queue exhausted",
        _ => "fixed scheduler pool exhausted",
    };
    let alias = format!(
        "#ifndef EMELA_NEGATIVE_PROBE_H\n#define EMELA_NEGATIVE_PROBE_H\n#include <stddef.h>\n#include \"emela_embedded.h\"\n#include \"emela_board.h\"\n#define EMELA_NEGATIVE_SCHEDULER {}\n#define EMELA_NEGATIVE_TASK {}\n#define EMELA_NEGATIVE_STRING {}\n#define EMELA_NEGATIVE_EVENT {}\n#define EMELA_NEGATIVE_KIND EMELA_DEFECT_{kind}\n#define EMELA_NEGATIVE_MESSAGE \"{message}\"\nextern char emela_negative_bytes[3];\n#if EMELA_NEGATIVE_TASK\nextern EmelaTask emela_negative_tasks[2];\nextern volatile uint32_t emela_negative_first_accepted;\n#endif\nstatic inline void emela_negative_run(void){{{run}}}\n#endif\n",
        u8::from(scheduler),
        u8::from(mode == "task" || mode == "event"),
        u8::from(mode == "string"),
        u8::from(mode == "event")
    );
    std::fs::create_dir_all(directory).unwrap();
    for file in output.files {
        std::fs::write(directory.join(file.path), file.contents).unwrap();
    }
    std::fs::write(directory.join("negative_probe.h"), alias).unwrap();
}
