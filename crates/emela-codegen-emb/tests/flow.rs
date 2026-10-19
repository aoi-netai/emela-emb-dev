use emela_codegen_emb::{flow_from_core, verify_flow};

#[test]
fn lowers_and_verifies_core_as_flow_ir() {
    let mut core = emela_core::Module::new();
    let main = core.declare("main", true);
    core.define(
        main,
        Vec::new(),
        emela_core::TypeId::UNIT,
        emela_core::Expr::Lit(emela_core::Lit::Unit),
    );

    let flow = flow_from_core(&core).expect("Core should lower to valid Flow IR");

    assert_eq!(flow.stage, emela_lir::Stage::Flow);
    assert!(
        flow.functions
            .iter()
            .any(|(_, function)| function.name == "main")
    );
    verify_flow(&flow).expect("the Flow IR verifier should accept the result");
}
