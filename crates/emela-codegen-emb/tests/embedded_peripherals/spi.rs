use super::*;
use std::path::Path;

#[test]
fn spi_transactions_preserve_results_limits_error_sources_and_cleanup_failures() {
    let mut output = source_output(include_str!("spi.emel"));
    let aliases = ["create", "transaction", "recover", "direct", "classify"]
        .map(|name| format!("#define {name} {}\n", export_name(&output, name)))
        .concat();
    let source = &mut output
        .files
        .iter_mut()
        .find(|f| f.path == "emela_embedded.c")
        .unwrap()
        .contents;
    source.push_str(
        r#"
size_t fixture_heap_live(void) {
  size_t used = 0;
  if (!emela_heap_initialized) return 0;
  for (unsigned char *p = emela_heap_storage; p < emela_heap_end();) {
    emela_heap_block *block = (emela_heap_block *)(void *)p;
    if (!block->is_free) used += block->size;
    p += block->size;
  }
  return used;
}

"#,
    );
    compile_spi_and_run(
        output,
        &(aliases
            + &include_str!("spi_harness.c").replace(
                "/* @DRIVER@ */",
                include_str!("../../boards/common/spi_driver.h"),
            )),
    );
}

fn compile_spi_and_run(output: Output, harness: &str) {
    let temp = TempDir::new();
    for file in output.files {
        fs::write(temp.0.join(file.path), file.contents).unwrap();
    }
    fs::write(temp.0.join("harness.c"), harness).unwrap();
    let executable = temp.0.join("spi");
    let compile = Command::new("cc")
        .args([
            "-std=c11",
            "-O2",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-fsanitize=undefined,address",
            "-fno-omit-frame-pointer",
            "-I",
        ])
        .arg(&temp.0)
        .arg(temp.0.join("emela_embedded.c"))
        .arg(temp.0.join("harness.c"))
        .arg("-lm")
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    assert!(
        compile.status.success(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(executable)
        .env("ASAN_OPTIONS", "detect_leaks=0")
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn spi_bsp_register_models_cover_modes_widths_order_fifo_and_chip_select() {
    let output = source_output(include_str!("spi.emel"));
    for (board, bsp) in [
        (
            "TEST_STM32",
            concat!(
                include_str!("../../boards/stm32/stm32f4/monotonic.inc"),
                "\n",
                include_str!("../../boards/stm32/stm32f4/spi.inc")
            ),
        ),
        (
            "TEST_ESP32",
            concat!(
                include_str!("../../boards/esp32c3/monotonic.inc"),
                "\n",
                include_str!("../../boards/esp32c3/spi.inc")
            ),
        ),
    ] {
        let temp = TempDir::new();
        for file in &output.files {
            fs::write(temp.0.join(&file.path), &file.contents).unwrap();
        }
        fs::write(
            temp.0.join("spi_driver.h"),
            include_str!("../../boards/common/spi_driver.h"),
        )
        .unwrap();
        fs::write(
            temp.0.join("registers.cpp"),
            include_str!("spi_registers.cpp").replace("/* @BSP@ */", bsp),
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
                "-Wno-unused-variable",
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
fn spi_example_and_bsp_translation_units_pass_c_syntax_checks() {
    let output = source_output(include_str!(
        "../../../../examples/spi_devices/src/main.emel"
    ));
    let main = export_name(&output, "main");
    let temp = TempDir::new();
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

#[test]
fn spi_synchronous_operations_are_rejected_in_gpio_isrs() {
    let source = r#"
import Std.Embedded.Spi
import Std.Embedded.Gpio
fn event(e: Gpio.GpioEvent) -> () fails {} use Spi.Spi {
  (use Spi.Spi).recover(Spi.Spi2) escape { Spi.SpiTransactionError(_, _) -> () }
}
pub fn setup() -> () fails Gpio.GpioError { Gpio.bind_interrupt(Gpio.GpioPin(0), event) }
"#;
    let core = support::try_lower(&[("main.emel", source)], Roots::Exported).unwrap();
    let errors = compile(&core, &Options::default().with_heap_bytes(8192))
        .expect_err("synchronous SPI is forbidden in ISRs");
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("forbidden in a GPIO ISR")),
        "{errors:?}"
    );
}

#[test]
fn spi_transaction_capacity_boundaries_generate_valid_c() {
    let core =
        support::try_lower(&[("main.emel", include_str!("spi.emel"))], Roots::Exported).unwrap();
    for capacity in [1, u16::MAX] {
        let output = compile(
            &core,
            &Options::default()
                .with_heap_bytes(8192)
                .with_peripheral_buffer_capacity(capacity),
        )
        .unwrap();
        let aliases = ["create", "transaction", "recover", "direct", "classify"]
            .map(|name| format!("#define {name} {}\n", export_name(&output, name)))
            .concat();
        let temp = TempDir::new();
        for file in output.files {
            fs::write(temp.0.join(file.path), file.contents).unwrap();
        }
        fs::write(
            temp.0.join("harness.c"),
            aliases
                + &include_str!("spi_harness.c").replace(
                    "/* @DRIVER@ */",
                    include_str!("../../boards/common/spi_driver.h"),
                ),
        )
        .unwrap();
        for file in ["emela_embedded.c", "harness.c"] {
            let result = Command::new("cc")
                .args([
                    "-std=c11",
                    "-Wall",
                    "-Wextra",
                    "-Werror",
                    "-fsyntax-only",
                    "-I",
                ])
                .arg(&temp.0)
                .arg(temp.0.join(file))
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "capacity {capacity}: {}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
    }
}
