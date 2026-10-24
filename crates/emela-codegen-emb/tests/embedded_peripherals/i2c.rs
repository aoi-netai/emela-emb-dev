use super::*;
use std::path::Path;

#[test]
fn i2c_events_classify_errors_and_call_private_isrs_without_allocation() {
    let mut output = source_output(include_str!("i2c.emel"));
    let header = &output
        .files
        .iter()
        .find(|f| f.path == "emela_i2c.h")
        .unwrap()
        .contents;
    assert!(!header.contains("EmelaI2cEvent"));
    let aliases = format!(
        "#define initialize {}\n#define classify {}\n",
        export_name(&output, "initialize"),
        export_name(&output, "classify")
    );
    let source = &mut output
        .files
        .iter_mut()
        .find(|f| f.path == "emela_embedded.c")
        .unwrap()
        .contents;
    *source = source.replace("static void *emela_heap_allocate(size_t payload_size, uint32_t kind) {", "static unsigned i2c_test_allocations;\nstatic void *emela_heap_allocate(size_t payload_size, uint32_t kind) {\n i2c_test_allocations++;");
    source.push_str("\nunsigned fixture_allocations(void) { return i2c_test_allocations; }\n");
    compile_and_run(output, &(aliases + include_str!("i2c_harness.c")));
}

#[test]
fn i2c_emela_recovers_from_nack_and_timeout_and_consumes_isr_notifications() {
    let output = source_output(include_str!("i2c_recovery.emel"));
    let alias = format!("#define recover {}\n", export_name(&output, "recover"));
    compile_and_run(
        output,
        &(alias
            + &include_str!("i2c_recovery.c").replace(
                "/* @DRIVER@ */",
                include_str!("../../boards/common/i2c_driver.h"),
            )),
    );
}

fn reject(source: &str, reason: &str) {
    let core = support::try_lower(&[("main.emel", source)], Roots::Exported).unwrap();
    let errors = compile(&core, &Options::default().with_heap_bytes(8192)).expect_err(reason);
    assert!(
        errors.iter().any(|e| e.message.contains(reason)),
        "{errors:?}"
    );
}

#[test]
fn i2c_bindings_reject_dynamic_ids_duplicates_unsafe_callbacks_and_queue_conflicts() {
    let prefix = "import Std.Embedded.I2c\nfn event(id: I2c.I2cId, e: I2c.I2cEvent) -> () fails {} use {} { () }\n";
    for (source, reason) in [
        (
            "pub fn setup(id: I2c.I2cId) -> () fails I2c.I2cError { I2c.bind_interrupt(id, event) }",
            "statically resolved I2cId",
        ),
        (
            "pub fn setup() -> () fails I2c.I2cError { I2c.bind_interrupt(I2c.I2c1, fn(id, e) { () }) }",
            "capture-free named callback",
        ),
        (
            "const ID = I2c.I2c1\npub fn setup() -> () fails I2c.I2cError { I2c.bind_interrupt(ID, event)\n I2c.bind_interrupt(I2c.I2c1, event) }",
            "duplicate I2C interrupt binding",
        ),
    ] {
        reject(&(prefix.to_owned() + source), reason);
    }
    reject(
        "import Std.Embedded.I2c\nfn event(id: I2c.I2cId, e: I2c.I2cEvent) -> () fails {} use {I2c.I2c} { (use I2c.I2c).master_transmit(id, 64, [], Int64.from_int(10)) escape { I2c.I2cError(_) -> () } }\npub fn setup() -> () fails I2c.I2cError { I2c.bind_interrupt(I2c.I2c1, event) }",
        "forbidden in a I2C ISR",
    );
    reject(
        "import Std.Embedded.I2c\nfn event(id: I2c.I2cId, e: I2c.I2cEvent) -> () fails {} use {} { xs = [1]\n () }\npub fn setup() -> () fails I2c.I2cError { I2c.bind_interrupt(I2c.I2c1, event) }",
        "heap-backed value reachable from ISR",
    );
    let producer = "import Std.Embedded.I2c\nimport Std.Embedded.Uart\nimport Std.Embedded.Interrupt\nfn i(id: I2c.I2cId, e: I2c.I2cEvent) -> () fails {} use {} { b = Interrupt.try_push(Interrupt.Channel0, 1)\n () }\nfn u(id: Uart.UartId, e: Uart.UartEvent) -> () fails {} use {} { b = Interrupt.try_push(Interrupt.Channel0, 2)\n () }\n";
    reject(
        &(producer.to_owned()
            + "pub fn setup() -> () fails I2c.I2cError | Uart.UartError { I2c.bind_interrupt(I2c.I2c1, i)\n Uart.bind_interrupt(Uart.Uart1, u) }"),
        "multiple ISR producers",
    );
    reject(
        &(producer.to_owned()
            + "pub fn setup() -> () fails I2c.I2cError { I2c.bind_interrupt(I2c.I2c1, i)\n b = Interrupt.try_push(Interrupt.Channel0, 3)\n () }"),
        "main producer can conflict",
    );
}

