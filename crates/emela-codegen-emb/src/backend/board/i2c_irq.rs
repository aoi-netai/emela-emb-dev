//! Static I2C interrupt binding and evidence-free ISR specializations.
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
    gpio_irq::collect_bindings(module, "I2c")
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
            "I2C",
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
                &format!("I2C ID {}", binding.pin),
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
        writer.begin_block("int32_t emela_i2c_has_binding(int32_t id)");
        writer.line("(void)id;");
        for binding in &self.bindings {
            writer.line(&format!("if (id == INT32_C({})) return 1;", binding.pin));
        }
        writer.ret(Some("0"));
        writer.end_block();
        for (signature, variant) in [
            (
                "void emela_i2c_transfer_complete(int32_t id, int32_t operation)",
                0,
            ),
            (
                "void emela_i2c_transfer_error(int32_t id, int32_t operation, int32_t status)",
                1,
            ),
            ("void emela_i2c_device_ready(int32_t id, int32_t ready)", 2),
        ] {
            writer.begin_block(signature);
            writer.line("(void)id;");
            if variant != 2 {
                writer.line(&format!(
                    "if (operation < 1 || operation > {}) return;",
                    if variant == 0 { 4 } else { 5 }
                ));
                // Public operation codes are independent of the Emela enum layout.
                writer.line("int32_t kind = 0;");
                writer.begin_block("switch (operation)");
                for (code, name) in [
                    (1, "MasterTransmit"),
                    (2, "MasterReceive"),
                    (3, "MemoryWrite"),
                    (4, "MemoryRead"),
                    (5, "IsDeviceReady"),
                ] {
                    let def = flow
                        .core
                        .enums
                        .iter()
                        .find(|(_, def)| def.name == "Std.Embedded.I2c.I2cOperationKind");
                    let index = def
                        .and_then(|(_, def)| def.variants.iter().position(|v| v.name == name))
                        .unwrap_or(code - 1);
                    writer.line(&format!("case {code}: kind = {index}; break;"));
                }
                writer.line("default: return;");
                writer.end_block();
                writer.line("(void)kind;");
            }
            if variant == 1 {
                writer.line("if (status == 0) return;");
                let def = flow
                    .core
                    .enums
                    .iter()
                    .find(|(_, def)| def.name == "Std.Embedded.I2c.I2cErrorKind");
                writer.line("int32_t error = 0;");
                writer.begin_block("switch (status)");
                for &(status, _, common) in peripheral_status::STATUSES {
                    let name = peripheral_status::error_kind("i2c", status, common);
                    let index = def
                        .and_then(|(_, def)| def.variants.iter().position(|v| v.name == name))
                        .unwrap_or(11);
                    writer.line(&format!(
                        "case EMELA_PERIPHERAL_{status}: error = {index}; break;"
                    ));
                }
                let other = def
                    .and_then(|(_, def)| def.variants.iter().position(|v| v.name == "Other"))
                    .unwrap_or(11);
                writer.line(&format!("default: error = {other}; break;"));
                writer.end_block();
                writer.line("(void)error;");
            }
            if variant == 2 {
                writer.line("if (ready != 0 && ready != 1) return;");
            }
            for binding in &self.bindings {
                let function = &flow.functions[binding.callback];
                let event = function.vars[function.params[1]].ty;
                let payload = match variant {
                    0 => ".payload.v0.f0 = kind",
                    1 => ".payload.v1 = { .f0 = kind, .f1 = error }",
                    _ => ".payload.v2.f0 = ready",
                };
                writer.line(&format!("if (id == INT32_C({})) {{ {}(id, ({}){{ .tag = {variant}, {payload} }}); return; }}", binding.pin, names[&binding.callback], layouts.c_type(event)));
            }
            writer.end_block();
        }
    }
}
