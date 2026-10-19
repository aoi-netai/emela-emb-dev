use crate::writer::Writer;
use crate::{Error, ErrorKind, GeneratedFile, Options, Output};

const HEADER_FILE: &str = "emela_embedded.h";
const SOURCE_FILE: &str = "emela_embedded.c";

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
