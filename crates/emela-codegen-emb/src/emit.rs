use std::collections::{HashMap, HashSet};

use emela_core::{Builtin, CtorRef, EnumId, Type, TypeId, TypeTable};
use emela_lir::{
    Callee, CaseKey, Const, FnId, Function, LabelId, OnFail, Operand, PrimOp, PrimTy, Repr,
    RuntimeOp, SeqId, Stmt, StmtId, SwitchOn, Term, Value, VarId,
};

use crate::const_eval::{self, Outcome as ConstOutcome};
use crate::writer::Writer;
use crate::{Error, ErrorKind, GeneratedFile, Options, Output};

const HEADER_FILE: &str = "emela_embedded.h";
const SOURCE_FILE: &str = "emela_embedded.c";
const BOARD_HEADER_FILE: &str = "emela_board.h";

pub(crate) fn emit(flow: &emela_lir::Module, options: &Options) -> Result<Output, Vec<Error>> {
    if !(1..=512).contains(&options.string_capacity()) {
        return Err(vec![Error {
            kind: ErrorKind::InvalidOptions,
            message: format!(
                "string capacity must be between 1 and 512 bytes, found {}",
                options.string_capacity()
            ),
        }]);
    }
    let emitted_functions = runtime_reachable(flow);
    let layouts = TypeLayouts::new(flow, &emitted_functions)?;
    let heap_required = flow.functions.iter().any(|(id, function)| {
        emitted_functions.contains(&id) && function_needs_list_heap(function, &layouts)
    });
    if options.heap_bytes() == 0 && heap_required {
        let (_, function) = flow
            .functions
            .iter()
            .find(|(id, function)| {
                emitted_functions.contains(id) && function_needs_list_heap(function, &layouts)
            })
            .expect("heap requirement came from an emitted function");
        return Err(unsupported(
            &function.name,
            "heap-backed List operations with heap_bytes = 0; configure Options::with_heap_bytes",
        ));
    }
    let mut const_outcomes = HashMap::new();
    for (id, function) in flow
        .functions
        .iter()
        .filter(|(id, function)| emitted_functions.contains(id) && function.constant)
    {
        match const_eval::evaluate(flow, id) {
            Ok(outcome) => {
                const_outcomes.insert(id, outcome);
            }
            Err(reason) => {
                let caller = flow.functions.iter().find_map(|(caller_id, caller)| {
                    (emitted_functions.contains(&caller_id)
                        && !caller.constant
                        && directly_calls(caller, id))
                    .then_some(caller)
                });
                let location = caller.map_or(function.name.as_str(), |caller| caller.name.as_str());
                return Err(unsupported(
                    location,
                    &format!(
                        "const `{}` cannot be evaluated statically: {reason}",
                        function.name
                    ),
                ));
            }
        }
    }

    let mut names = HashMap::new();
    let mut public_names = HashMap::new();
    for (id, function) in flow
        .functions
        .iter()
        .filter(|(id, _)| emitted_functions.contains(id))
    {
        let name = if function.external.is_some() {
            if function.exported {
                return Err(unsupported(&function.name, "exported external functions"));
            }
            validate_external(flow, function)?
        } else {
            function_name(id, function)
        };
        public_names.insert(id, name.clone());
        let implementation_name = if function.exported && function.fails {
            format!("{name}_impl")
        } else {
            name
        };
        names.insert(id, implementation_name);
    }
    validate_isr_entries(flow, &emitted_functions, &layouts, &names, options)?;
    let main_entry = flow.functions.iter().find(|(id, function)| {
        emitted_functions.contains(id) && function.exported && function.name == "main"
    });
    if let Some((_, function)) = main_entry
        && !function.params.is_empty()
    {
        return Err(unsupported(
            &function.name,
            "an embedded main function with parameters",
        ));
    }

    let mut header = Writer::new();
    header.line("#ifndef EMELA_EMBEDDED_H");
    header.line("#define EMELA_EMBEDDED_H");
    header.blank();
    header.line("#include <stdint.h>");
    header.blank();
    layouts.emit_string_declaration(&mut header);
    layouts.emit_declarations(&mut header);
    layouts.emit_error_declaration(&mut header);
    if !layouts.aggregates.is_empty() {
        header.blank();
    }

    for (id, function) in flow
        .functions
        .iter()
        .filter(|(id, f)| emitted_functions.contains(id) && f.exported)
    {
        if function.suspends {
            return Err(unsupported(
                &function.name,
                "suspending function signatures",
            ));
        }
        let name = &public_names[&id];
        header.line(&format!(
            "{};",
            signature(&layouts, function, name, false, false)
        ));
    }
    if main_entry.is_some() {
        header.line("void emela_embedded_start(void);");
    }
    if heap_required {
        header.line("/* Retains an owned List value before copying it. */");
        header.line("void emela_embedded_retain(void *value);");
        header.line("/* Releases an owned List value returned by this module. */");
        header.line("void emela_embedded_release(void *value);");
        layouts.emit_public_release_declarations(&mut header);
    }

    header.blank();
    header.line("#endif");

    let mut board_header = Writer::new();
    board_header.line("#ifndef EMELA_BOARD_H");
    board_header.line("#define EMELA_BOARD_H");
    board_header.blank();
    board_header.line("#include <stdint.h>");
    board_header.blank();
    board_header.line("typedef enum {");
    for (index, kind) in [
        "PANIC",
        "TODO",
        "DIVISION_BY_ZERO",
        "NO_MATCH",
        "FLOAT_TO_INT",
        "NEGATIVE_SHIFT",
        "HEAP_EXHAUSTED",
        "STRING_CAPACITY",
        "UNSUPPORTED_UNICODE",
        "UNHANDLED_ERROR",
        "ASSERT",
    ]
    .iter()
    .enumerate()
    {
        let suffix = if index == 10 { "" } else { "," };
        board_header.line(&format!("  EMELA_DEFECT_{kind}{suffix}"));
    }
    board_header.line("} EmelaDefectKind;");
    board_header.blank();
    let needs_defect_hook = heap_required
        || has_prim(
            flow,
            &emitted_functions,
            PrimTy::I32,
            &[PrimOp::Div, PrimOp::Rem],
        )
        || has_prim(
            flow,
            &emitted_functions,
            PrimTy::I64,
            &[PrimOp::Div, PrimOp::Rem],
        )
        || const_outcomes.values().any(|outcome| {
            matches!(
                outcome,
                ConstOutcome::Defect { .. } | ConstOutcome::SelfReference { .. }
            )
        })
        || has_runtime_string_append(flow, &emitted_functions)
        || has_runtime_string_show(flow, &emitted_functions)
        || has_runtime_int_string(flow, &emitted_functions)
        || has_runtime_string_from_code_point(flow, &emitted_functions)
        || has_builtin(
            flow,
            &emitted_functions,
            &[
                Builtin::FloatFloor,
                Builtin::FloatCeil,
                Builtin::FloatRound,
                Builtin::FloatTruncate,
            ],
        )
        || has_builtin(
            flow,
            &emitted_functions,
            &[
                Builtin::IntShiftLeft,
                Builtin::IntShiftRight,
                Builtin::IntShiftRightUnsigned,
                Builtin::Int64ShiftLeft,
                Builtin::Int64ShiftRight,
                Builtin::Int64ShiftRightUnsigned,
            ],
        )
        || flow.functions.iter().any(|(id, function)| {
            if !emitted_functions.contains(&id) {
                return false;
            }
            if function.exported && function.fails {
                return true;
            }
            !function.constant
                && (function
                    .seqs
                    .iter()
                    .any(|(_, sequence)| matches!(&sequence.term, Term::Defect(_)))
                    || function.stmts.iter().any(|(_, statement)| {
                        matches!(
                            statement,
                            Stmt::Switch {
                                on: SwitchOn::Tag(_),
                                default: None,
                                ..
                            }
                        )
                    }))
        });
    if needs_defect_hook {
        board_header.line(
            "_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length);",
        );
    }
    if main_entry.is_some() {
        board_header.line("void emela_board_init(void);");
        board_header.line("void emela_board_idle(void);");
    }
    if has_runtime_op(flow, &emitted_functions, RuntimeOp::DebugPrint) {
        board_header.line("void emela_board_debug(const char *bytes, uint16_t length);");
    }
    board_header.blank();
    board_header.line("#endif");

    let mut source = Writer::new();
    source.line(&format!("#include \"{HEADER_FILE}\""));
    source.line(&format!("#include \"{BOARD_HEADER_FILE}\""));
    source.line("#include <math.h>");
    source.line("#include <stddef.h>");
    source.blank();
    if heap_required {
        layouts.emit_list_node_declarations(&mut source);
        crate::heap::emit_runtime(&mut source, options.heap_bytes());
        layouts.emit_heap_ownership_helpers(&mut source);
        layouts.emit_heap_destroy_switch(&mut source);
        layouts.emit_public_release_helpers(&mut source);
        crate::heap::emit_public_retain(&mut source);
        crate::heap::emit_public_release(&mut source);
    } else {
        source.line("static inline void emela_heap_retain(void *value) { (void)value; }");
        source.line("static inline void emela_heap_release(void *value) { (void)value; }");
        source.blank();
    }
    let has_int_shift = has_builtin(
        flow,
        &emitted_functions,
        &[
            Builtin::IntShiftLeft,
            Builtin::IntShiftRight,
            Builtin::IntShiftRightUnsigned,
        ],
    );
    let has_i64_shift = has_builtin(
        flow,
        &emitted_functions,
        &[
            Builtin::Int64ShiftLeft,
            Builtin::Int64ShiftRight,
            Builtin::Int64ShiftRightUnsigned,
        ],
    );
    let has_shift = has_int_shift || has_i64_shift;
    if has_prim(
        flow,
        &emitted_functions,
        PrimTy::I32,
        &[PrimOp::Add, PrimOp::Sub, PrimOp::Mul, PrimOp::Neg],
    ) || has_int_shift
        || has_builtin(flow, &emitted_functions, &[Builtin::Int64ToInt])
    {
        source.begin_block("static inline int32_t emela_i32_from_bits(uint32_t bits)");
        source.begin_block("if (bits <= (uint32_t)INT32_MAX)");
        source.line("return (int32_t)bits;");
        source.end_block();
        source.line("return -INT32_C(1) - (int32_t)(UINT32_MAX - bits);");
        source.end_block();
        source.blank();
    }
    if has_int_shift {
        emit_i32_shift_helpers(&mut source);
    }
    if has_shift {
        emit_negative_shift_helper(&mut source);
    }
    if has_prim(
        flow,
        &emitted_functions,
        PrimTy::I32,
        &[PrimOp::Div, PrimOp::Rem],
    ) {
        emit_i32_division_helpers(&mut source);
    }
    if has_prim(
        flow,
        &emitted_functions,
        PrimTy::I64,
        &[PrimOp::Add, PrimOp::Sub, PrimOp::Mul, PrimOp::Neg],
    ) || has_i64_shift
    {
        emit_i64_wrap_helper(&mut source);
    }
    if has_i64_shift {
        emit_i64_shift_helpers(&mut source);
    }
    if has_prim(
        flow,
        &emitted_functions,
        PrimTy::I64,
        &[PrimOp::Div, PrimOp::Rem],
    ) {
        emit_i64_division_helpers(&mut source);
    }
    if has_builtin(
        flow,
        &emitted_functions,
        &[
            Builtin::FloatFloor,
            Builtin::FloatCeil,
            Builtin::FloatRound,
            Builtin::FloatTruncate,
        ],
    ) {
        emit_float_to_int_helper(&mut source);
    }
    if has_dynamic_defect(flow, &emitted_functions) {
        emit_dynamic_defect_kind_helper(&mut source);
    }
    if has_builtin(
        flow,
        &emitted_functions,
        &[
            Builtin::StringContains,
            Builtin::StringStartsWith,
            Builtin::StringEndsWith,
        ],
    ) {
        emit_string_matches_at_helper(&mut source);
    }
    if has_builtin(flow, &emitted_functions, &[Builtin::StringContains]) {
        emit_string_contains_helper(&mut source);
    }
    if has_builtin(flow, &emitted_functions, &[Builtin::StringStartsWith]) {
        emit_string_starts_with_helper(&mut source);
    }
    if has_builtin(flow, &emitted_functions, &[Builtin::StringEndsWith]) {
        emit_string_ends_with_helper(&mut source);
    }
    if has_builtin(flow, &emitted_functions, &[Builtin::StringTrim]) {
        emit_string_trim_helper(&mut source);
    }
    if has_runtime_string_append(flow, &emitted_functions) {
        emit_string_append_helper(&mut source);
    }
    if has_runtime_string_show(flow, &emitted_functions) {
        emit_string_show_helpers(&mut source);
    }
    if has_runtime_int_string(flow, &emitted_functions) {
        emit_int_to_string_helper(&mut source);
    }
    if has_runtime_int_parse(flow, &emitted_functions) {
        emit_int_parse_helper(&mut source);
    }

    for (id, function) in flow
        .functions
        .iter()
        .filter(|(id, _)| emitted_functions.contains(id))
    {
        let name = &names[&id];
        if function.external.is_some() {
            source.line(&format!(
                "extern {};",
                signature(&layouts, function, name, false, false)
            ));
        } else if !function.exported || function.fails {
            source.line(&format!(
                "{};",
                signature(&layouts, function, name, true, function.fails)
            ));
        }
    }
    source.blank();

    for (id, function) in flow
        .functions
        .iter()
        .filter(|(id, _)| emitted_functions.contains(id))
    {
        if function.suspends {
            return Err(unsupported(&function.name, "suspending function bodies"));
        }

        if function.external.is_some() {
            if function.body.is_some() {
                return Err(vec![Error {
                    kind: ErrorKind::InvalidFlowIr,
                    message: format!(
                        "invalid Flow IR in function `{}`: external function has a body",
                        function.name
                    ),
                }]);
            }
            continue;
        }
        let Some(body) = function.body else {
            return Err(unsupported(&function.name, "function without a body"));
        };
        let sequence = &function.seqs[body];
        let name = &names[&id];
        let is_static = !function.exported || function.fails;
        source.begin_block(&signature(
            &layouts,
            function,
            name,
            is_static,
            function.fails,
        ));
        if function.constant {
            match &const_outcomes[&id] {
                ConstOutcome::Value(emela_lir::Const::Unit) => source.ret(None),
                ConstOutcome::Value(value) => {
                    let expression = constant_expression(&value).map_err(|kind| {
                        unsupported(&function.name, &format!("{kind} in a const value"))
                    })?;
                    let Some(expression) = expression else {
                        return Err(invalid_flow_ir(
                            &function.name,
                            "non-Unit const value has no C expression",
                        ));
                    };
                    source.ret(Some(&expression));
                }
                ConstOutcome::Defect { kind, message } => {
                    let length = u16::try_from(message.len()).map_err(|_| {
                        unsupported(
                            &function.name,
                            "a const defect message longer than the embedded C ABI supports",
                        )
                    })?;
                    source.line(&format!(
                        "emela_board_defect(EMELA_DEFECT_{kind}, {}, UINT16_C({length}));",
                        c_string_literal(message.as_bytes())
                    ));
                }
                ConstOutcome::SelfReference { message } => {
                    let length = u16::try_from(message.len()).map_err(|_| {
                        unsupported(
                            &function.name,
                            "a const defect message longer than the embedded C ABI supports",
                        )
                    })?;
                    source.line(&format!(
                        "emela_board_defect(EMELA_DEFECT_PANIC, {}, UINT16_C({length}));",
                        c_string_literal(message.as_bytes())
                    ));
                }
            }
            source.end_block();
            source.blank();
            continue;
        }
        if function.fails {
            source.line("*emela_error_out = (EmelaError){0};");
        }
        let string_buffers = StringBufferPlan::new(&layouts, function, options.string_capacity());
        if !string_buffers.declarations.is_empty()
            && (type_contains_string(&layouts, function.ret, &mut HashSet::new())
                || failure_value_contains_string(&layouts, function))
        {
            return Err(unsupported(
                &function.name,
                "a String scratch buffer that could outlive its function stack frame",
            ));
        }
        emit_local_declarations(&layouts, function, &string_buffers, &mut source);
        let mut loops = Vec::new();
        let mut blocks = Vec::new();
        let mut protects = Vec::new();
        let mut next_temp = 0;
        let mut next_label = 0;
        for &statement in &sequence.stmts {
            emit_statement(
                &layouts,
                function,
                &function.stmts[statement],
                &mut source,
                &names,
                &string_buffers,
                &mut loops,
                &mut blocks,
                &mut protects,
                &mut next_temp,
                &mut next_label,
            )?;
        }
        emit_return(&layouts, function, &sequence.term, &mut source)?;
        source.end_block();
        source.blank();
    }
    for (id, function) in flow
        .functions
        .iter()
        .filter(|(id, f)| emitted_functions.contains(id) && f.exported && f.fails)
    {
        emit_fallible_wrapper(
            &layouts,
            function,
            &public_names[&id],
            &names[&id],
            &mut source,
        );
        source.blank();
    }
    if let Some((id, _function)) = main_entry {
        let main_name = &public_names[&id];
        source.begin_block("void emela_embedded_start(void)");
        source.line("emela_board_init();");
        source.line(&format!("(void){main_name}();"));
        source.begin_block("for (;;)");
        source.line("emela_board_idle();");
        source.end_block();
        source.end_block();
        source.blank();
    }

    Ok(Output {
        files: vec![
            GeneratedFile {
                path: HEADER_FILE.to_owned(),
                contents: header.finish(),
            },
            GeneratedFile {
                path: BOARD_HEADER_FILE.to_owned(),
                contents: board_header.finish(),
            },
            GeneratedFile {
                path: SOURCE_FILE.to_owned(),
                contents: source.finish(),
            },
        ],
    })
}

fn emit_local_declarations(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    string_buffers: &StringBufferPlan,
    writer: &mut Writer,
) {
    for (id, variable) in function.vars.iter() {
        if !function.params.contains(&id) {
            if layouts.is_managed_value(variable.ty) {
                let initial = if layouts.is_managed_list(variable.ty) {
                    "NULL"
                } else {
                    "{0}"
                };
                writer.decl(
                    &layouts.c_type(variable.ty),
                    &variable_name(id),
                    Some(initial),
                );
                writer.decl("int", &owned_variable_name(id), Some("INT32_C(0)"));
            } else {
                writer.decl(&layouts.c_type(variable.ty), &variable_name(id), None);
            }
        }
    }
    for (name, capacity) in &string_buffers.declarations {
        writer.line(&format!("char {name}[{capacity}];"));
    }
}

fn emit_release_if_owned(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    variable: VarId,
    writer: &mut Writer,
) {
    let ty = function.vars[variable].ty;
    if !layouts.is_managed_value(ty) {
        return;
    }
    writer.begin_block(&format!(
        "if ({} != INT32_C(0))",
        owned_variable_name(variable)
    ));
    layouts.emit_release_value_expression(ty, &variable_name(variable), writer);
    writer.end_block();
    writer.assign(&owned_variable_name(variable), "INT32_C(0)");
}

fn emit_retain_operand(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    operand: &Operand,
    writer: &mut Writer,
) {
    if let Operand::Var(variable) = operand
        && layouts.is_managed_value(function.vars[*variable].ty)
    {
        layouts.emit_retain_value_expression(
            function.vars[*variable].ty,
            &variable_name(*variable),
            writer,
        );
    }
}

fn emit_owned_cleanup(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    except: Option<VarId>,
    writer: &mut Writer,
) {
    for (variable, data) in function.vars.iter() {
        if except == Some(variable)
            || function.params.contains(&variable)
            || !layouts.is_managed_value(data.ty)
        {
            continue;
        }
        emit_release_if_owned(layouts, function, variable, writer);
    }
}

struct StringBufferPlan {
    buffers: HashMap<VarId, (usize, u16)>,
    declarations: Vec<(String, usize)>,
}