fn driver_headers() -> Output {
    source_output(include_str!("i2c.emel"))
}

#[test]
fn i2c_driver_validates_before_start_and_notifies_once_after_cleanup() {
    let temp = TempDir::new();
    for file in driver_headers().files {
        fs::write(temp.0.join(file.path), file.contents).unwrap();
    }
    fs::write(
        temp.0.join("driver.c"),
        include_str!("i2c_driver.c").replace(
            "/* @DRIVER@ */",
            include_str!("../../boards/common/i2c_driver.h"),
        ),
    )
    .unwrap();
    let executable = temp.0.join("driver");
    let compile = Command::new("cc")
        .args([
            "-std=c11",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-fsanitize=undefined,address",
            "-fno-omit-frame-pointer",
            "-I",
        ])
        .arg(&temp.0)
        .arg(temp.0.join("driver.c"))
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    assert!(
        compile.status.success(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    assert!(
        Command::new(executable)
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .status()
            .unwrap()
            .success()
    );
}

#[test]
fn i2c_bsp_register_models_preserve_wire_protocol_and_recover_from_errors() {
    let output = driver_headers();
    for (board, bsp) in [
        (
            "TEST_STM32",
            include_str!("../../boards/stm32/stm32f4/i2c.inc"),
        ),
        ("TEST_ESP32", include_str!("../../boards/esp32c3/i2c.inc")),
    ] {
        let temp = TempDir::new();
        for file in &output.files {
            fs::write(temp.0.join(&file.path), &file.contents).unwrap();
        }
        fs::write(
            temp.0.join("i2c_driver.h"),
            include_str!("../../boards/common/i2c_driver.h"),
        )
        .unwrap();
        fs::write(
            temp.0.join("registers.cpp"),
            include_str!("i2c_registers.cpp").replace(
                "/* @BSP@ */",
                &bsp.replace("(void)APP_I2C(", "(void)(uint32_t)APP_I2C("),
            ),
        )
        .unwrap();
        let executable = temp.0.join("registers");
        let compile = Command::new("c++")
            .args([
                "-std=c++17",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-Wno-unused-function",
                "-Wno-unused-parameter",
                "-fsanitize=undefined,address",
                "-fno-omit-frame-pointer",
                "-D",
            ])
            .arg(board)
            .arg("-I")
            .arg(&temp.0)
            .arg(temp.0.join("registers.cpp"))
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap();
        assert!(
            compile.status.success(),
            "{board}: {}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let run = Command::new(executable)
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .output()
            .unwrap();
        assert!(
            run.status.success(),
            "{board}: {}",
            String::from_utf8_lossy(&run.stderr)
        );
    }
}

#[test]
fn i2c_example_and_both_bsp_translation_units_pass_c_syntax_checks() {
    let output = source_output(include_str!(
        "../../../../examples/i2c_sensor/src/main.emel"
    ));
    let temp = TempDir::new();
    let main = export_name(&output, "main");
    for file in output.files {
        fs::write(temp.0.join(file.path), file.contents).unwrap();
    }
    fs::write(
        temp.0.join("emela_embedded_exports.h"),
        format!("#include \"emela_embedded.h\"\n#define board_main_start {main}\n"),
    )
    .unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let bsp = root.join("boards/stm32/stm32f4");
    let configure = Command::new("python3").arg("-c").arg("import importlib.util, pathlib, sys; p = pathlib.Path(sys.argv[1]); spec = importlib.util.spec_from_file_location('bsp', p / 'build.py'); bsp = importlib.util.module_from_spec(spec); spec.loader.exec_module(bsp); profile, chip = bsp.selected_profile(p / 'boards/stm32f407vg.json'); bsp.configure(pathlib.Path(sys.argv[2]), profile, chip)").arg(&bsp).arg(&temp.0).env("PYTHONDONTWRITEBYTECODE", "1").output().unwrap();
    assert!(
        configure.status.success(),
        "{}",
        String::from_utf8_lossy(&configure.stderr)
    );
    for source in [
        temp.0.join("emela_embedded.c"),
        bsp.join("runtime.c"),
        root.join("boards/esp32c3/board.c"),
    ] {
        let compile = Command::new("cc")
            .args([
                "-std=c11",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-fsyntax-only",
                "-I",
            ])
            .arg(&temp.0)
            .arg(&source)
            .output()
            .unwrap();
        assert!(
            compile.status.success(),
            "{}: {}",
            source.display(),
            String::from_utf8_lossy(&compile.stderr)
        );
    }
}
