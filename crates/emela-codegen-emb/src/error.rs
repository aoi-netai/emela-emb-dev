//! Ownership of failure metadata, separate from ordinary error enum values.
use super::*;

impl TypeLayouts<'_> {
    pub(super) fn has_error_notes(&self) -> bool {
        self.has_scopes && self.has_error_abi
    }

    pub(super) fn error_notes_kind(&self) -> u32 {
        self.cause_kind() + 1
    }

    pub(super) fn error_note_kind(&self) -> u32 {
        self.cause_kind() + 2
    }
}

pub(super) fn emit_types(layouts: &TypeLayouts<'_>, writer: &mut Writer) {
    if layouts.has_error_notes() {
        writer.line("typedef struct emela_error_note { struct emela_error_note *next; uint16_t length; char bytes[]; } emela_error_note;");
        writer.line(
            "typedef struct { emela_error_note *head; emela_error_note *tail; } emela_error_notes;",
        );
    }
}

pub(super) fn emit_ownership(layouts: &TypeLayouts<'_>, writer: &mut Writer) {
    if !layouts.has_error_abi {
        return;
    }
    writer.begin_block("static inline void emela_error_retain(EmelaError error)");
    if layouts.has_error_notes() {
        writer.line("emela_heap_retain(error.notes);");
    } else {
        writer.line("(void)error;");
    }
    writer.end_block();
    writer.begin_block("static inline void emela_error_release(EmelaError error)");
    if layouts.has_error_notes() {
        writer.line("emela_heap_release(error.notes);");
    } else {
        writer.line("(void)error;");
    }
    writer.end_block();
    writer.begin_block("static inline void emela_error_clear(EmelaError *error)");
    writer.line("emela_error_release(*error); *error = (EmelaError){0};");
    writer.end_block();
}