impl StringBufferPlan {
    fn new(layouts: &TypeLayouts<'_>, function: &Function, capacity: u16) -> Self {
        let mut append_args = HashMap::new();
        let mut buffer_producers = HashSet::new();
        let mut producer_positions = HashMap::new();
        for (statement_id, statement) in function.stmts.iter() {
            let Stmt::Let {
                dst: Some(dst),
                value,
                ..
            } = statement
            else {
                continue;
            };
            match value {
                Value::Builtin {
                    op: Builtin::StringAppend,
                    args,
                } if args.len() == 2
                    && !append_is_static(args)
                    && !append_has_empty_constant(args)
                    && matches!(layouts.types[function.vars[*dst].ty], Type::String) =>
                {
                    append_args.insert(*dst, args.clone());
                    buffer_producers.insert(*dst);
                    producer_positions.insert(*dst, statement_id);
                }
                Value::Runtime {
                    op: RuntimeOp::ShowString,
                    args,
                } if args.len() == 1
                    && !matches!(args[0], Operand::Const(Const::String(_)))
                    && matches!(layouts.types[function.vars[*dst].ty], Type::String) =>
                {
                    buffer_producers.insert(*dst);
                    producer_positions.insert(*dst, statement_id);
                }
                Value::Builtin {
                    op: Builtin::IntToString | Builtin::Int64ToString,
                    args,
                } if args.len() == 1
                    && matches!(args[0], Operand::Var(_))
                    && matches!(layouts.types[function.vars[*dst].ty], Type::String) =>
                {
                    buffer_producers.insert(*dst);
                    producer_positions.insert(*dst, statement_id);
                }
                Value::Builtin {
                    op: Builtin::StringFromCodePoint,
                    args,
                } if args.len() == 1
                    && matches!(args[0], Operand::Var(_))
                    && option_payload_type(layouts, function.vars[*dst].ty)
                        .is_some_and(|ty| matches!(layouts.types[ty], Type::String)) =>
                {
                    buffer_producers.insert(*dst);
                    producer_positions.insert(*dst, statement_id);
                }
                _ => {}
            }
        }

        let mut uses = HashMap::new();
        let mut count = |variable| *uses.entry(variable).or_insert(0usize) += 1;
        for (_, statement) in function.stmts.iter() {
            match statement {
                Stmt::Let { value, .. } => value.for_each_var(&mut count),
                Stmt::If { cond, .. } => count_operand(cond, &mut count),
                Stmt::Switch { on, .. } => match on {
                    SwitchOn::Tag(variable) | SwitchOn::Lit(variable) => count(*variable),
                },
                Stmt::Loop { inits, .. } => {
                    for operand in inits {
                        count_operand(operand, &mut count);
                    }
                }
                Stmt::Dup(variable) | Stmt::Drop(variable) => count(*variable),
                Stmt::Block { .. } | Stmt::Protect { .. } => {}
            }
        }
        for (_, sequence) in function.seqs.iter() {
            match &sequence.term {
                Term::Yield(Some(operand))
                | Term::Return(Some(operand))
                | Term::Fail(operand)
                | Term::Defect(operand) => count_operand(operand, &mut count),
                Term::Yield(None) | Term::Return(None) => {}
                Term::Break { value, .. } => {
                    if let Some(value) = value {
                        count_operand(value, &mut count);
                    }
                }
                Term::Recur(args) => {
                    for operand in args {
                        count_operand(operand, &mut count);
                    }
                }
                Term::Unreachable | Term::Unwind => {}
            }
        }
        drop(count);

        let (statement_positions, sequence_positions) = function_positions(function);
        let mut last_uses = HashMap::new();
        let mut non_reusable_variables = HashSet::new();
        for (statement_id, statement) in function.stmts.iter() {
            let position = statement_positions.get(&statement_id).copied().unwrap_or(0);
            let mut mark_use = |variable| {
                last_uses
                    .entry(variable)
                    .and_modify(|last: &mut usize| *last = (*last).max(position))
                    .or_insert(position);
            };
            match statement {
                Stmt::Let { value, .. } => {
                    value.for_each_var(&mut mark_use);
                    let reusable_consumer = match value {
                        Value::Builtin {
                            op:
                                Builtin::StringByteSize
                                | Builtin::StringContains
                                | Builtin::StringStartsWith
                                | Builtin::StringEndsWith
                                | Builtin::StringAppend,
                            args,
                        } => !append_has_empty_constant(args),
                        Value::Runtime {
                            op: RuntimeOp::ShowString | RuntimeOp::DebugPrint,
                            ..
                        } => true,
                        _ => false,
                    };
                    if !reusable_consumer {
                        value.for_each_var(|variable| {
                            non_reusable_variables.insert(variable);
                        });
                    }
                    if let Value::Builtin {
                        op: Builtin::StringAppend,
                        args,
                    } = value
                        && append_has_empty_constant(args)
                    {
                        value.for_each_var(|variable| {
                            non_reusable_variables.insert(variable);
                        });
                    }
                }
                Stmt::If { cond, .. } => {
                    count_operand(cond, &mut mark_use);
                    if let Operand::Var(variable) = cond {
                        non_reusable_variables.insert(*variable);
                    }
                }
                Stmt::Switch { on, .. } => {
                    let variable = match on {
                        SwitchOn::Tag(variable) | SwitchOn::Lit(variable) => *variable,
                    };
                    mark_use(variable);
                    non_reusable_variables.insert(variable);
                }
                Stmt::Loop { inits, .. } => {
                    for operand in inits {
                        count_operand(operand, &mut mark_use);
                        if let Operand::Var(variable) = operand {
                            non_reusable_variables.insert(*variable);
                        }
                    }
                }
                Stmt::Dup(variable) | Stmt::Drop(variable) => {
                    mark_use(*variable);
                    non_reusable_variables.insert(*variable);
                }
                Stmt::Block { .. } | Stmt::Protect { .. } => {}
            }
        }
        for (sequence_id, sequence) in function.seqs.iter() {
            let position = sequence_positions.get(&sequence_id).copied().unwrap_or(0);
            let mut mark_use = |variable| {
                last_uses
                    .entry(variable)
                    .and_modify(|last: &mut usize| *last = (*last).max(position))
                    .or_insert(position);
                non_reusable_variables.insert(variable);
            };
            match &sequence.term {
                Term::Yield(Some(operand))
                | Term::Return(Some(operand))
                | Term::Fail(operand)
                | Term::Defect(operand) => count_operand(operand, &mut mark_use),
                Term::Yield(None) | Term::Return(None) => {}
                Term::Break { value, .. } => {
                    if let Some(value) = value {
                        count_operand(value, &mut mark_use);
                    }
                }
                Term::Recur(args) => {
                    for operand in args {
                        count_operand(operand, &mut mark_use);
                    }
                }
                Term::Unreachable | Term::Unwind => {}
            }
        }

        let mut bounds: HashMap<VarId, usize> = function
            .vars
            .iter()
            .filter_map(|(id, variable)| {
                matches!(layouts.types[variable.ty], Type::String)
                    .then_some((id, usize::from(capacity)))
            })
            .collect();
        for _ in 0..=function.vars.len() {
            let mut changed = false;
            for (_, statement) in function.stmts.iter() {
                let Stmt::Let {
                    dst: Some(dst),
                    value,
                    ..
                } = statement
                else {
                    continue;
                };
                let is_string = matches!(layouts.types[function.vars[*dst].ty], Type::String);
                let is_codepoint_option = matches!(
                    value,
                    Value::Builtin {
                        op: Builtin::StringFromCodePoint,
                        ..
                    }
                ) && option_payload_type(layouts, function.vars[*dst].ty)
                    .is_some_and(|ty| matches!(layouts.types[ty], Type::String));
                if !is_string && !is_codepoint_option {
                    continue;
                }
                let bound = match value {
                    Value::Const(Const::String(value)) => value.len(),
                    Value::Builtin {
                        op: Builtin::StringTrim,
                        args,
                    } if args.len() == 1 => operand_string_bound(&args[0], &bounds, capacity),
                    Value::Builtin {
                        op: Builtin::StringAppend,
                        args,
                    } if args.len() == 2 => {
                        let left = operand_string_bound(&args[0], &bounds, capacity);
                        let right = operand_string_bound(&args[1], &bounds, capacity);
                        if append_is_static(args) {
                            left.saturating_add(right)
                        } else if append_has_empty_constant(args) {
                            left.max(right)
                        } else {
                            left.saturating_add(right).min(usize::from(capacity))
                        }
                    }
                    Value::Builtin {
                        op: op @ (Builtin::IntToString | Builtin::Int64ToString),
                        args,
                    } if args.len() == 1 => match (&args[0], op) {
                        (Operand::Const(Const::Int(value)), _) => value.to_string().len(),
                        (Operand::Const(Const::Int64(value)), _) => value.to_string().len(),
                        (_, Builtin::IntToString) => 11.min(usize::from(capacity)),
                        (_, Builtin::Int64ToString) => 20.min(usize::from(capacity)),
                        _ => unreachable!(),
                    },
                    Value::Builtin {
                        op: Builtin::FloatToString,
                        args,
                    } if args.len() == 1 => match &args[0] {
                        Operand::Const(Const::Float(value)) => {
                            const_eval::format_float(*value).len()
                        }
                        _ => 24.min(usize::from(capacity)),
                    },
                    Value::Builtin {
                        op: Builtin::StringFromCodePoint,
                        ..
                    } => 4.min(usize::from(capacity)),
                    _ => usize::from(capacity),
                };
                let bound = match value {
                    Value::Runtime {
                        op: RuntimeOp::ShowString,
                        args,
                    } if args.len() == 1 => match &args[0] {
                        Operand::Const(Const::String(value)) => show_string(value).len(),
                        operand => operand_string_bound(operand, &bounds, capacity)
                            .saturating_mul(6)
                            .saturating_add(2)
                            .min(usize::from(capacity)),
                    },
                    _ => bound,
                };
                if bounds.get(dst).copied() != Some(bound) {
                    bounds.insert(*dst, bound);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }

        let mut roots: HashMap<VarId, VarId> =
            buffer_producers.iter().map(|&id| (id, id)).collect();
        for _ in 0..=append_args.len() {
            let mut changed = false;
            for (&dst, args) in &append_args {
                let Some(Operand::Var(left)) = args.first() else {
                    continue;
                };
                if uses.get(left).copied() != Some(1) || !roots.contains_key(left) {
                    continue;
                }
                let root = roots[left];
                if roots.get(&dst).copied() != Some(root) {
                    roots.insert(dst, root);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }

        let mut root_capacities: HashMap<VarId, usize> = HashMap::new();
        let mut root_intervals: HashMap<VarId, (usize, usize)> = HashMap::new();
        let mut non_reusable_roots = HashSet::new();
        for (&variable, &root) in &roots {
            let bound = bounds
                .get(&variable)
                .copied()
                .unwrap_or(usize::from(capacity))
                .min(usize::from(capacity));
            root_capacities
                .entry(root)
                .and_modify(|current| *current = (*current).max(bound))
                .or_insert(bound);
            let definition = producer_positions
                .get(&variable)
                .and_then(|statement| statement_positions.get(statement))
                .copied()
                .unwrap_or(0);
            let end = last_uses.get(&variable).copied().unwrap_or(definition);
            root_intervals
                .entry(root)
                .and_modify(|(start, last)| {
                    *start = (*start).min(definition);
                    *last = (*last).max(end);
                })
                .or_insert((definition, end));
            if non_reusable_variables.contains(&variable) {
                non_reusable_roots.insert(root);
            }
        }
        let mut groups: Vec<_> = root_capacities
            .iter()
            .map(|(&root, &size)| {
                let (start, end) = root_intervals.get(&root).copied().unwrap_or((0, 0));
                (root, start, end, size, !non_reusable_roots.contains(&root))
            })
            .collect();
        groups.sort_by_key(|(root, start, _, _, _)| (*start, format!("{root:?}")));

        let mut slot_ends: Vec<Option<usize>> = Vec::new();
        let mut slot_sizes: Vec<usize> = Vec::new();
        let mut root_slots = HashMap::new();
        for (root, start, end, size, reusable) in groups {
            let slot = reusable
                .then(|| {
                    slot_ends
                        .iter()
                        .position(|last| last.is_some_and(|last| last < start))
                })
                .flatten();
            let slot = if let Some(slot) = slot {
                slot_ends[slot] = Some(end);
                slot_sizes[slot] = slot_sizes[slot].max(size.max(1));
                slot
            } else {
                let slot = slot_ends.len();
                slot_ends.push(reusable.then_some(end));
                slot_sizes.push(size.max(1));
                slot
            };
            root_slots.insert(root, slot);
        }
        let declarations = slot_sizes
            .iter()
            .enumerate()
            .map(|(slot, size)| (string_buffer_name(slot), *size))
            .collect();
        let buffers = roots
            .into_iter()
            .map(|(variable, root)| {
                let buffer_capacity = root_capacities[&root] as u16;
                (variable, (root_slots[&root], buffer_capacity))
            })
            .collect();
        Self {
            buffers,
            declarations,
        }
    }

    fn buffer(&self, variable: VarId) -> Option<(String, u16)> {
        self.buffers
            .get(&variable)
            .map(|(slot, capacity)| (string_buffer_name(*slot), *capacity))
    }
}

fn function_positions(function: &Function) -> (HashMap<StmtId, usize>, HashMap<SeqId, usize>) {
    fn visit(
        function: &Function,
        sequence: SeqId,
        next: &mut usize,
        visited: &mut HashSet<SeqId>,
        statement_positions: &mut HashMap<StmtId, usize>,
        sequence_positions: &mut HashMap<SeqId, usize>,
    ) {
        if !visited.insert(sequence) {
            return;
        }
        for &statement_id in &function.seqs[sequence].stmts {
            *next += 1;
            statement_positions.insert(statement_id, *next);
            match &function.stmts[statement_id] {
                Stmt::If { then, else_, .. } => {
                    visit(
                        function,
                        *then,
                        next,
                        visited,
                        statement_positions,
                        sequence_positions,
                    );
                    visit(
                        function,
                        *else_,
                        next,
                        visited,
                        statement_positions,
                        sequence_positions,
                    );
                }
                Stmt::Switch { cases, default, .. } => {
                    for case in cases {
                        visit(
                            function,
                            case.body,
                            next,
                            visited,
                            statement_positions,
                            sequence_positions,
                        );
                    }
                    if let Some(default) = default {
                        visit(
                            function,
                            *default,
                            next,
                            visited,
                            statement_positions,
                            sequence_positions,
                        );
                    }
                }
                Stmt::Loop { body, .. } | Stmt::Block { body, .. } => visit(
                    function,
                    *body,
                    next,
                    visited,
                    statement_positions,
                    sequence_positions,
                ),
                Stmt::Protect { body, cleanup, .. } => {
                    visit(
                        function,
                        *body,
                        next,
                        visited,
                        statement_positions,
                        sequence_positions,
                    );
                    visit(
                        function,
                        *cleanup,
                        next,
                        visited,
                        statement_positions,
                        sequence_positions,
                    );
                }
                Stmt::Let { .. } | Stmt::Dup(_) | Stmt::Drop(_) => {}
            }
        }
        *next += 1;
        sequence_positions.insert(sequence, *next);
    }

    let mut statement_positions = HashMap::new();
    let mut sequence_positions = HashMap::new();
    let mut visited = HashSet::new();
    let mut next = 0;
    if let Some(body) = function.body {
        visit(
            function,
            body,
            &mut next,
            &mut visited,
            &mut statement_positions,
            &mut sequence_positions,
        );
    }
    (statement_positions, sequence_positions)
}

fn count_operand(operand: &Operand, count: &mut impl FnMut(VarId)) {
    if let Operand::Var(variable) = operand {
        count(*variable);
    }
}

fn operand_string_bound(operand: &Operand, bounds: &HashMap<VarId, usize>, fallback: u16) -> usize {
    match operand {
        Operand::Const(Const::String(value)) => value.len(),
        Operand::Var(variable) => bounds
            .get(variable)
            .copied()
            .unwrap_or(usize::from(fallback)),
        _ => usize::from(fallback),
    }
}

fn append_is_static(args: &[Operand]) -> bool {
    matches!(
        args,
        [
            Operand::Const(Const::String(_)),
            Operand::Const(Const::String(_))
        ]
    )
}

fn append_has_empty_constant(args: &[Operand]) -> bool {
    matches!(args.first(), Some(Operand::Const(Const::String(value))) if value.is_empty())
        || matches!(args.get(1), Some(Operand::Const(Const::String(value))) if value.is_empty())
}

fn string_buffer_name(slot: usize) -> String {
    format!("emela_string_buffer_{slot}")
}

fn type_contains_string(layouts: &TypeLayouts<'_>, ty: TypeId, seen: &mut HashSet<TypeId>) -> bool {
    let mut types = layouts.types.clone();
    type_contains_string_with_types(layouts, &mut types, ty, seen)
}

fn failure_value_contains_string(layouts: &TypeLayouts<'_>, function: &Function) -> bool {
    function.seqs.iter().any(|(_, sequence)| {
        let Term::Fail(operand) = &sequence.term else {
            return false;
        };
        match operand {
            Operand::Var(variable) => {
                type_contains_string(layouts, function.vars[*variable].ty, &mut HashSet::new())
            }
            Operand::Const(Const::String(_)) => true,
            Operand::Const(_) => false,
        }
    })
}

fn type_contains_string_with_types(
    layouts: &TypeLayouts<'_>,
    types: &mut TypeTable,
    ty: TypeId,
    seen: &mut HashSet<TypeId>,
) -> bool {
    if !seen.insert(ty) {
        return false;
    }
    match types[ty].clone() {
        Type::String | Type::Any => true,
        Type::Tuple(fields) => fields
            .into_iter()
            .any(|field| type_contains_string_with_types(layouts, types, field, seen)),
        Type::List(element) | Type::Cell(element) => {
            type_contains_string_with_types(layouts, types, element, seen)
        }
        Type::Enum(enum_id, args) => layouts.core.enums[enum_id].variants.iter().any(|variant| {
            variant.tys.iter().any(|&field| {
                let field = types.subst(field, &args);
                type_contains_string_with_types(layouts, types, field, seen)
            })
        }),
        _ => false,
    }
}

fn runtime_reachable(flow: &emela_lir::Module) -> HashSet<FnId> {
    let mut pending: Vec<FnId> = flow
        .functions
        .iter()
        .filter_map(|(id, function)| function.exported.then_some(id))
        .collect();
    if pending.is_empty() {
        pending.extend(
            flow.functions
                .iter()
                .filter_map(|(id, function)| (!function.constant).then_some(id)),
        );
    }

    let mut reachable = HashSet::new();
    while let Some(id) = pending.pop() {
        if !reachable.insert(id) {
            continue;
        }
        let function = &flow.functions[id];
        if function.constant {
            continue;
        }
        for (_, statement) in function.stmts.iter() {
            if let Stmt::Let {
                value:
                    Value::Call {
                        callee: Callee::Direct(callee),
                        ..
                    },
                ..
            } = statement
            {
                pending.push(*callee);
            }
        }
    }
    reachable
}

fn validate_isr_entries(
    flow: &emela_lir::Module,
    emitted: &HashSet<FnId>,
    layouts: &TypeLayouts<'_>,
    names: &HashMap<FnId, String>,
    options: &Options,
) -> Result<(), Vec<Error>> {
    for entry_name in &options.isr_entries {
        let Some((entry, _)) = flow.functions.iter().find(|(id, function)| {
            emitted.contains(id) && function.exported && function.name == *entry_name
        }) else {
            return Err(vec![Error {
                kind: ErrorKind::InvalidOptions,
                message: format!(
                    "ISR entry `{entry_name}` must name an emitted exported Emela function"
                ),
            }]);
        };
        validate_isr_function(
            flow,
            entry,
            entry_name,
            layouts,
            names,
            options,
            &mut HashSet::new(),
            &mut HashSet::new(),
        )?;
    }
    Ok(())
}

fn validate_isr_function(
    flow: &emela_lir::Module,
    id: FnId,
    entry_name: &str,
    layouts: &TypeLayouts<'_>,
    names: &HashMap<FnId, String>,
    options: &Options,
    active: &mut HashSet<FnId>,
    complete: &mut HashSet<FnId>,
) -> Result<(), Vec<Error>> {
    if active.contains(&id) {
        return Err(unsupported(
            &flow.functions[id].name,
            &format!("recursive call reachable from ISR entry `{entry_name}`"),
        ));
    }
    if complete.contains(&id) {
        return Ok(());
    }
    let function = &flow.functions[id];
    if function.suspends {
        return Err(unsupported(
            &function.name,
            &format!("suspending function reachable from ISR entry `{entry_name}`"),
        ));
    }
    if std::iter::once(function.ret)
        .chain(function.vars.iter().map(|(_, variable)| variable.ty))
        .any(|ty| layouts.contains_list_ref(ty, &mut HashSet::new()))
    {
        return Err(unsupported(
            &function.name,
            &format!("heap-backed List value reachable from ISR entry `{entry_name}`"),
        ));
    }

    active.insert(id);
    for (_, statement) in function.stmts.iter() {
        let Stmt::Let {
            value: Value::Call {
                callee, suspends, ..
            },
            ..
        } = statement
        else {
            continue;
        };
        if *suspends {
            active.remove(&id);
            return Err(unsupported(
                &function.name,
                &format!("suspending call reachable from ISR entry `{entry_name}`"),
            ));
        }
        match callee {
            Callee::Direct(callee_id) => {
                let callee = &flow.functions[*callee_id];
                if callee.suspends {
                    active.remove(&id);
                    return Err(unsupported(
                        &function.name,
                        &format!("suspending call reachable from ISR entry `{entry_name}`"),
                    ));
                }
                if callee.external.is_some() {
                    let symbol = &names[callee_id];
                    if !options.isr_safe_externals.iter().any(|safe| safe == symbol) {
                        active.remove(&id);
                        return Err(unsupported(
                            &function.name,
                            &format!(
                                "external C function `{symbol}` reachable from ISR entry `{entry_name}` is not marked ISR-safe"
                            ),
                        ));
                    }
                } else if !callee.constant {
                    validate_isr_function(
                        flow, *callee_id, entry_name, layouts, names, options, active, complete,
                    )?;
                }
            }
            Callee::Indirect(_) | Callee::Op { .. } => {
                active.remove(&id);
                return Err(unsupported(
                    &function.name,
                    &format!("dynamic call reachable from ISR entry `{entry_name}`"),
                ));
            }
        }
    }
    active.remove(&id);
    complete.insert(id);
    Ok(())
}

fn function_needs_list_heap(function: &Function, layouts: &TypeLayouts<'_>) -> bool {
    let mut known_empty = HashSet::new();
    for (_, statement) in function.stmts.iter() {
        let Stmt::Let {
            dst: Some(dst),
            value: Value::List { elems, tail },
            ..
        } = statement
        else {
            continue;
        };
        if !elems.is_empty() {
            continue;
        }
        match tail {
            None => {
                known_empty.insert(*dst);
            }
            Some(Operand::Var(tail)) if known_empty.contains(tail) => {
                known_empty.insert(*dst);
            }
            _ => {}
        }
    }

    let statement_needs_heap = function.stmts.iter().any(|(_, statement)| match statement {
        Stmt::Let {
            value:
                Value::List {
                    elems,
                    tail: Some(tail),
                },
            ..
        } if !elems.is_empty()
            || !matches!(tail, Operand::Var(variable) if known_empty.contains(variable)) =>
        {
            true
        }
        Stmt::Let {
            value: Value::List { elems, tail: None },
            ..
        } => !elems.is_empty(),
        Stmt::Let {
            value: Value::ListHead(_) | Value::ListTail(_),
            ..
        } => true,
        _ => false,
    });
    let returns_managed_alias = matches!(layouts.types[function.ret], Type::List(_))
        && function.seqs.iter().any(|(_, sequence)| {
            matches!(
                sequence.term,
                Term::Return(Some(Operand::Var(variable)))
                    if function.vars[variable].ty == function.ret
                        && !known_empty.contains(&variable)
            )
        });
    let local_managed_value = function.vars.iter().any(|(variable, data)| {
        !function.params.contains(&variable)
            && layouts.is_managed_value(data.ty)
            && !known_empty.contains(&variable)
    });
    let returns_managed_aggregate = !matches!(layouts.types[function.ret], Type::List(_))
        && layouts.is_managed_value(function.ret);
    statement_needs_heap
        || returns_managed_alias
        || local_managed_value
        || returns_managed_aggregate
}

fn directly_calls(caller: &Function, target: FnId) -> bool {
    caller.stmts.iter().any(|(_, statement)| {
        matches!(
            statement,
            Stmt::Let {
                value: Value::Call {
                    callee: Callee::Direct(id),
                    ..
                },
                ..
            } if *id == target
        )
    })
}

fn operand_is_string(layouts: &TypeLayouts<'_>, function: &Function, operand: &Operand) -> bool {
    match operand {
        Operand::Const(Const::String(_)) => true,
        Operand::Var(variable) => {
            matches!(layouts.types[function.vars[*variable].ty], Type::String)
        }
        _ => false,
    }
}

fn operand_is_int(layouts: &TypeLayouts<'_>, function: &Function, operand: &Operand) -> bool {
    match operand {
        Operand::Const(Const::Int(_)) => true,
        Operand::Var(variable) => matches!(layouts.types[function.vars[*variable].ty], Type::Int),
        _ => false,
    }
}

fn operand_is_int64(layouts: &TypeLayouts<'_>, function: &Function, operand: &Operand) -> bool {
    match operand {
        Operand::Const(Const::Int64(_)) => true,
        Operand::Var(variable) => {
            matches!(layouts.types[function.vars[*variable].ty], Type::Int64)
        }
        _ => false,
    }
}

fn operand_is_float(layouts: &TypeLayouts<'_>, function: &Function, operand: &Operand) -> bool {
    match operand {
        Operand::Const(Const::Float(_)) => true,
        Operand::Var(variable) => matches!(layouts.types[function.vars[*variable].ty], Type::Float),
        _ => false,
    }
}

fn option_payload_type(layouts: &TypeLayouts<'_>, ty: TypeId) -> Option<TypeId> {
    let Type::Enum(enum_id, _) = layouts.types[ty] else {
        return None;
    };
    let definition = &layouts.core.enums[enum_id];
    if definition.name != "Option" {
        return None;
    }
    let some_index = definition
        .variants
        .iter()
        .position(|variant| variant.name == "Some")?;
    let fields = layouts.enum_fields(ty)?.get(some_index)?;
    (fields.len() == 1).then_some(fields[0])
}

fn option_variant_expression(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    ty: TypeId,
    variant_name: &str,
    value: Option<&str>,
) -> Result<String, Vec<Error>> {
    let Type::Enum(enum_id, _) = layouts.types[ty] else {
        return Err(invalid_flow_ir(
            &function.name,
            "Option builtin result is not an enum",
        ));
    };
    let definition = &layouts.core.enums[enum_id];
    if definition.name != "Option" {
        return Err(invalid_flow_ir(
            &function.name,
            "Option builtin result has the wrong enum type",
        ));
    }
    let Some(variant_index) = definition
        .variants
        .iter()
        .position(|variant| variant.name == variant_name)
    else {
        return Err(invalid_flow_ir(
            &function.name,
            "Option enum is missing a required variant",
        ));
    };
    let Some(fields) = layouts
        .enum_fields(ty)
        .and_then(|variants| variants.get(variant_index))
    else {
        return Err(invalid_flow_ir(
            &function.name,
            "Option result has no inline enum layout",
        ));
    };
    let mut initializers = vec![format!(".tag = {}", int32_literal(variant_index as i32))];
    match value {
        Some(value) if fields.len() == 1 && layouts.type_repr(fields[0]) != Repr::Void => {
            initializers.push(format!(".payload.v{variant_index}.f0 = {value}"));
        }
        Some(_) => {
            return Err(invalid_flow_ir(
                &function.name,
                "Option Some payload does not match its runtime representation",
            ));
        }
        None if fields
            .iter()
            .all(|field| layouts.type_repr(*field) == Repr::Void) =>
        {
            initializers.push(format!(".payload.v{variant_index}._unit = UINT8_C(0)"));
        }
        None => {
            return Err(invalid_flow_ir(
                &function.name,
                "Option None variant unexpectedly carries a runtime payload",
            ));
        }
    }
    Ok(format!(
        "({}){{ {} }}",
        layouts.c_type(ty),
        initializers.join(", ")
    ))
}

fn emit_builtin(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    op: Builtin,
    args: &[Operand],
    string_buffers: &StringBufferPlan,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let (arity, result_type) = match op {
        Builtin::StringAppend => (2, Type::String),
        Builtin::StringByteSize => (1, Type::Int),
        Builtin::StringContains | Builtin::StringStartsWith | Builtin::StringEndsWith => {
            (2, Type::Bool)
        }
        Builtin::StringTrim => (1, Type::String),
        Builtin::StringFromCodePoint => (1, Type::String),
        Builtin::IntToString => (1, Type::String),
        Builtin::IntParse => (1, Type::Int),
        Builtin::IntBitAnd | Builtin::IntBitOr | Builtin::IntBitXor => (2, Type::Int),
        Builtin::IntBitNot => (1, Type::Int),
        Builtin::IntCheckedAdd => (2, Type::Int),
        Builtin::IntShiftLeft | Builtin::IntShiftRight | Builtin::IntShiftRightUnsigned => {
            (2, Type::Int)
        }
        Builtin::Int64BitAnd | Builtin::Int64BitOr | Builtin::Int64BitXor => (2, Type::Int64),
        Builtin::Int64BitNot => (1, Type::Int64),
        Builtin::Int64ToString => (1, Type::String),
        Builtin::FloatToString => (1, Type::String),
        Builtin::FloatFromInt => (1, Type::Float),
        Builtin::FloatParse => (1, Type::Float),
        Builtin::FloatFloor | Builtin::FloatCeil | Builtin::FloatRound | Builtin::FloatTruncate => {
            (1, Type::Int)
        }
        Builtin::Int64ShiftLeft | Builtin::Int64ShiftRight | Builtin::Int64ShiftRightUnsigned => {
            (2, Type::Int64)
        }
        Builtin::Int64FromInt => (1, Type::Int64),
        Builtin::Int64ToInt => (1, Type::Int),
        _ => return Err(unsupported(&function.name, &format!("builtin {op:?}"))),
    };
    if args.len() != arity {
        return Err(invalid_flow_ir(
            &function.name,
            &format!(
                "builtin {op:?} has {} arguments, expected {arity}",
                args.len()
            ),
        ));
    }
    let arguments_match = match op {
        Builtin::IntBitAnd
        | Builtin::IntBitOr
        | Builtin::IntBitXor
        | Builtin::IntBitNot
        | Builtin::IntShiftLeft
        | Builtin::IntShiftRight
        | Builtin::IntShiftRightUnsigned => args
            .iter()
            .all(|operand| operand_is_int(layouts, function, operand)),
        Builtin::IntCheckedAdd => args
            .iter()
            .all(|operand| operand_is_int(layouts, function, operand)),
        Builtin::IntToString => args
            .iter()
            .all(|operand| operand_is_int(layouts, function, operand)),
        Builtin::FloatFromInt => args
            .iter()
            .all(|operand| operand_is_int(layouts, function, operand)),
        Builtin::FloatToString => operand_is_float(layouts, function, &args[0]),
        Builtin::FloatParse => operand_is_string(layouts, function, &args[0]),
        Builtin::FloatFloor | Builtin::FloatCeil | Builtin::FloatRound | Builtin::FloatTruncate => {
            operand_is_float(layouts, function, &args[0])
        }
        Builtin::StringFromCodePoint => args
            .iter()
            .all(|operand| operand_is_int(layouts, function, operand)),
        Builtin::IntParse => args
            .iter()
            .all(|operand| operand_is_string(layouts, function, operand)),
        Builtin::Int64ShiftLeft | Builtin::Int64ShiftRight | Builtin::Int64ShiftRightUnsigned => {
            operand_is_int64(layouts, function, &args[0])
                && operand_is_int(layouts, function, &args[1])
        }
        Builtin::Int64BitAnd
        | Builtin::Int64BitOr
        | Builtin::Int64BitXor
        | Builtin::Int64BitNot => args
            .iter()
            .all(|operand| operand_is_int64(layouts, function, operand)),
        Builtin::Int64FromInt => operand_is_int(layouts, function, &args[0]),
        Builtin::Int64ToInt => operand_is_int64(layouts, function, &args[0]),
        Builtin::Int64ToString => args
            .iter()
            .all(|operand| operand_is_int64(layouts, function, operand)),
        _ => args
            .iter()
            .all(|operand| operand_is_string(layouts, function, operand)),
    };
    if !arguments_match {
        return Err(invalid_flow_ir(
            &function.name,
            &format!("builtin {op:?} has an argument with the wrong type"),
        ));
    }
    let Some(dst) = dst else {
        return Err(invalid_flow_ir(
            &function.name,
            &format!("builtin {op:?} has no result destination"),
        ));
    };
    let result_matches = if matches!(op, Builtin::IntCheckedAdd | Builtin::IntParse) {
        option_payload_type(layouts, function.vars[dst].ty)
            .is_some_and(|ty| matches!(layouts.types[ty], Type::Int))
    } else if op == Builtin::FloatParse {
        option_payload_type(layouts, function.vars[dst].ty)
            .is_some_and(|ty| matches!(layouts.types[ty], Type::Float))
    } else if op == Builtin::StringFromCodePoint {
        option_payload_type(layouts, function.vars[dst].ty)
            .is_some_and(|ty| matches!(layouts.types[ty], Type::String))
    } else {
        layouts.types[function.vars[dst].ty] == result_type
    };
    if !result_matches {
        return Err(invalid_flow_ir(
            &function.name,
            &format!("builtin {op:?} has a result with the wrong type"),
        ));
    }

    let expressions = args
        .iter()
        .map(operand_expression)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|variant| unsupported(&function.name, variant))?;
    if op == Builtin::IntCheckedAdd {
        let ty = function.vars[dst].ty;
        let sum_name = format!("{}_checked_sum", variable_name(dst));
        writer.decl(
            "int64_t",
            &sum_name,
            Some(&format!(
                "((int64_t)({}) + (int64_t)({}))",
                expressions[0], expressions[1]
            )),
        );
        let none = option_variant_expression(layouts, function, ty, "None", None)?;
        let some = option_variant_expression(
            layouts,
            function,
            ty,
            "Some",
            Some(&format!("(int32_t){sum_name}")),
        )?;
        writer.begin_block(&format!(
            "if ({sum_name} < (int64_t)INT32_MIN || {sum_name} > (int64_t)INT32_MAX)"
        ));
        writer.assign(&variable_name(dst), &none);
        writer.end_block();
        writer.begin_block("else");
        writer.assign(&variable_name(dst), &some);
        writer.end_block();
        return Ok(());
    }
    if op == Builtin::IntParse {
        let ty = function.vars[dst].ty;
        let none = option_variant_expression(layouts, function, ty, "None", None)?;
        let parsed_name = format!("{}_parsed", variable_name(dst));
        let some = option_variant_expression(layouts, function, ty, "Some", Some(&parsed_name))?;
        if let Operand::Const(Const::String(value)) = &args[0] {
            let expression = match value.parse::<i32>() {
                Ok(parsed) => option_variant_expression(
                    layouts,
                    function,
                    ty,
                    "Some",
                    Some(&int32_literal(parsed)),
                )?,
                Err(_) => none,
            };
            writer.assign(&variable_name(dst), &expression);
        } else {
            writer.decl("int32_t", &parsed_name, None);
            writer.begin_block(&format!(
                "if (emela_emb_int_parse({}, &{parsed_name}))",
                expressions[0]
            ));
            writer.assign(&variable_name(dst), &some);
            writer.end_block();
            writer.begin_block("else");
            writer.assign(&variable_name(dst), &none);
            writer.end_block();
        }
        return Ok(());
    }
    if op == Builtin::FloatParse {
        let ty = function.vars[dst].ty;
        let none = option_variant_expression(layouts, function, ty, "None", None)?;
        let Some(Operand::Const(Const::String(value))) = args.first() else {
            return Err(unsupported(
                &function.name,
                "dynamic Float.parse requires an allocation-free bounded decimal parser",
            ));
        };
        let expression = match const_eval::parse_float(value) {
            Some(parsed) => option_variant_expression(
                layouts,
                function,
                ty,
                "Some",
                Some(&float_literal(parsed)),
            )?,
            None => none,
        };
        writer.assign(&variable_name(dst), &expression);
        return Ok(());
    }
    if op == Builtin::StringFromCodePoint {
        let ty = function.vars[dst].ty;
        let none = option_variant_expression(layouts, function, ty, "None", None)?;
        if let Operand::Const(Const::Int(value)) = &args[0] {
            let expression = match u32::try_from(*value).ok().and_then(char::from_u32) {
                Some(character) => {
                    let string = constant_expression(&Const::String(character.to_string()))
                        .map_err(|variant| unsupported(&function.name, variant))?
                        .ok_or_else(|| {
                            invalid_flow_ir(
                                &function.name,
                                "String.from_code_point constant has no C expression",
                            )
                        })?;
                    option_variant_expression(layouts, function, ty, "Some", Some(&string))?
                }
                None => none,
            };
            writer.assign(&variable_name(dst), &expression);
        } else {
            let Some((buffer, capacity)) = string_buffers.buffer(dst) else {
                return Err(invalid_flow_ir(
                    &function.name,
                    "dynamic String.from_code_point has no planned output buffer",
                ));
            };
            let codepoint = &expressions[0];
            let value_name = format!("{}_code_point", variable_name(dst));
            let length_name = format!("{}_byte_length", variable_name(dst));
            let string_value =
                format!("(EmelaString){{ .data = {buffer}, .length = (uint16_t){length_name} }}");
            let some =
                option_variant_expression(layouts, function, ty, "Some", Some(&string_value))?;
            writer.begin_block(&format!(
                "if ({codepoint} < INT32_C(0) || {codepoint} > INT32_C(0x10ffff) || ({codepoint} >= INT32_C(0xd800) && {codepoint} <= INT32_C(0xdfff)))"
            ));
            writer.assign(&variable_name(dst), &none);
            writer.end_block();
            writer.begin_block("else");
            writer.decl(
                "uint32_t",
                &value_name,
                Some(&format!("(uint32_t)({codepoint})")),
            );
            writer.decl("uint16_t", &length_name, None);
            writer.begin_block(&format!("if ({value_name} <= UINT32_C(0x7f))"));
            writer.assign(&length_name, "UINT16_C(1)");
            writer.end_block();
            writer.begin_block(&format!("else if ({value_name} <= UINT32_C(0x7ff))"));
            writer.assign(&length_name, "UINT16_C(2)");
            writer.end_block();
            writer.begin_block(&format!("else if ({value_name} <= UINT32_C(0xffff))"));
            writer.assign(&length_name, "UINT16_C(3)");
            writer.end_block();
            writer.begin_block("else");
            writer.assign(&length_name, "UINT16_C(4)");
            writer.end_block();
            writer.begin_block(&format!("if ({length_name} > UINT16_C({capacity}))"));
            writer.line(
                "emela_board_defect(EMELA_DEFECT_STRING_CAPACITY, \"string capacity exceeded\", UINT16_C(24));",
            );
            writer.end_block();
            writer.begin_block(&format!("if ({length_name} == UINT16_C(1))"));
            writer.assign(&format!("{buffer}[0]"), &format!("(char){value_name}"));
            writer.end_block();
            writer.begin_block(&format!("else if ({length_name} == UINT16_C(2))"));
            writer.assign(
                &format!("{buffer}[0]"),
                &format!("(char)(UINT32_C(0xc0) | ({value_name} >> UINT32_C(6)))"),
            );
            writer.assign(
                &format!("{buffer}[1]"),
                &format!("(char)(UINT32_C(0x80) | ({value_name} & UINT32_C(0x3f)))"),
            );
            writer.end_block();
            writer.begin_block(&format!("else if ({length_name} == UINT16_C(3))"));
            writer.assign(
                &format!("{buffer}[0]"),
                &format!("(char)(UINT32_C(0xe0) | ({value_name} >> UINT32_C(12)))"),
            );
            writer.assign(
                &format!("{buffer}[1]"),
                &format!(
                    "(char)(UINT32_C(0x80) | (({value_name} >> UINT32_C(6)) & UINT32_C(0x3f)))"
                ),
            );
            writer.assign(
                &format!("{buffer}[2]"),
                &format!("(char)(UINT32_C(0x80) | ({value_name} & UINT32_C(0x3f)))"),
            );
            writer.end_block();
            writer.begin_block("else");
            writer.assign(
                &format!("{buffer}[0]"),
                &format!("(char)(UINT32_C(0xf0) | ({value_name} >> UINT32_C(18)))"),
            );
            writer.assign(
                &format!("{buffer}[1]"),
                &format!(
                    "(char)(UINT32_C(0x80) | (({value_name} >> UINT32_C(12)) & UINT32_C(0x3f)))"
                ),
            );
            writer.assign(
                &format!("{buffer}[2]"),
                &format!(
                    "(char)(UINT32_C(0x80) | (({value_name} >> UINT32_C(6)) & UINT32_C(0x3f)))"
                ),
            );
            writer.assign(
                &format!("{buffer}[3]"),
                &format!("(char)(UINT32_C(0x80) | ({value_name} & UINT32_C(0x3f)))"),
            );
            writer.end_block();
            writer.assign(&variable_name(dst), &some);
            writer.end_block();
        }
        return Ok(());
    }
    if matches!(
        op,
        Builtin::IntShiftLeft
            | Builtin::IntShiftRight
            | Builtin::IntShiftRightUnsigned
            | Builtin::Int64ShiftLeft
            | Builtin::Int64ShiftRight
            | Builtin::Int64ShiftRightUnsigned
    ) {
        writer.begin_block(&format!("if ({} < INT32_C(0))", expressions[1]));
        writer.line(&format!(
            "emela_emb_negative_shift_defect({});",
            expressions[1]
        ));
        writer.end_block();
    }
    let expression = match op {
        Builtin::StringAppend => {
            if let [
                Operand::Const(Const::String(left)),
                Operand::Const(Const::String(right)),
            ] = args
            {
                constant_expression(&Const::String(format!("{left}{right}")))
                    .map_err(|variant| unsupported(&function.name, variant))?
                    .ok_or_else(|| {
                        invalid_flow_ir(&function.name, "String append has no C expression")
                    })?
            } else if let [Operand::Const(Const::String(value)), other] = args
                && value.is_empty()
            {
                operand_expression(other).map_err(|variant| unsupported(&function.name, variant))?
            } else if let [other, Operand::Const(Const::String(value))] = args
                && value.is_empty()
            {
                operand_expression(other).map_err(|variant| unsupported(&function.name, variant))?
            } else {
                let Some((buffer, capacity)) = string_buffers.buffer(dst) else {
                    return Err(invalid_flow_ir(
                        &function.name,
                        "dynamic String append has no planned output buffer",
                    ));
                };
                format!(
                    "emela_emb_string_append({}, {}, {buffer}, UINT16_C({capacity}))",
                    expressions[0], expressions[1]
                )
            }
        }
        Builtin::StringByteSize => format!("(int32_t)(({}).length)", expressions[0]),
        Builtin::IntToString => match &args[0] {
            Operand::Const(Const::Int(value)) => {
                constant_expression(&Const::String(value.to_string()))
                    .map_err(|variant| unsupported(&function.name, variant))?
                    .expect("String constant has an expression")
            }
            _ => {
                let Some((buffer, capacity)) = string_buffers.buffer(dst) else {
                    return Err(invalid_flow_ir(
                        &function.name,
                        "dynamic Int.to_string has no planned output buffer",
                    ));
                };
                format!(
                    "emela_emb_int_to_string((int64_t)({}), {buffer}, UINT16_C({capacity}))",
                    expressions[0]
                )
            }
        },
        Builtin::IntBitAnd => format!("({} & {})", expressions[0], expressions[1]),
        Builtin::IntBitOr => format!("({} | {})", expressions[0], expressions[1]),
        Builtin::IntBitXor => format!("({} ^ {})", expressions[0], expressions[1]),
        Builtin::IntBitNot => format!("(~{})", expressions[0]),
        Builtin::IntShiftLeft => format!(
            "emela_i32_shift_left({}, {})",
            expressions[0], expressions[1]
        ),
        Builtin::IntShiftRight => format!(
            "emela_i32_shift_right({}, {})",
            expressions[0], expressions[1]
        ),
        Builtin::IntShiftRightUnsigned => format!(
            "emela_i32_shift_right_unsigned({}, {})",
            expressions[0], expressions[1]
        ),
        Builtin::Int64ShiftLeft => format!(
            "emela_i64_shift_left({}, {})",
            expressions[0], expressions[1]
        ),
        Builtin::Int64ShiftRight => format!(
            "emela_i64_shift_right({}, {})",
            expressions[0], expressions[1]
        ),
        Builtin::Int64ShiftRightUnsigned => format!(
            "emela_i64_shift_right_unsigned({}, {})",
            expressions[0], expressions[1]
        ),
        Builtin::Int64BitAnd => format!("({} & {})", expressions[0], expressions[1]),
        Builtin::Int64BitOr => format!("({} | {})", expressions[0], expressions[1]),
        Builtin::Int64BitXor => format!("({} ^ {})", expressions[0], expressions[1]),
        Builtin::Int64BitNot => format!("(~{})", expressions[0]),
        Builtin::Int64FromInt => format!("((int64_t)({}))", expressions[0]),
        Builtin::Int64ToInt => {
            format!("emela_i32_from_bits((uint32_t)({}))", expressions[0])
        }
        Builtin::Int64ToString => match &args[0] {
            Operand::Const(Const::Int64(value)) => {
                constant_expression(&Const::String(value.to_string()))
                    .map_err(|variant| unsupported(&function.name, variant))?
                    .expect("String constant has an expression")
            }
            _ => {
                let Some((buffer, capacity)) = string_buffers.buffer(dst) else {
                    return Err(invalid_flow_ir(
                        &function.name,
                        "dynamic Int64.to_string has no planned output buffer",
                    ));
                };
                format!(
                    "emela_emb_int_to_string({}, {buffer}, UINT16_C({capacity}))",
                    expressions[0]
                )
            }
        },
        Builtin::FloatToString => match &args[0] {
            Operand::Const(Const::Float(value)) => {
                constant_expression(&Const::String(const_eval::format_float(*value)))
                    .map_err(|variant| unsupported(&function.name, variant))?
                    .expect("String constant has an expression")
            }
            _ => {
                return Err(unsupported(
                    &function.name,
                    "dynamic Float.to_string needs an allocation-free shortest-decimal runtime",
                ));
            }
        },
        Builtin::FloatFromInt => format!("((double)({}))", expressions[0]),
        Builtin::FloatFloor => format!("emela_emb_float_to_int({}, UINT8_C(0))", expressions[0]),
        Builtin::FloatCeil => format!("emela_emb_float_to_int({}, UINT8_C(1))", expressions[0]),
        Builtin::FloatRound => format!("emela_emb_float_to_int({}, UINT8_C(2))", expressions[0]),
        Builtin::FloatTruncate => format!("emela_emb_float_to_int({}, UINT8_C(3))", expressions[0]),
        Builtin::StringContains => format!(
            "emela_emb_string_contains({}, {})",
            expressions[0], expressions[1]
        ),
        Builtin::StringStartsWith => format!(
            "emela_emb_string_starts_with({}, {})",
            expressions[0], expressions[1]
        ),
        Builtin::StringEndsWith => format!(
            "emela_emb_string_ends_with({}, {})",
            expressions[0], expressions[1]
        ),
        Builtin::StringTrim => format!("emela_emb_string_trim({})", expressions[0]),
        _ => unreachable!("builtin was validated above"),
    };
    writer.assign(&variable_name(dst), &expression);
    Ok(())
}

fn emit_runtime(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    op: RuntimeOp,
    args: &[Operand],
    string_buffers: &StringBufferPlan,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    if op == RuntimeOp::DebugPrint {
        if args.len() != 1 || !operand_is_string(layouts, function, &args[0]) {
            return Err(invalid_flow_ir(
                &function.name,
                "RuntimeOp::DebugPrint must have one String argument",
            ));
        }
        if dst.is_some() {
            return Err(invalid_flow_ir(
                &function.name,
                "RuntimeOp::DebugPrint has a result destination",
            ));
        }
        let value =
            operand_expression(&args[0]).map_err(|variant| unsupported(&function.name, variant))?;
        writer.line(&format!(
            "emela_board_debug(({value}).data, ({value}).length);"
        ));
        return Ok(());
    }
    if op != RuntimeOp::ShowString {
        return Err(unsupported(&function.name, &format!("RuntimeOp::{op:?}")));
    }
    if args.len() != 1 || !operand_is_string(layouts, function, &args[0]) {
        return Err(invalid_flow_ir(
            &function.name,
            "RuntimeOp::ShowString must have one String argument",
        ));
    }
    let Some(dst) = dst else {
        return Err(invalid_flow_ir(
            &function.name,
            "RuntimeOp::ShowString has no String destination",
        ));
    };
    if !matches!(layouts.types[function.vars[dst].ty], Type::String) {
        return Err(invalid_flow_ir(
            &function.name,
            "RuntimeOp::ShowString destination is not String",
        ));
    }

    let expression = match &args[0] {
        Operand::Const(Const::String(value)) => {
            let shown = constant_expression(&Const::String(show_string(value)))
                .map_err(|variant| unsupported(&function.name, variant))?
                .ok_or_else(|| {
                    invalid_flow_ir(&function.name, "RuntimeOp::ShowString has no C expression")
                })?;
            shown
        }
        operand => {
            let input = operand_expression(operand)
                .map_err(|variant| unsupported(&function.name, variant))?;
            let Some((buffer, capacity)) = string_buffers.buffer(dst) else {
                return Err(invalid_flow_ir(
                    &function.name,
                    "dynamic RuntimeOp::ShowString has no planned output buffer",
                ));
            };
            format!("emela_emb_string_show({input}, {buffer}, UINT16_C({capacity}))")
        }
    };
    writer.assign(&variable_name(dst), &expression);
    Ok(())
}

fn show_string(value: &str) -> String {
    let mut output = String::with_capacity(value.len().saturating_add(2));
    output.push('"');
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\t' => output.push_str("\\t"),
            '\r' => output.push_str("\\r"),
            '#' if characters.peek() == Some(&'{') => output.push_str("\\#"),
            control if control <= '\u{1f}' || control == '\u{7f}' => {
                output.push_str(&format!("\\u{{{:x}}}", control as u32));
            }
            character => output.push(character),
        }
    }
    output.push('"');
    output
}

fn emit_string_matches_at_helper(writer: &mut Writer) {
    writer.begin_block(
        "static inline int32_t emela_emb_string_matches_at(EmelaString value, EmelaString needle, uint32_t offset)",
    );
    writer.begin_block(
        "if (offset > (uint32_t)value.length || (uint32_t)needle.length > (uint32_t)value.length - offset)",
    );
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.begin_block("for (uint32_t i = 0; i < (uint32_t)needle.length; ++i)");
    writer.begin_block("if (value.data[offset + i] != needle.data[i])");
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.end_block();
    writer.line("return INT32_C(1);");
    writer.end_block();
    writer.blank();
}

fn emit_string_contains_helper(writer: &mut Writer) {
    writer.begin_block(
        "static inline int32_t emela_emb_string_contains(EmelaString value, EmelaString needle)",
    );
    writer.begin_block("if (needle.length > value.length)");
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.line("uint32_t last = (uint32_t)value.length - needle.length;");
    writer.begin_block("for (uint32_t offset = 0; offset <= last; ++offset)");
    writer.begin_block("if (emela_emb_string_matches_at(value, needle, offset))");
    writer.line("return INT32_C(1);");
    writer.end_block();
    writer.end_block();
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.blank();
}

fn emit_string_starts_with_helper(writer: &mut Writer) {
    writer.begin_block(
        "static inline int32_t emela_emb_string_starts_with(EmelaString value, EmelaString prefix)",
    );
    writer.line("return emela_emb_string_matches_at(value, prefix, UINT32_C(0));");
    writer.end_block();
    writer.blank();
}

fn emit_string_ends_with_helper(writer: &mut Writer) {
    writer.begin_block(
        "static inline int32_t emela_emb_string_ends_with(EmelaString value, EmelaString suffix)",
    );
    writer.begin_block("if (suffix.length > value.length)");
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.line("uint32_t offset = (uint32_t)value.length - suffix.length;");
    writer.line("return emela_emb_string_matches_at(value, suffix, offset);");
    writer.end_block();
    writer.blank();
}

fn emit_string_append_helper(writer: &mut Writer) {
    writer.begin_block(
        "static inline EmelaString emela_emb_string_append(EmelaString left, EmelaString right, char *buffer, uint16_t capacity)",
    );
    writer.begin_block(
        "if (left.length > capacity || right.length > (uint16_t)(capacity - left.length))",
    );
    writer.line(
        "emela_board_defect(EMELA_DEFECT_STRING_CAPACITY, \"string capacity exceeded\", UINT16_C(24));",
    );
    writer.end_block();
    writer.begin_block("if (left.data != buffer)");
    writer.begin_block("for (uint32_t i = UINT32_C(0); i < (uint32_t)left.length; ++i)");
    writer.line("buffer[i] = left.data[i];");
    writer.end_block();
    writer.end_block();
    writer.begin_block("for (uint32_t i = UINT32_C(0); i < (uint32_t)right.length; ++i)");
    writer.line("buffer[(uint32_t)left.length + i] = right.data[i];");
    writer.end_block();
    writer.line(
        "return (EmelaString){ .data = buffer, .length = (uint16_t)(left.length + right.length) };",
    );
    writer.end_block();
    writer.blank();
}

fn emit_int_to_string_helper(writer: &mut Writer) {
    writer.begin_block(
        "static inline EmelaString emela_emb_int_to_string(int64_t value, char *buffer, uint16_t capacity)",
    );
    writer.line("uint64_t magnitude = value < INT64_C(0) ? (uint64_t)(-(value + INT64_C(1))) + UINT64_C(1) : (uint64_t)value;");
    writer.line("uint32_t digit_count = UINT32_C(1);");
    writer.begin_block("for (uint64_t remaining = magnitude; remaining >= UINT64_C(10); remaining /= UINT64_C(10))");
    writer.line("++digit_count;");
    writer.end_block();
    writer
        .line("uint32_t length = digit_count + (value < INT64_C(0) ? UINT32_C(1) : UINT32_C(0));");
    writer.begin_block("if (length > (uint32_t)capacity)");
    writer.line(
        "emela_board_defect(EMELA_DEFECT_STRING_CAPACITY, \"string capacity exceeded\", UINT16_C(24));",
    );
    writer.end_block();
    writer.line("uint32_t cursor = length;");
    writer.begin_block("do");
    writer.line("buffer[--cursor] = (char)('0' + magnitude % UINT64_C(10));");
    writer.line("magnitude /= UINT64_C(10);");
    writer.end_block();
    writer.line("while (magnitude != UINT64_C(0));");
    writer.begin_block("if (value < INT64_C(0))");
    writer.line("buffer[0] = '-';");
    writer.end_block();
    writer.line("return (EmelaString){ .data = buffer, .length = (uint16_t)length };");
    writer.end_block();
    writer.blank();
}

fn emit_int_parse_helper(writer: &mut Writer) {
    writer.begin_block("static inline int emela_emb_int_parse(EmelaString value, int32_t *result)");
    writer.begin_block("if (value.length == UINT16_C(0))");
    writer.line("return 0;");
    writer.end_block();
    writer.line("uint32_t index = UINT32_C(0);");
    writer.line("int negative = 0;");
    writer.line("const uint8_t first = (uint8_t)value.data[index];");
    writer.begin_block("if (first == UINT8_C(0x2b) || first == UINT8_C(0x2d))");
    writer.line("negative = first == UINT8_C(0x2d);");
    writer.line("++index;");
    writer.end_block();
    writer.begin_block("if (index >= (uint32_t)value.length)");
    writer.line("return 0;");
    writer.end_block();
    writer.line("uint64_t limit = negative ? UINT64_C(2147483648) : UINT64_C(2147483647);");
    writer.line("uint64_t magnitude = UINT64_C(0);");
    writer.begin_block("for (; index < (uint32_t)value.length; ++index)");
    writer.line("const uint8_t byte = (uint8_t)value.data[index];");
    writer.begin_block("if (byte < UINT8_C(0x30) || byte > UINT8_C(0x39))");
    writer.line("return 0;");
    writer.end_block();
    writer.line("const uint64_t digit = (uint64_t)(byte - UINT8_C(0x30));");
    writer.begin_block("if (magnitude > (limit - digit) / UINT64_C(10))");
    writer.line("return 0;");
    writer.end_block();
    writer.line("magnitude = magnitude * UINT64_C(10) + digit;");
    writer.end_block();
    writer.begin_block("if (negative && magnitude == UINT64_C(2147483648))");
    writer.line("*result = INT32_MIN;");
    writer.end_block();
    writer.line("else if (negative)");
    writer.begin_block("");
    writer.line("*result = -(int32_t)magnitude;");
    writer.end_block();
    writer.line("else");
    writer.begin_block("");
    writer.line("*result = (int32_t)magnitude;");
    writer.end_block();
    writer.line("return 1;");
    writer.end_block();
    writer.blank();
}

fn emit_string_show_helpers(writer: &mut Writer) {
    writer.begin_block(
        "static inline void emela_emb_string_show_put(char *buffer, uint16_t capacity, uint32_t *length, uint8_t byte)",
    );
    writer.begin_block("if (*length >= (uint32_t)capacity)");
    writer.line(
        "emela_board_defect(EMELA_DEFECT_STRING_CAPACITY, \"string capacity exceeded\", UINT16_C(24));",
    );
    writer.end_block();
    writer.line("buffer[*length] = (char)byte;");
    writer.line("++*length;");
    writer.end_block();
    writer.blank();

    writer.begin_block(
        "static inline EmelaString emela_emb_string_show(EmelaString value, char *buffer, uint16_t capacity)",
    );
    writer.line("uint32_t length = UINT32_C(0);");
    writer.line("emela_emb_string_show_put(buffer, capacity, &length, UINT8_C(0x22));");
    writer.begin_block("for (uint32_t i = UINT32_C(0); i < (uint32_t)value.length; ++i)");
    writer.line("const uint8_t byte = (uint8_t)value.data[i];");
    writer.begin_block("if (byte == UINT8_C(0x22) || byte == UINT8_C(0x5c))");
    writer.line("emela_emb_string_show_put(buffer, capacity, &length, UINT8_C(0x5c));");
    writer.line("emela_emb_string_show_put(buffer, capacity, &length, byte);");
    writer.end_block();
    writer.begin_block(
        "else if (byte == UINT8_C(0x23) && i + UINT32_C(1) < (uint32_t)value.length && value.data[i + UINT32_C(1)] == '{')",
    );
    writer.line("emela_emb_string_show_put(buffer, capacity, &length, UINT8_C(0x5c));");
    writer.line("emela_emb_string_show_put(buffer, capacity, &length, byte);");
    writer.end_block();
    writer.begin_block(
        "else if (byte == UINT8_C(0x0a) || byte == UINT8_C(0x09) || byte == UINT8_C(0x0d))",
    );
    writer.line("emela_emb_string_show_put(buffer, capacity, &length, UINT8_C(0x5c));");
    writer.line("const uint8_t escaped = byte == UINT8_C(0x0a) ? UINT8_C(0x6e) : (byte == UINT8_C(0x09) ? UINT8_C(0x74) : UINT8_C(0x72));");
    writer.line("emela_emb_string_show_put(buffer, capacity, &length, escaped);");
    writer.end_block();
    writer.begin_block("else if (byte < UINT8_C(0x20) || byte == UINT8_C(0x7f))");
    writer.line("static const char digits[] = \"0123456789abcdef\";");
    writer.line("emela_emb_string_show_put(buffer, capacity, &length, UINT8_C(0x5c));");
    writer.line("emela_emb_string_show_put(buffer, capacity, &length, UINT8_C(0x75));");
    writer.line("emela_emb_string_show_put(buffer, capacity, &length, UINT8_C(0x7b));");
    writer.begin_block("if (byte >= UINT8_C(0x10))");
    writer
        .line("emela_emb_string_show_put(buffer, capacity, &length, (uint8_t)digits[byte >> 4]);");
    writer.end_block();
    writer.line(
        "emela_emb_string_show_put(buffer, capacity, &length, (uint8_t)digits[byte & UINT8_C(0x0f)]);",
    );
    writer.line("emela_emb_string_show_put(buffer, capacity, &length, UINT8_C(0x7d));");
    writer.end_block();
    writer.begin_block("else");
    writer.line("emela_emb_string_show_put(buffer, capacity, &length, byte);");
    writer.end_block();
    writer.end_block();
    writer.line("emela_emb_string_show_put(buffer, capacity, &length, UINT8_C(0x22));");
    writer.line("return (EmelaString){ .data = buffer, .length = (uint16_t)length };");
    writer.end_block();
    writer.blank();
}

fn emit_string_trim_helper(writer: &mut Writer) {
    writer.begin_block(
        "static inline int32_t emela_emb_utf8_decode(EmelaString value, uint32_t offset, uint32_t *code_point, uint32_t *width)",
    );
    writer.begin_block("if (offset >= (uint32_t)value.length)");
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.line("const uint8_t first = (uint8_t)value.data[offset];");
    writer.begin_block("if (first <= UINT8_C(0x7f))");
    writer.line("*code_point = first;");
    writer.line("*width = UINT32_C(1);");
    writer.line("return INT32_C(1);");
    writer.end_block();
    writer.line("uint32_t count;");
    writer.line("uint32_t decoded;");
    writer.begin_block("if (first >= UINT8_C(0xc2) && first <= UINT8_C(0xdf))");
    writer.line("count = UINT32_C(2);");
    writer.line("decoded = (uint32_t)(first & UINT8_C(0x1f));");
    writer.end_block();
    writer.line("else if (first >= UINT8_C(0xe0) && first <= UINT8_C(0xef))");
    writer.begin_block("");
    writer.line("count = UINT32_C(3);");
    writer.line("decoded = (uint32_t)(first & UINT8_C(0x0f));");
    writer.end_block();
    writer.line("else if (first >= UINT8_C(0xf0) && first <= UINT8_C(0xf4))");
    writer.begin_block("");
    writer.line("count = UINT32_C(4);");
    writer.line("decoded = (uint32_t)(first & UINT8_C(0x07));");
    writer.end_block();
    writer.line("else");
    writer.begin_block("");
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.begin_block("if (offset + count > (uint32_t)value.length)");
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.begin_block("for (uint32_t i = UINT32_C(1); i < count; ++i)");
    writer.line("const uint8_t next = (uint8_t)value.data[offset + i];");
    writer.begin_block("if ((next & UINT8_C(0xc0)) != UINT8_C(0x80))");
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.line("decoded = (decoded << 6) | (uint32_t)(next & UINT8_C(0x3f));");
    writer.end_block();
    writer.begin_block(
        "if (count == UINT32_C(3) && (decoded < UINT32_C(0x800) || (decoded >= UINT32_C(0xd800) && decoded <= UINT32_C(0xdfff))))",
    );
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.begin_block(
        "if (count == UINT32_C(4) && (decoded < UINT32_C(0x10000) || decoded > UINT32_C(0x10ffff)))",
    );
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.line("*code_point = decoded;");
    writer.line("*width = count;");
    writer.line("return INT32_C(1);");
    writer.end_block();
    writer.blank();

    writer.begin_block("static inline int32_t emela_emb_is_white_space(uint32_t code_point)");
    writer.line(
        "return (code_point >= UINT32_C(0x0009) && code_point <= UINT32_C(0x000d)) || code_point == UINT32_C(0x0020) || code_point == UINT32_C(0x0085) || code_point == UINT32_C(0x00a0) || code_point == UINT32_C(0x1680) || (code_point >= UINT32_C(0x2000) && code_point <= UINT32_C(0x200a)) || code_point == UINT32_C(0x2028) || code_point == UINT32_C(0x2029) || code_point == UINT32_C(0x202f) || code_point == UINT32_C(0x205f) || code_point == UINT32_C(0x3000);",
    );
    writer.end_block();
    writer.blank();

    writer.begin_block("static inline EmelaString emela_emb_string_trim(EmelaString value)");
    writer.begin_block("if (value.length == UINT16_C(0))");
    writer.line("return value;");
    writer.end_block();
    writer.line("uint32_t start = UINT32_C(0);");
    writer.line("uint32_t end = (uint32_t)value.length;");
    writer.line("uint32_t code_point;");
    writer.line("uint32_t width;");
    writer.begin_block("while (start < end)");
    writer.begin_block("if (!emela_emb_utf8_decode(value, start, &code_point, &width))");
    writer.line("break;");
    writer.end_block();
    writer.begin_block("if (!emela_emb_is_white_space(code_point))");
    writer.line("break;");
    writer.end_block();
    writer.line("start += width;");
    writer.end_block();
    writer.begin_block("while (end > start)");
    writer.line("uint32_t begin = end - UINT32_C(1);");
    writer.begin_block(
        "while (begin > start && ((uint8_t)value.data[begin] & UINT8_C(0xc0)) == UINT8_C(0x80))",
    );
    writer.line("--begin;");
    writer.end_block();
    writer.begin_block(
        "if (!emela_emb_utf8_decode(value, begin, &code_point, &width) || begin + width != end)",
    );
    writer.line("break;");
    writer.end_block();
    writer.begin_block("if (!emela_emb_is_white_space(code_point))");
    writer.line("break;");
    writer.end_block();
    writer.line("end = begin;");
    writer.end_block();
    writer.line(
        "return (EmelaString){ .data = value.data + start, .length = (uint16_t)(end - start) };",
    );
    writer.end_block();
    writer.blank();
}

fn emit_statement(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    statement: &Stmt,
    writer: &mut Writer,
    names: &HashMap<FnId, String>,
    string_buffers: &StringBufferPlan,
    loops: &mut Vec<Vec<VarId>>,
    blocks: &mut Vec<BlockTarget>,
    protects: &mut Vec<ProtectTarget>,
    next_temp: &mut usize,
    next_label: &mut usize,
) -> Result<(), Vec<Error>> {
    let managed_dst = statement
        .dst()
        .filter(|&dst| layouts.is_managed_value(function.vars[dst].ty));
    if let Some(dst) = managed_dst {
        emit_release_if_owned(layouts, function, dst, writer);
    }
    let retain_result = match statement {
        Stmt::Let {
            value:
                Value::ListHead(_) | Value::ListTail(_) | Value::Field { .. } | Value::TupleGet { .. },
            ..
        } => true,
        Stmt::Let {
            value: Value::List {
                elems,
                tail: Some(_),
            },
            ..
        } => elems.is_empty(),
        _ => false,
    };
    emit_statement_inner(
        layouts,
        function,
        statement,
        writer,
        names,
        string_buffers,
        loops,
        blocks,
        protects,
        next_temp,
        next_label,
    )?;
    if let Some(dst) = managed_dst {
        if retain_result {
            layouts.emit_retain_value_expression(
                function.vars[dst].ty,
                &variable_name(dst),
                writer,
            );
        }
        writer.assign(&owned_variable_name(dst), "INT32_C(1)");
    }
    Ok(())
}

fn emit_statement_inner(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    statement: &Stmt,
    writer: &mut Writer,
    names: &HashMap<FnId, String>,
    string_buffers: &StringBufferPlan,
    loops: &mut Vec<Vec<VarId>>,
    blocks: &mut Vec<BlockTarget>,
    protects: &mut Vec<ProtectTarget>,
    next_temp: &mut usize,
    next_label: &mut usize,
) -> Result<(), Vec<Error>> {
    match statement {
        Stmt::Let {
            dst,
            value,
            on_defect,
            on_suspend,
        } => {
            if on_defect.is_some() || on_suspend.is_some() {
                return Err(vec![Error {
                    kind: ErrorKind::InvalidFlowIr,
                    message: format!(
                        "invalid Flow IR in function `{}`: Flow-stage Let has a landing edge",
                        function.name
                    ),
                }]);
            }
            match value {
                Value::Const(value) => {
                    let expression = constant_expression(value).map_err(|kind| {
                        unsupported(&function.name, &format!("{kind} in Value::Const"))
                    })?;
                    match (dst, expression) {
                        (Some(dst), Some(expression)) => {
                            writer.assign(&variable_name(*dst), &expression);
                        }
                        (None, None) => {}
                        (Some(_), None) => {
                            return Err(vec![Error {
                                kind: ErrorKind::InvalidFlowIr,
                                message: format!(
                                    "invalid Flow IR in function `{}`: Unit constant has a destination",
                                    function.name
                                ),
                            }]);
                        }
                        (None, Some(_)) => {}
                    }
                }
                Value::Prim { op, ty, args } => {
                    let expression = primitive_expression(*op, *ty, args)
                        .map_err(|variant| unsupported(&function.name, variant))?;
                    if let Some(dst) = dst {
                        writer.assign(&variable_name(*dst), &expression);
                    }
                }
                Value::Builtin { op, args } => {
                    emit_builtin(layouts, function, *dst, *op, args, string_buffers, writer)?;
                }
                Value::Runtime { op, args } => {
                    emit_runtime(layouts, function, *dst, *op, args, string_buffers, writer)?;
                }
                Value::Call {
                    callee: Callee::Direct(id),
                    args,
                    suspends: false,
                    on_fail,
                } => {
                    let name = names
                        .get(id)
                        .ok_or_else(|| unsupported(&function.name, "call to missing function"))?;
                    let mut args = args
                        .iter()
                        .map(operand_expression)
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|variant| unsupported(&function.name, variant))?;
                    if let Some(edge) = on_fail {
                        let error_name = format!("emela_call_error_{}", *next_temp);
                        *next_temp += 1;
                        writer.begin_block("");
                        writer.decl("EmelaError", &error_name, Some("{0}"));
                        args.push(format!("&{error_name}"));
                        let expression = format!("{name}({})", args.join(", "));
                        if let Some(dst) = dst {
                            writer.assign(&variable_name(*dst), &expression);
                        } else {
                            writer.line(&format!("{expression};"));
                        }
                        writer.begin_block(&format!("if ({error_name}.tag != UINT32_C(0))"));
                        emit_call_failure_edge(
                            layouts,
                            function,
                            *edge,
                            &error_name,
                            writer,
                            names,
                            string_buffers,
                            loops,
                            blocks,
                            protects,
                            next_temp,
                            next_label,
                        )?;
                        writer.end_block();
                        writer.end_block();
                    } else {
                        let expression = format!("{name}({})", args.join(", "));
                        if let Some(dst) = dst {
                            writer.assign(&variable_name(*dst), &expression);
                        } else {
                            writer.line(&format!("{expression};"));
                        }
                    }
                }
                Value::Call { .. } => {
                    return Err(unsupported(&function.name, "indirect or suspending calls"));
                }
                Value::Make { ctor, args } => {
                    emit_make(layouts, function, *dst, *ctor, args, writer)?;
                }
                Value::Field { base, ctor, index } => {
                    emit_field(layouts, function, *dst, *base, *ctor, *index, writer)?;
                }
                Value::Tag(base) => {
                    emit_tag(layouts, function, *dst, *base, writer)?;
                }
                Value::Tuple(args) => {
                    emit_tuple(layouts, function, *dst, args, writer)?;
                }
                Value::TupleGet { base, index } => {
                    emit_tuple_get(layouts, function, *dst, *base, *index, writer)?;
                }
                Value::List { elems, tail } => emit_list_value(
                    layouts,
                    function,
                    *dst,
                    elems,
                    tail.as_ref(),
                    writer,
                    next_temp,
                )?,
                Value::ListIsEmpty(list) => {
                    let Some(dst) = dst else {
                        return Err(invalid_flow_ir(
                            &function.name,
                            "Value::ListIsEmpty has no Bool destination",
                        ));
                    };
                    writer.assign(
                        &variable_name(*dst),
                        &format!(
                            "({} == NULL) ? INT32_C(1) : INT32_C(0)",
                            variable_name(*list)
                        ),
                    );
                }
                Value::ListHead(list) => {
                    emit_list_head(layouts, function, *dst, *list, writer)?;
                }
                Value::ListTail(list) => {
                    emit_list_tail(layouts, function, *dst, *list, writer)?;
                }
                Value::IsError { error, enum_id } => {
                    if !matches!(layouts.types[function.vars[*error].ty], Type::Any) {
                        return Err(invalid_flow_ir(
                            &function.name,
                            "IsError operand is not an EmelaError value",
                        ));
                    }
                    let Some(dst) = dst else {
                        return Err(invalid_flow_ir(
                            &function.name,
                            "IsError has no Bool destination",
                        ));
                    };
                    if !layouts.core.enums[*enum_id].error {
                        return Err(invalid_flow_ir(
                            &function.name,
                            "IsError target is not an error enum",
                        ));
                    }
                    writer.assign(
                        &variable_name(*dst),
                        &format!(
                            "({}).tag == UINT32_C({}) ? INT32_C(1) : INT32_C(0)",
                            variable_name(*error),
                            enum_index(*enum_id) + 1
                        ),
                    );
                }
                Value::AsError { error, enum_id } => {
                    if !matches!(layouts.types[function.vars[*error].ty], Type::Any) {
                        return Err(invalid_flow_ir(
                            &function.name,
                            "AsError operand is not an EmelaError value",
                        ));
                    }
                    let Some(dst) = dst else {
                        return Err(invalid_flow_ir(
                            &function.name,
                            "AsError has no error enum destination",
                        ));
                    };
                    if layouts.error_enum(function.vars[*dst].ty) != Some(*enum_id) {
                        return Err(invalid_flow_ir(
                            &function.name,
                            "AsError destination does not match its error enum",
                        ));
                    }
                    writer.assign(
                        &variable_name(*dst),
                        &format!(
                            "{}.payload.e{}",
                            variable_name(*error),
                            enum_index(*enum_id)
                        ),
                    );
                }
                other => return Err(unsupported(&function.name, value_name(other))),
            }
            Ok(())
        }
        Stmt::If {
            dst,
            cond,
            then,
            else_,
        } => {
            let condition =
                operand_expression(cond).map_err(|variant| unsupported(&function.name, variant))?;
            writer.begin_block(&format!("if (({condition}) != INT32_C(0))"));
            emit_compound_sequence(
                layouts,
                function,
                *then,
                *dst,
                writer,
                names,
                string_buffers,
                loops,
                blocks,
                protects,
                next_temp,
                next_label,
            )?;
            writer.end_block();
            writer.begin_block("else");
            emit_compound_sequence(
                layouts,
                function,
                *else_,
                *dst,
                writer,
                names,
                string_buffers,
                loops,
                blocks,
                protects,
                next_temp,
                next_label,
            )?;
            writer.end_block();
            Ok(())
        }
        Stmt::Switch {
            dst,
            on,
            cases,
            default,
        } => emit_switch(
            layouts,
            function,
            *dst,
            *on,
            cases,
            *default,
            writer,
            names,
            string_buffers,
            loops,
            blocks,
            protects,
            next_temp,
            next_label,
        ),
        Stmt::Loop {
            dst,
            vars,
            inits,
            body,
        } => emit_loop(
            layouts,
            function,
            *dst,
            vars,
            inits,
            *body,
            writer,
            names,
            string_buffers,
            loops,
            blocks,
            protects,
            next_temp,
            next_label,
        ),
        Stmt::Block { dst, label, body } => {
            let exit = format!("emela_block_exit_{}", *next_label);
            *next_label += 1;
            writer.begin_block("");
            blocks.push(BlockTarget {
                label: *label,
                dst: *dst,
                exit: exit.clone(),
                used: false,
                protect_depth: protects.len(),
            });
            let result = emit_compound_sequence(
                layouts,
                function,
                *body,
                *dst,
                writer,
                names,
                string_buffers,
                loops,
                blocks,
                protects,
                next_temp,
                next_label,
            );
            let block = blocks.pop().expect("block target stack underflow");
            writer.end_block();
            if block.used {
                writer.line(&format!("{exit}: ;"));
            }
            result
        }
        Stmt::Protect { dst, body, cleanup } => emit_protect(
            layouts,
            function,
            *dst,
            *body,
            *cleanup,
            writer,
            names,
            string_buffers,
            loops,
            blocks,
            protects,
            next_temp,
            next_label,
        ),
        other => Err(unsupported(&function.name, statement_name(other))),
    }
}

