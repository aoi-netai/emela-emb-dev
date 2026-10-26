use emela_core::HostRef;

pub(super) fn global(host: &HostRef) -> &str {
    let HostRef::Global(name) = host else {
        unreachable!("global binding")
    };
    name
}

pub(super) fn import(host: &HostRef) -> &str {
    let HostRef::Import { module, name } = host else {
        unreachable!("import binding")
    };
    super::host_modules::symbol(module, name)
}

pub(in crate::backend) fn symbol(host: &HostRef) -> &str {
    match host {
        HostRef::Global(_) => global(host),
        HostRef::Import { .. } => import(host),
    }
}
