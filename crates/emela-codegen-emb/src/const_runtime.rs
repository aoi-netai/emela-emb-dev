//! Lazy const preparation and heap-independent diagnostic storage.

use std::collections::HashSet;

use emela_lir::{FnId, Module};

use crate::{Error, ErrorKind, Options, const_eval, writer::Writer};

pub(crate) fn prepare(
    module: &mut Module,
    reachable: fn(&Module) -> HashSet<FnId>,
) -> HashSet<FnId> {
    let mut runtime = HashSet::new();
    loop {
        let newly_runtime: Vec<_> = reachable(module)
            .into_iter()
            .filter(|&id| {
                module.functions[id].constant && const_eval::evaluate(module, id).is_err()
            })
            .collect();
        if newly_runtime.is_empty() {
            return runtime;
        }
        for id in newly_runtime {
            module.functions[id].constant = false;
            runtime.insert(id);
        }
    }
}

pub(crate) fn emit_runtime(
    module: &Module,
    runtime: &HashSet<FnId>,
    options: &Options,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let prefix_bytes: usize = runtime
        .iter()
        .map(|&id| "while evaluating const : ".len() + module.functions[id].name.len())
        .sum();
    let emergency = prefix_bytes + 64;
    if emergency > usize::from(u16::MAX) || options.const_diagnostic_bytes == 0 {
        return Err(vec![Error {
            kind: ErrorKind::InvalidOptions,
            message: "runtime const diagnostics require a nonzero diagnostic capacity and const context names fitting uint16_t".into(),
        }]);
    }
    writer.line(&format!(
        "#define EMELA_CONST_DIAGNOSTIC_BYTES UINT32_C({})",
        options.const_diagnostic_bytes
    ));
    writer.line(&format!(
        "#define EMELA_CONST_EMERGENCY_BYTES {}",
        emergency.max(1)
    ));
    writer.line(include_str!("runtime/const.c"));
    Ok(())
}
