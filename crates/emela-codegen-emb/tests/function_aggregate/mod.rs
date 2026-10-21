use super::*;

#[test]
fn native_aggregate_callbacks_own_results_after_captures_and_arguments_are_released() {
    let core = support::try_lower(
        &[(
            "main.emel",
            include_str!("../../examples/support/function_aggregate.emel"),
        )],
        Roots::Exported,
    )
    .unwrap();
    let flow = emela_codegen_emb::flow_from_core(&core).unwrap();
    let (maker, maker_fn) = flow
        .functions
        .iter()
        .find(|(_, f)| f.name == "maker")
        .unwrap();
    let Type::Fn(ft) = &flow.core.types[maker_fn.ret] else {
        panic!()
    };
    let pair = ft.params[0].index();
    let packet = ft.ret.index();
    let callback = maker_fn.ret.index();
    let maker = u32::from(maker.into_raw());
    let native = u32::from(
        flow.functions
            .iter()
            .find(|(_, f)| f.name == "native")
            .unwrap()
            .0
            .into_raw(),
    );
    let mut output = compile(
        &core,
        &Options::default()
            .with_heap_bytes(2048)
            .with_string_capacity(64),
    )
    .unwrap();
    add_heap_check(&mut output);
    let release = release_helper(&output, packet);
    compile_and_run(
        output,
        &format!(
            r#"
#include <assert.h>
#include <stdlib.h>
#include <stdio.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) {{ fprintf(stderr, "%d: %.*s\n", k,n,m); abort(); }}
int main(void) {{
  for (int i = 0; i < 1000; ++i) {{
    char text[] = {{ 'a', 0, 'b' }}, note[] = "note";
    emela_type_{pair} input = {{ .f0 = {{ .data = text, .length = 3 }}, .f1 = {{ .tag = 0, .payload.v0.f0 = {{.data = note, .length = 4}} }} }};
    emela_function_type_{callback} f = emela_export_maker_{maker}(42);
    emela_type_{packet} result = f.call(f.env, input);
    emela_embedded_release_function_{callback}(&f);
    memset(text, 'X', 3); memset(note, 'X', 4);
    assert(result.tag == 0 && result.payload.v0.f0.f0.owner);
    assert(result.payload.v0.f0.f0.length == 5 && memcmp(result.payload.v0.f0.f0.data, "a\0b42", 5) == 0);
    assert(result.payload.v0.f0.f1.tag == 0 && result.payload.v0.f0.f1.payload.v0.f0.owner);
    assert(memcmp(result.payload.v0.f0.f1.payload.v0.f0.data, "note", 4) == 0);
    {release}(&result);
    result = emela_export_native_{native}(42);
    assert(result.tag == 0 && result.payload.v0.f0.f0.length == 4 && memcmp(result.payload.v0.f0.f0.data, "4342", 4) == 0);
    {release}(&result);
    assert(test_heap_empty());
  }}
}}
"#
        ),
    );
}

#[test]
fn heapless_callbacks_accept_and_return_aggregates_containing_function_values() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
enum Bundle { Call(fn(Int) -> Int, (Int, Bool)) }
fn apply(f: fn(Bundle) -> (Int, Bool), x: Bundle) -> (Int, Bool) { f(x) }
fn unpack(x: Bundle) -> (Int, Bool) {
  x match { Call(f, pair) -> { (n, flag) = pair
    (f(n), flag) } }
}

fn plus(n: Int) -> Int { n + 1 }
pub fn work(n: Int) -> (Int, Bool) { apply(unpack, Call(plus, (n, True))) }
"#,
        )],
        Roots::Exported,
    )
    .unwrap();
    let flow = emela_codegen_emb::flow_from_core(&core).unwrap();
    let (id, f) = flow
        .functions
        .iter()
        .find(|(_, f)| f.name == "work")
        .unwrap();
    let ty = f.ret.index();
    let id = u32::from(id.into_raw());
    let output = compile(&core, &Options::default()).unwrap();
    assert!(
        !output
            .files
            .iter()
            .find(|f| f.path == "emela_embedded.c")
            .unwrap()
            .contents
            .contains("emela_heap_storage")
    );
    compile_and_run(
        output,
        &format!(
            r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) {{ (void)k; (void)m; (void)n; abort(); }}
int main(void) {{ emela_type_{ty} value = emela_export_work_{id}(41); assert(value.f0 == 42 && value.f1); }}
"#
        ),
    );
}

