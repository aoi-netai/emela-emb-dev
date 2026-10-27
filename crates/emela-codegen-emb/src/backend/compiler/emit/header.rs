//! Public C types, runtime interfaces, and exported function declarations.
use super::super::{HEADER_FILE, external, layouts::TypeLayouts, suspend, task_values};
use super::functions::signature;
use crate::{GeneratedFile, writer::Writer};
use emela_core::{Type, TypeId};
use emela_lir::{FnId, Module};
use std::collections::{HashMap, HashSet};

/// Inputs resolved by the existing module assembly pass. No input is mutated.
pub(super) struct Context<'a, 'ir> {
    pub flow: &'ir Module,
    pub layouts: &'a TypeLayouts<'ir>,
    pub emitted_functions: &'a HashSet<FnId>,
    pub external_bindings: &'a external::Bindings,
    pub names: &'a HashMap<FnId, String>,
    pub public_names: &'a HashMap<FnId, String>,
    pub string_output_functions: &'a HashSet<FnId>,
    pub task_value_types: &'a [TypeId],
    pub has_scheduler: bool,
    pub has_tasks: bool,
    pub heap_required: bool,
    pub entry_declaration: Option<&'static str>,
}

pub(super) fn emit(context: Context<'_, '_>) -> GeneratedFile {
    let Context {
        flow,
        layouts,
        emitted_functions,
        external_bindings,
        names,
        public_names,
        string_output_functions,
        task_value_types,
        has_scheduler,
        has_tasks,
        heap_required,
        entry_declaration,
    } = context;
    let mut header = Writer::new();
    header.line("#ifndef EMELA_EMBEDDED_H");
    header.line("#define EMELA_EMBEDDED_H");
    header.blank();
    header.line("#include <stdint.h>");
    header.blank();
    layouts.emit_string_declaration(&mut header);
    if has_scheduler
        || layouts
            .function_types
            .iter()
            .any(|&ty| matches!(&layouts.types[ty], Type::Fn(ft) if ft.suspends))
    {
        header.line("typedef struct emela_fiber emela_fiber;");
    }
    if layouts.has_error_abi || layouts.any_abi {
        header.line("typedef struct EmelaError EmelaError;");
        if layouts.any_abi {
            header.line("typedef struct EmelaError EmelaAny;");
        }
        header.blank();
    }
    layouts.emit_declarations(&mut header);
    external::emit_header(flow, external_bindings, layouts, heap_required, &mut header);
    if has_scheduler {
        header.line(include_str!("../../../runtime/scheduler.h"));
    }
    if has_tasks {
        header.line("#define EMELA_HAS_TASK_RUNTIME 1");
        header.line(include_str!("../../../runtime/task.h"));
        if !task_value_types.is_empty() || suspend::has_failing_tasks(flow, emitted_functions) {
            header.line(include_str!("../../../runtime/task_context.h"));
        }
        if suspend::has_failing_tasks(flow, emitted_functions) {
            header.line(include_str!("../../../runtime/task_failure.h"));
        }
        task_values::emit_header(layouts, task_value_types, &mut header);
        for (id, function) in flow
            .functions
            .iter()
            .filter(|(id, f)| emitted_functions.contains(id) && f.suspends && f.external.is_some())
        {
            if !suspend::scalar(&layouts.types, function.ret) {
                header.line(&format!("/* Success: emela_task_complete_value_{}. Arguments are borrowed only during start. */", function.ret.index()));
            }
            header.line(&format!(
                "{};",
                suspend::task_signature(layouts, function, &names[&id])
            ));
        }
    }
    if !layouts.aggregates.is_empty() {
        header.blank();
    }

    for (id, function) in flow
        .functions
        .iter()
        .filter(|(id, f)| emitted_functions.contains(id) && f.exported)
    {
        if function.suspends {
            header.line(&format!(
                "{};",
                suspend::start_signature(layouts, function, &public_names[&id])
            ));
            continue;
        }
        let name = &public_names[&id];
        let takes_string_output = string_output_functions.contains(&id);
        if takes_string_output {
            header.line("/* Caller provides this output buffer; the returned EmelaString points into it. */");
        }
        header.line(&format!(
            "{};",
            signature(layouts, function, name, false, false, takes_string_output)
        ));
    }
    if let Some(declaration) = entry_declaration {
        header.line(declaration);
    }
    if heap_required {
        header.line("/* Retains a value allocated from the fixed heap. */");
        header.line("void emela_embedded_retain(void *value);");
        header.line("/* Releases a value allocated from the fixed heap. */");
        header.line("void emela_embedded_release(void *value);");
        header.line("/* Retains the backing owner of an EmelaString value. */");
        header.line("void emela_embedded_retain_string(const EmelaString *value);");
        header.line("/* Releases the backing owner of an EmelaString value. */");
        header.line("void emela_embedded_release_string(EmelaString *value);");
        layouts.emit_public_release_declarations(&mut header);
        layouts.emit_public_function_ownership_declarations(&mut header);
    }

    header.blank();
    header.line("#endif");

    GeneratedFile {
        path: HEADER_FILE.into(),
        contents: header.finish(),
    }
}
