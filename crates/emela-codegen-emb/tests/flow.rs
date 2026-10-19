use emela_codegen_emb::{Options, compile, flow_from_core, verify_flow};

#[test]
fn emits_c_files_for_an_empty_module() {
    let output = compile(&emela_core::Module::new(), &Options::default())
        .expect("an empty Core module should produce an empty C translation unit");

    assert_eq!(output.files.len(), 3);
    assert_eq!(output.files[0].path, "emela_embedded.h");
    assert_eq!(output.files[1].path, "emela_board.h");
    assert!(
        output.files[1]
            .contents
            .contains("EMELA_DEFECT_DIVISION_BY_ZERO")
    );
    assert!(output.files[1].contents.contains("emela_board_defect"));
    assert_eq!(output.files[2].path, "emela_embedded.c");
    assert!(output.files[2].contents.contains("emela_embedded.h"));
    assert!(output.files[2].contents.contains("emela_board.h"));
}

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
