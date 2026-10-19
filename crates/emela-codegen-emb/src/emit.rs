use std::collections::HashMap;

use emela_lir::{FnId, Function, Operand, Repr, Term, VarId};

use crate::writer::Writer;
use crate::{Error, ErrorKind, GeneratedFile, Options, Output};

const HEADER_FILE: &str = "emela_embedded.h";
const SOURCE_FILE: &str = "emela_embedded.c";

pub(crate) fn emit(flow: &emela_lir::Module, _options: &Options) -> Result<Output, Vec<Error>> {
    let names: HashMap<FnId, String> = flow
        .functions
        .iter()
        .map(|(id, function)| (id, function_name(id, function)))
        .collect();

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

    header.blank();
    header.line("#endif");

    let mut source = Writer::new();
    source.line(&format!("#include \"{HEADER_FILE}\""));
    source.blank();

    for (id, function) in flow.functions.iter() {
        if function.fails || function.suspends {
            return Err(unsupported(
                &function.name,
                "fallible or suspending function bodies",
            ));
        }

        let Some(body) = function.body else {
            return Err(unsupported(&function.name, "external function bodies"));
        };
        let sequence = &function.seqs[body];
        if let Some(&statement) = sequence.stmts.first() {
            return Err(unsupported(
                &function.name,
                statement_name(&function.stmts[statement]),
            ));
        }

        let name = &names[&id];
        source.begin_block(&signature(flow, function, name, !function.exported));
        emit_return(flow, function, &sequence.term, &mut source)?;
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
                path: SOURCE_FILE.to_owned(),
                contents: source.finish(),
            },
        ],
    })
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
