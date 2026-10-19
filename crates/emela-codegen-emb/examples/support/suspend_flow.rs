use emela_core::TypeId;
use emela_lir::{Callee, Const, FnBuilder, Module, PrimOp, PrimTy, Term, Value};

pub fn recursive_flow() -> Module {
    let mut flow = Module::new(emela_core::Module::new());
    let descend = flow.declare("descend");
    let mut b = FnBuilder::new(&flow.core, "descend");
    b.set_exported().set_suspends().set_ret(TypeId::INT);
    let n = b.param("n", TypeId::INT);
    let empty = b.assign(
        "empty",
        TypeId::BOOL,
        Value::Prim {
            op: PrimOp::Eq,
            ty: PrimTy::I32,
            args: vec![n.into(), Const::Int(0).into()],
        },
    );
    let result = b.var("result", TypeId::INT);
    b.if_(
        Some(result),
        empty,
        |b| b.term(Term::Yield(Some(Const::Int(0).into()))),
        |b| {
            let less = b.assign(
                "less",
                TypeId::INT,
                Value::Prim {
                    op: PrimOp::Sub,
                    ty: PrimTy::I32,
                    args: vec![n.into(), Const::Int(1).into()],
                },
            );
            let child = b.assign(
                "child",
                TypeId::INT,
                Value::Call {
                    callee: Callee::Direct(descend),
                    args: vec![less.into()],
                    suspends: true,
                    on_fail: None,
                },
            );
            let sum = b.assign(
                "sum",
                TypeId::INT,
                Value::Prim {
                    op: PrimOp::Add,
                    ty: PrimTy::I32,
                    args: vec![n.into(), child.into()],
                },
            );
            b.term(Term::Yield(Some(sum.into())));
        },
    );
    b.term(Term::Return(Some(result.into())));
    flow.functions[descend] = b.finish();
    flow
}
