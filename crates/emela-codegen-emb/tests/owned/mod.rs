use super::*;

fn append_source(output: &mut Output, text: &str) {
    output
        .files
        .iter_mut()
        .find(|f| f.path == "emela_embedded.c")
        .unwrap()
        .contents
        .push_str(text);
}

fn release_helper(output: &Output, ty: TypeId) -> String {
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
                && line.contains(&format!("(emela_type_{} *value)", ty.index()))
        })
        .unwrap()
        .strip_prefix("void ")
        .unwrap()
        .split('(')
        .next()
        .unwrap()
        .to_owned()
}

fn add_heap_probe(output: &mut Output) {
    append_source(
        output,
        r#"
size_t test_heap_used(void) {
  size_t used = 0;
  if (!emela_heap_initialized) return 0;
  for (unsigned char *at = emela_heap_storage; at < emela_heap_end();) {
    emela_heap_block *block = (emela_heap_block *)(void *)at;
    if (!block->is_free) used += block->size;
    at += block->size;
  }
  return used;
}
size_t test_owner_capacity(void *owner) {
  return emela_heap_header(owner)->size - emela_heap_header_size();
}
"#,
    );
}

#[test]
fn returned_list_owns_formatted_strings_after_the_producer_stack_returns() {
    let core = support::try_lower(
        &[(
            "main.emel",
            include_str!("../../examples/support/owned_payload.emel"),
        )],
        Roots::Exported,
    )
    .unwrap();
    let mut output = compile(&core, &Options::default().with_heap_bytes(1024)).unwrap();
    add_heap_probe(&mut output);
    compile_and_run(
        output,
        r#"
#include <assert.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
size_t test_heap_used(void);
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { (void)k; (void)m; (void)n; abort(); }
int main(void) {
  for (int i = 0; i < 1000; ++i) {
    void *list = emela_export_store_0(INT32_MIN);
    EmelaString value = emela_export_read_1(list);
    emela_embedded_release(list);
    assert(value.owner && value.length == 11 && memcmp(value.data, "-2147483648", 11) == 0);
    emela_embedded_release_string(&value);
    assert(test_heap_used() == 0);
  }
}
"#,
    );
}

#[test]
fn recursive_heap_payload_promotion_and_destruction_do_not_follow_value_depth_on_stack() {
    let mut core = emela_core::Module::new();
    let chain = core.add_enum(EnumDef {
        name: "TextChain".into(),
        params: 0,
        variants: vec![],
        error: false,
        external: false,
    });
    let chain_ty = core.enum_type(chain, vec![]);
    core.enums[chain].variants = vec![
        VariantDef::unit("End"),
        VariantDef::positional("Link", vec![TypeId::STRING, chain_ty]),
    ];
    let list_ty = core.list(chain_ty);
    let mut flow = Module::new(core);
    let mut b = FnBuilder::new(&flow.core, "link");
    b.set_ret(chain_ty).set_exported();
    let text = b.param("text", TypeId::STRING);
    let tail = b.param("tail", chain_ty);
    let value = b.assign(
        "value",
        chain_ty,
        Value::Make {
            ctor: CtorRef {
                enum_id: chain,
                variant: 1,
            },
            args: vec![text.into(), tail.into()],
        },
    );
    b.term(Term::Return(Some(value.into())));
    flow.add(b.finish());
    let mut b = FnBuilder::new(&flow.core, "hold");
    b.set_ret(list_ty).set_exported();
    let value = b.param("value", chain_ty);
    let list = b.assign(
        "list",
        list_ty,
        Value::List {
            elems: vec![value.into()],
            tail: None,
        },
    );
    b.term(Term::Return(Some(list.into())));
    flow.add(b.finish());
    let mut output = compile_flow(&flow, &Options::default().with_heap_bytes(700_000)).unwrap();
    let release = release_helper(&output, chain_ty);
    add_heap_probe(&mut output);
    append_source(
        &mut output,
        &format!(
            r#"
#include <assert.h>
#include <string.h>
EmelaString test_hold_text(void *list) {{
  return emela_heap_own_string(((emela_list_node_0 *)list)->head.payload.v1.f0);
}}
void test_chain(void *list, int count) {{
  const emela_type_{ty} *value = &((emela_list_node_0 *)list)->head;
  for (int i = 0; i < count; ++i) {{
    assert(value->tag == 1);
    assert(value->payload.v1.f0.owner && value->payload.v1.f0.length == 3);
    assert(memcmp(value->payload.v1.f0.data, "abc", 3) == 0);
    value = (const emela_type_{ty} *)value->payload.v1.f1;
  }}
  assert(value->tag == 0);
}}
"#,
            ty = chain_ty.index()
        ),
    );
    compile_and_run(
        output,
        &format!(
            r#"
#include <assert.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include "emela_embedded.h"
#include "emela_board.h"
size_t test_heap_used(void);
EmelaString test_hold_text(void *list);
void test_chain(void *list, int count);
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) {{ (void)k; (void)m; (void)n; abort(); }}
int main(void) {{
  struct rlimit limit = {{65536, 65536}};
  assert(setrlimit(RLIMIT_STACK, &limit) == 0);
  for (int run = 0; run < 10; ++run) {{
    char bytes[] = "abc";
    emela_type_{ty} end = {{.tag = 0}};
    emela_type_{ty} first = emela_export_link_0((EmelaString){{.data = bytes, .length = 3}}, end);
    emela_type_{ty} second = emela_export_link_0((EmelaString){{.data = bytes, .length = 3}}, first);
    void *borrowed = emela_export_hold_1(second);
    {release}(&first);
    {release}(&second);
    memset(bytes, 'x', 3);
    test_chain(borrowed, 2);
    emela_embedded_release(borrowed);
    assert(test_heap_used() == 0);
    memcpy(bytes, "abc", 3);
    first = emela_export_link_0((EmelaString){{.data = bytes, .length = 3}}, end);
    void *seed = emela_export_hold_1(first);
    EmelaString stable = test_hold_text(seed);
    {release}(&first);
    emela_embedded_release(seed);
    emela_type_{ty} value = end;
    for (int i = 0; i < 6000; ++i) {{
      emela_type_{ty} next = emela_export_link_0(stable, value);
      {release}(&value);
      value = next;
    }}
    void *list = emela_export_hold_1(value);
    {release}(&value);
    emela_embedded_release_string(&stable);
    memset(bytes, 'x', 3);
    test_chain(list, 6000);
    emela_embedded_release(list);
    assert(test_heap_used() == 0);
  }}
}}
"#,
            ty = chain_ty.index()
        ),
    );
}
