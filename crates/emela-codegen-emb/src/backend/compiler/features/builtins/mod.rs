// Keep analysis dispatch exhaustive: every new variant needs an explicit planning decision.
use super::*;
pub(in crate::backend) mod cells;
pub(in crate::backend) mod float;
pub(in crate::backend) mod float_parse;
pub(in crate::backend) mod float_to_string;
pub(in crate::backend) mod int64_to_string;
pub(in crate::backend) mod int_checked_add;
pub(in crate::backend) mod int_parse;
pub(in crate::backend) mod int_to_string;
pub(in crate::backend) mod integer;
pub(in crate::backend) mod integer64;
pub(in crate::backend) mod rejected;
pub(in crate::backend) mod string_append;
pub(in crate::backend) mod string_byte_size;
pub(in crate::backend) mod string_chars;
pub(in crate::backend) mod string_code_points;
pub(in crate::backend) mod string_contains;
pub(in crate::backend) mod string_ends_with;
pub(in crate::backend) mod string_from_code_point;
pub(in crate::backend) mod string_join;
pub(in crate::backend) mod string_length;
pub(in crate::backend) mod string_split;
pub(in crate::backend) mod string_starts_with;
pub(in crate::backend) mod string_trim;
pub(in crate::backend) fn emit(
    context: &mut BuiltinContext<'_, '_>,
    dst: Option<VarId>,
    op: Builtin,
    args: &[Operand],
) -> Result<(), Vec<Error>> {
    match op {
        Builtin::StringLength => string_length::string_length(context, dst, args),
        Builtin::StringByteSize => string_byte_size::string_byte_size(context, dst, args),
        Builtin::StringAppend => string_append::string_append(context, dst, args),
        Builtin::StringContains => string_contains::string_contains(context, dst, args),
        Builtin::StringStartsWith => string_starts_with::string_starts_with(context, dst, args),
        Builtin::StringEndsWith => string_ends_with::string_ends_with(context, dst, args),
        Builtin::StringSplit => string_split::string_split(context, dst, args),
        Builtin::StringChars => string_chars::string_chars(context, dst, args),
        Builtin::StringJoin => string_join::string_join(context, dst, args),
        Builtin::StringTrim => string_trim::string_trim(context, dst, args),
        Builtin::StringCodePoints => string_code_points::string_code_points(context, dst, args),
        Builtin::StringFromCodePoint => {
            string_from_code_point::string_from_code_point(context, dst, args)
        }
        Builtin::ShowNested => rejected::show_nested(context, dst, args),
        Builtin::IntToString => int_to_string::int_to_string(context, dst, args),
        Builtin::IntParse => int_parse::int_parse(context, dst, args),
        Builtin::IntCheckedAdd => int_checked_add::int_checked_add(context, dst, args),
        Builtin::IntBitAnd => integer::int_bit_and(context, dst, args),
        Builtin::IntBitOr => integer::int_bit_or(context, dst, args),
        Builtin::IntBitXor => integer::int_bit_xor(context, dst, args),
        Builtin::IntBitNot => integer::int_bit_not(context, dst, args),
        Builtin::IntShiftLeft => integer::int_shift_left(context, dst, args),
        Builtin::IntShiftRight => integer::int_shift_right(context, dst, args),
        Builtin::IntShiftRightUnsigned => integer::int_shift_right_unsigned(context, dst, args),
        Builtin::Int64FromInt => integer64::int64_from_int(context, dst, args),
        Builtin::Int64ToString => int64_to_string::int64_to_string(context, dst, args),
        Builtin::Int64ToInt => integer64::int64_to_int(context, dst, args),
        Builtin::Int64BitAnd => integer64::int64_bit_and(context, dst, args),
        Builtin::Int64BitOr => integer64::int64_bit_or(context, dst, args),
        Builtin::Int64BitXor => integer64::int64_bit_xor(context, dst, args),
        Builtin::Int64BitNot => integer64::int64_bit_not(context, dst, args),
        Builtin::Int64ShiftLeft => integer64::int64_shift_left(context, dst, args),
        Builtin::Int64ShiftRight => integer64::int64_shift_right(context, dst, args),
        Builtin::Int64ShiftRightUnsigned => {
            integer64::int64_shift_right_unsigned(context, dst, args)
        }
        Builtin::FloatToString => float_to_string::float_to_string(context, dst, args),
        Builtin::FloatFromInt => float::float_from_int(context, dst, args),
        Builtin::FloatParse => float_parse::float_parse(context, dst, args),
        Builtin::FloatFloor => float::float_floor(context, dst, args),
        Builtin::FloatCeil => float::float_ceil(context, dst, args),
        Builtin::FloatRound => float::float_round(context, dst, args),
        Builtin::FloatTruncate => float::float_truncate(context, dst, args),
        Builtin::Panic => rejected::panic(context, dst, args),
        Builtin::Todo => rejected::todo(context, dst, args),
        Builtin::Dbg => rejected::dbg(context, dst, args),
        Builtin::Inspect => rejected::inspect(context, dst, args),
        Builtin::CellNew => cells::cell_new(context, dst, args),
        Builtin::CellGet => cells::cell_get(context, dst, args),
        Builtin::CellSet => cells::cell_set(context, dst, args),
        Builtin::ShowString => rejected::show_string(context, dst, args),
        Builtin::DebugPrint => rejected::debug_print(context, dst, args),
    }
}

