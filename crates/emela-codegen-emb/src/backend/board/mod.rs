//! Board policy and adapters over the target-independent compiled program.
use super::compiler::*;

mod interrupt_validation;
mod peripheral;

pub(super) fn link(compiled: &CompiledModule, options: &Options) -> Result<Output, Vec<Error>> {
    if options.peripheral_buffer_capacity() == 0 {
        return Err(vec![Error {
            kind: ErrorKind::InvalidOptions,
            message: "peripheral_buffer_capacity must be positive (uint16_t words)".into(),
        }]);
    }
    super::compiler::emit(compiled, &Board, options)
}

struct Board;

impl Target for Board {
    fn external_binding(
        &self,
        flow: &emela_lir::Module,
        id: FnId,
        function: &Function,
    ) -> Result<Option<external::Binding>, Vec<Error>> {
        Ok(
            peripheral::resolve(flow, function)?.map(|_| external::Binding {
                symbol: function_name(id, function),
                result: crate::ExternalResult::Owned,
                adapted: true,
                target_owned: true,
            }),
        )
    }

    fn task_symbol<'a>(&self, function: &'a Function) -> Option<&'a str> {
        peripheral::task_symbol(function)
    }

    fn headers(
        &self,
        flow: &emela_lir::Module,
        emitted: &HashSet<FnId>,
        _options: &Options,
    ) -> Result<Vec<GeneratedFile>, Vec<Error>> {
        peripheral::headers(flow, emitted)
    }

    fn validate(&self, context: &LinkContext<'_, '_>) -> Result<(), Vec<Error>> {
        interrupt_validation::validate_isr_entries(
            context.flow,
            context.emitted,
            context.heap_functions,
            context.layouts,
            context.names,
            context.options,
        )
    }

    fn emit_adapter(
        &self,
        context: &LinkContext<'_, '_>,
        function: &Function,
        name: &str,
        writer: &mut Writer,
    ) -> Result<bool, Vec<Error>> {
        peripheral::emit_adapter(
            context.flow,
            context.layouts,
            function,
            name,
            context.options.peripheral_buffer_capacity(),
            writer,
        )
    }

    fn entry_declaration(&self) -> &'static str {
        "void emela_embedded_start(void);"
    }

    fn emit_entry(
        &self,
        context: &LinkContext<'_, '_>,
        function: &Function,
        name: &str,
        string_output: bool,
        writer: &mut Writer,
    ) {
        writer.begin_block("void emela_embedded_start(void)");
        writer.line("emela_board_init();");
        if function.suspends {
            let args = if context.layouts.type_repr(function.ret) == Repr::Void {
                String::new()
            } else {
                writer.decl(&context.layouts.c_type(function.ret), "result", Some("{0}"));
                "&result".to_owned()
            };
            writer.line(&format!("EmelaFiber main_fiber = {name}_start({args});"));
            writer.begin_block("while (!emela_fiber_done(main_fiber))");
            writer.line("(void)emela_scheduler_run(UINT32_C(64));");
            writer.line("emela_board_idle();");
            writer.end_block();
            writer.line("emela_fiber_release(main_fiber);");
        } else {
            let args = if string_output {
                let capacity = context.options.string_capacity();
                writer.line(&format!("char output[{capacity}];"));
                format!("output, UINT16_C({capacity})")
            } else {
                String::new()
            };
            writer.line(&format!("(void){name}({args});"));
        }
        writer.begin_block("for (;;)");
        writer.line("emela_board_idle();");
        writer.end_block();
        writer.end_block();
        writer.blank();
    }
}
