use std::collections::HashMap;

use emela_lir::{Callee, FnId, Function, Operand, PrimOp, PrimTy, Repr, Stmt, Term, Value, VarId};

use crate::writer::Writer;
use crate::{Error, ErrorKind, GeneratedFile, Options, Output};

const HEADER_FILE: &str = "emela_embedded.h";
const SOURCE_FILE: &str = "emela_embedded.c";
const BOARD_HEADER_FILE: &str = "emela_board.h";

pub(crate) fn emit(flow: &emela_lir::Module, _options: &Options) -> Result<Output, Vec<Error>> {
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

    for (id, function) in flow.functions.iter().filter(|(_, f)| f.exported) {
        if function.fails || function.suspends {
            return Err(unsupported(
                &function.name,
                "fallible or suspending function signatures",
            ));
        }
        let name = &names[&id];
        header.line(&format!("{};", signature(flow, function, name, false)));
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
    let needs_defect_hook = has_prim(flow, &[PrimOp::Div, PrimOp::Rem])
        || flow.functions.iter().any(|(_, function)| {
            function
                .seqs
                .iter()
                .any(|(_, sequence)| matches!(&sequence.term, Term::Defect(_)))
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
    source.blank();
    if has_prim(flow, &[PrimOp::Add, PrimOp::Sub, PrimOp::Mul, PrimOp::Neg]) {
        source.begin_block("static inline int32_t emela_i32_from_bits(uint32_t bits)");
        source.begin_block("if (bits <= (uint32_t)INT32_MAX)");
        source.line("return (int32_t)bits;");
        source.end_block();
        source.line("return -INT32_C(1) - (int32_t)(UINT32_MAX - bits);");
        source.end_block();
        source.blank();
    }
    if has_prim(flow, &[PrimOp::Div, PrimOp::Rem]) {
        emit_i32_division_helpers(&mut source);
    }

    for (id, function) in flow.functions.iter() {
        let name = &names[&id];
        if function.external.is_some() {
            source.line(&format!(
                "extern {};",
                signature(flow, function, name, false)
            ));
        } else if !function.exported {
            source.line(&format!("{};", signature(flow, function, name, true)));
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
        source.begin_block(&signature(flow, function, name, !function.exported));
        emit_local_declarations(function, &mut source);
        for &statement in &sequence.stmts {
            emit_statement(function, &function.stmts[statement], &mut source, &names)?;
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

fn emit_local_declarations(function: &Function, writer: &mut Writer) {
    for (id, variable) in function.vars.iter() {
        if !function.params.contains(&id) {
            writer.decl(c_type(variable.repr), &variable_name(id), None);
        }
    }
}

fn emit_statement(
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
            emit_compound_sequence(function, *then, *dst, writer, names)?;
            writer.end_block();
            writer.begin_block("else");
            emit_compound_sequence(function, *else_, *dst, writer, names)?;
            writer.end_block();
            Ok(())
        }
        other => Err(unsupported(&function.name, statement_name(other))),
    }
}

fn emit_compound_sequence(
    function: &Function,
    sequence: emela_lir::SeqId,
    dst: Option<VarId>,
    writer: &mut Writer,
    names: &HashMap<FnId, String>,
) -> Result<(), Vec<Error>> {
    let sequence = &function.seqs[sequence];
    for &statement in &sequence.stmts {
        emit_statement(function, &function.stmts[statement], writer, names)?;
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
    if ty != PrimTy::I32 {
        return Err("non-Int32 Value::Prim");
    }

    let args: Vec<String> = args
        .iter()
        .map(operand_expression)
        .collect::<Result<_, _>>()?;
    match (op, args.as_slice()) {
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

fn emit_i32_division_helpers(writer: &mut Writer) {
    writer.begin_block("static inline int32_t emela_i32_div(int32_t lhs, int32_t rhs)");
    writer.begin_block("if (rhs == INT32_C(0))");
    writer.line(
        "emela_board_defect(EMELA_DEFECT_DIVISION_BY_ZERO, \"division by zero\", UINT16_C(17));",
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
        "emela_board_defect(EMELA_DEFECT_DIVISION_BY_ZERO, \"division by zero\", UINT16_C(17));",
    );
    writer.end_block();
    writer.begin_block("if (lhs == INT32_MIN && rhs == -INT32_C(1))");
    writer.line("return INT32_C(0);");
    writer.end_block();
    writer.line("return lhs % rhs;");
    writer.end_block();
    writer.blank();
}

fn has_prim(flow: &emela_lir::Module, ops: &[PrimOp]) -> bool {
    flow.functions.iter().any(|(_, function)| {
        function.stmts.iter().any(|(_, statement)| {
            matches!(
                statement,
                Stmt::Let {
                    value: Value::Prim { op, .. },
                    ..
                } if ops.contains(op)
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
        emela_lir::Const::Int64(_) => Err("Int64 constant"),
        emela_lir::Const::Float(_) => Err("Float constant"),
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

fn signature(flow: &emela_lir::Module, function: &Function, name: &str, is_static: bool) -> String {
    let return_type = c_type(emela_lir::repr_of(&flow.core, function.ret));
    let params = function
        .params
        .iter()
        .map(|&variable| {
            let data = &function.vars[variable];
            format!("{} {}", c_type(data.repr), variable_name(variable))
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