pub(super) fn string_behavior(op: Builtin, args: &[Operand]) -> StringBehavior {
    match op {
        Builtin::StringAppend => string_append::string_behavior(args),
        Builtin::StringByteSize => string_byte_size::string_behavior(args),
        Builtin::StringContains => string_contains::string_behavior(args),
        Builtin::StringStartsWith => string_starts_with::string_behavior(args),
        Builtin::StringEndsWith => string_ends_with::string_behavior(args),
        Builtin::StringJoin => string_join::string_behavior(args),
        Builtin::IntToString => int_to_string::string_behavior(args),
        Builtin::Int64ToString => int64_to_string::string_behavior(args),
        Builtin::FloatToString => float_to_string::string_behavior(args),
        Builtin::StringFromCodePoint => string_from_code_point::string_behavior(args),
        Builtin::StringLength
        | Builtin::StringSplit
        | Builtin::StringChars
        | Builtin::StringTrim
        | Builtin::StringCodePoints
        | Builtin::ShowNested
        | Builtin::IntParse
        | Builtin::IntCheckedAdd
        | Builtin::IntBitAnd
        | Builtin::IntBitOr
        | Builtin::IntBitXor
        | Builtin::IntBitNot
        | Builtin::IntShiftLeft
        | Builtin::IntShiftRight
        | Builtin::IntShiftRightUnsigned
        | Builtin::Int64FromInt
        | Builtin::Int64ToInt
        | Builtin::Int64BitAnd
        | Builtin::Int64BitOr
        | Builtin::Int64BitXor
        | Builtin::Int64BitNot
        | Builtin::Int64ShiftLeft
        | Builtin::Int64ShiftRight
        | Builtin::Int64ShiftRightUnsigned
        | Builtin::FloatFromInt
        | Builtin::FloatParse
        | Builtin::FloatFloor
        | Builtin::FloatCeil
        | Builtin::FloatRound
        | Builtin::FloatTruncate
        | Builtin::Panic
        | Builtin::Todo
        | Builtin::Dbg
        | Builtin::Inspect
        | Builtin::CellNew
        | Builtin::CellGet
        | Builtin::CellSet
        | Builtin::ShowString
        | Builtin::DebugPrint => StringBehavior::default(),
    }
}

