#!/usr/bin/env python3
"""Check native GPIO firmware, external edges, ISR dispatch and stopping in Renode 1.16.1."""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import sys
import tempfile

sys.dont_write_bytecode = True
from board_probe_common import atomic_json, claim_output, digest, elf_symbols, metric, run_captured, verify_manifest
from board_probe_stm32f4 import copy_platforms

CRATE = Path(__file__).resolve().parents[1]


def observe(args):
    output = args.output.resolve()
    lease = claim_output(output, "gpio")
    report = {"schema_version": 1, "status": "running"}
    atomic_json(output / "report.json", report)
    try:
        for name in ["uart.log", "model.log", "renode.log", "run.resc"]:
            (output / name).unlink(missing_ok=True)
        firmware = args.firmware_dir.resolve()
        manifest = verify_manifest(firmware)
        board = manifest["board"]
        if board not in {"stm32f407vg", "esp32c3"}:
            raise ValueError("the GPIO IRQ scenario requires stm32f407vg or esp32c3")
        c3 = board == "esp32c3"
        symbols = {name: value[0] for name, value in elf_symbols(firmware / "firmware.elf", 243 if c3 else 40).items()}
        renode = args.renode.expanduser().resolve()
        report.update(board=board, optimization=manifest["optimization"], firmware_sha256=manifest["sha256"],
                      boot="ELF SRAM entry (no ROM/flash validation)" if c3 else "raw flash reset",
                      model="repository GPIO test SoC on generic RV32" if c3 else "Renode STM32F4 + SYSCFG routing",
                      limitations=["binary GPIO; no analog voltage, electrical glitch or IRQ latency guarantee"])
        if c3:
            report["limitations"] += ["test SoC models only the GPIO/UART/timer/controller subset", "controller enable mirrored to generic CPU mie; no IRQ nesting or priority arbitration validation"]
        with tempfile.TemporaryDirectory(prefix="emela-gpio-monitor-", dir="/tmp") as temporary:
            monitor = Path(temporary) / "artifacts"
            monitor.symlink_to(output)
            observer = "STM32F4ActuatorObserver.cs"
            shutil.copyfile(CRATE / "tools/models" / observer, output / observer)
            script = f'logFile @{monitor}/model.log\nlogLevel 1 file\nlogLevel 3 console\ninclude @{monitor}/{observer}\n'
            if c3:
                model = "ESP32C3GpioTestSoC.cs"
                shutil.copyfile(CRATE / "tools/models" / model, output / model)
                report["model_sha256"] = digest(output / model)
                script += f'include @{monitor}/{model}\nmach create\n'
                platform = 'ram: Memory.MappedMemory @ { sysbus 0x40380000; sysbus 0x3fc80000 }\n    size: 0x20000\ncpu: CPU.ESP32C3GpioTestCPU @ sysbus\nsoc: Miscellaneous.ESP32C3GpioTestSoC @ sysbus 0x60000000\n    cpu: cpu\n    3 -> probeObserver@0\nprobeObserver: Miscellaneous.STM32F4ActuatorObserver @ sysbus\n'
                shutil.copyfile(firmware / "firmware.elf", output / "firmware.elf")
                script += f'machine LoadPlatformDescription @{monitor}/machine.repl\nsysbus LoadELF @{monitor}/firmware.elf\n'
                endpoint = "soc"
            else:
                copy_platforms(renode, output, monitor)
                repl = output / "platforms/cpus/stm32f4.repl"
                text = repl.read_text()
                port = -1
                lines = []
                for line in text.splitlines():
                    match = re.match(r"gpioPort([A-K]):", line)
                    if match:
                        port = ord(match[1]) - ord("A")
                    if line.strip() == "[0-15] -> exti@[0-15]":
                        line = f"    [0-15] -> syscfg#{port}@[0-15]"
                    lines.append(line)
                repl.write_text("\n".join(lines) + "\n")
                platform = f'using "{monitor}/platforms/cpus/stm32f4.repl"\nnvic:\n    systickFrequency: 16000000\nflash:\n    size: 0x100000\nsram:\n    size: 0x20000\nsyscfg: Miscellaneous.STM32_SYSCFG @ sysbus 0x40013800\n    [0-15] -> exti@[0-15]\nprobeObserver: Miscellaneous.STM32F4ActuatorObserver @ sysbus\ngpioPortD:\n    12 -> syscfg#3@12 | probeObserver@0\n'
                shutil.copyfile(firmware / "firmware.bin", output / "firmware.bin")
                script += f'mach create\nmachine LoadPlatformDescription @{monitor}/machine.repl\nsysbus LoadBinary @{monitor}/firmware.bin 0x08000000\nsysbus.usart2 CreateFileBackend @{monitor}/uart.log true\nsysbus.cpu Reset\n'
                endpoint = "gpioPortA"
            (output / "machine.repl").write_text(platform)
            script += 'sysbus.cpu PerformanceInMips 100\n'
            expected = []

            def run(milliseconds, events, state):
                nonlocal script
                script += f'emulation RunFor "0:00:00.{milliseconds * 1000:06d}"\n'
                index = len(expected)
                script += f'echo "EMELA_EVENTS_{index}"\nsysbus ReadDoubleWord 0x{symbols["board_gpio_events"]:x}\necho "EMELA_STATE_{index}"\nsysbus.probeObserver State\n'
                expected.append((events, state))

            def edge(pin, level):
                nonlocal script
                script += f'sysbus.{endpoint} OnGPIO {pin} {str(level).lower()}\n'

            run(3, 0, 1)
            edge(6, True); run(2, 1, 1)
            edge(6, False); run(2, 2, 0)
            edge(7, True); run(2, 3, 1)
            edge(6, True); edge(7, False); run(2, 5, 0)
            run(12, 5, 0)
            # Leave an old hardware pending bit on the masked first pin. Its
            # next enable must clear it, without clearing the second pin's edge.
            script += f'sysbus WriteDoubleWord 0x{0x60004048 if c3 else 0x40013c10:x} 0x40\n'
            edge(6, False); edge(7, True); run(2, 6, 1)
            edge(7, False); run(2, 7, 0)
            edge(6, True); run(16, 7, 0)
            edge(6, False); run(2, 7, 0)
            edge(6, True); run(2, 8, 1)
            edge(6, False); run(2, 8, 1)
            run(14, 8, 1)
            edge(6, True); run(2, 8, 1)
            edge(6, False); run(2, 9, 0)
            edge(6, True); run(2, 9, 0)
            run(23, 9, 0)
            edge(6, False); edge(7, True); run(3, 9, 0)
            failures = "board_failures" if c3 else "board_app_failures"
            guard = "board_stack_guard_ok" if c3 else "emela_validation_stack_guard_ok"
            stack = "board_stack_used" if c3 else "emela_validation_stack_used"
            names = ["board_app_completed", "board_app_result", failures, guard, stack]
            if c3:
                names += ["board_trap_cause", "board_trap_pc"]
            for name in names:
                script += f'echo "EMELA_METRIC_{name}"\nsysbus ReadDoubleWord 0x{symbols[name]:x}\n'
            if c3:
                script += 'echo "EMELA_CONSOLE"\nsysbus.soc ConsoleText\n'
            (output / "run.resc").write_text(script + "quit\n")
            (output / "renode-config/renode").mkdir(parents=True, exist_ok=True)
            with tempfile.TemporaryDirectory(prefix="renode-tmp-", dir=output) as process_tmp:
                env = dict(os.environ, TMPDIR=process_tmp, XDG_CONFIG_HOME=str(output / "renode-config"))
                code, log, timed_out = run_captured([renode, "--disable-xwt", "--console", "--plain", "-e", f"include @{monitor}/run.resc"], cwd=CRATE, env=env, timeout=args.timeout)
            (output / "renode.log").write_text(log)
        model_log = (output / "model.log").read_text(encoding="utf-8-sig")
        version = re.search(r"Renode, version (\S+)", log)
        if not version or version[1] != "1.16.1":
            raise ValueError("this GPIO scenario requires Renode 1.16.1")
        report["renode_version"] = version[1]
        known_warnings = []
        if not c3:
            # Renode 1.16.1 tags OTYPER but does not implement open-drain drive.
            warning = r"^.*\[WARNING\] gpioPortD: Unhandled write to offset 0x4\. Unhandled bits: \[12\] when writing value 0x1000\. Tags: OT12 \(0x1\)\.\r?$"
            known_warnings = re.findall(warning, model_log, re.MULTILINE)
            if len(known_warnings) != 1:
                raise ValueError("expected exactly the documented STM32 open-drain model warning")
            model_log = re.sub(warning, "", model_log, flags=re.MULTILINE)
            report["limitations"] += ["STM32 model does not implement OTYPER/open-drain electrical behavior"]
        report["known_model_warnings"] = known_warnings
        if code != 0 or timed_out or re.search(r"\[(?:ERROR|WARNING)\]|There was an error executing command|Unhandled Exception", log + model_log):
            raise ValueError(f"Renode failed or reported an unexpected warning; see {output}/renode.log and model.log")
        observed = [(metric(log, f"EMELA_EVENTS_{i}"), metric(log, f"EMELA_STATE_{i}")) for i in range(len(expected))]
        if observed != expected:
            raise ValueError(f"GPIO event/output mismatch: expected={expected}, observed={observed}")
        values = {name: metric(log, "EMELA_METRIC_" + name) for name in names}
        if any(values[name] != value for name, value in [("board_app_completed", 1), ("board_app_result", 42), (failures, 0), (guard, 1)]):
            raise ValueError(f"firmware completion or stack guard failed: {values}")
        console = log if c3 else (output / "uart.log").read_text()
        for message in ["both enabled", "first stopped", "rising enabled", "falling enabled", "complete"]:
            if console.count("gpio: " + message) != 1:
                raise ValueError(f"missing or repeated console stage: {message}")
        report.update(status="passed", observations=observed, metrics=values, gpio_irq_events=9,
                      stack_reserved_bytes=16384, renode_log_sha256=digest(output / "renode.log"))
        print(f"PASSED {board}/{manifest['optimization']}: 9 GPIO IRQ events, 18 input/output checkpoints, stack {values[stack]}/16384 bytes")
    except Exception as error:
        report.update(status="failed", error=str(error))
        raise
    finally:
        atomic_json(output / "report.json", report)
        lease.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--firmware-dir", required=True, type=Path)
    parser.add_argument("--renode", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--timeout", type=float, default=60)
    args = parser.parse_args()
    if not 0 < args.timeout <= 3600:
        parser.error("timeout must be in (0, 3600]")
    try:
        observe(args)
    except (OSError, ValueError) as error:
        print(error, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
