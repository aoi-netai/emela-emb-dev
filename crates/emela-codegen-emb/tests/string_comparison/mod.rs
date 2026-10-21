use super::*;

fn c_source(output: &Output) -> &str {
    &output
        .files
        .iter()
        .find(|f| f.path == "emela_embedded.c")
        .unwrap()
        .contents
}

const PRELUDE: &str = r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *m,uint16_t n) { fprintf(stderr,"%d: %.*s\n",k,n,m); abort(); }
"#;

#[test]
fn all_constant_comparisons_match_runtime_order_without_emitting_the_helper() {
    let core = emela_core::Module::new();
    let mut flow = Module::new(core);
    let pairs = [
        ("", ""),
        ("a\0b", "a\0c"),
        ("ab", "a"),
        ("\u{e000}", "\u{10000}"),
        ("é", "e\u{301}"),
    ];
    let ops = [
        PrimOp::Eq,
        PrimOp::Ne,
        PrimOp::Lt,
        PrimOp::Le,
        PrimOp::Gt,
        PrimOp::Ge,
    ];
    let mut checks = String::new();
    for (a, b) in pairs {
        for op in ops {
            let index = flow.functions.len();
            let mut builder = FnBuilder::new(&flow.core, &format!("compare_{index}"));
            builder.set_ret(TypeId::BOOL).set_exported().set_constant();
            let value = builder.assign(
                "result",
                TypeId::BOOL,
                Value::Prim {
                    op,
                    ty: PrimTy::String,
                    args: vec![
                        Const::String(a.into()).into(),
                        Const::String(b.into()).into(),
                    ],
                },
            );
            builder.term(Term::Return(Some(value.into())));
            flow.add(builder.finish());
            let expected = match op {
                PrimOp::Eq => a == b,
                PrimOp::Ne => a != b,
                PrimOp::Lt => a < b,
                PrimOp::Le => a <= b,
                PrimOp::Gt => a > b,
                PrimOp::Ge => a >= b,
                _ => unreachable!(),
            };
            checks.push_str(&format!(
                "assert(emela_export_compare_{index}_{index}()=={});\n",
                u8::from(expected)
            ));
        }
    }
    let output = compile_flow(&flow, &Options::default()).unwrap();
    assert!(!c_source(&output).contains("emela_emb_string_compare"));
    compile_and_run(
        output,
        &format!("{PRELUDE}\nint main(void) {{ {checks} return 0; }}\n"),
    );
}
