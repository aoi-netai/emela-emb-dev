//! Ownership of synchronous error payloads.
use super::*;

pub(super) fn emit_ownership_declarations(layouts: &TypeLayouts<'_>, writer: &mut Writer) {
    if !layouts.has_error_abi {
        return;
    }
    writer.line("static inline void emela_error_retain(EmelaError error);");
    writer.line("static inline void emela_error_own(EmelaError *error);");
    writer.line("static inline void emela_error_release(EmelaError error);");
    writer.line("static inline void emela_error_enqueue(EmelaError error, emela_heap_block **pending, emela_heap_block **tail);");
}

pub(super) fn emit_ownership(layouts: &TypeLayouts<'_>, heap: bool, writer: &mut Writer) {
    if !layouts.has_error_abi {
        return;
    }
    writer.begin_block("static inline void emela_error_own(EmelaError *error)");
    writer.line("(void)error;");
    if heap && layouts.managed_error_payloads {
        writer.begin_block("switch (error->tag)");
        for &(id, ty) in &layouts.error_types {
            if !layouts.is_managed_value(ty) {
                continue;
            }
            writer.line(&format!("case UINT32_C({}):", enum_index(id) + 1));
            writer.begin_block("");
            layouts.emit_own_value_in_place(
                ty,
                &format!("error->payload.e{}", enum_index(id)),
                writer,
            );
            writer.line("break;");
            writer.end_block();
        }
        writer.line("default: break;");
        writer.end_block();
    }
    writer.end_block();
    writer.begin_block("static inline void emela_error_retain(EmelaError error)");
    writer.line("(void)error;");
    if layouts.managed_error_payloads {
        emit_payloads(layouts, true, writer);
    }
    writer.end_block();
    if heap {
        writer.begin_block("static inline void emela_error_enqueue(EmelaError error, emela_heap_block **pending, emela_heap_block **tail)");
        writer.line("(void)error; (void)pending; (void)tail;");
        if layouts.managed_error_payloads {
            emit_payloads(layouts, false, writer);
        }
        writer.end_block();
    }
    writer.begin_block("static inline void emela_error_release(EmelaError error)");
    if layouts.managed_error_payloads {
        writer.line("emela_heap_block *pending = NULL, *tail = NULL;");
        writer.line("emela_error_enqueue(error, &pending, &tail);");
        writer.line("emela_heap_drain(pending, tail);");
    } else {
        writer.line("(void)error;");
    }
    writer.end_block();
}

fn emit_payloads(layouts: &TypeLayouts<'_>, retain: bool, writer: &mut Writer) {
    writer.begin_block("switch (error.tag)");
    for &(id, ty) in &layouts.error_types {
        if !layouts.is_managed_value(ty) {
            continue;
        }
        writer.line(&format!("case UINT32_C({}):", enum_index(id) + 1));
        writer.begin_block("");
        let value = format!("error.payload.e{}", enum_index(id));
        if retain {
            layouts.emit_retain_value_expression(ty, &value, writer);
        } else {
            layouts.emit_enqueue_value_expression(ty, &value, "pending", "tail", writer);
        }
        writer.line("break;");
        writer.end_block();
    }
    writer.line("default: break;");
    writer.end_block();
}
