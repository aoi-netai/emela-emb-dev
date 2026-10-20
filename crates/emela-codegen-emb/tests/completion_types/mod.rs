use super::*;

fn export_name(output: &Output, stem: &str) -> String {
    let header = &output
        .files
        .iter()
        .find(|f| f.path == "emela_embedded.h")
        .unwrap()
        .contents;
    let prefix = format!("emela_export_{stem}_");
    let line = header.lines().find(|line| line.contains(&prefix)).unwrap();
    line[line.find(&prefix).unwrap()..]
        .split('(')
        .next()
        .unwrap()
        .to_owned()
}

fn add_heap_check(output: &mut Output) {
    output
        .files
        .iter_mut()
        .find(|f| f.path == "emela_embedded.c")
        .unwrap()
        .contents
        .push_str(
            r#"
int test_heap_empty(void) {
  if(!emela_heap_initialized) return 1;
  for(unsigned char *p=emela_heap_storage;p<emela_heap_end();) {
    emela_heap_block *b=(emela_heap_block *)(void *)p;
    if(!b->is_free) return 0;
    p+=b->size;
  }
  return 1;
}
"#,
        );
}

#[test]
fn finite_changing_recursive_enum_arguments_run_with_owned_back_edges() {
    let core = support::try_lower(
        &[(
            "main.emel",
            r#"
pub enum Nest[A] { End(A), Next(Nest[Bool]) }
fn count(value: Nest[Bool]) -> Int {
  value match { End(ok) -> if ok { 1 } else { 0 }
    Next(rest) -> count(rest) + 1 }
}
pub fn work(n: Int) -> Int {
  value: Nest[Int] = if n == 0 { End(42) } else { Next(Next(End(n == 1))) }
  value match { End(number) -> number
    Next(rest) -> count(rest) + 1 }
}
"#,
        )],
        Roots::Exported,
    )
    .unwrap();
    let mut output = compile(&core, &Options::default().with_heap_bytes(4096))
        .expect("finite changing recursive enum types");
    let run = export_name(&output, "work");
    add_heap_check(&mut output);
    compile_and_run(
        output,
        &format!(
            r#"
#include <assert.h>
#include <stdlib.h>
#include "emela_embedded.h"
#include "emela_board.h"
int test_heap_empty(void);
_Noreturn void emela_board_defect(EmelaDefectKind k,const char *s,uint16_t n) {{ (void)k;(void)s;(void)n;abort(); }}
int main(void) {{for(unsigned i=0;i<1000;i++) {{assert({run}(0)==42);assert(test_heap_empty());assert({run}(1)==3);assert(test_heap_empty());}}return 0;}}
"#
        ),
    );
}