fn emit_switch(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    on: SwitchOn,
    cases: &[emela_lir::Case],
    default: Option<emela_lir::SeqId>,
    writer: &mut Writer,
    names: &HashMap<FnId, String>,
    string_buffers: &StringBufferPlan,
    loops: &mut Vec<Vec<VarId>>,
    blocks: &mut Vec<BlockTarget>,
    protects: &mut Vec<ProtectTarget>,
    next_temp: &mut usize,
    next_label: &mut usize,
) -> Result<(), Vec<Error>> {
    let (scrutinee, tag_switch) = match on {
        SwitchOn::Tag(value) => {
            let ty = function.vars[value].ty;
            let Type::Enum(enum_id, _) = layouts.types[ty] else {
                return Err(invalid_flow_ir(
                    &function.name,
                    "tag switch scrutinee is not an enum",
                ));
            };
            if layouts.enum_is_i32(enum_id) {
                (variable_name(value), true)
            } else if layouts.enum_fields(ty).is_some() {
                (format!("{}.tag", variable_name(value)), true)
            } else {
                return Err(invalid_flow_ir(
                    &function.name,
                    "tag switch enum has no supported C representation",
                ));
            }
        }
        SwitchOn::Lit(value) => {
            let ty = function.vars[value].ty;
            if !matches!(layouts.types[ty], Type::Int | Type::Int64) {
                return Err(invalid_flow_ir(
                    &function.name,
                    "literal switch scrutinee is not Int or Int64",
                ));
            }
            (variable_name(value), false)
        }
    };

    if !tag_switch && default.is_none() {
        return Err(invalid_flow_ir(
            &function.name,
            "literal switch has no default sequence",
        ));
    }

    writer.begin_block(&format!("switch ({scrutinee})"));
    for case in cases {
        let label = match (&case.key, on) {
            (CaseKey::Variant(variant), SwitchOn::Tag(value)) => {
                let ty = function.vars[value].ty;
                let Type::Enum(enum_id, _) = layouts.types[ty] else {
                    unreachable!("tag switch was validated above")
                };
                if *variant >= layouts.core.enums[enum_id].variants.len() {
                    return Err(invalid_flow_ir(
                        &function.name,
                        "tag switch variant is out of range",
                    ));
                }
                let tag = i32::try_from(*variant).map_err(|_| {
                    invalid_flow_ir(&function.name, "tag switch variant does not fit Int32")
                })?;
                int32_literal(tag)
            }
            (CaseKey::Lit(Const::Int(value)), SwitchOn::Lit(scrutinee))
                if matches!(layouts.types[function.vars[scrutinee].ty], Type::Int) =>
            {
                int32_literal(*value)
            }
            (CaseKey::Lit(Const::Int64(value)), SwitchOn::Lit(scrutinee))
                if matches!(layouts.types[function.vars[scrutinee].ty], Type::Int64) =>
            {
                int64_literal(*value)
            }
            _ => {
                return Err(invalid_flow_ir(
                    &function.name,
                    "switch case key does not match its scrutinee",
                ));
            }
        };
        writer.begin_block(&format!("case {label}:"));
        emit_compound_sequence(
            layouts,
            function,
            case.body,
            dst,
            writer,
            names,
            string_buffers,
            loops,
            blocks,
            protects,
            next_temp,
            next_label,
        )?;
        writer.line("break;");
        writer.end_block();
    }

    if let Some(default) = default {
        writer.begin_block("default:");
        emit_compound_sequence(
            layouts,
            function,
            default,
            dst,
            writer,
            names,
            string_buffers,
            loops,
            blocks,
            protects,
            next_temp,
            next_label,
        )?;
        writer.line("break;");
        writer.end_block();
    } else {
        writer.begin_block("default:");
        writer.line("emela_board_defect(EMELA_DEFECT_NO_MATCH, \"no match\", UINT16_C(8));");
        writer.end_block();
    }
    writer.end_block();
    Ok(())
}

