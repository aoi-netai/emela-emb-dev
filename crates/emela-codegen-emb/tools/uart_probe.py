#!/usr/bin/env python3
"""Exercise native UART RX/TX IRQs and synchronous operations in Renode 1.16.1."""
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
    lease = claim_output(output, "uart")
    report = {"schema_version": 1, "status": "running"}
    atomic_json(output / "report.json", report)
    try:
        firmware = args.firmware_dir.resolve()
        manifest = verify_manifest(firmware)
        board = manifest["board"]
        if board not in {"stm32f407vg", "esp32c3"}:
            raise ValueError("UART IRQ validation requires stm32f407vg or esp32c3")
        c3 = board == "esp32c3"
        symbols = {name: value[0] for name, value in elf_symbols(firmware / "firmware.elf", 243 if c3 else 40).items()}
        renode = args.renode.expanduser().resolve()
        report.update(board=board, optimization=manifest["optimization"], firmware_sha256=manifest["sha256"],
                      resources=json.loads((firmware / "resources.json").read_text()),
                      limitations=["digital word injection, no electrical/baud-error/parity waveform validation", "no IRQ nesting or latency validation"])
        for name in ["console.log", "transmit.log", "model.log", "renode.log"]:
            (output / name).unlink(missing_ok=True)
        with tempfile.TemporaryDirectory(prefix="emela-uart-monitor-", dir="/tmp") as temporary:
            monitor = Path(temporary) / "artifacts"
            monitor.symlink_to(output)
            observer = "STM32F4ActuatorObserver.cs"
            shutil.copyfile(CRATE / "tools/models" / observer, output / observer)
            script = f'logFile @{monitor}/model.log\nlogLevel 1 file\nlogLevel 3 console\ninclude @{monitor}/{observer}\n'
            if c3:
                model = "ESP32C3GpioTestSoC.cs"
                shutil.copyfile(CRATE / "tools/models" / model, output / model)
                report.update(model="limited native-MMIO test SoC on generic RV32, SRAM ELF entry", model_sha256=digest(output / model))
                script += f'include @{monitor}/{model}\nmach create\n'
                platform = 'ram: Memory.MappedMemory @ { sysbus 0x40380000; sysbus 0x3fc80000 }\n    size: 0x20000\ncpu: CPU.ESP32C3GpioTestCPU @ sysbus\nsoc: Miscellaneous.ESP32C3GpioTestSoC @ sysbus 0x60000000\n    cpu: cpu\n    3 -> probeObserver@0\nprobeObserver: Miscellaneous.STM32F4ActuatorObserver @ sysbus\n'
                shutil.copyfile(firmware / "firmware.elf", output / "firmware.elf")
                script += f'machine LoadPlatformDescription @{monitor}/machine.repl\nsysbus LoadELF @{monitor}/firmware.elf\n'
            else:
                copy_platforms(renode, output, monitor)
                report.update(model="Renode STM32F4 USART1/NVIC, raw flash reset")
                platform = f'using "{monitor}/platforms/cpus/stm32f4.repl"\nnvic:\n    systickFrequency: 16000000\nflash:\n    size: 0x100000\nsram:\n    size: 0x20000\nprobeObserver: Miscellaneous.STM32F4ActuatorObserver @ sysbus\ngpioPortD:\n    12 -> probeObserver@0\n'
                shutil.copyfile(firmware / "firmware.bin", output / "firmware.bin")
                script += f'mach create\nmachine LoadPlatformDescription @{monitor}/machine.repl\nsysbus LoadBinary @{monitor}/firmware.bin 0x08000000\nsysbus.usart2 CreateFileBackend @{monitor}/console.log true\nsysbus.usart1 CreateFileBackend @{monitor}/transmit.log true\nsysbus.cpu Reset\n'
            (output / "machine.repl").write_text(platform)
            script += 'sysbus.cpu PerformanceInMips 100\n'
            expected = []

            def run(ms, events=None, level=None):
                nonlocal script
                script += f'emulation RunFor "0:00:00.{ms * 1000:06d}"\n'
                if events is not None:
                    index = len(expected)
                    script += f'echo "EMELA_EVENTS_{index}"\nsysbus ReadDoubleWord 0x{symbols["board_uart_events"]:x}\necho "EMELA_STATE_{index}"\nsysbus.probeObserver State\n'
                    expected.append((events, level))

            def receive(value):
                nonlocal script
                script += f'sysbus.soc ReceiveWord {value}\n' if c3 else f'sysbus.usart1 WriteChar {value}\n'

            extra = 0
            if args.mode == "polling":
                run(4, 1, 0)
                for word in [0, 10, 255]:
                    receive(word); run(1, 1, 0)
                run(7, 1, 0)
                for word in [3, 4, 5]:
                    receive(word); run(1, 1, 0)
                run(12, 1, 0)
                expected_events = 1
            else:
                run(3, 1, 1)
                receive(1); run(1, 2, 1)
                receive(0); run(1, 3, 0)
                if c3:
                    script += 'sysbus.soc ReceiveError 12\n'
                    run(1, 4, 0)
                    extra = 1
                run(8, 3 + extra, 0)
                receive(1); run(1, 3 + extra, 0)
                run(8, 3 + extra, 0)
                receive(1); run(1, 4 + extra, 1)
                receive(0); run(1, 5 + extra, 0)
                run(12, 5 + extra, 0)
                expected_events = 5 + extra
            failures = "board_failures" if c3 else "board_app_failures"
            guard = "board_stack_guard_ok" if c3 else "emela_validation_stack_guard_ok"
            stack = "board_stack_used" if c3 else "emela_validation_stack_used"
            names = ["board_app_completed", "board_app_result", failures, guard, stack]
            for name in names:
                script += f'echo "EMELA_METRIC_{name}"\nsysbus ReadDoubleWord 0x{symbols[name]:x}\n'
            if c3:
                script += 'echo "EMELA_TX_WORDS"\nsysbus.soc UartTxWords\necho "EMELA_CONSOLE"\nsysbus.soc ConsoleText\n'
            (output / "run.resc").write_text(script + 'quit\n')
            (output / "renode-config/renode").mkdir(parents=True, exist_ok=True)
            with tempfile.TemporaryDirectory(prefix="renode-tmp-", dir=output) as process_tmp:
                env = dict(os.environ, TMPDIR=process_tmp, XDG_CONFIG_HOME=str(output / "renode-config"))
                code, log, timed_out = run_captured([renode, "--disable-xwt", "--console", "--plain", "-e", f"include @{monitor}/run.resc"], cwd=CRATE, env=env, timeout=args.timeout)
            (output / "renode.log").write_text(log)
        model_log = (output / "model.log").read_text(encoding="utf-8-sig")
        if not c3:
            # Renode 1.16.1 tags PEIE/EIE but does not model parity/framing errors.
            patterns = [
                r"^.*\[WARNING\] usart1: Unhandled write to offset 0xC\. Unhandled bits: \[8\] when writing value 0x212C\. Tags: PEIE \(0x1\)\.\r?$",
                r"^.*\[WARNING\] usart1: Unhandled write to offset 0x14\. Unhandled bits: \[0\] when writing value 0x1\. Tags: EIE \(0x1\)\.\r?$",
            ]
            report["known_model_warnings"] = []
            for pattern in patterns:
                matches = re.findall(pattern, model_log, re.MULTILINE)
                if len(matches) != (4 if args.mode == "polling" else 3):
                    raise ValueError("unexpected STM32 USART error-interrupt model warning count")
                report["known_model_warnings"] += matches
                model_log = re.sub(pattern, "", model_log, flags=re.MULTILINE)
            report["limitations"] += ["STM32 PEIE/EIE are unimplemented model tags; TC is immediate in this model"]
        version = re.search(r"Renode, version (\S+)", log)
        if not version or version[1] != "1.16.1":
            raise ValueError("this UART scenario requires Renode 1.16.1")
        if code or timed_out or re.search(r"\[(?:ERROR|WARNING)\]|There was an error executing command|Unhandled Exception", log + model_log):
            raise ValueError(f"Renode failed or reported an unexpected warning; see {output}/renode.log and model.log")
        observed = [(metric(log, f"EMELA_EVENTS_{i}"), metric(log, f"EMELA_STATE_{i}")) for i in range(len(expected))]
        if observed != expected:
            raise ValueError(f"UART event/output mismatch: expected={expected}, observed={observed}")
        values = {name: metric(log, "EMELA_METRIC_" + name) for name in names}
        if any(values[name] != value for name, value in [("board_app_completed", 1), ("board_app_result", 42), (failures, 0), (guard, 1)]):
            raise ValueError(f"UART completion or stack guard failed: {values}")
        console = log if c3 else (output / "console.log").read_text()
        for stage in (["polling", "discard", "complete"] if args.mode == "polling" else ["interrupt", "stopped", "restarted", "complete"]):
            if console.count("uart: " + stage) != 1:
                raise ValueError(f"missing/repeated UART stage: {stage}")
        if c3:
            if not re.search(r"EMELA_TX_WORDS\s+0,10,255", log):
                raise ValueError("UART1 TX word mismatch")
        elif (output / "transmit.log").read_bytes() != bytes([0, 10, 255]):
            raise ValueError("USART1 TX byte mismatch")
        report.update(status="passed", observations=observed, metrics=values, renode_version=version[1],
                      mode=args.mode, uart_irq_events=expected_events, renode_log_sha256=digest(output / "renode.log"))
        print(f"PASSED {board}/{manifest['optimization']}: {args.mode} RX, TX/RX IRQ, {expected_events} events, stack {values[stack]} bytes")
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
    parser.add_argument("--mode", choices=["polling", "interrupt"], default="interrupt")
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
