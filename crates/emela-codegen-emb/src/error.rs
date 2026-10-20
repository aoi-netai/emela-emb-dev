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
    if layouts.has_error_notes() {
        writer.line("static inline void emela_error_ensure_notes(EmelaError *error);");
        writer
            .line("static inline void emela_error_add_note(EmelaError *first, EmelaError other);");
    }
}

pub(super) fn needs_display(layouts: &TypeLayouts<'_>) -> bool {
    layouts.has_error_abi && (layouts.has_causes || layouts.has_scopes || layouts.has_resumes)
}

pub(super) fn emit_display(
    layouts: &TypeLayouts<'_>,
    names: &HashMap<FnId, String>,
    output_calls: &HashSet<FnId>,
    capacity: u16,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    if !needs_display(layouts) {
        return Ok(());
    }
    writer.begin_block("static inline EmelaString emela_error_to_string(EmelaError error, char *output, uint16_t capacity)");
    writer.line("if (capacity < 7) emela_board_defect(EMELA_DEFECT_STRING_CAPACITY, \"string capacity exceeded\", UINT16_C(24));");
    writer.line("EmelaString text = {0};");
    writer.begin_block("switch (error.tag)");
    for &(error, _) in &layouts.error_types {
        let (id, name) = layouts
            .error_shows
            .iter()
            .find(|(e, _)| *e == error)
            .and_then(|(_, id)| names.get(id).map(|name| (*id, name)))
            .ok_or_else(|| unsupported("error display", "error without a show function"))?;
        let output = if output_calls.contains(&id) {
            ", output + 7, (uint16_t)(capacity - 7)"
        } else {
            ""
        };
        writer.line(&format!(
            "case UINT32_C({}): text = {name}(error.payload.e{}{output}); break;",
            enum_index(error) + 1,
            enum_index(error)
        ));
    }
    writer
        .line("default: emela_board_defect(EMELA_DEFECT_ASSERT, \"invalid error\", UINT16_C(13));");
    writer.end_block();
    writer.line("if (text.length > capacity - 7) emela_board_defect(EMELA_DEFECT_STRING_CAPACITY, \"string capacity exceeded\", UINT16_C(24));");
    writer.line("if (text.data != output + 7) for (uint32_t i = 0; i < text.length; i++) output[7 + i] = text.data[i];");
    writer.line("for (uint32_t i = 0; i < 7; i++) output[i] = \"error: \"[i];");
    writer.line("uint32_t length = 7 + text.length; emela_heap_release(text.owner);");
    if layouts.has_error_notes() {
        const PREFIX: &str = "\nnote: another fiber also failed: ";
        writer.line("emela_error_notes *notes = (emela_error_notes *)error.notes;");
        writer.begin_block("for (emela_error_note *note = notes == NULL ? NULL : notes->head; note != NULL; note = note->next)");
        writer.line(&format!("if (length + {} + note->length > capacity) emela_board_defect(EMELA_DEFECT_STRING_CAPACITY, \"string capacity exceeded\", UINT16_C(24));", PREFIX.len()));
        writer.line(&format!(
            "for (uint32_t i = 0; i < {}; i++) output[length++] = {}[i];",
            PREFIX.len(),
            c_string_literal(PREFIX.as_bytes())
        ));
        writer
            .line("for (uint32_t i = 0; i < note->length; i++) output[length++] = note->bytes[i];");
        writer.end_block();
    }
    writer.line("return (EmelaString){.data = output, .length = (uint16_t)length};");
    writer.end_block();
    if layouts.has_error_notes() {
        writer.begin_block("static inline void emela_error_ensure_notes(EmelaError *error)");
        writer.begin_block("if (error->notes == NULL)");
        writer.line(&format!(
            "error->notes = emela_heap_allocate(sizeof(emela_error_notes), UINT32_C({}));",
            layouts.error_notes_kind()
        ));
        writer.line("*(emela_error_notes *)error->notes = (emela_error_notes){0};");
        writer.end_block();
        writer.end_block();
        writer.begin_block(
            "static inline void emela_error_add_note(EmelaError *first, EmelaError other)",
        );
        writer.line("emela_error_ensure_notes(first);");
        writer.line(&format!(
            "char *scratch = (char *)emela_heap_allocate(UINT16_C({capacity}), UINT32_C({}));",
            layouts.string_owner_kind()
        ));
        writer.line(&format!(
            "EmelaString text = emela_error_to_string(other, scratch, UINT16_C({capacity}));"
        ));
        writer.line(&format!("emela_error_note *note = (emela_error_note *)emela_heap_allocate(sizeof(*note) + text.length, UINT32_C({}));", layouts.error_note_kind()));
        writer.line("note->next = NULL; note->length = text.length;");
        writer.line("for (uint32_t i = 0; i < text.length; i++) note->bytes[i] = scratch[i];");
        writer.line("emela_heap_release(scratch);");
        writer.line("emela_error_notes *notes = (emela_error_notes *)first->notes;");
        writer.line("if (notes->tail == NULL) notes->head = note; else notes->tail->next = note;");
        writer.line("notes->tail = note;");
        writer.end_block();
    }
    Ok(())
}