#[test]
fn function_signatures_can_refer_back_to_the_aggregate_containing_them() {
    let mut core = emela_core::Module::new();
    let node_id = core.add_enum(EnumDef {
        name: "Node".into(),
        params: 0,
        variants: vec![],
        error: false,
        external: false,
    });
    let node = core.enum_type(node_id, vec![]);
    let callback = core.types.intern(Type::Fn(FnType::pure(vec![node], node)));
    core.enums[node_id].variants = vec![
        VariantDef::positional("Step", vec![callback]),
        VariantDef::unit("End"),
    ];
    let mut flow = Module::new(core);
    let mut b = FnBuilder::new(&flow.core, "identity");
    b.set_ret(node);
    let value = b.param("value", node);
    b.term(Term::Return(Some(value.into())));
    let identity = flow.add(b.finish());
    let mut b = FnBuilder::new(&flow.core, "work");
    b.set_exported().set_ret(node);
    let f = b.assign("callback", callback, Value::FnRef(identity));
    let value = b.assign(
        "value",
        node,
        Value::Make {
            ctor: CtorRef {
                enum_id: node_id,
                variant: 0,
            },
            args: vec![f.into()],
        },
    );
    let out = b.assign(
        "out",
        node,
        Value::Call {
            callee: Callee::Indirect(f),
            args: vec![value.into()],
            suspends: false,
            on_fail: None,
        },
    );
    b.term(Term::Return(Some(out.into())));
    flow.add(b.finish());
    let output = compile_flow(&flow, &Options::default()).unwrap();
    let ty = node.index();
    compile_and_run(
        output,
        &format!(
            r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) {{ (void)k; (void)m; (void)n; abort(); }}
int main(void) {{
  emela_type_{ty} value = emela_export_work_1(); assert(value.tag == 0);
  emela_type_{ty} end = {{.tag = 1}};
  value = value.payload.v0.f0.call(value.payload.v0.f0.env, end); assert(value.tag == 1);
}}
"#
        ),
    );
}

#[test]
fn signature_cycles_through_nested_tuples_do_not_require_recursive_heap_boxes() {
    let mut core = emela_core::Module::new();
    let id = core.add_enum(EnumDef {
        name: "Node".into(),
        params: 0,
        variants: vec![],
        error: false,
        external: false,
    });
    let node = core.enum_type(id, vec![]);
    let pair = core.tuple(vec![node, TypeId::BOOL]);
    let callback = core.types.intern(Type::Fn(FnType::pure(vec![node], pair)));
    core.enums[id].variants = vec![
        VariantDef::positional("Step", vec![callback]),
        VariantDef::unit("End"),
    ];
    let mut flow = Module::new(core);
    let mut b = FnBuilder::new(&flow.core, "wrap");
    b.set_ret(pair);
    let value = b.param("value", node);
    let out = b.assign(
        "out",
        pair,
        Value::Tuple(vec![value.into(), Const::Bool(true).into()]),
    );
    b.term(Term::Return(Some(out.into())));
    let wrap = flow.add(b.finish());
    let mut b = FnBuilder::new(&flow.core, "work");
    b.set_exported().set_ret(node);
    let f = b.assign("callback", callback, Value::FnRef(wrap));
    let value = b.assign(
        "value",
        node,
        Value::Make {
            ctor: CtorRef {
                enum_id: id,
                variant: 0,
            },
            args: vec![f.into()],
        },
    );
    b.term(Term::Return(Some(value.into())));
    flow.add(b.finish());
    let output = compile_flow(&flow, &Options::default()).unwrap();
    assert!(
        !output
            .files
            .iter()
            .find(|f| f.path == "emela_embedded.c")
            .unwrap()
            .contents
            .contains("emela_heap_storage")
    );
    let ty = node.index();
    let pair = pair.index();
    compile_and_run(
        output,
        &format!(
            r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) {{ (void)k; (void)m; (void)n; abort(); }}
int main(void) {{
  emela_type_{ty} value = emela_export_work_1(); assert(value.tag == 0);
  emela_type_{pair} pair = value.payload.v0.f0.call(value.payload.v0.f0.env, value);
  assert(pair.f0.tag == 0 && pair.f1);
}}
"#
        ),
    );
}