fn invalid_flow_ir(function: &str, reason: &str) -> Vec<Error> {
    vec![Error {
        kind: ErrorKind::InvalidFlowIr,
        message: format!("invalid Flow IR in function `{function}`: {reason}"),
    }]
}

fn emit_list_value(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    elems: &[Operand],
    tail: Option<&Operand>,
    writer: &mut Writer,
    next_temp: &mut usize,
) -> Result<(), Vec<Error>> {
    let Some(dst) = dst else {
        return Err(invalid_flow_ir(
            &function.name,
            "Value::List has no destination",
        ));
    };
    let list_ty = function.vars[dst].ty;
    let Type::List(element_ty) = layouts.types[list_ty].clone() else {
        return Err(invalid_flow_ir(
            &function.name,
            "Value::List destination is not a List",
        ));
    };
    if elems.is_empty() {
        let expression = match tail {
            Some(tail) => {
                operand_expression(tail).map_err(|variant| unsupported(&function.name, variant))?
            }
            None => "NULL".to_owned(),
        };
        writer.assign(&variable_name(dst), &expression);
        return Ok(());
    }

    let list_name = layouts.list_node_name(list_ty);
    let list_kind = layouts.list_index(list_ty);
    let temp = format!("emela_list_build_{}", *next_temp);
    *next_temp += 1;
    let node_temp = format!("emela_list_node_tmp_{}", *next_temp);
    *next_temp += 1;
    let initial_tail = match tail {
        Some(tail) => {
            operand_expression(tail).map_err(|variant| unsupported(&function.name, variant))?
        }
        None => "NULL".to_owned(),
    };
    writer.decl("void *", &temp, Some(&initial_tail));
    writer.decl("void *", &node_temp, None);
    if tail.is_some() {
        writer.line(&format!("emela_heap_retain({temp});"));
    }
    for elem in elems.iter().rev() {
        writer.assign(
            &node_temp,
            &format!(
                "({list_name} *)emela_heap_allocate(sizeof({list_name}), UINT32_C({list_kind}))"
            ),
        );
        if layouts.type_repr(element_ty) == Repr::Void {
            writer.assign(
                &format!("(({list_name} *){node_temp})->_unit"),
                "UINT8_C(0)",
            );
        } else {
            if layouts.is_managed_value(element_ty) {
                emit_retain_operand(layouts, function, elem, writer);
            }
            let expression =
                operand_expression(elem).map_err(|variant| unsupported(&function.name, variant))?;
            writer.assign(&format!("(({list_name} *){node_temp})->head"), &expression);
        }
        writer.assign(&format!("(({list_name} *){node_temp})->tail"), &temp);
        writer.assign(&temp, &node_temp);
    }
    writer.assign(&variable_name(dst), &temp);
    Ok(())
}

