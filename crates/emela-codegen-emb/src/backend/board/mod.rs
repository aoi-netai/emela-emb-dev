//! Board connection is a consumer of a compiled artifact, never a compiler prerequisite.
use super::compiler::*;
use interrupt_validation::{validate_isr_entries, validate_isr_function};

mod gpio_irq;
mod i2c_irq;
mod interrupt_validation;
mod peripheral;
mod peripheral_status;
mod spi_bridge;
mod uart_irq;

/// Each link works on its own copy: a native link can reuse the same artifact afterwards.
pub(super) fn link(compiled: &CompiledModule, options: &Options) -> Result<Output, Vec<Error>> {
    if options.peripheral_buffer_capacity() == 0 {
        return Err(vec![Error {
            kind: ErrorKind::InvalidOptions,
            message: "peripheral_buffer_capacity must be positive (uint16_t words)".into(),
        }]);
    }
    // Inspect the retained source, so a lifted lambda cannot masquerade as a named callback.
    let gpio = gpio_irq::collect(compiled.source())?;
    let uart = uart_irq::collect(compiled.source())?;
    let i2c = i2c_irq::collect(compiled.source())?;
    let mut preparation = compiled.prepare_link();
    let gpio = gpio_irq::prepare(&mut preparation, gpio)?;
    let uart = uart_irq::prepare(&mut preparation, uart)?;
    let i2c = i2c_irq::prepare(&mut preparation, i2c)?;
    gpio_irq::erase_registration_callbacks(&mut preparation)?;
    let roots = gpio
        .roots()
        .chain(uart.roots())
        .chain(i2c.roots())
        .collect();
    let target = Board { gpio, uart, i2c };
    super::compiler::emit(preparation.finish(roots)?, &target, options)
}

struct Board {
    gpio: gpio_irq::Plan,
    uart: uart_irq::Plan,
    i2c: i2c_irq::Plan,
}

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
        options: &Options,
    ) -> Result<Vec<GeneratedFile>, Vec<Error>> {
        peripheral::headers(flow, emitted, options.peripheral_buffer_capacity())
    }

    fn validate(&self, context: &LinkContext<'_, '_>) -> Result<(), Vec<Error>> {
        let LinkContext {
            flow,
            layouts,
            names,
            emitted,
            heap_functions,
            options,
        } = context;
        validate_isr_entries(flow, emitted, heap_functions, layouts, names, options)?;
        let mut owners = HashMap::new();
        self.gpio.claim_channels(flow, &mut owners)?;
        self.uart.claim_channels(flow, &mut owners)?;
        self.i2c.claim_channels(flow, &mut owners)?;
        gpio_irq::validate_main_producers(flow, emitted, &owners)?;
        self.gpio
            .validate(flow, heap_functions, layouts, names, options)?;
        self.uart
            .validate(flow, heap_functions, layouts, names, options)?;
        self.i2c
            .validate(flow, heap_functions, layouts, names, options)?;
        Ok(())
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

    fn emit_declarations(&self, headers: &[GeneratedFile], writer: &mut Writer) {
        if headers.iter().any(|file| file.path == "emela_interrupt.h") {
            writer.line(include_str!("../../runtime/interrupt_queue.h"));
        }
    }

    fn needs_event_lock(&self, headers: &[GeneratedFile]) -> bool {
        headers.iter().any(|file| file.path == "emela_interrupt.h")
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

    fn emit_helpers(
        &self,
        context: &LinkContext<'_, '_>,
        headers: &[GeneratedFile],
        writer: &mut Writer,
    ) {
        for name in ["emela_gpio.h", "emela_uart.h", "emela_i2c.h"] {
            if !headers.iter().any(|file| file.path == name) {
                continue;
            }
            match name {
                "emela_gpio.h" => {
                    self.gpio
                        .emit(context.flow, context.layouts, context.names, writer)
                }
                "emela_uart.h" => {
                    self.uart
                        .emit(context.flow, context.layouts, context.names, writer)
                }
                "emela_i2c.h" => {
                    self.i2c
                        .emit(context.flow, context.layouts, context.names, writer)
                }
                _ => {}
            }
        }
    }
}
