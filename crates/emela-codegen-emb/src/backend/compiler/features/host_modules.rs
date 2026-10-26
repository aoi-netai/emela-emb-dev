//! C bindings already carry their resolved symbol. Source module identity does not rename it.
use emela_core::HostModule;

pub(super) fn named<'a>(_source: &str, symbol: &'a str) -> &'a str {
    symbol
}
pub(super) fn file<'a>(_source: &str, symbol: &'a str) -> &'a str {
    symbol
}
pub(super) fn bundled<'a>(_source: &str, symbol: &'a str) -> &'a str {
    symbol
}

pub(super) fn symbol<'a>(module: &HostModule, name: &'a str) -> &'a str {
    match module {
        HostModule::Named(source) => named(source, name),
        HostModule::File(source) => file(source, name),
        HostModule::Bundled(source) => bundled(source, name),
    }
}