fn emit_list_head(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    list: VarId,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let list_ty = function.vars[list].ty;
    let Type::List(element_ty) = layouts.types[list_ty].clone() else {
        return Err(invalid_flow_ir(
            &function.name,
            "ListHead operand is not a List",
        ));
    };
    let has_repr = layouts.type_repr(element_ty) != Repr::Void;
    if has_repr != dst.is_some() {
        return Err(invalid_flow_ir(
            &function.name,
            "ListHead destination does not match its element representation",
        ));
    }
    if let Some(dst) = dst {
        let node_type = layouts.list_node_name(list_ty);
        writer.assign(
            &variable_name(dst),
            &format!("(({node_type} *){})->head", variable_name(list)),
        );
    }
    Ok(())
}

fn emit_list_tail(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    list: VarId,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let Some(dst) = dst else {
        return Err(invalid_flow_ir(
            &function.name,
            "ListTail has no List destination",
        ));
    };
    let list_ty = function.vars[list].ty;
    if !matches!(layouts.types[list_ty], Type::List(_)) || function.vars[dst].ty != list_ty {
        return Err(invalid_flow_ir(
            &function.name,
            "ListTail operand and destination types differ",
        ));
    }
    let node_type = layouts.list_node_name(list_ty);
    writer.assign(
        &variable_name(dst),
        &format!("(({node_type} *){})->tail", variable_name(list)),
    );
    Ok(())
}

fn emit_loop(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    vars: &[VarId],
    inits: &[Operand],
    body: emela_lir::SeqId,
    writer: &mut Writer,
    names: &HashMap<FnId, String>,
    string_buffers: &StringBufferPlan,
    loops: &mut Vec<Vec<VarId>>,
    blocks: &mut Vec<BlockTarget>,
    protects: &mut Vec<ProtectTarget>,
    next_temp: &mut usize,
    next_label: &mut usize,
) -> Result<(), Vec<Error>> {
    if vars.len() != inits.len() {
        return Err(invalid_flow_ir(
            &function.name,
            "loop variable and initializer counts differ",
        ));
    }
    for (&variable, initial) in vars.iter().zip(inits) {
        let expression =
            operand_expression(initial).map_err(|variant| unsupported(&function.name, variant))?;
        emit_retain_operand(layouts, function, initial, writer);
        emit_release_if_owned(layouts, function, variable, writer);
        writer.assign(&variable_name(variable), &expression);
        if layouts.is_managed_value(function.vars[variable].ty) {
            writer.assign(&owned_variable_name(variable), "INT32_C(1)");
        }
    }
    writer.begin_block("for (;;)");
    loops.push(vars.to_vec());
    let result = emit_compound_sequence(
        layouts,
        function,
        body,
        dst,
        writer,
        names,
        string_buffers,
        loops,
        blocks,
        protects,
        next_temp,
        next_label,
    );
    loops.pop();
    result?;
    writer.line("break;");
    writer.end_block();
    Ok(())
}

fn emit_compound_sequence(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    sequence: emela_lir::SeqId,
    dst: Option<VarId>,
    writer: &mut Writer,
    names: &HashMap<FnId, String>,
    string_buffers: &StringBufferPlan,
    loops: &mut Vec<Vec<VarId>>,
    blocks: &mut Vec<BlockTarget>,
    protects: &mut Vec<ProtectTarget>,
    next_temp: &mut usize,
    next_label: &mut usize,
) -> Result<(), Vec<Error>> {
    let sequence = &function.seqs[sequence];
    for &statement in &sequence.stmts {
        emit_statement(
            layouts,
            function,
            &function.stmts[statement],
            writer,
            names,
            string_buffers,
            loops,
            blocks,
            protects,
            next_temp,
            next_label,
        )?;
    }
    match &sequence.term {
        Term::Yield(value) => match (dst, value) {
            (Some(dst), Some(value)) => {
                emit_retain_operand(layouts, function, value, writer);
                let expression = operand_expression(value)
                    .map_err(|variant| unsupported(&function.name, variant))?;
                writer.assign(&variable_name(dst), &expression);
            }
            (Some(_), None) => {
                return Err(invalid_flow_ir(
                    &function.name,
                    "compound statement destination has a valueless arm",
                ));
            }
            (None, _) => {}
        },
        Term::Recur(args) => emit_recur(layouts, function, args, loops, writer, next_temp)?,
        Term::Break { label, value } => {
            let Some(target_index) = blocks.iter().rposition(|target| target.label == *label)
            else {
                return Err(invalid_flow_ir(
                    &function.name,
                    "Break target is not an enclosing emitted block",
                ));
            };
            let target_dst = blocks[target_index].dst;
            let exit = blocks[target_index].exit.clone();
            let protect_depth = blocks[target_index].protect_depth;
            match (target_dst, value) {
                (Some(dst), Some(value)) => {
                    emit_retain_operand(layouts, function, value, writer);
                    let expression = operand_expression(value)
                        .map_err(|variant| unsupported(&function.name, variant))?;
                    writer.assign(&variable_name(dst), &expression);
                }
                (Some(_), None) => {
                    return Err(invalid_flow_ir(
                        &function.name,
                        "Break omits the enclosing block value",
                    ));
                }
                (None, _) => {}
            }
            emit_pending_cleanups(
                layouts,
                function,
                protect_depth,
                writer,
                names,
                string_buffers,
                loops,
                blocks,
                protects,
                next_temp,
                next_label,
            )?;
            blocks[target_index].used = true;
            writer.line(&format!("goto {exit};"));
        }
        Term::Return(_) => {
            emit_pending_cleanups(
                layouts,
                function,
                0,
                writer,
                names,
                string_buffers,
                loops,
                blocks,
                protects,
                next_temp,
                next_label,
            )?;
            emit_return(layouts, function, &sequence.term, writer)?;
        }
        Term::Fail(error) => {
            if !function.fails {
                return Err(invalid_flow_ir(
                    &function.name,
                    "Fail appears in a function without a failure channel",
                ));
            }
            emit_pending_cleanups(
                layouts,
                function,
                0,
                writer,
                names,
                string_buffers,
                loops,
                blocks,
                protects,
                next_temp,
                next_label,
            )?;
            emit_error_assignment(layouts, function, error, "*emela_error_out", writer)?;
            emit_failure_return(layouts, function, writer);
        }
        Term::Defect(message) => emit_defect(layouts, function, message, writer)?,
        other => return Err(unsupported(&function.name, term_name(other))),
    }
    Ok(())
}

struct BlockTarget {
    label: LabelId,
    dst: Option<VarId>,
    exit: String,
    used: bool,
    protect_depth: usize,
}

#[derive(Clone, Copy)]
struct ProtectTarget {
    cleanup: emela_lir::SeqId,
}

fn emit_recur(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    args: &[Operand],
    loops: &[Vec<VarId>],
    writer: &mut Writer,
    next_temp: &mut usize,
) -> Result<(), Vec<Error>> {
    let Some(vars) = loops.last() else {
        return Err(invalid_flow_ir(
            &function.name,
            "Recur appears outside an emitted loop",
        ));
    };
    if vars.len() != args.len() {
        return Err(invalid_flow_ir(
            &function.name,
            "Recur argument and loop variable counts differ",
        ));
    }

    let mut temporaries = Vec::with_capacity(args.len());
    for (&variable, argument) in vars.iter().zip(args) {
        let expression =
            operand_expression(argument).map_err(|variant| unsupported(&function.name, variant))?;
        let temporary = format!("emela_recur_tmp_{}", *next_temp);
        *next_temp += 1;
        writer.decl(
            &layouts.c_type(function.vars[variable].ty),
            &temporary,
            Some(&expression),
        );
        temporaries.push(temporary);
    }
    for (&variable, temporary) in vars.iter().zip(&temporaries) {
        if layouts.is_managed_value(function.vars[variable].ty) {
            layouts.emit_retain_value_expression(function.vars[variable].ty, temporary, writer);
        }
    }
    for &variable in vars {
        emit_release_if_owned(layouts, function, variable, writer);
    }
    for (&variable, temporary) in vars.iter().zip(temporaries) {
        writer.assign(&variable_name(variable), &temporary);
        if layouts.is_managed_value(function.vars[variable].ty) {
            writer.assign(&owned_variable_name(variable), "INT32_C(1)");
        }
    }
    writer.line("continue;");
    Ok(())
}

