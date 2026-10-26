//! Contract for a connection pass. The compiler never imports a concrete target.
use super::*;

pub(in crate::backend) struct LinkContext<'a, 'ir> {
    pub flow: &'ir emela_lir::Module,
    pub layouts: &'a TypeLayouts<'ir>,
    pub names: &'a HashMap<FnId, String>,
    pub emitted: &'a HashSet<FnId>,
    pub heap_functions: &'a HashSet<FnId>,
    pub options: &'a Options,
}

pub(in crate::backend) trait Target {
    fn external_binding(
        &self,
        _flow: &emela_lir::Module,
        _id: FnId,
        _function: &Function,
    ) -> Result<Option<external::Binding>, Vec<Error>> {
        Ok(None)
    }

    fn task_symbol<'a>(&self, _function: &'a Function) -> Option<&'a str> {
        None
    }

    fn headers(
        &self,
        _flow: &emela_lir::Module,
        _emitted: &HashSet<FnId>,
        _options: &Options,
    ) -> Result<Vec<GeneratedFile>, Vec<Error>> {
        Ok(vec![])
    }

    fn validate(&self, _context: &LinkContext<'_, '_>) -> Result<(), Vec<Error>> {
        Ok(())
    }

    fn emit_adapter(
        &self,
        _context: &LinkContext<'_, '_>,
        _function: &Function,
        _name: &str,
        _writer: &mut Writer,
    ) -> Result<bool, Vec<Error>> {
        Ok(false)
    }

    fn emit_declarations(&self, _headers: &[GeneratedFile], _writer: &mut Writer) {}

    fn needs_event_lock(&self, _headers: &[GeneratedFile]) -> bool {
        false
    }

    fn entry_declaration(&self) -> &'static str;

    fn emit_entry(
        &self,
        context: &LinkContext<'_, '_>,
        function: &Function,
        name: &str,
        string_output: bool,
        writer: &mut Writer,
    );

    fn emit_helpers(
        &self,
        _context: &LinkContext<'_, '_>,
        _headers: &[GeneratedFile],
        _writer: &mut Writer,
    ) {
    }
}