#[test]
fn aggregate_callbacks_keep_any_fields_complete_and_owned() {
    let mut core = emela_core::Module::new();
    let pair = core.tuple(vec![TypeId::ANY, TypeId::BOOL]);
    let callback = core.types.intern(Type::Fn(FnType::pure(vec![pair], pair)));
    let mut flow = Module::new(core);
    let mut b = FnBuilder::new(&flow.core, "identity");
    b.set_ret(pair);
    let value = b.param("value", pair);
    b.term(Term::Return(Some(value.into())));
    let identity = flow.add(b.finish());
    let mut b = FnBuilder::new(&flow.core, "work");
    b.set_exported().set_ret(TypeId::INT);
    let number = b.assign("number", TypeId::INT, Value::Const(Const::Int(42)));
    let boxed = b.assign("boxed", TypeId::ANY, Value::Coerce(number));
    let value = b.assign(
        "value",
        pair,
        Value::Tuple(vec![boxed.into(), Const::Bool(true).into()]),
    );
    let f = b.assign("callback", callback, Value::FnRef(identity));
    let result = b.assign(
        "result",
        pair,
        Value::Call {
            callee: Callee::Indirect(f),
            args: vec![value.into()],
            suspends: false,
            on_fail: None,
        },
    );
    let boxed = b.assign(
        "field",
        TypeId::ANY,
        Value::TupleGet {
            base: result,
            index: 0,
        },
    );
    let number = b.assign("number", TypeId::INT, Value::Coerce(boxed));
    b.term(Term::Return(Some(number.into())));
    flow.add(b.finish());
    let mut output = compile_flow(&flow, &Options::default().with_heap_bytes(1024)).unwrap();
    add_heap_check(&mut output);
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { (void)k; (void)m; (void)n; abort(); }
int main(void) { for (int i = 0; i < 1000; ++i) { assert(emela_export_work_1() == 42); assert(test_heap_empty()); } }
"#,
    );
}

#[test]
fn growing_generic_recursion_through_a_function_signature_is_diagnosed() {
    let mut core = emela_core::Module::new();
    let id = core.add_enum(EnumDef {
        name: "Grow".into(),
        params: 1,
        variants: vec![],
        error: false,
        external: false,
    });
    let param = core.types.intern(Type::Param(0));
    let list = core.types.intern(Type::List(param));
    let larger = core.enum_type(id, vec![list]);
    let callback = core.types.intern(Type::Fn(FnType::pure(vec![], larger)));
    core.enums[id].variants = vec![VariantDef::positional("Step", vec![callback])];
    let node = core.enum_type(id, vec![TypeId::INT]);
    let mut b = FnBuilder::new(&core, "work");
    b.set_exported().set_ret(node);
    let value = b.param("value", node);
    b.term(Term::Return(Some(value.into())));
    let f = b.finish();
    let mut flow = Module::new(core);
    flow.add(f);
    let errors = compile_flow(&flow, &Options::default()).unwrap_err();
    assert!(errors.iter().any(|e| {
        e.kind == ErrorKind::Unsupported
            && e.message
                .contains("recursive enum type that changes its type arguments")
    }));
}

pub(super) fn add_heap_check(output: &mut Output) {
    output
        .files
        .iter_mut()
        .find(|f| f.path == "emela_embedded.c")
        .unwrap()
        .contents
        .push_str(
            r#"
int test_heap_empty(void) {
  if (!emela_heap_initialized) return 1;
  for (unsigned char *at = emela_heap_storage; at < emela_heap_end();) {
    emela_heap_block *block = (emela_heap_block *)(void *)at;
    if (!block->is_free || block->size == 0) return 0;
    at += block->size;
  }
  return 1;
}

"#,
        );
}
pub(super) fn release_helper(output: &Output, ty: usize) -> String {
    let header = &output
        .files
        .iter()
        .find(|f| f.path == "emela_embedded.h")
        .unwrap()
        .contents;
    header
        .lines()
        .find(|line| {
            line.starts_with("void emela_embedded_release_value_")
                && line.contains(&format!("(emela_type_{ty} *value)"))
        })
        .unwrap()
        .strip_prefix("void ")
        .unwrap()
        .split('(')
        .next()
        .unwrap()
        .to_owned()
}
