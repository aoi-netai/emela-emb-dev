use super::*;

#[test]
fn gpio_config_errors_and_private_isr_coexist_with_managed_main_errors() {
    let mut output = source_output(include_str!("gpio.emel"));
    let gpio = &output
        .files
        .iter()
        .find(|f| f.path == "emela_gpio.h")
        .unwrap()
        .contents;
    assert!(!gpio.contains("EmelaGpioConfig"));
    assert!(!gpio.contains("EmelaGpioEvent"));
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
    *source = source.replace("static void *emela_heap_allocate(size_t payload_size, uint32_t kind) {", "static unsigned gpio_test_allocations;\nstatic void *emela_heap_allocate(size_t payload_size, uint32_t kind) {\n  gpio_test_allocations++;");
    source.push_str("\nunsigned fixture_allocations(void) { return gpio_test_allocations; }\n");
    compile_and_run(output, &(aliases + include_str!("gpio_harness.c")));
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
fn gpio_binding_rejects_dynamic_pins_closures_and_duplicate_declarations() {
    let prefix =
        "import Std.Embedded.Gpio\nfn edge(e: Gpio.GpioEvent) -> () fails {} use {} { () }\n";
    reject(
        &(prefix.to_owned()
            + "pub fn setup(pin: Int) -> () fails Gpio.GpioError { Gpio.bind_interrupt(Gpio.GpioPin(pin), edge) }"),
        "statically resolved GpioPin",
    );
    reject(
        &(prefix.to_owned()
            + "pub fn setup() -> () fails Gpio.GpioError { Gpio.bind_interrupt(Gpio.GpioPin(0), fn(e) { () }) }"),
        "capture-free named callback",
    );
    reject(
        &(prefix.to_owned()
            + "pub fn setup() -> () fails Gpio.GpioError {\n Gpio.bind_interrupt(Gpio.GpioPin(0), edge)\n Gpio.bind_interrupt(Gpio.GpioPin(0), edge)\n}"),
        "duplicate GPIO interrupt binding",
    );
    reject(
        &(prefix.to_owned()
            + "pub fn setup(b: Bool) -> () fails Gpio.GpioError { if b { Gpio.bind_interrupt(Gpio.GpioPin(0), edge) } else { Gpio.bind_interrupt(Gpio.GpioPin(0), edge) } }"),
        "duplicate GPIO interrupt binding",
    );
}

#[test]
fn gpio_isr_rejects_forbidden_operations_heap_and_recursion() {
    for operation in [
        "configure(Gpio.GpioPin(1), Gpio.Input(Gpio.NoPull))",
        "toggle(Gpio.GpioPin(1))",
        "enable_interrupt(Gpio.GpioPin(1), Gpio.Rising, Interrupt.Low)",
        "disable_interrupt(Gpio.GpioPin(1))",
    ] {
        reject(
            &format!(
                "import Std.Embedded.Gpio\nimport Std.Embedded.Interrupt\nfn edge(e: Gpio.GpioEvent) -> () fails {{}} use {{Gpio.Gpio}} {{ (use Gpio.Gpio).{operation} escape {{ Gpio.GpioError(_) -> () }} }}\npub fn setup() -> () fails Gpio.GpioError {{ Gpio.bind_interrupt(Gpio.GpioPin(0), edge) }}"
            ),
            "forbidden in a GPIO ISR",
        );
    }
    reject(
        "import Std.Embedded.Gpio\nfn edge(e: Gpio.GpioEvent) -> () fails {} use {} { xs = [e.pin.id]\n () }\npub fn setup() -> () fails Gpio.GpioError { Gpio.bind_interrupt(Gpio.GpioPin(0), edge) }",
        "heap-backed value reachable from ISR",
    );
    reject(
        "import Std.Embedded.Gpio\nfn edge(e: Gpio.GpioEvent) -> () fails {} use {} { edge(e) }\npub fn setup() -> () fails Gpio.GpioError { Gpio.bind_interrupt(Gpio.GpioPin(0), edge) }",
        "recursive call reachable from ISR",
    );
}

#[test]
fn gpio_binding_checks_aliases_and_isr_effects_transitively() {
    reject(
        "import Std.Embedded.Gpio\nfn edge(e: Gpio.GpioEvent) -> () fails {} use {} { () }\nconst PIN = Gpio.GpioPin(2 - 2)\npub fn setup() -> () fails Gpio.GpioError {\n Gpio.bind_interrupt(PIN, edge)\n Gpio.bind_interrupt(Gpio.GpioPin(0), edge)\n}",
        "duplicate GPIO interrupt binding",
    );
    reject(
        "import Std.Embedded.Gpio\nfn edge(e: Gpio.GpioEvent) -> () fails {} use {} { () }\npub fn setup(b: Bool) -> () fails Gpio.GpioError {\n registration = if b { Gpio.bind_interrupt } else { Gpio.bind_interrupt }\n registration(Gpio.GpioPin(0), edge)\n}",
        "GPIO binding function must be used in a static call",
    );
    reject(
        "import Std.Embedded.Gpio\nimport Std.Console.{Console}\nfn edge(e: Gpio.GpioEvent) -> () fails {} use Console { (use Console).print_line(\"edge\") }\npub fn setup() -> () fails Gpio.GpioError { Gpio.bind_interrupt(Gpio.GpioPin(0), edge) }",
        "forbidden in a GPIO ISR",
    );
    reject(
        "import Std.Embedded.Gpio\nerror TextError(message: String)\nfn edge(e: Gpio.GpioEvent) -> () fails {} use {} { (fail TextError(\"edge\")) escape { TextError(_) -> () } }\npub fn setup() -> () fails Gpio.GpioError { Gpio.bind_interrupt(Gpio.GpioPin(0), edge) }",
        "heap-backed value reachable from ISR",
    );
}
