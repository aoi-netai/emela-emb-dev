//! Static UART interrupt binding and evidence-free ISR specializations.
use super::*;
use emela_lir::Module;

pub(super) struct Binding {
    pin: i32,
    callback: FnId,
}

#[derive(Default)]
pub(super) struct Plan {
    bindings: Vec<Binding>,
}

pub(super) fn collect(module: &Module) -> Result<Vec<(i32, FnId)>, Vec<Error>> {
    gpio_irq::collect_uart(module)
}

pub(super) fn prepare(
    preparation: &mut LinkPreparation,
    bindings: Vec<(i32, FnId)>,
) -> Result<Plan, Vec<Error>> {
    let mut plan = Plan::default();
    let mut specialized = HashMap::new();
    let mut operations = HashMap::new();
    for (pin, callback) in bindings {
        let callback = gpio_irq::specialize(
            preparation,
            callback,
            &mut specialized,
            &mut operations,
            "UART",
        )?;
        plan.bindings.push(Binding { pin, callback });
    }
    Ok(plan)
}

impl Plan {
    pub(super) fn roots(&self) -> impl Iterator<Item = FnId> + '_ {
        self.bindings.iter().map(|binding| binding.callback)
    }

    pub(super) fn claim_channels(
        &self,
        flow: &Module,
        owners: &mut HashMap<i32, String>,
    ) -> Result<(), Vec<Error>> {
        for binding in &self.bindings {
            gpio_irq::claim_channels(
                flow,
                binding.callback,
                &format!("UART ID {}", binding.pin),
                owners,
            )?;
        }
        Ok(())
    }

    pub(super) fn validate(
        &self,
        flow: &Module,
        heap: &HashSet<FnId>,
        layouts: &TypeLayouts<'_>,
        names: &HashMap<FnId, String>,
        options: &Options,
    ) -> Result<(), Vec<Error>> {
        for binding in &self.bindings {
            validate_isr_function(
                flow,
                binding.callback,
                &flow.functions[binding.callback].name,
                heap,
                layouts,
                names,
                options,
                &mut HashSet::new(),
                &mut HashSet::new(),
            )?;
        }
        Ok(())
    }
    pub(super) fn emit(
        &self,
        flow: &Module,
        layouts: &TypeLayouts<'_>,
        names: &HashMap<FnId, String>,
        writer: &mut Writer,
    ) {
        writer.begin_block("int32_t emela_uart_has_binding(int32_t id)");
        writer.line("(void)id;");
        for binding in &self.bindings {
            writer.line(&format!("if (id == INT32_C({})) return 1;", binding.pin));
        }
        writer.ret(Some("0"));
        writer.end_block();
        for (signature, variant) in [
            ("void emela_uart_received(int32_t id, int32_t value)", 0),
            ("void emela_uart_transmit_complete(int32_t id)", 1),
            (
                "void emela_uart_receive_error(int32_t id, int32_t parity, int32_t framing, int32_t overrun, int32_t other)",
                2,
            ),
        ] {
            writer.begin_block(signature);
            writer.line("(void)id;");
            if variant == 0 {
                writer.line("(void)value; if (value < 0 || value > 511) return;");
            }
            if variant == 2 {
                writer.line("if (!(parity || framing || overrun || other)) return;");
            }
            for binding in &self.bindings {
                let function = &flow.functions[binding.callback];
                let event = function.vars[function.params[1]].ty;
                let payload = match variant {
                    0 => ", .payload.v0.f0 = value".to_owned(),
                    1 => String::new(),
                    _ => {
                        let errors = layouts.enum_fields(event).expect("UART event fields")[2][0];
                        format!(
                            ", .payload.v2.f0 = ({}){{ .tag = 0, .payload.v0 = {{ .f0 = parity != 0, .f1 = framing != 0, .f2 = overrun != 0, .f3 = other != 0 }} }}",
                            layouts.c_type(errors)
                        )
                    }
                };
                writer.line(&format!("if (id == INT32_C({})) {{ {}(id, ({}){{ .tag = {variant}{payload} }}); return; }}", binding.pin, names[&binding.callback], layouts.c_type(event)));
            }
            writer.end_block();
        }
    }
}
