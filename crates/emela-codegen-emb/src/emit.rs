use std::collections::HashMap;

use emela_core::{CtorRef, Type, TypeId, TypeTable};
use emela_lir::{
    Callee, CaseKey, Const, FnId, Function, Operand, PrimOp, PrimTy, Repr, Stmt, SwitchOn, Term,
    Value, VarId,
};

use crate::writer::Writer;
use crate::{Error, ErrorKind, GeneratedFile, Options, Output};

const HEADER_FILE: &str = "emela_embedded.h";
const SOURCE_FILE: &str = "emela_embedded.c";
const BOARD_HEADER_FILE: &str = "emela_board.h";

pub(crate) fn emit(flow: &emela_lir::Module, _options: &Options) -> Result<Output, Vec<Error>> {
    let layouts = TypeLayouts::new(flow)?;
    let mut names = HashMap::new();
    for (id, function) in flow.functions.iter() {
        let name = if function.external.is_some() {
            if function.exported {
                return Err(unsupported(&function.name, "exported external functions"));
            }
            validate_external(flow, function)?
        } else {
            function_name(id, function)
        };
        names.insert(id, name);
    }
    let main_entry = flow
        .functions
        .iter()
        .find(|(_, function)| function.exported && function.name == "main");
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
    layouts.emit_declarations(&mut header);
    if !layouts.aggregates.is_empty() {
        header.blank();
    }

    for (id, function) in flow.functions.iter().filter(|(_, f)| f.exported) {
        if function.fails || function.suspends {
            return Err(unsupported(
                &function.name,
                "fallible or suspending function signatures",
            ));
        }
        let name = &names[&id];
        header.line(&format!("{};", signature(&layouts, function, name, false)));
    }
    if main_entry.is_some() {
        header.line("void emela_embedded_start(void);");
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
    let needs_defect_hook = has_prim(flow, PrimTy::I32, &[PrimOp::Div, PrimOp::Rem])
        || has_prim(flow, PrimTy::I64, &[PrimOp::Div, PrimOp::Rem])
        || flow.functions.iter().any(|(_, function)| {
            function
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
                })
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
    board_header.blank();
    board_header.line("#endif");

    let mut source = Writer::new();
    source.line(&format!("#include \"{HEADER_FILE}\""));
    source.line(&format!("#include \"{BOARD_HEADER_FILE}\""));
    source.line("#include <math.h>");
    source.blank();
    if has_prim(
        flow,
        PrimTy::I32,
        &[PrimOp::Add, PrimOp::Sub, PrimOp::Mul, PrimOp::Neg],
    ) {
        source.begin_block("static inline int32_t emela_i32_from_bits(uint32_t bits)");
        source.begin_block("if (bits <= (uint32_t)INT32_MAX)");
        source.line("return (int32_t)bits;");
        source.end_block();
        source.line("return -INT32_C(1) - (int32_t)(UINT32_MAX - bits);");
        source.end_block();
        source.blank();
    }
    if has_prim(flow, PrimTy::I32, &[PrimOp::Div, PrimOp::Rem]) {
        emit_i32_division_helpers(&mut source);
    }
    if has_prim(
        flow,
        PrimTy::I64,
        &[PrimOp::Add, PrimOp::Sub, PrimOp::Mul, PrimOp::Neg],
    ) {
        emit_i64_wrap_helper(&mut source);
    }
    if has_prim(flow, PrimTy::I64, &[PrimOp::Div, PrimOp::Rem]) {
        emit_i64_division_helpers(&mut source);
    }

    for (id, function) in flow.functions.iter() {
        let name = &names[&id];
        if function.external.is_some() {
            source.line(&format!(
                "extern {};",
                signature(&layouts, function, name, false)
            ));
        } else if !function.exported {
            source.line(&format!("{};", signature(&layouts, function, name, true)));
        }
    }
    source.blank();

    for (id, function) in flow.functions.iter() {
        if function.fails || function.suspends {
            return Err(unsupported(
                &function.name,
                "fallible or suspending function bodies",
            ));
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
        source.begin_block(&signature(&layouts, function, name, !function.exported));
        emit_local_declarations(&layouts, function, &mut source);
        for &statement in &sequence.stmts {
            emit_statement(
                &layouts,
                function,
                &function.stmts[statement],
                &mut source,
                &names,
            )?;
        }
        emit_return(flow, function, &sequence.term, &mut source)?;
        source.end_block();
        source.blank();
    }
    if let Some((id, _function)) = main_entry {
        let main_name = &names[&id];
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

fn emit_local_declarations(layouts: &TypeLayouts<'_>, function: &Function, writer: &mut Writer) {
    for (id, variable) in function.vars.iter() {
        if !function.params.contains(&id) {
            writer.decl(&layouts.c_type(variable.ty), &variable_name(id), None);
        }
    }
}

fn emit_statement(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    statement: &Stmt,
    writer: &mut Writer,
    names: &HashMap<FnId, String>,
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
                Value::Call {
                    callee: Callee::Direct(id),
                    args,
                    suspends: false,
                    on_fail: None,
                } => {
                    let name = names
                        .get(id)
                        .ok_or_else(|| unsupported(&function.name, "call to missing function"))?;
                    let args = args
                        .iter()
                        .map(operand_expression)
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|variant| unsupported(&function.name, variant))?;
                    let expression = format!("{name}({})", args.join(", "));
                    if let Some(dst) = dst {
                        writer.assign(&variable_name(*dst), &expression);
                    } else {
                        writer.line(&format!("{expression};"));
                    }
                }
                Value::Call { .. } => {
                    return Err(unsupported(
                        &function.name,
                        "indirect, failing, or suspending calls",
                    ));
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
            emit_compound_sequence(layouts, function, *then, *dst, writer, names)?;
            writer.end_block();
            writer.begin_block("else");
            emit_compound_sequence(layouts, function, *else_, *dst, writer, names)?;
            writer.end_block();
            Ok(())
        }
        Stmt::Switch {
            dst,
            on,
            cases,
            default,
        } => emit_switch(layouts, function, *dst, *on, cases, *default, writer, names),
        Stmt::Loop {
            dst,
            vars,
            inits,
            body,
        } => emit_loop(layouts, function, *dst, vars, inits, *body, writer, names),
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
        emit_compound_sequence(layouts, function, case.body, dst, writer, names)?;
        writer.line("break;");
        writer.end_block();
    }

    if let Some(default) = default {
        writer.begin_block("default:");
        emit_compound_sequence(layouts, function, default, dst, writer, names)?;
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

fn emit_loop(
    layouts: &TypeLayouts<'_>,
    function: &Function,
    dst: Option<VarId>,
    vars: &[VarId],
    inits: &[Operand],
    body: emela_lir::SeqId,
    writer: &mut Writer,
    names: &HashMap<FnId, String>,
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
        writer.assign(&variable_name(variable), &expression);
    }
    writer.begin_block("for (;;)");
    emit_compound_sequence(layouts, function, body, dst, writer, names)?;
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
) -> Result<(), Vec<Error>> {
    let sequence = &function.seqs[sequence];
    for &statement in &sequence.stmts {
        emit_statement(layouts, function, &function.stmts[statement], writer, names)?;
    }
    let Term::Yield(value) = &sequence.term else {
        return Err(unsupported(&function.name, term_name(&sequence.term)));
    };
    match (dst, value) {
        (Some(dst), Some(value)) => {
            let expression = operand_expression(value)
                .map_err(|variant| unsupported(&function.name, variant))?;
            writer.assign(&variable_name(dst), &expression);
        }
        (Some(_), None) => {
            return Err(vec![Error {
                kind: ErrorKind::InvalidFlowIr,
                message: format!(
                    "invalid Flow IR in function `{}`: If destination has a valueless arm",
                    function.name
                ),
            }]);
        }
        (None, _) => {}
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

fn has_prim(flow: &emela_lir::Module, ty: PrimTy, ops: &[PrimOp]) -> bool {
    flow.functions.iter().any(|(_, function)| {
        function.stmts.iter().any(|(_, statement)| {
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

fn comparison_expression(lhs: &str, op: &str, rhs: &str) -> Result<String, &'static str> {
    Ok(format!("(({lhs}) {op} ({rhs}) ? INT32_C(1) : INT32_C(0))"))
}

fn emit_return(
    flow: &emela_lir::Module,
    function: &Function,
    term: &Term,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    match term {
        Term::Return(None) => writer.ret(None),
        Term::Return(Some(operand)) => {
            let expression = operand_expression(operand)
                .map_err(|variant| unsupported(&function.name, variant))?;
            writer.ret(Some(&expression));
        }
        other => return Err(unsupported(&function.name, term_name(other))),
    }

    let return_repr = emela_lir::repr_of(&flow.core, function.ret);
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
        emela_lir::Const::String(_) => Err("String constant"),
    }
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
) -> String {
    let return_type = layouts.c_type(function.ret);
    let params = function
        .params
        .iter()
        .map(|&variable| {
            let data = &function.vars[variable];
            format!("{} {}", layouts.c_type(data.ty), variable_name(variable))
        })
        .collect::<Vec<_>>();
    let params = if params.is_empty() {
        "void".to_owned()
    } else {
        params.join(", ")
    };
    let linkage = if is_static { "static " } else { "" };
    format!("{linkage}{return_type} {name}({params})")
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
}

impl<'a> TypeLayouts<'a> {
    fn new(flow: &'a emela_lir::Module) -> Result<Self, Vec<Error>> {
        let mut layouts = TypeLayouts {
            core: &flow.core,
            types: flow.core.types.clone(),
            aggregates: Vec::new(),
            aggregate_ids: HashMap::new(),
        };
        let mut states = HashMap::new();
        for (_, function) in flow.functions.iter() {
            layouts.collect(function.ret, &function.name, &mut states)?;
            for (_, variable) in function.vars.iter() {
                layouts.collect(variable.ty, &function.name, &mut states)?;
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
            Type::Tuple(_) => type_name(ty),
            Type::Enum(id, _) if self.enum_is_i32(id) => "int32_t".to_owned(),
            Type::Enum(id, _) if self.enum_layout_required(id) => type_name(ty),
            _ => c_type(self.type_repr(ty)).to_owned(),
        }
    }

    fn enum_fields(&self, ty: TypeId) -> Option<&[Vec<TypeId>]> {
        let index = *self.aggregate_ids.get(&ty)?;
        match &self.aggregates[index].kind {
            AggregateKind::Enum(variants) => Some(variants),
            AggregateKind::Tuple(_) => None,
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