fn emit_call_failure_edge(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    edge: OnFail,
    error_name: &str,
    writer: &mut Writer,
    names: &HashMap<FnId, String>,
    string_buffers: &StringBufferPlan,
    loops: &mut Vec<Vec<VarId>>,
    blocks: &mut Vec<BlockTarget>,
    protects: &mut Vec<ProtectTarget>,
    next_temp: &mut usize,
    next_label: &mut usize,
) -> Result<(), Vec<Error>> {
    match edge {
        OnFail::Break(label) => {
            let Some(target_index) = blocks.iter().rposition(|target| target.label == label) else {
                return Err(invalid_flow_ir(
                    &function.name,
                    "failing call target is not an enclosing emitted block",
                ));
            };
            let target_dst = blocks[target_index].dst;
            let protect_depth = blocks[target_index].protect_depth;
            let Some(target_dst) = target_dst else {
                return Err(invalid_flow_ir(
                    &function.name,
                    "failing call target block has no error destination",
                ));
            };
            if !matches!(layouts.types[function.vars[target_dst].ty], Type::Any) {
                return Err(invalid_flow_ir(
                    &function.name,
                    "failing call target does not use the EmelaError representation",
                ));
            }
            writer.assign(&variable_name(target_dst), error_name);
            emit_pending_cleanups(
                layouts,
                function,
                protect_depth,
                writer,
                names,
                string_buffers,
                loops,
                blocks,
                protects,
                next_temp,
                next_label,
            )?;
            blocks[target_index].used = true;
            writer.line(&format!("goto {};", blocks[target_index].exit));
            Ok(())
        }
        OnFail::Propagate => {
            if !function.fails {
                return Err(invalid_flow_ir(
                    &function.name,
                    "failure propagates through a function without a failure channel",
                ));
            }
            emit_pending_cleanups(
                layouts,
                function,
                0,
                writer,
                names,
                string_buffers,
                loops,
                blocks,
                protects,
                next_temp,
                next_label,
            )?;
            writer.assign("*emela_error_out", error_name);
            emit_failure_return(layouts, function, writer);
            Ok(())
        }
        OnFail::Handle { .. } => Err(invalid_flow_ir(
            &function.name,
            "Heap IR failure landing appears in Flow IR",
        )),
    }
}

fn emit_error_assignment(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    error: &Operand,
    target: &str,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let Operand::Var(variable) = error else {
        return Err(invalid_flow_ir(
            &function.name,
            "Fail operand is not an error variable",
        ));
    };
    let ty = function.vars[*variable].ty;
    if matches!(layouts.types[ty], Type::Any) {
        writer.assign(target, &variable_name(*variable));
        return Ok(());
    }
    let Some(enum_id) = layouts.error_enum(ty) else {
        return Err(invalid_flow_ir(
            &function.name,
            "Fail operand is not an error enum or propagated EmelaError",
        ));
    };
    if layouts.error_type(enum_id).is_none() {
        return Err(invalid_flow_ir(
            &function.name,
            "Fail operand error enum is missing from EmelaError layout",
        ));
    }
    let object = format!("({target})");
    writer.assign(
        &format!("{object}.tag"),
        &format!("UINT32_C({})", enum_index(enum_id) + 1),
    );
    writer.assign(
        &format!("{object}.payload.e{}", enum_index(enum_id)),
        &variable_name(*variable),
    );
    Ok(())
}

fn emit_failure_return(layouts: &TypeLayouts<'_>, function: &Function, writer: &mut Writer) {
    emit_owned_cleanup(layouts, function, None, writer);
    if layouts.type_repr(function.ret) == Repr::Void {
        writer.ret(None);
    } else {
        writer.ret(Some(&format!("({}){{0}}", layouts.c_type(function.ret))));
    }
}

fn emit_defect(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    message: &Operand,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    match message {
        Operand::Const(Const::String(message)) => {
            let length = u16::try_from(message.len()).map_err(|_| {
                unsupported(
                    &function.name,
                    "a defect message longer than the embedded C ABI supports",
                )
            })?;
            writer.line(&format!(
                "emela_board_defect(EMELA_DEFECT_{}, {}, UINT16_C({length}));",
                defect_kind_for_message(message),
                c_string_literal(message.as_bytes())
            ));
            Ok(())
        }
        Operand::Var(variable)
            if matches!(layouts.types[function.vars[*variable].ty], Type::String) =>
        {
            let value = variable_name(*variable);
            writer.line(&format!(
                "emela_board_defect(emela_emb_defect_kind({value}), {value}.data, {value}.length);"
            ));
            Ok(())
        }
        _ => Err(invalid_flow_ir(
            &function.name,
            "Defect message is not a String value",
        )),
    }
}

fn defect_kind_for_message(message: &str) -> &'static str {
    if message == "not yet implemented" {
        "TODO"
    } else if message.starts_with("unreachable: no match arm matched") {
        "NO_MATCH"
    } else if message.starts_with("assertion failed") {
        "ASSERT"
    } else {
        "PANIC"
    }
}

fn has_dynamic_defect(flow: &emela_lir::Module, emitted: &HashSet<FnId>) -> bool {
    flow.functions.iter().any(|(id, function)| {
        emitted.contains(&id)
            && !function.constant
            && function.seqs.iter().any(|(_, sequence)| {
                matches!(
                    &sequence.term,
                    Term::Defect(Operand::Var(variable))
                        if matches!(flow.core.ty(function.vars[*variable].ty), Type::String)
                )
            })
    })
}

fn emit_dynamic_defect_kind_helper(writer: &mut Writer) {
    writer.begin_block(
        "static inline int emela_emb_string_matches(EmelaString value, const char *expected, uint16_t length, int exact)",
    );
    writer.begin_block("if (value.length < length || (exact && value.length != length))");
    writer.line("return 0;");
    writer.end_block();
    writer.begin_block("for (uint16_t i = 0; i < length; ++i)");
    writer.begin_block("if (value.data[i] != expected[i])");
    writer.line("return 0;");
    writer.end_block();
    writer.end_block();
    writer.line("return 1;");
    writer.end_block();
    writer.blank();
    writer.begin_block("static inline EmelaDefectKind emela_emb_defect_kind(EmelaString message)");
    for (message, kind, exact) in [
        ("not yet implemented", "TODO", "1"),
        ("unreachable: no match arm matched", "NO_MATCH", "1"),
        ("assertion failed", "ASSERT", "0"),
    ] {
        writer.begin_block(&format!(
            "if (emela_emb_string_matches(message, {}, UINT16_C({}), {exact}))",
            c_string_literal(message.as_bytes()),
            message.len(),
        ));
        writer.line(&format!("return EMELA_DEFECT_{kind};"));
        writer.end_block();
    }
    writer.line("return EMELA_DEFECT_PANIC;");
    writer.end_block();
    writer.blank();
}

fn emit_protect(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    body: emela_lir::SeqId,
    cleanup: emela_lir::SeqId,
    writer: &mut Writer,
    names: &HashMap<FnId, String>,
    string_buffers: &StringBufferPlan,
    loops: &mut Vec<Vec<VarId>>,
    blocks: &mut Vec<BlockTarget>,
    protects: &mut Vec<ProtectTarget>,
    next_temp: &mut usize,
    next_label: &mut usize,
) -> Result<(), Vec<Error>> {
    let body_yields = matches!(function.seqs[body].term, Term::Yield(_));
    writer.begin_block("");
    protects.push(ProtectTarget { cleanup });
    let body_result = emit_compound_sequence(
        layouts,
        function,
        body,
        dst,
        writer,
        names,
        string_buffers,
        loops,
        blocks,
        protects,
        next_temp,
        next_label,
    );
    protects.pop();
    let cleanup_result = if body_result.is_ok() && body_yields {
        emit_compound_sequence(
            layouts,
            function,
            cleanup,
            None,
            writer,
            names,
            string_buffers,
            loops,
            blocks,
            protects,
            next_temp,
            next_label,
        )
    } else {
        Ok(())
    };
    writer.end_block();
    body_result?;
    cleanup_result
}

fn emit_pending_cleanups(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    keep_depth: usize,
    writer: &mut Writer,
    names: &HashMap<FnId, String>,
    string_buffers: &StringBufferPlan,
    loops: &mut Vec<Vec<VarId>>,
    blocks: &mut Vec<BlockTarget>,
    protects: &mut Vec<ProtectTarget>,
    next_temp: &mut usize,
    next_label: &mut usize,
) -> Result<(), Vec<Error>> {
    if keep_depth > protects.len() {
        return Err(invalid_flow_ir(
            &function.name,
            "control transfer escapes a block outside the active protect stack",
        ));
    }
    let saved = protects.clone();
    for index in (keep_depth..saved.len()).rev() {
        protects.truncate(index);
        let result = emit_compound_sequence(
            layouts,
            function,
            saved[index].cleanup,
            None,
            writer,
            names,
            string_buffers,
            loops,
            blocks,
            protects,
            next_temp,
            next_label,
        );
        *protects = saved.clone();
        result?;
    }
    Ok(())
}

fn emit_make(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    ctor: CtorRef,
    args: &[Operand],
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let Some(dst) = dst else {
        return Ok(());
    };
    let ty = function.vars[dst].ty;
    let Type::Enum(enum_id, _) = layouts.types[ty] else {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: enum constructor defines a non-enum value",
                function.name
            ),
        }]);
    };
    if enum_id != ctor.enum_id {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: enum constructor and destination types differ",
                function.name
            ),
        }]);
    }
    let Some(variant) = layouts.core.enums[enum_id].variants.get(ctor.variant) else {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: enum constructor index is out of range",
                function.name
            ),
        }]);
    };
    if args.len() != variant.tys.len() {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: enum constructor has {} arguments, expected {}",
                function.name,
                args.len(),
                variant.tys.len()
            ),
        }]);
    }

    let target = variable_name(dst);
    if layouts.enum_is_i32(enum_id) {
        let tag = i32::try_from(ctor.variant)
            .map_err(|_| unsupported(&function.name, "enum with too many variants"))?;
        writer.assign(&target, &int32_literal(tag));
        return Ok(());
    }

    let Some(fields) = layouts
        .enum_fields(ty)
        .and_then(|variants| variants.get(ctor.variant))
    else {
        return Err(unsupported(
            &function.name,
            "enum value without an inline C layout",
        ));
    };
    let tag = i32::try_from(ctor.variant)
        .map_err(|_| unsupported(&function.name, "enum with too many variants"))?;
    writer.assign(&format!("{target}.tag"), &int32_literal(tag));

    let mut represented = false;
    for (index, (field_ty, arg)) in fields.iter().zip(args).enumerate() {
        if layouts.type_repr(*field_ty) == Repr::Void {
            continue;
        }
        represented = true;
        if layouts.is_managed_value(*field_ty) {
            emit_retain_operand(layouts, function, arg, writer);
        }
        let expression =
            operand_expression(arg).map_err(|variant| unsupported(&function.name, variant))?;
        writer.assign(
            &format!("{target}.payload.v{}.f{index}", ctor.variant),
            &expression,
        );
    }
    if !represented {
        writer.assign(
            &format!("{target}.payload.v{}._unit", ctor.variant),
            "UINT8_C(0)",
        );
    }
    Ok(())
}

fn emit_field(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    base: VarId,
    ctor: CtorRef,
    index: usize,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let base_ty = function.vars[base].ty;
    let Type::Enum(enum_id, _) = layouts.types[base_ty] else {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: field base is not an enum",
                function.name
            ),
        }]);
    };
    if enum_id != ctor.enum_id {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: field constructor and base types differ",
                function.name
            ),
        }]);
    }
    let Some(fields) = layouts
        .enum_fields(base_ty)
        .and_then(|variants| variants.get(ctor.variant))
    else {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: field base has no inline enum layout",
                function.name
            ),
        }]);
    };
    let Some(&field_ty) = fields.get(index) else {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: enum field index is out of range",
                function.name
            ),
        }]);
    };
    let has_repr = layouts.type_repr(field_ty) != Repr::Void;
    if has_repr != dst.is_some() {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: enum field destination does not match its representation",
                function.name
            ),
        }]);
    }
    if let Some(dst) = dst {
        writer.assign(
            &variable_name(dst),
            &format!("{}.payload.v{}.f{index}", variable_name(base), ctor.variant),
        );
    }
    Ok(())
}

fn emit_tag(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    base: VarId,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let Some(dst) = dst else {
        return Ok(());
    };
    let base_ty = function.vars[base].ty;
    let Type::Enum(enum_id, _) = layouts.types[base_ty] else {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: tag base is not an enum",
                function.name
            ),
        }]);
    };
    let expression = if layouts.enum_is_i32(enum_id) {
        variable_name(base)
    } else if layouts.enum_fields(base_ty).is_some() {
        format!("{}.tag", variable_name(base))
    } else {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: tag base has no supported enum representation",
                function.name
            ),
        }]);
    };
    writer.assign(&variable_name(dst), &expression);
    Ok(())
}

fn emit_tuple(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    args: &[Operand],
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let Some(dst) = dst else {
        return Ok(());
    };
    let ty = function.vars[dst].ty;
    let Type::Tuple(fields) = &layouts.types[ty] else {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: tuple value defines a non-tuple variable",
                function.name
            ),
        }]);
    };
    if args.len() != fields.len() {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: tuple has {} elements, expected {}",
                function.name,
                args.len(),
                fields.len()
            ),
        }]);
    }

    let target = variable_name(dst);
    let mut represented = false;
    for (index, (&field_ty, arg)) in fields.iter().zip(args).enumerate() {
        if layouts.type_repr(field_ty) == Repr::Void {
            continue;
        }
        represented = true;
        if layouts.is_managed_value(field_ty) {
            emit_retain_operand(layouts, function, arg, writer);
        }
        let expression =
            operand_expression(arg).map_err(|variant| unsupported(&function.name, variant))?;
        writer.assign(&format!("{target}.f{index}"), &expression);
    }
    if !represented {
        writer.assign(&format!("{target}._unit"), "UINT8_C(0)");
    }
    Ok(())
}

fn emit_tuple_get(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    base: VarId,
    index: usize,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    let base_ty = function.vars[base].ty;
    let Type::Tuple(fields) = &layouts.types[base_ty] else {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: tuple projection base is not a tuple",
                function.name
            ),
        }]);
    };
    let Some(&field_ty) = fields.get(index) else {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: tuple field index is out of range",
                function.name
            ),
        }]);
    };
    let has_repr = layouts.type_repr(field_ty) != Repr::Void;
    if has_repr != dst.is_some() {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: tuple projection destination does not match its representation",
                function.name
            ),
        }]);
    }
    if let Some(dst) = dst {
        writer.assign(
            &variable_name(dst),
            &format!("{}.f{index}", variable_name(base)),
        );
    }
    Ok(())
}

fn validate_external(flow: &emela_lir::Module, function: &Function) -> Result<String, Vec<Error>> {
    let Some(external) = &function.external else {
        return Err(unsupported(&function.name, "missing external binding"));
    };
    let Some(binding) = external.binding("c") else {
        return Err(unsupported(
            &function.name,
            "external function without a C binding",
        ));
    };
    let symbol = match &binding.host {
        emela_core::HostRef::Global(name) => name.clone(),
        emela_core::HostRef::Import { name, .. } => name.clone(),
    };
    if !is_c_identifier(&symbol) {
        return Err(unsupported(
            &function.name,
            "external function with an invalid C symbol",
        ));
    }

    let supported_type = |ty| {
        matches!(
            flow.core.ty(ty),
            emela_core::Type::Int
                | emela_core::Type::Bool
                | emela_core::Type::Unit
                | emela_core::Type::Never
        )
    };
    if function
        .params
        .iter()
        .any(|&param| !supported_type(function.vars[param].ty))
        || !supported_type(function.ret)
    {
        return Err(unsupported(
            &function.name,
            "external function with a non-Int32 or non-Unit signature",
        ));
    }
    Ok(symbol)
}

fn is_c_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first.is_ascii_alphabetic())
        || !chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
    {
        return false;
    }
    !matches!(
        value,
        "auto"
            | "break"
            | "case"
            | "char"
            | "const"
            | "continue"
            | "default"
            | "do"
            | "double"
            | "else"
            | "enum"
            | "extern"
            | "float"
            | "for"
            | "goto"
            | "if"
            | "inline"
            | "int"
            | "long"
            | "register"
            | "restrict"
            | "return"
            | "short"
            | "signed"
            | "sizeof"
            | "static"
            | "struct"
            | "switch"
            | "typedef"
            | "union"
            | "unsigned"
            | "void"
            | "volatile"
            | "while"
            | "_Alignas"
            | "_Alignof"
            | "_Atomic"
            | "_Bool"
            | "_Complex"
            | "_Generic"
            | "_Imaginary"
            | "_Noreturn"
            | "_Static_assert"
            | "_Thread_local"
    )
}

fn primitive_expression(op: PrimOp, ty: PrimTy, args: &[Operand]) -> Result<String, &'static str> {
    let args: Vec<String> = args
        .iter()
        .map(operand_expression)
        .collect::<Result<_, _>>()?;
    match ty {
        PrimTy::I32 => primitive_i32_expression(op, args.as_slice()),
        PrimTy::I64 => primitive_i64_expression(op, args.as_slice()),
        PrimTy::F64 => primitive_f64_expression(op, args.as_slice()),
        _ => Err("unsupported Value::Prim type"),
    }
}

fn primitive_i32_expression(op: PrimOp, args: &[String]) -> Result<String, &'static str> {
    match (op, args) {
        (PrimOp::Add, [lhs, rhs]) => Ok(format!(
            "emela_i32_from_bits((uint32_t)((uint64_t)(uint32_t)({lhs}) + (uint64_t)(uint32_t)({rhs})))"
        )),
        (PrimOp::Sub, [lhs, rhs]) => Ok(format!(
            "emela_i32_from_bits((uint32_t)((uint64_t)(uint32_t)({lhs}) - (uint64_t)(uint32_t)({rhs})))"
        )),
        (PrimOp::Mul, [lhs, rhs]) => Ok(format!(
            "emela_i32_from_bits((uint32_t)((uint64_t)(uint32_t)({lhs}) * (uint64_t)(uint32_t)({rhs})))"
        )),
        (PrimOp::Neg, [value]) => Ok(format!(
            "emela_i32_from_bits((uint32_t)(UINT64_C(0) - (uint64_t)(uint32_t)({value})))"
        )),
        (PrimOp::Not, [value]) => Ok(format!(
            "(({value}) == INT32_C(0) ? INT32_C(1) : INT32_C(0))"
        )),
        (PrimOp::Eq, [lhs, rhs]) => comparison_expression(lhs, "==", rhs),
        (PrimOp::Ne, [lhs, rhs]) => comparison_expression(lhs, "!=", rhs),
        (PrimOp::Lt, [lhs, rhs]) => comparison_expression(lhs, "<", rhs),
        (PrimOp::Le, [lhs, rhs]) => comparison_expression(lhs, "<=", rhs),
        (PrimOp::Gt, [lhs, rhs]) => comparison_expression(lhs, ">", rhs),
        (PrimOp::Ge, [lhs, rhs]) => comparison_expression(lhs, ">=", rhs),
        (PrimOp::Div, [lhs, rhs]) => Ok(format!("emela_i32_div({lhs}, {rhs})")),
        (PrimOp::Rem, [lhs, rhs]) => Ok(format!("emela_i32_rem({lhs}, {rhs})")),
        _ => Err("Value::Prim with an invalid arity"),
    }
}

fn primitive_i64_expression(op: PrimOp, args: &[String]) -> Result<String, &'static str> {
    match (op, args) {
        (PrimOp::Add, [lhs, rhs]) => Ok(format!(
            "emela_i64_from_bits((uint64_t)({lhs}) + (uint64_t)({rhs}))"
        )),
        (PrimOp::Sub, [lhs, rhs]) => Ok(format!(
            "emela_i64_from_bits((uint64_t)({lhs}) - (uint64_t)({rhs}))"
        )),
        (PrimOp::Mul, [lhs, rhs]) => Ok(format!(
            "emela_i64_from_bits((uint64_t)({lhs}) * (uint64_t)({rhs}))"
        )),
        (PrimOp::Neg, [value]) => Ok(format!(
            "emela_i64_from_bits(UINT64_C(0) - (uint64_t)({value}))"
        )),
        (PrimOp::Eq, [lhs, rhs]) => comparison_expression(lhs, "==", rhs),
        (PrimOp::Ne, [lhs, rhs]) => comparison_expression(lhs, "!=", rhs),
        (PrimOp::Lt, [lhs, rhs]) => comparison_expression(lhs, "<", rhs),
        (PrimOp::Le, [lhs, rhs]) => comparison_expression(lhs, "<=", rhs),
        (PrimOp::Gt, [lhs, rhs]) => comparison_expression(lhs, ">", rhs),
        (PrimOp::Ge, [lhs, rhs]) => comparison_expression(lhs, ">=", rhs),
        (PrimOp::Div, [lhs, rhs]) => Ok(format!("emela_i64_div({lhs}, {rhs})")),
        (PrimOp::Rem, [lhs, rhs]) => Ok(format!("emela_i64_rem({lhs}, {rhs})")),
        _ => Err("Value::Prim with an invalid arity"),
    }
}

fn primitive_f64_expression(op: PrimOp, args: &[String]) -> Result<String, &'static str> {
    match (op, args) {
        (PrimOp::Add, [lhs, rhs]) => Ok(format!("(({lhs}) + ({rhs}))")),
        (PrimOp::Sub, [lhs, rhs]) => Ok(format!("(({lhs}) - ({rhs}))")),
        (PrimOp::Mul, [lhs, rhs]) => Ok(format!("(({lhs}) * ({rhs}))")),
        (PrimOp::Div, [lhs, rhs]) => Ok(format!("(({lhs}) / ({rhs}))")),
        (PrimOp::Rem, [lhs, rhs]) => Ok(format!("fmod(({lhs}), ({rhs}))")),
        (PrimOp::Neg, [value]) => Ok(format!("(-({value}))")),
        (PrimOp::Eq, [lhs, rhs]) => comparison_expression(lhs, "==", rhs),
        (PrimOp::Ne, [lhs, rhs]) => comparison_expression(lhs, "!=", rhs),
        (PrimOp::Lt, [lhs, rhs]) => comparison_expression(lhs, "<", rhs),
        (PrimOp::Le, [lhs, rhs]) => comparison_expression(lhs, "<=", rhs),
        (PrimOp::Gt, [lhs, rhs]) => comparison_expression(lhs, ">", rhs),
        (PrimOp::Ge, [lhs, rhs]) => comparison_expression(lhs, ">=", rhs),
        _ => Err("Value::Prim with an invalid arity"),
    }
}