pub(super) fn string_bound(
    op: Builtin,
    args: &[Operand],
    bounds: &StringBounds<'_>,
) -> Option<usize> {
    match op {
        Builtin::StringAppend => string_append::string_bound(args, bounds),
        Builtin::StringTrim => string_trim::string_bound(args, bounds),
        Builtin::StringJoin => string_join::string_bound(args, bounds),
        Builtin::IntToString => int_to_string::string_bound(args, bounds),
        Builtin::Int64ToString => int64_to_string::string_bound(args, bounds),
        Builtin::FloatToString => float_to_string::string_bound(args, bounds),
        Builtin::StringFromCodePoint => string_from_code_point::string_bound(args, bounds),
        Builtin::StringLength
        | Builtin::StringByteSize
        | Builtin::StringContains
        | Builtin::StringStartsWith
        | Builtin::StringEndsWith
        | Builtin::StringSplit
        | Builtin::StringChars
        | Builtin::StringCodePoints
        | Builtin::ShowNested
        | Builtin::IntParse
        | Builtin::IntCheckedAdd
        | Builtin::IntBitAnd
        | Builtin::IntBitOr
        | Builtin::IntBitXor
        | Builtin::IntBitNot
        | Builtin::IntShiftLeft
        | Builtin::IntShiftRight
        | Builtin::IntShiftRightUnsigned
        | Builtin::Int64FromInt
        | Builtin::Int64ToInt
        | Builtin::Int64BitAnd
        | Builtin::Int64BitOr
        | Builtin::Int64BitXor
        | Builtin::Int64BitNot
        | Builtin::Int64ShiftLeft
        | Builtin::Int64ShiftRight
        | Builtin::Int64ShiftRightUnsigned
        | Builtin::FloatFromInt
        | Builtin::FloatParse
        | Builtin::FloatFloor
        | Builtin::FloatCeil
        | Builtin::FloatRound
        | Builtin::FloatTruncate
        | Builtin::Panic
        | Builtin::Todo
        | Builtin::Dbg
        | Builtin::Inspect
        | Builtin::CellNew
        | Builtin::CellGet
        | Builtin::CellSet
        | Builtin::ShowString
        | Builtin::DebugPrint => None,
    }
}

pub(super) fn result_ownership(op: Builtin, args: &[Operand]) -> ResultOwnership {
    match op {
        Builtin::StringTrim => string_trim::result_ownership(),
        Builtin::StringAppend => string_append::result_ownership(args),
        Builtin::StringLength
        | Builtin::StringByteSize
        | Builtin::StringContains
        | Builtin::StringStartsWith
        | Builtin::StringEndsWith
        | Builtin::StringSplit
        | Builtin::StringChars
        | Builtin::StringJoin
        | Builtin::StringCodePoints
        | Builtin::StringFromCodePoint
        | Builtin::ShowNested
        | Builtin::IntToString
        | Builtin::IntParse
        | Builtin::IntCheckedAdd
        | Builtin::IntBitAnd
        | Builtin::IntBitOr
        | Builtin::IntBitXor
        | Builtin::IntBitNot
        | Builtin::IntShiftLeft
        | Builtin::IntShiftRight
        | Builtin::IntShiftRightUnsigned
        | Builtin::Int64FromInt
        | Builtin::Int64ToString
        | Builtin::Int64ToInt
        | Builtin::Int64BitAnd
        | Builtin::Int64BitOr
        | Builtin::Int64BitXor
        | Builtin::Int64BitNot
        | Builtin::Int64ShiftLeft
        | Builtin::Int64ShiftRight
        | Builtin::Int64ShiftRightUnsigned
        | Builtin::FloatToString
        | Builtin::FloatFromInt
        | Builtin::FloatParse
        | Builtin::FloatFloor
        | Builtin::FloatCeil
        | Builtin::FloatRound
        | Builtin::FloatTruncate
        | Builtin::Panic
        | Builtin::Todo
        | Builtin::Dbg
        | Builtin::Inspect
        | Builtin::CellNew
        | Builtin::CellGet
        | Builtin::CellSet
        | Builtin::ShowString
        | Builtin::DebugPrint => ResultOwnership::Owned,
    }
}

