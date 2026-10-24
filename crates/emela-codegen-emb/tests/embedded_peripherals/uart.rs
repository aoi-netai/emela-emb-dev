use super::*;

#[test]
fn uart_events_use_scalar_c_entries_and_no_isr_allocation() {
    let mut output = source_output(include_str!("uart.emel"));
    let header = &output
        .files
        .iter()
        .find(|f| f.path == "emela_uart.h")
        .unwrap()
        .contents;
    assert!(!header.contains("EmelaUartEvent"));
    assert!(!header.contains("EmelaUartReceiveErrors"));
    let aliases = format!(
        "#define initialize {}\n#define classify {}\n#define utilities {}\n",
        export_name(&output, "initialize"),
        export_name(&output, "classify"),
        export_name(&output, "utilities")
    );
    let source = &mut output
        .files
        .iter_mut()
        .find(|f| f.path == "emela_embedded.c")
        .unwrap()
        .contents;
    *source = source.replace("static void *emela_heap_allocate(size_t payload_size, uint32_t kind) {", "static unsigned uart_test_allocations;\nstatic void *emela_heap_allocate(size_t payload_size, uint32_t kind) {\n uart_test_allocations++;");
    source.push_str("\nunsigned fixture_allocations(void) { return uart_test_allocations; }\n");
    compile_and_run(output, &(aliases + include_str!("uart_harness.c")));
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
fn uart_bindings_reject_duplicates_dynamic_ids_and_unsafe_callbacks() {
    let prefix = "import Std.Embedded.Uart\nfn event(id: Uart.UartId, e: Uart.UartEvent) -> () fails {} use {} { () }\n";
    reject(
        &(prefix.to_owned()
            + "pub fn setup(id: Uart.UartId) -> () fails Uart.UartError { Uart.bind_interrupt(id, event) }"),
        "statically resolved UartId",
    );
    reject(
        &(prefix.to_owned()
            + "pub fn setup() -> () fails Uart.UartError { Uart.bind_interrupt(Uart.Uart1, fn(id, e) { () }) }"),
        "capture-free named callback",
    );
    reject(
        &(prefix.to_owned()
            + "const ID = Uart.Uart1\npub fn setup() -> () fails Uart.UartError { Uart.bind_interrupt(ID, event)\n Uart.bind_interrupt(Uart.Uart1, event) }"),
        "duplicate UART interrupt binding",
    );
    reject(
        "import Std.Embedded.Uart\nimport Std.Console.{Console}\nfn event(id: Uart.UartId, e: Uart.UartEvent) -> () fails {} use Console { (use Console).print_line(\"uart\") }\npub fn setup() -> () fails Uart.UartError { Uart.bind_interrupt(Uart.Uart1, event) }",
        "forbidden in a UART ISR",
    );
    reject(
        "import Std.Embedded.Uart\nfn event(id: Uart.UartId, e: Uart.UartEvent) -> () fails {} use {} { xs = [1]\n () }\npub fn setup() -> () fails Uart.UartError { Uart.bind_interrupt(Uart.Uart1, event) }",
        "heap-backed value reachable from ISR",
    );
}

#[test]
fn fixed_word_queues_are_bounded_fifo_and_separate_channels() {
    let output = source_output(
        r#"
import Std.Embedded.Interrupt
pub fn push(channel: Int, value: Int) -> Bool {
  Interrupt.try_push(if channel == 0 { Interrupt.Channel0 } else { Interrupt.Channel1 }, value)
}
pub fn pop(channel: Int) -> Int {
  result = Interrupt.try_pop(if channel == 0 { Interrupt.Channel0 } else { Interrupt.Channel1 })
  if result.available { result.value } else { -1 }
}
"#,
    );
    let aliases = format!(
        "#define push {}\n#define pop {}\n",
        export_name(&output, "push"),
        export_name(&output, "pop")
    );
    compile_and_run(
        output,
        &(aliases
            + r#"
#include <assert.h>
#include "emela_embedded.h"
static unsigned locked;
uint32_t emela_board_event_lock(void) { assert(!locked); locked = 1; return 7; }
void emela_board_event_unlock(uint32_t previous) { assert(locked && previous == 7); locked = 0; }
int main(void) {
  assert(pop(0) == -1);
  for (int i = 0; i < 16; i++) assert(push(0, i));
  assert(!push(0, 99));
  assert(push(1, 123));
  for (int i = 0; i < 16; i++) assert(pop(0) == i);
  assert(pop(0) == -1 && pop(1) == 123 && pop(1) == -1);
  for (int i = 0; i < 100; i++) { assert(push(0, i)); assert(pop(0) == i); }
}
"#),
    );
    reject(
        "import Std.Embedded.Uart\nimport Std.Embedded.Interrupt\nfn event(id: Uart.UartId, e: Uart.UartEvent) -> () fails {} use {} { word = Interrupt.try_pop(Interrupt.Channel0)\n () }\npub fn setup() -> () fails Uart.UartError { Uart.bind_interrupt(Uart.Uart1, event) }",
        "not marked ISR-safe",
    );
}

#[test]
fn bsp_uart_state_machine_preserves_deadlines_errors_and_receive_modes() {
    let output = source_output(
        "import Std.Embedded.Board.{BoardUart}\nimport Std.Embedded.Uart\npub fn setup() -> () fails Uart.UartError { with BoardUart { (use Uart.Uart).configure(Uart.Uart1, Uart.UartConfig(115200, Uart.Bits8, Uart.One, Uart.NoParity, Uart.Both, Uart.NoFlowControl, Uart.By16)) } }",
    );
    let temp = TempDir::new();
    for file in output.files.iter().filter(|f| f.path.ends_with(".h")) {
        fs::write(temp.0.join(&file.path), &file.contents).unwrap();
    }
    let driver = include_str!("../../boards/common/uart_driver.h");
    let source = include_str!("uart_driver.c").replace("/* @DRIVER@ */", driver);
    fs::write(temp.0.join("driver.c"), source).unwrap();
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
fn fixed_queues_reject_multiple_isr_producers_and_main_conflicts() {
    let prefix = "import Std.Embedded.Uart\nimport Std.Embedded.Interrupt\nfn event(id: Uart.UartId, e: Uart.UartEvent) -> () fails {} use {} { accepted = Interrupt.try_push(Interrupt.Channel0, 1)\n () }\n";
    reject(
        &(prefix.to_owned()
            + "pub fn setup() -> () fails Uart.UartError { Uart.bind_interrupt(Uart.Uart1, event)\n Uart.bind_interrupt(Uart.Uart2, event) }"),
        "multiple ISR producers",
    );
    reject(
        &(prefix.to_owned()
            + "pub fn setup() -> () fails Uart.UartError { Uart.bind_interrupt(Uart.Uart1, event)\n accepted = Interrupt.try_push(Interrupt.Channel0, 2)\n () }"),
        "main producer can conflict",
    );
    reject(
        "import Std.Embedded.Uart\nimport Std.Embedded.Interrupt\nfn event(id: Uart.UartId, e: Uart.UartEvent) -> () fails {} use {} { channel = id match { Uart.Uart1 -> Interrupt.Channel0\n _ -> Interrupt.Channel1 }\n accepted = Interrupt.try_push(channel, 1)\n () }\npub fn setup() -> () fails Uart.UartError { Uart.bind_interrupt(Uart.Uart1, event) }",
        "statically resolved WordChannel",
    );
}

const UART_QUEUE_PRODUCER: &str = r#"
import Std.Embedded.Uart
import Std.Embedded.Interrupt
fn event(id: Uart.UartId, e: Uart.UartEvent) -> () fails {} use {} {
  accepted = Interrupt.try_push(Interrupt.Channel0, 1)
  ()
}
"#;

#[test]
fn fixed_queues_reject_escaped_main_push_functions() {
    for (helpers, operation) in [
        (
            "fn apply(push: fn(Interrupt.WordChannel, Int) -> Bool use {}) -> Bool { push(Interrupt.Channel0, 2) }",
            "apply(Interrupt.try_push)",
        ),
        (
            "const PUSH = Interrupt.try_push\nfn apply(push: fn(Interrupt.WordChannel, Int) -> Bool use {}) -> Bool { push(Interrupt.Channel0, 2) }",
            "apply(PUSH)",
        ),
        (
            "fn sender() -> fn(Interrupt.WordChannel, Int) -> Bool use {} { Interrupt.try_push }",
            "sender()(Interrupt.Channel0, 2)",
        ),
        (
            "fn sender() -> fn(Interrupt.WordChannel, Int) -> Bool use {} { Interrupt.try_push }",
            "{ factory = sender\n push = factory()\n push(Interrupt.Channel0, 2) }",
        ),
        (
            "type Sender(push: fn(Interrupt.WordChannel, Int) -> Bool use {})",
            "{ sender = Sender(Interrupt.try_push)\n sender.push(Interrupt.Channel0, 2) }",
        ),
        (
            "",
            "{ push = Interrupt.try_push\n push(Interrupt.Channel0, 2) }",
        ),
        (
            "",
            "{ push = if choose { Interrupt.try_push } else { Interrupt.try_push }\n push(Interrupt.Channel0, 2) }",
        ),
        (
            "",
            "{ push = Interrupt.try_push\n send = fn() { push(Interrupt.Channel0, 2) }\n send() }",
        ),
    ] {
        reject(
            &format!(
                "{UART_QUEUE_PRODUCER}\n{helpers}\npub fn setup(choose: Bool) -> Bool fails Uart.UartError {{\n Uart.bind_interrupt(Uart.Uart1, event)\n {operation}\n}}"
            ),
            "main producer can conflict",
        );
    }
}

#[test]
fn fixed_queues_check_indirect_main_producers_for_gpio_owners() {
    reject(
        r#"
import Std.Embedded.Gpio
import Std.Embedded.Interrupt
fn edge(e: Gpio.GpioEvent) -> () fails {} use {} {
  accepted = Interrupt.try_push(Interrupt.Channel0, 1)
  ()
}
fn apply(push: fn(Interrupt.WordChannel, Int) -> Bool use {}) -> Bool {
  push(Interrupt.Channel0, 2)
}
pub fn setup() -> Bool fails Gpio.GpioError {
  Gpio.bind_interrupt(Gpio.GpioPin(0), edge)
  apply(Interrupt.try_push)
}
"#,
        "main producer can conflict",
    );
}

#[test]
fn fixed_queues_allow_static_aliases_and_wrappers_for_unowned_channels() {
    source_output(&format!(
        r#"{UART_QUEUE_PRODUCER}
const PUSH = Interrupt.try_push
fn sender() -> fn(Interrupt.WordChannel, Int) -> Bool use {{}} {{ Interrupt.try_push }}
fn send(value: Int) -> Bool {{ Interrupt.try_push(Interrupt.Channel1, value) }}
fn apply(push: fn(Int) -> Bool use {{}}) -> Bool {{ push(3) }}
pub fn setup() -> Bool fails Uart.UartError {{
  Uart.bind_interrupt(Uart.Uart1, event)
  push = Interrupt.try_push
  alias = push
  first = alias(Interrupt.Channel1, 2)
  second = PUSH(Interrupt.Channel1, 3)
  third = apply(send)
  fourth = sender()(Interrupt.Channel1, 4)
  first && second && third && fourth
}}
"#
    ));
}

#[test]
fn fixed_queues_allow_higher_order_push_without_isr_owners() {
    source_output(
        r#"
import Std.Embedded.Interrupt
fn apply(push: fn(Interrupt.WordChannel, Int) -> Bool use {}) -> Bool {
  push(Interrupt.Channel0, 2)
}
pub fn push() -> Bool { apply(Interrupt.try_push) }
"#,
    );
}