fn emit_i32_shift_helpers(writer: &mut Writer) {
    writer.begin_block("static inline int32_t emela_i32_shift_left(int32_t value, int32_t by)");
    writer.begin_block("if (by >= INT32_C(32))");
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.line("return emela_i32_from_bits((uint32_t)value << (uint32_t)by);");
    writer.end_block();
    writer.blank();

    writer.begin_block("static inline int32_t emela_i32_shift_right(int32_t value, int32_t by)");
    writer.begin_block("if (by >= INT32_C(32))");
    writer.line("return value < INT32_C(0) ? -INT32_C(1) : INT32_C(0);");
    writer.end_block();
    writer.line("uint32_t shifted = (uint32_t)value >> (uint32_t)by;");
    writer.begin_block("if (by > INT32_C(0) && value < INT32_C(0))");
    writer.line("shifted |= UINT32_MAX << (UINT32_C(32) - (uint32_t)by);");
    writer.end_block();
    writer.line("return emela_i32_from_bits(shifted);");
    writer.end_block();
    writer.blank();

    writer.begin_block(
        "static inline int32_t emela_i32_shift_right_unsigned(int32_t value, int32_t by)",
    );
    writer.begin_block("if (by >= INT32_C(32))");
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.line("return emela_i32_from_bits((uint32_t)value >> (uint32_t)by);");
    writer.end_block();
    writer.blank();
}

fn emit_i64_shift_helpers(writer: &mut Writer) {
    writer.begin_block("static inline int64_t emela_i64_shift_left(int64_t value, int32_t by)");
    writer.begin_block("if (by >= INT32_C(64))");
    writer.line("return INT64_C(0);");
    writer.end_block();
    writer.line("return emela_i64_from_bits((uint64_t)value << (uint32_t)by);");
    writer.end_block();
    writer.blank();

    writer.begin_block("static inline int64_t emela_i64_shift_right(int64_t value, int32_t by)");
    writer.begin_block("if (by >= INT32_C(64))");
    writer.line("return value < INT64_C(0) ? -INT64_C(1) : INT64_C(0);");
    writer.end_block();
    writer.line("uint64_t shifted = (uint64_t)value >> (uint32_t)by;");
    writer.begin_block("if (by > INT32_C(0) && value < INT64_C(0))");
    writer.line("shifted |= UINT64_MAX << (UINT32_C(64) - (uint32_t)by);");
    writer.end_block();
    writer.line("return emela_i64_from_bits(shifted);");
    writer.end_block();
    writer.blank();

    writer.begin_block(
        "static inline int64_t emela_i64_shift_right_unsigned(int64_t value, int32_t by)",
    );
    writer.begin_block("if (by >= INT32_C(64))");
    writer.line("return INT64_C(0);");
    writer.end_block();
    writer.line("return emela_i64_from_bits((uint64_t)value >> (uint32_t)by);");
    writer.end_block();
    writer.blank();
}

fn emit_negative_shift_helper(writer: &mut Writer) {
    writer.begin_block("static inline void emela_emb_negative_shift_defect(int32_t amount)");
    writer.line("char message[48];");
    writer.line("static const char prefix[] = \"negative shift amount: \";");
    writer.line("uint16_t prefix_length = (uint16_t)(sizeof prefix - 1);");
    writer.line("uint32_t magnitude = (uint32_t)(-(amount + INT32_C(1))) + UINT32_C(1);");
    writer.line("char *digits = message + sizeof message;");
    writer.begin_block("do");
    writer.line("*--digits = (char)('0' + magnitude % UINT32_C(10));");
    writer.line("magnitude /= UINT32_C(10);");
    writer.end_block();
    writer.line("while (magnitude != UINT32_C(0));");
    writer.line("*--digits = '-';");
    writer.line("uint16_t number_length = (uint16_t)((message + sizeof message) - digits);");
    writer.begin_block("for (uint16_t i = 0; i < prefix_length; ++i)");
    writer.line("message[i] = prefix[i];");
    writer.end_block();
    writer.begin_block("for (uint16_t i = 0; i < number_length; ++i)");
    writer.line("message[prefix_length + i] = digits[i];");
    writer.end_block();
    writer.line(
        "emela_board_defect(EMELA_DEFECT_NEGATIVE_SHIFT, message, (uint16_t)(prefix_length + number_length));",
    );
    writer.end_block();
    writer.blank();
}

fn emit_i32_division_helpers(writer: &mut Writer) {
    writer.begin_block("static inline int32_t emela_i32_div(int32_t lhs, int32_t rhs)");
    writer.begin_block("if (rhs == INT32_C(0))");
    writer.line(
        "emela_board_defect(EMELA_DEFECT_DIVISION_BY_ZERO, \"division by zero\", UINT16_C(16));",
    );
    writer.end_block();
    writer.begin_block("if (lhs == INT32_MIN && rhs == -INT32_C(1))");
    writer.line("return INT32_MIN;");
    writer.end_block();
    writer.line("return lhs / rhs;");
    writer.end_block();
    writer.blank();

    writer.begin_block("static inline int32_t emela_i32_rem(int32_t lhs, int32_t rhs)");
    writer.begin_block("if (rhs == INT32_C(0))");
    writer.line(
        "emela_board_defect(EMELA_DEFECT_DIVISION_BY_ZERO, \"division by zero\", UINT16_C(16));",
    );
    writer.end_block();
    writer.begin_block("if (lhs == INT32_MIN && rhs == -INT32_C(1))");
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.line("return lhs % rhs;");
    writer.end_block();
    writer.blank();
}

fn emit_i64_wrap_helper(writer: &mut Writer) {
    writer.begin_block("static inline int64_t emela_i64_from_bits(uint64_t bits)");
    writer.begin_block("if (bits <= (uint64_t)INT64_MAX)");
    writer.line("return (int64_t)bits;");
    writer.end_block();
    writer.line("return -INT64_C(1) - (int64_t)(UINT64_MAX - bits);");
    writer.end_block();
    writer.blank();
}

fn emit_i64_division_helpers(writer: &mut Writer) {
    writer.begin_block("static inline int64_t emela_i64_div(int64_t lhs, int64_t rhs)");
    writer.begin_block("if (rhs == INT64_C(0))");
    writer.line(
        "emela_board_defect(EMELA_DEFECT_DIVISION_BY_ZERO, \"division by zero\", UINT16_C(16));",
    );
    writer.end_block();
    writer.begin_block("if (lhs == INT64_MIN && rhs == -INT64_C(1))");
    writer.line("return INT64_MIN;");
    writer.end_block();
    writer.line("return lhs / rhs;");
    writer.end_block();
    writer.blank();

    writer.begin_block("static inline int64_t emela_i64_rem(int64_t lhs, int64_t rhs)");
    writer.begin_block("if (rhs == INT64_C(0))");
    writer.line(
        "emela_board_defect(EMELA_DEFECT_DIVISION_BY_ZERO, \"division by zero\", UINT16_C(16));",
    );
    writer.end_block();
    writer.begin_block("if (lhs == INT64_MIN && rhs == -INT64_C(1))");
    writer.line("return INT64_C(0);");
    writer.end_block();
    writer.line("return lhs % rhs;");
    writer.end_block();
    writer.blank();
}

fn emit_float_to_int_helper(writer: &mut Writer) {
    writer.begin_block(
        "static inline int32_t emela_emb_float_to_int(double value, uint8_t operation)",
    );
    writer.begin_block("if (!(value > -2147483649.0 && value < 2147483648.0))");
    writer.line(
        "emela_board_defect(EMELA_DEFECT_FLOAT_TO_INT, \"cannot convert Float to Int\", UINT16_C(27));",
    );
    writer.end_block();
    writer.line("int64_t rounded = (int64_t)value;");
    writer.begin_block("if (operation == UINT8_C(0) && (double)rounded > value)");
    writer.line("--rounded;");
    writer.end_block();
    writer.begin_block("else if (operation == UINT8_C(1) && (double)rounded < value)");
    writer.line("++rounded;");
    writer.end_block();
    writer.begin_block("else if (operation == UINT8_C(2))");
    writer.begin_block("if (value >= 0.0 && value - (double)rounded >= 0.5)");
    writer.line("++rounded;");
    writer.end_block();
    writer.begin_block("else if (value < 0.0 && (double)rounded - value >= 0.5)");
    writer.line("--rounded;");
    writer.end_block();
    writer.end_block();
    writer.begin_block("if (rounded < INT32_MIN || rounded > INT32_MAX)");
    writer.line(
        "emela_board_defect(EMELA_DEFECT_FLOAT_TO_INT, \"cannot convert Float to Int\", UINT16_C(27));",
    );
    writer.end_block();
    writer.line("return (int32_t)rounded;");
    writer.end_block();
    writer.blank();
}

fn has_prim(flow: &emela_lir::Module, emitted: &HashSet<FnId>, ty: PrimTy, ops: &[PrimOp]) -> bool {
    flow.functions.iter().any(|(id, function)| {
        emitted.contains(&id)
            && !function.constant
            && function.stmts.iter().any(|(_, statement)| {
                matches!(
                    statement,
                    Stmt::Let {
                        value: Value::Prim {
                            op, ty: found_ty, ..
                        },
                        ..
                    } if *found_ty == ty && ops.contains(op)
                )
            })
    })
}

fn has_builtin(flow: &emela_lir::Module, emitted: &HashSet<FnId>, ops: &[Builtin]) -> bool {
    flow.functions.iter().any(|(id, function)| {
        emitted.contains(&id)
            && !function.constant
            && function.stmts.iter().any(|(_, statement)| {
                matches!(
                    statement,
                    Stmt::Let {
                        value: Value::Builtin { op, .. },
                        ..
                    } if ops.contains(op)
                )
            })
    })
}

fn has_runtime_string_append(flow: &emela_lir::Module, emitted: &HashSet<FnId>) -> bool {
    flow.functions
        .iter()
        .filter(|(id, function)| emitted.contains(id) && !function.constant)
        .any(|(_, function)| {
            function.stmts.iter().any(|(_, statement)| {
                matches!(
                    statement,
                    Stmt::Let {
                        value: Value::Builtin {
                            op: Builtin::StringAppend,
                            args,
                        },
                        ..
                    } if args.len() == 2 && !append_is_static(args) && !append_has_empty_constant(args)
                )
            })
        })
}

fn has_runtime_int_string(flow: &emela_lir::Module, emitted: &HashSet<FnId>) -> bool {
    flow.functions
        .iter()
        .filter(|(id, function)| emitted.contains(id) && !function.constant)
        .any(|(_, function)| {
            function.stmts.iter().any(|(_, statement)| {
                matches!(
                    statement,
                    Stmt::Let {
                        value: Value::Builtin {
                            op: Builtin::IntToString | Builtin::Int64ToString,
                            args,
                        },
                        ..
                    } if args.len() == 1 && matches!(args[0], Operand::Var(_))
                )
            })
        })
}

fn has_runtime_int_parse(flow: &emela_lir::Module, emitted: &HashSet<FnId>) -> bool {
    flow.functions
        .iter()
        .filter(|(id, function)| emitted.contains(id) && !function.constant)
        .any(|(_, function)| {
            function.stmts.iter().any(|(_, statement)| {
                matches!(
                    statement,
                    Stmt::Let {
                        value: Value::Builtin {
                            op: Builtin::IntParse,
                            args,
                        },
                        ..
                    } if args.len() == 1 && matches!(args[0], Operand::Var(_))
                )
            })
        })
}

fn has_runtime_string_from_code_point(flow: &emela_lir::Module, emitted: &HashSet<FnId>) -> bool {
    flow.functions
        .iter()
        .filter(|(id, function)| emitted.contains(id) && !function.constant)
        .any(|(_, function)| {
            function.stmts.iter().any(|(_, statement)| {
                matches!(
                    statement,
                    Stmt::Let {
                        value: Value::Builtin {
                            op: Builtin::StringFromCodePoint,
                            args,
                        },
                        ..
                    } if args.len() == 1 && matches!(args[0], Operand::Var(_))
                )
            })
        })
}

fn has_runtime_string_show(flow: &emela_lir::Module, emitted: &HashSet<FnId>) -> bool {
    flow.functions
        .iter()
        .filter(|(id, function)| emitted.contains(id) && !function.constant)
        .any(|(_, function)| {
            function.stmts.iter().any(|(_, statement)| {
                matches!(
                    statement,
                    Stmt::Let {
                        value: Value::Runtime {
                            op: RuntimeOp::ShowString,
                            args,
                        },
                        ..
                    } if !matches!(args.as_slice(), [Operand::Const(Const::String(_))])
                )
            })
        })
}

fn has_runtime_op(flow: &emela_lir::Module, emitted: &HashSet<FnId>, op: RuntimeOp) -> bool {
    flow.functions
        .iter()
        .filter(|(id, function)| emitted.contains(id) && !function.constant)
        .any(|(_, function)| {
            function.stmts.iter().any(|(_, statement)| {
                matches!(
                    statement,
                    Stmt::Let {
                        value: Value::Runtime { op: found, .. },
                        ..
                    } if *found == op
                )
            })
        })
}

fn comparison_expression(lhs: &str, op: &str, rhs: &str) -> Result<String, &'static str> {
    Ok(format!("(({lhs}) {op} ({rhs}) ? INT32_C(1) : INT32_C(0))"))
}

fn emit_return(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    term: &Term,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    match term {
        Term::Return(None) => {
            emit_owned_cleanup(layouts, function, None, writer);
            writer.ret(None);
        }
        Term::Return(Some(operand)) => {
            let expression = operand_expression(operand)
                .map_err(|variant| unsupported(&function.name, variant))?;
            let mut moved = None;
            if layouts.is_managed_value(function.ret)
                && let Operand::Var(variable) = operand
            {
                if function.params.contains(variable) {
                    emit_retain_operand(layouts, function, operand, writer);
                } else {
                    writer.assign(&owned_variable_name(*variable), "INT32_C(0)");
                    moved = Some(*variable);
                }
            }
            emit_owned_cleanup(layouts, function, moved, writer);
            writer.ret(Some(&expression));
        }
        Term::Fail(error) => {
            if !function.fails {
                return Err(invalid_flow_ir(
                    &function.name,
                    "Fail appears in a function without a failure channel",
                ));
            }
            emit_error_assignment(layouts, function, error, "*emela_error_out", writer)?;
            emit_failure_return(layouts, function, writer);
        }
        Term::Defect(message) => emit_defect(layouts, function, message, writer)?,
        other => return Err(unsupported(&function.name, term_name(other))),
    }

    let return_repr = layouts.type_repr(function.ret);
    if return_repr == Repr::Void && matches!(term, Term::Return(Some(_))) {
        return Err(vec![Error {
            kind: ErrorKind::InvalidFlowIr,
            message: format!(
                "invalid Flow IR in function `{}`: void return has a value",
                function.name
            ),
        }]);
    }
    Ok(())
}

fn operand_expression(operand: &Operand) -> Result<String, &'static str> {
    match operand {
        Operand::Var(variable) => Ok(variable_name(*variable)),
        Operand::Const(value) => constant_expression(value)?.ok_or("Unit return operand"),
    }
}

fn constant_expression(value: &emela_lir::Const) -> Result<Option<String>, &'static str> {
    match value {
        emela_lir::Const::Int(value) => Ok(Some(int32_literal(*value))),
        emela_lir::Const::Bool(value) => Ok(Some(if *value {
            "INT32_C(1)".to_owned()
        } else {
            "INT32_C(0)".to_owned()
        })),
        emela_lir::Const::Unit => Ok(None),
        emela_lir::Const::Int64(value) => Ok(Some(int64_literal(*value))),
        emela_lir::Const::Float(value) => Ok(Some(float_literal(*value))),
        emela_lir::Const::String(value) => {
            let length = u16::try_from(value.len()).map_err(|_| "String constant is too long")?;
            Ok(Some(format!(
                "(EmelaString){{ .data = {}, .length = UINT16_C({length}) }}",
                c_string_literal(value.as_bytes())
            )))
        }
    }
}

fn c_string_literal(bytes: &[u8]) -> String {
    let escaped = bytes
        .iter()
        .map(|byte| format!("\\x{byte:02x}"))
        .collect::<String>();
    format!("\"{escaped}\"")
}

fn int32_literal(value: i32) -> String {
    if value == i32::MIN {
        "(-INT32_C(2147483647) - INT32_C(1))".to_owned()
    } else if value < 0 {
        format!("(-INT32_C({}))", -value)
    } else {
        format!("INT32_C({value})")
    }
}

fn int64_literal(value: i64) -> String {
    if value == i64::MIN {
        "(-INT64_C(9223372036854775807) - INT64_C(1))".to_owned()
    } else {
        format!("INT64_C({value})")
    }
}

fn float_literal(value: f64) -> String {
    if value.is_nan() {
        "NAN".to_owned()
    } else if value == f64::INFINITY {
        "INFINITY".to_owned()
    } else if value == f64::NEG_INFINITY {
        "(-INFINITY)".to_owned()
    } else {
        let literal = format!("{value:?}");
        if literal.contains(['.', 'e', 'E']) {
            literal
        } else {
            format!("{literal}.0")
        }
    }
}

fn signature(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    name: &str,
    is_static: bool,
    takes_error_out: bool,
) -> String {
    let return_type = layouts.c_type(function.ret);
    let mut params: Vec<String> = function
        .params
        .iter()
        .map(|&variable| {
            let data = &function.vars[variable];
            format!("{} {}", layouts.c_type(data.ty), variable_name(variable))
        })
        .collect::<Vec<_>>();
    if takes_error_out {
        params.push("EmelaError *emela_error_out".to_owned());
    }
    let params = if params.is_empty() {
        "void".to_owned()
    } else {
        params.join(", ")
    };
    let linkage = if is_static { "static " } else { "" };
    format!("{linkage}{return_type} {name}({params})")
}

fn emit_fallible_wrapper(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    public_name: &str,
    implementation_name: &str,
    writer: &mut Writer,
) {
    writer.begin_block(&signature(layouts, function, public_name, false, false));
    writer.decl("EmelaError", "emela_error", Some("{0}"));
    let mut args: Vec<String> = function
        .params
        .iter()
        .map(|&variable| variable_name(variable))
        .collect();
    args.push("&emela_error".to_owned());
    let call = format!("{implementation_name}({})", args.join(", "));
    if layouts.type_repr(function.ret) == Repr::Void {
        writer.line(&format!("{call};"));
    } else {
        writer.decl(&layouts.c_type(function.ret), "emela_result", Some(&call));
    }
    writer.begin_block("if (emela_error.tag != UINT32_C(0))");
    writer.line(
        "emela_board_defect(EMELA_DEFECT_UNHANDLED_ERROR, \"unhandled error\", UINT16_C(15));",
    );
    writer.end_block();
    if layouts.type_repr(function.ret) == Repr::Void {
        writer.ret(None);
    } else {
        writer.ret(Some("emela_result"));
    }
    writer.end_block();
}

#[derive(Debug)]
struct AggregateLayout {
    ty: TypeId,
    kind: AggregateKind,
}

#[derive(Debug)]
enum AggregateKind {
    Tuple(Vec<TypeId>),
    Enum(Vec<Vec<TypeId>>),
}

struct TypeLayouts<'a> {
    core: &'a emela_core::Module,
    types: TypeTable,
    aggregates: Vec<AggregateLayout>,
    aggregate_ids: HashMap<TypeId, usize>,
    list_types: Vec<TypeId>,
    list_ids: HashMap<TypeId, usize>,
    error_types: Vec<(EnumId, TypeId)>,
    has_error_abi: bool,
}

impl<'a> TypeLayouts<'a> {
    fn new(flow: &'a emela_lir::Module, emitted: &HashSet<FnId>) -> Result<Self, Vec<Error>> {
        let mut layouts = TypeLayouts {
            core: &flow.core,
            types: flow.core.types.clone(),
            aggregates: Vec::new(),
            aggregate_ids: HashMap::new(),
            list_types: Vec::new(),
            list_ids: HashMap::new(),
            error_types: Vec::new(),
            has_error_abi: flow.functions.iter().any(|(id, function)| {
                emitted.contains(&id)
                    && (function.fails
                        || function
                            .seqs
                            .iter()
                            .any(|(_, sequence)| matches!(sequence.term, Term::Fail(_)))
                        || function.stmts.iter().any(|(_, statement)| match statement {
                            Stmt::Let { value, .. } => match value {
                                Value::Call { on_fail, .. } => on_fail.is_some(),
                                Value::IsError { .. } | Value::AsError { .. } => true,
                                _ => false,
                            },
                            _ => false,
                        }))
            }),
        };
        let mut states = HashMap::new();
        if layouts.has_error_abi {
            let error_ids: Vec<EnumId> = flow
                .core
                .enums
                .iter()
                .filter_map(|(id, def)| def.error.then_some(id))
                .collect();
            for id in error_ids {
                let ty = layouts.types.enum_type(id, Vec::new());
                layouts.collect(ty, "EmelaError", &mut states)?;
                layouts.error_types.push((id, ty));
            }
            if layouts
                .error_types
                .iter()
                .any(|(_, ty)| layouts.contains_list_ref(*ty, &mut HashSet::new()))
            {
                return Err(unsupported(
                    "EmelaError",
                    "error enum values containing a List reference",
                ));
            }
        }
        for (id, function) in flow.functions.iter() {
            if !emitted.contains(&id) {
                continue;
            }
            layouts.collect(function.ret, &function.name, &mut states)?;
            for (_, variable) in function.vars.iter() {
                layouts.collect(variable.ty, &function.name, &mut states)?;
            }
        }
        let mut index = 0;
        while index < layouts.list_types.len() {
            let list_ty = layouts.list_types[index];
            let Type::List(element) = layouts.types[list_ty].clone() else {
                unreachable!("list layout has a non-List type")
            };
            layouts.collect(element, "List element", &mut states)?;
            index += 1;
        }
        for (id, function) in flow.functions.iter() {
            if !emitted.contains(&id) {
                continue;
            }
            let mut types = function
                .vars
                .iter()
                .map(|(_, variable)| variable.ty)
                .collect::<Vec<_>>();
            types.push(function.ret);
            types.extend(layouts.list_types.iter().copied());
            for ty in types {
                if !layouts.contains_list_ref(ty, &mut HashSet::new()) {
                    continue;
                }
                match layouts.types[ty].clone() {
                    Type::List(element)
                        if !type_contains_string(&layouts, element, &mut HashSet::new()) => {}
                    Type::List(_) => {
                        return Err(unsupported(
                            &function.name,
                            "List elements that contain a String view",
                        ));
                    }
                    Type::Any => {
                        return Err(unsupported(
                            &function.name,
                            "EmelaError values that contain a List reference",
                        ));
                    }
                    Type::Enum(enum_id, _) if layouts.core.enums[enum_id].error => {
                        return Err(unsupported(
                            &function.name,
                            "error enum values that contain a List reference",
                        ));
                    }
                    Type::Tuple(_) | Type::Enum(_, _)
                        if type_contains_string(&layouts, ty, &mut HashSet::new()) =>
                    {
                        return Err(unsupported(
                            &function.name,
                            "inline aggregate values that combine List references and String views",
                        ));
                    }
                    Type::Tuple(_) | Type::Enum(_, _) => {}
                    _ => {
                        return Err(unsupported(
                            &function.name,
                            "managed List reference in an unsupported value type",
                        ));
                    }
                }
            }
        }
        Ok(layouts)
    }