pub(super) fn require_runtime(
    op: Builtin,
    args: &[Operand],
    requirements: &mut runtime_plan::Requirements,
) {
    match op {
        Builtin::StringAppend => string_append::require_runtime(args, requirements),
        Builtin::IntToString => int_to_string::require_runtime(args, requirements),
        Builtin::Int64ToString => int64_to_string::require_runtime(args, requirements),
        Builtin::FloatToString => float_to_string::require_runtime(args, requirements),
        Builtin::IntParse => int_parse::require_runtime(args, requirements),
        Builtin::FloatParse => float_parse::require_runtime(args, requirements),
        Builtin::StringContains => string_contains::require_runtime(args, requirements),
        Builtin::StringStartsWith => string_starts_with::require_runtime(args, requirements),
        Builtin::StringEndsWith => string_ends_with::require_runtime(args, requirements),
        Builtin::StringTrim => string_trim::require_runtime(args, requirements),
        Builtin::StringJoin => string_join::require_runtime(args, requirements),
        Builtin::StringCodePoints => string_code_points::require_runtime(args, requirements),
        Builtin::StringLength => string_length::require_runtime(args, requirements),
        Builtin::StringChars => string_chars::require_runtime(args, requirements),
        Builtin::StringSplit => string_split::require_runtime(args, requirements),
        Builtin::StringFromCodePoint => string_from_code_point::require_runtime(args, requirements),
        Builtin::IntShiftLeft | Builtin::IntShiftRight | Builtin::IntShiftRightUnsigned => {
            integer::require_runtime(op, requirements)
        }
        Builtin::Int64ShiftLeft
        | Builtin::Int64ShiftRight
        | Builtin::Int64ShiftRightUnsigned
        | Builtin::Int64ToInt => integer64::require_runtime(op, requirements),
        Builtin::FloatFloor | Builtin::FloatCeil | Builtin::FloatRound | Builtin::FloatTruncate => {
            float::require_runtime(op, requirements)
        }
        Builtin::StringByteSize
        | Builtin::ShowNested
        | Builtin::IntCheckedAdd
        | Builtin::IntBitAnd
        | Builtin::IntBitOr
        | Builtin::IntBitXor
        | Builtin::IntBitNot
        | Builtin::Int64FromInt
        | Builtin::Int64BitAnd
        | Builtin::Int64BitOr
        | Builtin::Int64BitXor
        | Builtin::Int64BitNot
        | Builtin::FloatFromInt
        | Builtin::Panic
        | Builtin::Todo
        | Builtin::Dbg
        | Builtin::Inspect
        | Builtin::CellNew
        | Builtin::CellGet
        | Builtin::CellSet
        | Builtin::ShowString
        | Builtin::DebugPrint => {}
    }
}

pub(super) fn needs_heap(op: Builtin) -> bool {
    match op {
        Builtin::CellNew => cells::needs_heap(op),
        Builtin::StringLength
        | Builtin::StringByteSize
        | Builtin::StringAppend
        | Builtin::StringContains
        | Builtin::StringStartsWith
        | Builtin::StringEndsWith
        | Builtin::StringSplit
        | Builtin::StringChars
        | Builtin::StringJoin
        | Builtin::StringTrim
        | Builtin::StringCodePoints
        | Builtin::StringFromCodePoint
        | Builtin::ShowNested
        | Builtin::IntToString
        | Builtin::IntParse
        | Builtin::IntCheckedAdd
        | Builtin::IntBitAnd
        | Builtin::IntBitOr
        | Builtin::IntBitXor
        | Builtin::IntBitNot
        | Builtin::IntShiftLeft
        | Builtin::IntShiftRight
        | Builtin::IntShiftRightUnsigned
        | Builtin::Int64FromInt
        | Builtin::Int64ToString
        | Builtin::Int64ToInt
        | Builtin::Int64BitAnd
        | Builtin::Int64BitOr
        | Builtin::Int64BitXor
        | Builtin::Int64BitNot
        | Builtin::Int64ShiftLeft
        | Builtin::Int64ShiftRight
        | Builtin::Int64ShiftRightUnsigned
        | Builtin::FloatToString
        | Builtin::FloatFromInt
        | Builtin::FloatParse
        | Builtin::FloatFloor
        | Builtin::FloatCeil
        | Builtin::FloatRound
        | Builtin::FloatTruncate
        | Builtin::Panic
        | Builtin::Todo
        | Builtin::Dbg
        | Builtin::Inspect
        | Builtin::CellGet
        | Builtin::CellSet
        | Builtin::ShowString
        | Builtin::DebugPrint => false,
    }
}
