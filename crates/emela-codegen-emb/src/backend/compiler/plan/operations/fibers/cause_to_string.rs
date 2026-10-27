use super::super::*;

pub(in crate::backend::compiler::plan) fn string_behavior() -> StringBehavior {
    StringBehavior {
        output: StringOutput::Buffer,
        result: BufferResult::Any,
        ..StringBehavior::default()
    }
}
