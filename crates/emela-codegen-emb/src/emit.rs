use crate::writer::Writer;
use crate::{Error, ErrorKind, GeneratedFile, Options, Output};

const HEADER_FILE: &str = "emela_embedded.h";
const SOURCE_FILE: &str = "emela_embedded.c";

#[allow(dead_code)]
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

pub(crate) fn emit(flow: &emela_lir::Module, _options: &Options) -> Result<Output, Vec<Error>> {
    if let Some((_, function)) = flow.functions.iter().next() {
        return Err(vec![Error {
            kind: ErrorKind::Unsupported,
            message: format!(
                "C emission is not implemented yet for function `{}`",
                function.name
            ),
        }]);
    }

    let mut header = Writer::new();
    header.line("#ifndef EMELA_EMBEDDED_H");
    header.line("#define EMELA_EMBEDDED_H");
    header.blank();
    header.line("#include <stdint.h>");
    header.blank();
    header.line("#endif");

    let mut source = Writer::new();
    source.line(&format!("#include \"{HEADER_FILE}\""));

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

#[cfg(test)]
mod tests {
    use super::{constant_expression, int32_literal};
    use emela_lir::Const;

    #[test]
    fn emits_scalar_constant_expressions() {
        assert_eq!(
            constant_expression(&Const::Int(42)).unwrap(),
            Some("INT32_C(42)".to_owned())
        );
        assert_eq!(
            constant_expression(&Const::Int(-42)).unwrap(),
            Some("(-INT32_C(42))".to_owned())
        );
        assert_eq!(
            constant_expression(&Const::Bool(true)).unwrap(),
            Some("INT32_C(1)".to_owned())
        );
        assert_eq!(constant_expression(&Const::Unit).unwrap(), None);
    }

    #[test]
    fn emits_the_minimum_int_without_an_out_of_range_literal() {
        assert_eq!(
            int32_literal(i32::MIN),
            "(-INT32_C(2147483647) - INT32_C(1))"
        );
    }

    #[test]
    fn rejects_constants_outside_the_initial_scalar_subset() {
        assert_eq!(
            constant_expression(&Const::String("hello".to_owned())),
            Err("String constant")
        );
    }
}
