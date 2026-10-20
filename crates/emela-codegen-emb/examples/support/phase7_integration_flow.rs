use emela_core::{Builtin, External, ExternalBinding, HostRef, Roots, Type, TypeId};
use emela_lir::{FnBuilder, Module, Term, Value};

pub fn lower(with_collector: bool) -> Module {
    let core = super::support::try_lower(
        &[("main.emel", include_str!("phase7_integration.emel"))],
        Roots::Exported,
    )
    .expect("lower phase 7 integration fixture");
    let mut flow = emela_codegen_emb::flow_from_core(&core).expect("verified source Flow");
    let (_, request) = flow
        .functions
        .iter_mut()
        .find(|(_, f)| f.name == "request")
        .unwrap();
    assert!(request.suspends && request.fails);
    request.body = None;
    request.external = Some(External {
        bindings: vec![ExternalBinding {
            target: "c".into(),
            host: HostRef::Global("host_request".into()),
        }],
    });
    if with_collector {
        // A reachable Cell[Any] makes the compiler generate cycle tracing for this module.
        // The integration workload itself never creates a cycle.
        let cell_type = flow.core.types.intern(Type::Cell(TypeId::ANY));
        let mut b = FnBuilder::new(&flow.core, "gc_anchor");
        b.set_exported().set_ret(TypeId::ANY);
        let value = b.param("value", TypeId::ANY);
        let cell = b.assign(
            "cell",
            cell_type,
            Value::Builtin {
                op: Builtin::CellNew,
                args: vec![value.into()],
            },
        );
        let result = b.assign(
            "result",
            TypeId::ANY,
            Value::Builtin {
                op: Builtin::CellGet,
                args: vec![cell.into()],
            },
        );
        b.term(Term::Return(Some(result.into())));
        flow.add(b.finish());
    }
    flow
}
