//! Host C connection without peripheral-specific lowering or symbol recognition.
use super::compiler::{CompiledModule, Function, LinkContext, Repr, Target, Writer};
use crate::{Error, ErrorKind, GeneratedFile, Options, Output};

struct Native;
impl Target for Native {
    fn validate(&self, context: &LinkContext<'_, '_>) -> Result<(), Vec<Error>> {
        if !context.options.isr_entries.is_empty() {
            return Err(vec![Error {
                kind: ErrorKind::InvalidOptions,
                message: "the native sample does not support hardware ISR entries".into(),
            }]);
        }
        Ok(())
    }

    fn entry_declaration(&self) -> &'static str {
        "void emela_native_start(void);"
    }

    fn emit_entry(
        &self,
        context: &LinkContext<'_, '_>,
        function: &Function,
        name: &str,
        string_output: bool,
        writer: &mut Writer,
    ) {
        writer.begin_block("void emela_native_start(void)");
        if function.suspends {
            let args = if context.layouts.type_repr(function.ret) == Repr::Void {
                String::new()
            } else {
                writer.decl(&context.layouts.c_type(function.ret), "result", Some("{0}"));
                "&result".into()
            };
            writer.line(&format!("EmelaFiber main_fiber = {name}_start({args});"));
            writer.begin_block("while (!emela_fiber_done(main_fiber))");
            writer.line("(void)emela_scheduler_run(UINT32_C(64));");
            writer.end_block();
            writer.line("emela_fiber_release(main_fiber);");
            if context.layouts.type_repr(function.ret) != Repr::Void {
                context
                    .layouts
                    .emit_release_value_expression(function.ret, "result", writer);
            }
        } else if context.layouts.is_managed_value(function.ret) || string_output {
            let args = if string_output {
                let capacity = context.options.string_capacity();
                writer.line(&format!("char output[{capacity}];"));
                format!("output, UINT16_C({capacity})")
            } else {
                String::new()
            };
            writer.decl(
                &context.layouts.c_type(function.ret),
                "result",
                Some(&format!("{name}({args})")),
            );
            context
                .layouts
                .emit_release_value_expression(function.ret, "result", writer);
        } else {
            writer.line(&format!("(void){name}();"));
        }
        writer.end_block();
        writer.blank();
    }
}

pub(super) fn link(compiled: &CompiledModule, options: &Options) -> Result<Output, Vec<Error>> {
    let mut output = super::compiler::emit(compiled, &Native, options)?;
    output.files.push(GeneratedFile {
        path: "emela_native_platform.c".into(),
        contents: include_str!("platform.c").into(),
    });
    Ok(output)
}
