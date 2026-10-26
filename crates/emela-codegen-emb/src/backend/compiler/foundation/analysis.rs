//! Facts supplied by feature owners to shared planning passes.
use super::*;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::backend) enum StringOutput {
    #[default]
    None,
    Buffer,
    Call(FnId),
}

#[derive(Clone, Copy, Default)]
pub(in crate::backend) enum BufferResult {
    #[default]
    String,
    OptionalString,
    Any,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::backend) enum InputUse {
    ReadOnly,
    #[default]
    Escapes,
}

#[derive(Clone, Copy, Default)]
pub(in crate::backend) struct StringBehavior {
    pub output: StringOutput,
    pub result: BufferResult,
    pub reusable_input: Option<VarId>,
    /// Whether input references can outlive this operation.
    pub input_use: InputUse,
}

impl StringBehavior {
    pub fn needs_output(self, outputs: &HashSet<FnId>) -> bool {
        match self.output {
            StringOutput::None => false,
            StringOutput::Buffer => true,
            StringOutput::Call(id) => outputs.contains(&id),
        }
    }

    pub fn accepts(self, layouts: &TypeLayouts<'_>, ty: TypeId) -> bool {
        match self.result {
            BufferResult::String => matches!(layouts.types[ty], Type::String),
            BufferResult::OptionalString => option_payload_type(layouts, ty)
                .is_some_and(|ty| matches!(layouts.types[ty], Type::String)),
            BufferResult::Any => true,
        }
    }
}

pub(in crate::backend) struct StringBounds<'a> {
    pub values: &'a HashMap<VarId, usize>,
    pub capacity: u16,
}

impl StringBounds<'_> {
    pub fn operand(&self, operand: &Operand) -> usize {
        match operand {
            Operand::Const(Const::String(value)) => value.len(),
            Operand::Var(variable) => self
                .values
                .get(variable)
                .copied()
                .unwrap_or(usize::from(self.capacity)),
            _ => usize::from(self.capacity),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::backend) enum ResultOwnership {
    Owned,
    Borrowed,
}

pub(in crate::backend) struct HeapContext<'a, 'ir> {
    pub layouts: &'a TypeLayouts<'ir>,
    pub function: &'a Function,
    pub string_outputs: &'a HashSet<FnId>,
    pub known_empty: &'a HashSet<VarId>,
}
