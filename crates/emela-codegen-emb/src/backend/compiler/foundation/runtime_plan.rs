//! Aggregate feature requests, resolve helper dependencies, and preserve C declaration order.
use super::*;
use std::collections::BTreeSet;

// Declaration order preserves the existing generated C order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::backend) enum Helper {
    StringComparison,
    I32FromBits,
    I32Shift,
    NegativeShift,
    I32Division,
    I64FromBits,
    I64Shift,
    I64Division,
    FloatToInt,
    DynamicDefect,
    StringMatchesAt,
    StringContains,
    StringStartsWith,
    StringEndsWith,
    Utf8Decode,
    Grapheme,
    StringTrim,
    StringJoin,
    StringAppend,
    StringShow,
    IntToString,
    IntParse,
    FloatParse,
    FloatToString,
    StringOutput,
}

impl Helper {
    fn dependencies(self) -> &'static [Self] {
        use Helper::*;
        match self {
            I32Shift => &[I32FromBits],
            I64Shift => &[I64FromBits],
            StringContains | StringStartsWith | StringEndsWith => &[StringMatchesAt],
            Grapheme | StringTrim => &[Utf8Decode],
            _ => &[],
        }
    }

    fn emit(self, writer: &mut Writer) {
        match self {
            Self::StringComparison => {
                features::primitive_operations::emit_string_comparison_helper(writer)
            }
            Self::I32FromBits => numeric_runtime::emit_i32_from_bits_helper(writer),
            Self::I32Shift => features::builtins::integer::emit_i32_shift_helpers(writer),
            Self::NegativeShift => numeric_runtime::emit_negative_shift_helper(writer),
            Self::I32Division => features::primitive_operations::emit_i32_division_helpers(writer),
            Self::I64FromBits => numeric_runtime::emit_i64_wrap_helper(writer),
            Self::I64Shift => features::builtins::integer64::emit_i64_shift_helpers(writer),
            Self::I64Division => features::primitive_operations::emit_i64_division_helpers(writer),
            Self::FloatToInt => features::builtins::float::emit_float_to_int_helper(writer),
            Self::DynamicDefect => features::terms::defect::emit_dynamic_defect_kind_helper(writer),
            Self::StringMatchesAt => string_runtime::emit_string_matches_at_helper(writer),
            Self::StringContains => {
                features::builtins::string_contains::emit_string_contains_helper(writer)
            }
            Self::StringStartsWith => {
                features::builtins::string_starts_with::emit_string_starts_with_helper(writer)
            }
            Self::StringEndsWith => {
                features::builtins::string_ends_with::emit_string_ends_with_helper(writer)
            }
            Self::Utf8Decode => string_runtime::emit_utf8_decode_helper(writer),
            Self::Grapheme => crate::unicode_grapheme::emit_runtime(writer),
            Self::StringTrim => features::builtins::string_trim::emit_string_trim_helper(writer),
            Self::StringJoin => features::builtins::string_join::emit_string_join_helper(writer),
            Self::StringAppend => {
                features::builtins::string_append::emit_string_append_helper(writer)
            }
            Self::StringShow => features::runtime::emit_string_show_helpers(writer),
            Self::IntToString => string_runtime::emit_int_to_string_helper(writer),
            Self::IntParse => features::builtins::int_parse::emit_int_parse_helper(writer),
            Self::FloatParse => crate::float_parse::emit_runtime(writer),
            Self::FloatToString => crate::float_to_string::emit_runtime(writer),
            Self::StringOutput => string_buffers::emit_string_output_helper(writer),
        }
    }
}

#[derive(Default)]
pub(in crate::backend) struct Requirements {
    helpers: BTreeSet<Helper>,
    pub defects: bool,
    pub debug: bool,
}

impl Requirements {
    pub fn require(&mut self, helper: Helper) {
        if self.helpers.insert(helper) {
            for &dependency in helper.dependencies() {
                self.require(dependency);
            }
        }
    }

    pub fn collect(flow: &emela_lir::Module, emitted: &HashSet<FnId>) -> Self {
        let mut requirements = Self::default();
        for (_, function) in flow.functions.iter().filter(|(id, _)| emitted.contains(id)) {
            requirements.defects |= function.exported && function.fails && !function.suspends;
            if function.constant {
                continue;
            }
            let active = runtime_statements(function);
            for (id, statement) in function.stmts.iter() {
                features::statements::require_runtime(
                    statement,
                    active.contains(&id),
                    &mut requirements,
                );
            }
            for (_, sequence) in function.seqs.iter() {
                features::terms::require_runtime(flow, function, &sequence.term, &mut requirements);
            }
        }
        requirements
    }

    pub fn emit_helpers(&self, writer: &mut Writer) {
        for &helper in &self.helpers {
            helper.emit(writer);
        }
    }
}

#[test]
fn shared_runtime_dependencies_are_emitted_before_their_consumers() {
    for root in [
        Helper::I32Shift,
        Helper::I64Shift,
        Helper::StringContains,
        Helper::StringStartsWith,
        Helper::StringEndsWith,
        Helper::StringTrim,
        Helper::Grapheme,
    ] {
        let mut requirements = Requirements::default();
        requirements.require(root);
        requirements.require(root);
        let mut emitted = BTreeSet::new();
        for &helper in &requirements.helpers {
            for dependency in helper.dependencies() {
                assert!(
                    emitted.contains(dependency),
                    "{helper:?} needs {dependency:?}"
                );
            }
            assert!(emitted.insert(helper));
        }
        assert!(emitted.contains(&root));
    }
}