    fn collect(
        &mut self,
        ty: TypeId,
        function: &str,
        states: &mut HashMap<TypeId, u8>,
    ) -> Result<(), Vec<Error>> {
        if matches!(self.types[ty], Type::List(_)) {
            if !self.list_ids.contains_key(&ty) {
                let index = self.list_types.len();
                self.list_types.push(ty);
                self.list_ids.insert(ty, index);
            }
            return Ok(());
        }
        let kind = match self.types[ty].clone() {
            Type::Tuple(fields) => Some(AggregateKind::Tuple(fields)),
            Type::Enum(id, args) if self.enum_layout_required(id) => {
                let raw_fields: Vec<Vec<TypeId>> = self.core.enums[id]
                    .variants
                    .iter()
                    .map(|variant| variant.tys.clone())
                    .collect();
                let fields = raw_fields
                    .iter()
                    .map(|variant| {
                        variant
                            .iter()
                            .map(|&field| self.types.subst(field, &args))
                            .collect()
                    })
                    .collect();
                Some(AggregateKind::Enum(fields))
            }
            _ => None,
        };
        let Some(kind) = kind else {
            return Ok(());
        };

        match states.get(&ty) {
            Some(2) => return Ok(()),
            Some(1) => {
                return Err(unsupported(
                    function,
                    &format!("recursive inline aggregate type {}", type_name(ty)),
                ));
            }
            _ => {}
        }
        states.insert(ty, 1);

        let fields: Vec<TypeId> = match &kind {
            AggregateKind::Tuple(fields) => fields.clone(),
            AggregateKind::Enum(variants) => variants.iter().flatten().copied().collect(),
        };
        for field in fields {
            if self.type_repr(field) != Repr::Void {
                self.collect(field, function, states)?;
            }
        }

        let index = self.aggregates.len();
        self.aggregate_ids.insert(ty, index);
        self.aggregates.push(AggregateLayout { ty, kind });
        states.insert(ty, 2);
        Ok(())
    }

    fn enum_is_i32(&self, id: emela_core::EnumId) -> bool {
        let def = &self.core.enums[id];
        !def.error
            && !def.external
            && !def.variants.is_empty()
            && def.variants.iter().all(|variant| variant.fields.is_empty())
    }

    fn enum_layout_required(&self, id: emela_core::EnumId) -> bool {
        let def = &self.core.enums[id];
        !def.external && !def.variants.is_empty() && !self.enum_is_i32(id)
    }

    fn type_repr(&self, ty: TypeId) -> Repr {
        match self.types[ty] {
            Type::Int | Type::Bool => Repr::I32,
            Type::Int64 => Repr::I64,
            Type::Float => Repr::F64,
            Type::Unit | Type::Never | Type::Set(_) => Repr::Void,
            Type::Enum(id, _) if self.enum_is_i32(id) => Repr::I32,
            _ => Repr::Ref,
        }
    }

    fn c_type(&self, ty: TypeId) -> String {
        match self.types[ty] {
            Type::Int | Type::Bool => "int32_t".to_owned(),
            Type::Int64 => "int64_t".to_owned(),
            Type::Float => "double".to_owned(),
            Type::Unit | Type::Never | Type::Set(_) => "void".to_owned(),
            Type::String => "EmelaString".to_owned(),
            Type::Tuple(_) => type_name(ty),
            Type::Enum(id, _) if self.enum_is_i32(id) => "int32_t".to_owned(),
            Type::Enum(id, _) if self.enum_layout_required(id) => type_name(ty),
            Type::Any if self.has_error_abi => "EmelaError".to_owned(),
            _ => c_type(self.type_repr(ty)).to_owned(),
        }
    }

    fn error_enum(&self, ty: TypeId) -> Option<EnumId> {
        match self.types[ty] {
            Type::Enum(id, _) if self.core.enums[id].error => Some(id),
            _ => None,
        }
    }

    fn error_type(&self, enum_id: EnumId) -> Option<TypeId> {
        self.error_types
            .iter()
            .find_map(|(id, ty)| (*id == enum_id).then_some(*ty))
    }

    fn enum_fields(&self, ty: TypeId) -> Option<&[Vec<TypeId>]> {
        let index = *self.aggregate_ids.get(&ty)?;
        match &self.aggregates[index].kind {
            AggregateKind::Enum(variants) => Some(variants),
            AggregateKind::Tuple(_) => None,
        }
    }

    fn list_node_name(&self, ty: TypeId) -> String {
        format!("emela_list_node_{}", self.list_ids[&ty])
    }

    fn list_index(&self, ty: TypeId) -> u32 {
        u32::try_from(self.list_ids[&ty]).expect("too many concrete List layouts")
    }

    fn is_managed_list(&self, ty: TypeId) -> bool {
        matches!(self.types[ty], Type::List(_))
    }

    fn is_managed_value(&self, ty: TypeId) -> bool {
        self.contains_list_ref(ty, &mut HashSet::new())
    }

    fn contains_list_ref(&self, ty: TypeId, seen: &mut HashSet<TypeId>) -> bool {
        let mut types = self.types.clone();
        self.contains_list_ref_with_types(&mut types, ty, seen)
    }

    fn contains_list_ref_with_types(
        &self,
        types: &mut TypeTable,
        ty: TypeId,
        seen: &mut HashSet<TypeId>,
    ) -> bool {
        if !seen.insert(ty) {
            return false;
        }
        match types[ty].clone() {
            Type::List(_) => true,
            Type::Tuple(fields) => fields
                .into_iter()
                .any(|field| self.contains_list_ref_with_types(types, field, seen)),
            Type::Enum(enum_id, args) => self.core.enums[enum_id].variants.iter().any(|variant| {
                variant.tys.iter().any(|&field| {
                    let field = types.subst(field, &args);
                    self.contains_list_ref_with_types(types, field, seen)
                })
            }),
            Type::Any => self.core.enums.iter().any(|(_, enum_def)| {
                enum_def.error
                    && enum_def.variants.iter().any(|variant| {
                        variant
                            .tys
                            .iter()
                            .any(|&field| self.contains_list_ref_with_types(types, field, seen))
                    })
            }),
            _ => false,
        }
    }

    fn aggregate_helper_index(&self, ty: TypeId) -> Option<usize> {
        let index = *self.aggregate_ids.get(&ty)?;
        self.is_managed_value(ty).then_some(index)
    }

    fn emit_retain_value_expression(&self, ty: TypeId, expression: &str, writer: &mut Writer) {
        if self.is_managed_list(ty) {
            writer.line(&format!("emela_heap_retain({expression});"));
        } else if let Some(index) = self.aggregate_helper_index(ty) {
            writer.line(&format!(
                "emela_heap_retain_value_{index}(&({expression}));"
            ));
        }
    }

    fn emit_enqueue_value_expression(
        &self,
        ty: TypeId,
        expression: &str,
        pending: &str,
        tail: &str,
        writer: &mut Writer,
    ) {
        if self.is_managed_list(ty) {
            writer.line(&format!(
                "emela_heap_enqueue({expression}, {pending}, {tail});"
            ));
        } else if let Some(index) = self.aggregate_helper_index(ty) {
            writer.line(&format!(
                "emela_heap_enqueue_value_{index}(&({expression}), {pending}, {tail});"
            ));
        }
    }

    fn emit_release_value_expression(&self, ty: TypeId, expression: &str, writer: &mut Writer) {
        if self.is_managed_list(ty) {
            writer.line(&format!("emela_heap_release({expression});"));
        } else if let Some(index) = self.aggregate_helper_index(ty) {
            writer.line(&format!(
                "emela_heap_release_value_{index}(&({expression}));"
            ));
        }
    }

    fn emit_heap_ownership_helpers(&self, writer: &mut Writer) {
        for (index, aggregate) in self.aggregates.iter().enumerate() {
            if !self.is_managed_value(aggregate.ty) {
                continue;
            }
            let aggregate_type = self.c_type(aggregate.ty);
            writer.begin_block(&format!(
                "static void emela_heap_retain_value_{index}(const {aggregate_type} *value)"
            ));
            self.emit_aggregate_retain_body(aggregate, writer);
            writer.end_block();

            writer.begin_block(&format!(
                "static void emela_heap_enqueue_value_{index}(const {aggregate_type} *value, emela_heap_block **pending, emela_heap_block **tail)"
            ));
            self.emit_aggregate_enqueue_body(aggregate, writer);
            writer.end_block();

            writer.begin_block(&format!(
                "static void emela_heap_release_value_{index}(const {aggregate_type} *value)"
            ));
            writer.line("emela_heap_block *pending = NULL;");
            writer.line("emela_heap_block *tail = NULL;");
            writer.line(&format!(
                "emela_heap_enqueue_value_{index}(value, &pending, &tail);"
            ));
            writer.line("emela_heap_drain(pending, tail);");
            writer.end_block();
            writer.blank();
        }
    }

    fn emit_aggregate_retain_body(&self, aggregate: &AggregateLayout, writer: &mut Writer) {
        match &aggregate.kind {
            AggregateKind::Tuple(fields) => {
                for (index, &field_ty) in fields.iter().enumerate() {
                    if self.is_managed_value(field_ty) {
                        self.emit_retain_value_expression(
                            field_ty,
                            &format!("value->f{index}"),
                            writer,
                        );
                    }
                }
            }
            AggregateKind::Enum(variants) => {
                writer.begin_block("switch (value->tag)");
                for (variant, fields) in variants.iter().enumerate() {
                    writer.line(&format!("case INT32_C({variant}):"));
                    writer.begin_block("");
                    for (index, &field_ty) in fields.iter().enumerate() {
                        if self.is_managed_value(field_ty) {
                            self.emit_retain_value_expression(
                                field_ty,
                                &format!("value->payload.v{variant}.f{index}"),
                                writer,
                            );
                        }
                    }
                    writer.line("break;");
                    writer.end_block();
                }
                writer.line("default:");
                writer.begin_block("");
                writer.line("break;");
                writer.end_block();
                writer.end_block();
            }
        }
    }

    fn emit_aggregate_enqueue_body(&self, aggregate: &AggregateLayout, writer: &mut Writer) {
        match &aggregate.kind {
            AggregateKind::Tuple(fields) => {
                for (index, &field_ty) in fields.iter().enumerate() {
                    if self.is_managed_value(field_ty) {
                        self.emit_enqueue_value_expression(
                            field_ty,
                            &format!("value->f{index}"),
                            "pending",
                            "tail",
                            writer,
                        );
                    }
                }
            }
            AggregateKind::Enum(variants) => {
                writer.begin_block("switch (value->tag)");
                for (variant, fields) in variants.iter().enumerate() {
                    writer.line(&format!("case INT32_C({variant}):"));
                    writer.begin_block("");
                    for (index, &field_ty) in fields.iter().enumerate() {
                        if self.is_managed_value(field_ty) {
                            self.emit_enqueue_value_expression(
                                field_ty,
                                &format!("value->payload.v{variant}.f{index}"),
                                "pending",
                                "tail",
                                writer,
                            );
                        }
                    }
                    writer.line("break;");
                    writer.end_block();
                }
                writer.line("default:");
                writer.begin_block("");
                writer.line("break;");
                writer.end_block();
                writer.end_block();
            }
        }
    }

    fn emit_heap_destroy_switch(&self, writer: &mut Writer) {
        writer.begin_block(
            "static void emela_heap_destroy(uint32_t kind, void *value, emela_heap_block **pending, emela_heap_block **tail)",
        );
        writer.begin_block("switch (kind)");
        for &list_ty in &self.list_types {
            let Type::List(element) = self.types[list_ty] else {
                unreachable!("list layout has a non-List type")
            };
            let node_type = self.list_node_name(list_ty);
            let kind = self.list_index(list_ty);
            writer.line(&format!("case UINT32_C({kind}):"));
            writer.begin_block("");
            writer.line(&format!("{node_type} *node = ({node_type} *)value;"));
            if self.type_repr(element) != Repr::Void && self.is_managed_value(element) {
                self.emit_enqueue_value_expression(
                    element,
                    "node->head",
                    "pending",
                    "tail",
                    writer,
                );
            }
            writer.line("emela_heap_enqueue(node->tail, pending, tail);");
            writer.line("break;");
            writer.end_block();
        }
        writer.line("default:");
        writer.begin_block("");
        writer.line(&format!(
            "emela_board_defect(EMELA_DEFECT_PANIC, {}, UINT16_C({}));",
            c_string_literal(b"invalid fixed heap reference"),
            b"invalid fixed heap reference".len()
        ));
        writer.line("break;");
        writer.end_block();
        writer.end_block();
        writer.end_block();
        writer.blank();
    }

    fn emit_public_release_declarations(&self, writer: &mut Writer) {
        for (index, aggregate) in self.aggregates.iter().enumerate() {
            if !self.is_managed_value(aggregate.ty) {
                continue;
            }
            let aggregate_type = self.c_type(aggregate.ty);
            writer.line("/* Retains heap references before copying this aggregate value. */");
            writer.line(&format!(
                "void emela_embedded_retain_value_{index}(const {aggregate_type} *value);"
            ));
            writer.line("/* Releases the heap references owned by this aggregate value. */");
            writer.line(&format!(
                "void emela_embedded_release_value_{index}({aggregate_type} *value);"
            ));
        }
    }

    fn emit_public_release_helpers(&self, writer: &mut Writer) {
        for (index, aggregate) in self.aggregates.iter().enumerate() {
            if !self.is_managed_value(aggregate.ty) {
                continue;
            }
            let aggregate_type = self.c_type(aggregate.ty);
            writer.begin_block(&format!(
                "void emela_embedded_retain_value_{index}(const {aggregate_type} *value)"
            ));
            writer.begin_block("if (value != NULL)");
            writer.line(&format!("emela_heap_retain_value_{index}(value);"));
            writer.end_block();
            writer.end_block();
            writer.begin_block(&format!(
                "void emela_embedded_release_value_{index}({aggregate_type} *value)"
            ));
            writer.begin_block("if (value != NULL)");
            writer.line(&format!("emela_heap_release_value_{index}(value);"));
            writer.line(&format!("*value = ({aggregate_type}){{0}};"));
            writer.end_block();
            writer.end_block();
            writer.blank();
        }
    }

    fn emit_list_node_declarations(&self, writer: &mut Writer) {
        for &ty in &self.list_types {
            let Type::List(element) = self.types[ty].clone() else {
                unreachable!("list layout has a non-List type")
            };
            writer.line(&format!("typedef struct {} {{", self.list_node_name(ty)));
            if self.type_repr(element) == Repr::Void {
                writer.line("  uint8_t _unit;");
            } else {
                writer.line(&format!("  {} head;", self.c_type(element)));
            }
            writer.line("  void *tail;");
            writer.line(&format!("}} {};", self.list_node_name(ty)));
            writer.blank();
        }
    }

    fn emit_declarations(&self, writer: &mut Writer) {
        for aggregate in &self.aggregates {
            writer.line("typedef struct {");
            match &aggregate.kind {
                AggregateKind::Tuple(fields) => {
                    let mut represented = false;
                    for (index, &field) in fields.iter().enumerate() {
                        if self.type_repr(field) == Repr::Void {
                            continue;
                        }
                        represented = true;
                        writer.line(&format!("  {} f{index};", self.c_type(field)));
                    }
                    if !represented {
                        writer.line("  uint8_t _unit;");
                    }
                }
                AggregateKind::Enum(variants) => {
                    writer.line("  int32_t tag;");
                    writer.line("  union {");
                    for (variant_index, fields) in variants.iter().enumerate() {
                        let represented_fields = fields
                            .iter()
                            .enumerate()
                            .filter(|(_, field)| self.type_repr(**field) != Repr::Void)
                            .map(|(field_index, field)| {
                                format!("{} f{field_index};", self.c_type(*field))
                            })
                            .collect::<Vec<_>>();
                        let body = if represented_fields.is_empty() {
                            "uint8_t _unit;".to_owned()
                        } else {
                            represented_fields.join(" ")
                        };
                        writer.line(&format!("    struct {{ {body} }} v{variant_index};"));
                    }
                    writer.line("  } payload;");
                }
            }
            writer.line(&format!("}} {};", type_name(aggregate.ty)));
            writer.blank();
        }
    }

    fn emit_string_declaration(&self, writer: &mut Writer) {
        writer.line("typedef struct { const char *data; uint16_t length; } EmelaString;");
        writer.blank();
    }

    fn emit_error_declaration(&self, writer: &mut Writer) {
        if !self.has_error_abi {
            return;
        }
        writer.line("typedef struct {");
        writer.line("  uint32_t tag;");
        writer.line("  union {");
        if self.error_types.is_empty() {
            writer.line("    uint8_t _empty;");
        } else {
            for (enum_id, ty) in &self.error_types {
                writer.line(&format!(
                    "    {} e{};",
                    self.c_type(*ty),
                    enum_index(*enum_id)
                ));
            }
        }
        writer.line("  } payload;");
        writer.line("} EmelaError;");
        writer.blank();
    }
}

fn type_name(ty: TypeId) -> String {
    format!("emela_type_{}", ty.index())
}

fn c_type(repr: Repr) -> &'static str {
    match repr {
        Repr::I32 => "int32_t",
        Repr::I64 => "int64_t",
        Repr::F64 => "double",
        Repr::Ref => "void *",
        Repr::Void => "void",
    }
}

fn function_name(id: FnId, function: &Function) -> String {
    let index = u32::from(id.into_raw());
    if function.exported {
        format!("emela_export_{}_{}", c_identifier(&function.name), index)
    } else {
        format!("emela_fn_{index}")
    }
}

fn enum_index(id: EnumId) -> u32 {
    u32::from(id.into_raw())
}

fn c_identifier(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "function".to_owned()
    } else {
        sanitized
    }
}

fn variable_name(id: VarId) -> String {
    format!("emela_v_{}", u32::from(id.into_raw()))
}

fn owned_variable_name(id: VarId) -> String {
    format!("emela_owned_v_{}", u32::from(id.into_raw()))
}

fn statement_name(statement: &emela_lir::Stmt) -> &'static str {
    match statement {
        emela_lir::Stmt::Let { .. } => "Stmt::Let",
        emela_lir::Stmt::If { .. } => "Stmt::If",
        emela_lir::Stmt::Switch { .. } => "Stmt::Switch",
        emela_lir::Stmt::Loop { .. } => "Stmt::Loop",
        emela_lir::Stmt::Block { .. } => "Stmt::Block",
        emela_lir::Stmt::Protect { .. } => "Stmt::Protect",
        emela_lir::Stmt::Dup(_) => "Stmt::Dup",
        emela_lir::Stmt::Drop(_) => "Stmt::Drop",
    }
}

fn value_name(value: &Value) -> &'static str {
    match value {
        Value::Const(_) => "Value::Const",
        Value::Prim { .. } => "Value::Prim",
        Value::Call { .. } => "Value::Call",
        Value::Builtin { .. } => "Value::Builtin",
        Value::Make { .. } => "Value::Make",
        Value::Field { .. } => "Value::Field",
        Value::Tag(_) => "Value::Tag",
        Value::Tuple(_) => "Value::Tuple",
        Value::TupleGet { .. } => "Value::TupleGet",
        Value::List { .. } => "Value::List",
        Value::ListIsEmpty(_) => "Value::ListIsEmpty",
        Value::ListHead(_) => "Value::ListHead",
        Value::ListTail(_) => "Value::ListTail",
        Value::FnRef(_) => "Value::FnRef",
        Value::Closure(_) => "Value::Closure",
        Value::MakeClosure { .. } => "Value::MakeClosure",
        Value::EnvGet { .. } => "Value::EnvGet",
        Value::EvLookup { .. } => "Value::EvLookup",
        Value::EvExtend { .. } => "Value::EvExtend",
        Value::Handler { .. } => "Value::Handler",
        Value::Fiber { .. } => "Value::Fiber",
        Value::IsError { .. } => "Value::IsError",
        Value::AsError { .. } => "Value::AsError",
        Value::Runtime { .. } => "Value::Runtime",
        Value::Coerce { .. } => "Value::Coerce",
    }
}

fn term_name(term: &Term) -> &'static str {
    match term {
        Term::Yield(_) => "Term::Yield",
        Term::Return(_) => "Term::Return",
        Term::Fail(_) => "Term::Fail",
        Term::Break { .. } => "Term::Break",
        Term::Recur(_) => "Term::Recur",
        Term::Defect(_) => "Term::Defect",
        Term::Unwind => "Term::Unwind",
        Term::Unreachable => "Term::Unreachable",
    }
}

fn unsupported(function: &str, construct: &str) -> Vec<Error> {
    vec![Error {
        kind: ErrorKind::Unsupported,
        message: format!(
            "embedded C codegen does not support {construct} in function `{function}` yet"
        ),
    }]
}
