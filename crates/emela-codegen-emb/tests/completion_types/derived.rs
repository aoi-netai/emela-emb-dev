use super::*;

#[test]
fn growing_derivation_preserves_unit_named_fields_numeric_bits_and_escaped_strings() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r##"
pub enum Grow[A] { End(value: A), Next(rest: Grow[List[A]]) }
pub fn show(big: Int64, number: Float, text: String) -> String {
  value: Grow[(Int64, Float, String, ())] = End(value: (big, number, text, ()))
  "#{value}"
}
pub fn same(number: Float, text: String) -> Bool {
  value: Grow[(Float, String, ())] = Next(rest: End(value: [(number, text, ())]))
  value == value
}
pub fn units() -> String {
  value: Grow[List[()]] = End(value: [(), ()])
  "#{value}"
}
"##,
        )],
        Roots::Exported,
    )
    .unwrap();
    let mut output = compile(
        &core,
        &Options::default()
            .with_heap_bytes(2048)
            .with_string_capacity(128),
    )
    .unwrap();
    let show = export_name(&output, "show");
    let same = export_name(&output, "same");
    let units = export_name(&output, "units");
    add_heap_check(&mut output);
    compile_and_run(
        output,
        &format!(
            r#"
#include <assert.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *s,uint16_t n) {{(void)k;(void)s;(void)n;abort();}}
int main(void) {{static const char input[]={{'a',0,'"','\\','\n','#','{{'}};EmelaString text={{input,sizeof input,NULL}};char buffer[128];double nan;uint64_t bits=UINT64_C(0x7ff8000000000042);memcpy(&nan,&bits,sizeof nan);
for(int i=0;i<1000;i++) {{EmelaString result={show}(INT64_MIN,-0.0,text,buffer,sizeof buffer);static const char expected[]="End(value: (-9223372036854775808, 0.0, \"a\\u{{0}}\\\"\\\\\\n\\#{{\", ()))";if(result.length!=sizeof expected-1 || memcmp(result.data,expected,sizeof expected-1)!=0) fprintf(stderr,"got %.*s (%u) expected %s\n",result.length,result.data,result.length,expected);assert(result.length==sizeof expected-1 && memcmp(result.data,expected,sizeof expected-1)==0);assert(!{same}(nan,text));assert({same}(-0.0,text));EmelaString unit={units}(buffer,sizeof buffer);assert(unit.length==20 && memcmp(unit.data,"End(value: [(), ()])",20)==0);assert(test_heap_empty());}}}}
"#
        ),
    );
}

#[test]
fn growing_and_nested_generic_error_payloads_have_finite_display_functions() {
    for (definition, value, expected) in [
        (
            "enum Grow[A] { End(A), Next(Grow[List[A]]) }",
            "Next(End([42]))",
            "error: Boom(value: Next(End([42])))",
        ),
        (
            "enum Wrap[A] { Box(List[A]) }",
            "Box([42])",
            "error: Boom(value: Box([42]))",
        ),
    ] {
        let source = format!(
            r#"
import Std.Fiber.{{Fiber, Success, Failure}}
import Std.Cause.{{Cause}}
{definition}
error Boom(value: {ty})
fn bad() -> Int fails Boom {{fail Boom(value: {value})}}
pub fn show() -> String {{
  Fiber.attempt(bad) match {{Success(_) -> "unexpected"
    Failure(cause) -> Cause.to_string(cause)}}
}}
"#,
            ty = if definition.contains("Grow") {
                "Grow[Int]"
            } else {
                "Wrap[Int]"
            }
        );
        let core = support::try_lower(&[("main.emel", &source)], Roots::Exported).unwrap();
        let flow = emela_codegen_emb::flow_from_core(&core).unwrap();
        assert!(flow.functions.len() < 64);
        assert_eq!(flow.error_shows.len(), 1);
        let mut output = compile_flow(
            &flow,
            &Options::default()
                .with_heap_bytes(2048)
                .with_string_capacity(128),
        )
        .unwrap();
        let show = export_name(&output, "show");
        add_heap_check(&mut output);
        compile_and_run(
            output,
            &format!(
                r#"
#include <assert.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *s,uint16_t n) {{(void)k;(void)s;(void)n;abort();}}
int main(void) {{for(int i=0;i<1000;i++) {{char buffer[128];EmelaString result={show}(buffer,sizeof buffer);assert(result.length=={length} && memcmp(result.data,"{expected}",{length})==0);assert(test_heap_empty());}}}}
"#,
                length = expected.len()
            ),
        );
    }
}

#[test]
fn growing_show_freezes_its_single_buffer_across_suspend_and_releases_null_outputs() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r##"
@external(c,"host_wait")
suspend fn wait(n: Int) -> Int
effect Wait { suspend fn get(n: Int) -> Int }
handler Waiting implements Wait { suspend fn get(n, resume) { resume(wait(n)) } }
enum Grow[A] { End(A), Next(Grow[List[A]]) }
pub fn delayed(n: Int) -> String use Async {
  with Waiting {
    value: Grow[Int] = Next(End([n]))
    text = "#{value}"
    (use Wait).get(n)
    text
  }
}
"##,
        )],
        Roots::Exported,
    )
    .unwrap();
    let mut output = compile(
        &core,
        &Options::default()
            .with_heap_bytes(2048)
            .with_frame_bytes(768)
            .with_string_capacity(64)
            .with_task_adapter("host_wait", "board_wait_start"),
    )
    .unwrap();
    let delayed = export_name(&output, "delayed");
    add_heap_check(&mut output);
    output
        .files
        .iter_mut()
        .find(|f| f.path == "emela_embedded.c")
        .unwrap()
        .contents
        .push_str(
            "\nvoid test_string_drop(EmelaString value) {emela_heap_release(value.owner);}\n",
        );
    compile_and_run(
        output,
        &format!(
            r#"
#include <assert.h>
#include <stdlib.h>
#include <string.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);void test_string_drop(EmelaString);
static EmelaTask task;
uint32_t emela_board_event_lock(void) {{return 0;}}void emela_board_event_unlock(uint32_t p) {{(void)p;}}
void board_wait_start(EmelaTask t,int32_t n) {{(void)n;task=t;}}
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *s,uint16_t n) {{(void)k;(void)s;(void)n;abort();}}
int main(void) {{for(int i=0;i<1000;i++) {{EmelaString text={{0}};EmelaFiber fiber={delayed}(42,&text);while(emela_scheduler_run(1)){{}}assert(emela_task_complete_int(task,42));while(!emela_fiber_done(fiber)) assert(emela_scheduler_run(1));emela_fiber_release(fiber);assert(text.length==15 && memcmp(text.data,"Next(End([42]))",15)==0);test_string_drop(text);assert(test_heap_empty());fiber={delayed}(42,NULL);while(emela_scheduler_run(1)){{}}assert(emela_task_complete_int(task,42));while(!emela_fiber_done(fiber)) assert(emela_scheduler_run(1));emela_fiber_release(fiber);assert(test_heap_empty());}}}}
"#
        ),
    );
}
