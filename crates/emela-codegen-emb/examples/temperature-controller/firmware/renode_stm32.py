#!/usr/bin/env python3
"""Observe a CLI-built temperature controller after reset from a raw Flash image."""
import argparse
import fcntl
import hashlib
import json
import math
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile

CRATE = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(CRATE / "tools"))
from run_stm32f4 import atomic_json, metric
from validation_process import prepare_output, run_captured


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--firmware-dir", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--renode", type=Path, default=os.environ.get("RENODE"))
    parser.add_argument("--toolchain", type=Path, default=os.environ.get("ARM_TOOLCHAIN_PATH"))
    args = parser.parse_args()
    output = args.output.resolve()
    lease = None
    try:
        prepare_output(output)
        lease = (output / ".emela-temperature-flash-lock").open("a")
        fcntl.flock(lease, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except (OSError, ValueError) as error:
        if lease:
            lease.close()
        print(str(error), file=sys.stderr)
        return 1
    output.mkdir(parents=True, exist_ok=True)
    report = {"status": "running", "board": "stm32f446", "boot": "raw-flash-reset",
              "loads_elf": False, "sets_pc_sp": False}
    atomic_json(output / "report.json", report)
    monitor_directory = None
    try:
        firmware = args.firmware_dir.resolve()
        manifest = json.loads((firmware / "firmware.json").read_text())
        if not isinstance(manifest, dict) or manifest.get("board") != "stm32f446" or manifest.get("schema_version") != 1:
            raise ValueError("expected a stm32f446 firmware manifest")
        for name in ["firmware.elf", "firmware.bin", "firmware.map", "resources.json"]:
            if hashlib.sha256((firmware / name).read_bytes()).hexdigest() != manifest.get("sha256", {}).get(name):
                raise ValueError(f"firmware artifact does not match manifest: {name}")
        if args.renode is None:
            raise ValueError("set RENODE or pass --renode")
        renode = args.renode.expanduser().resolve()
        nm = str(args.toolchain.expanduser().resolve() / "bin/arm-none-eabi-nm") if args.toolchain else shutil.which("arm-none-eabi-nm")
        if not nm:
            raise ValueError("set ARM_TOOLCHAIN_PATH or put arm-none-eabi-nm on PATH")
        symbols = {}
        for line in subprocess.check_output([nm, "-n", firmware / "firmware.elf"], text=True).splitlines():
            fields = line.split()
            if len(fields) == 3:
                symbols[fields[-1]] = int(fields[0], 16)
        # The ELF is used for symbol addresses only; the emulated machine receives the BIN.
        shutil.copyfile(firmware / "firmware.bin", output / "firmware.bin")
        source = renode.parent / "platforms/cpus/stm32f4.repl"
        platform = "\n".join(line for line in source.read_text().splitlines() if not re.match(r"\s*ApplySVD\s", line)) + "\n"
        registration = "flash: Memory.MappedMemory @ sysbus 0x08000000"
        if platform.count(registration) != 1:
            raise ValueError("unexpected STM32F4 platform Flash registration")
        platform = platform.replace(registration, "flash: Memory.MappedMemory @ {\n        sysbus 0x08000000;\n        sysbus 0x00000000\n    }")
        (output / "stm32f4.repl").write_text(platform)
        monitor_directory = Path(tempfile.mkdtemp(prefix="emela-flash-monitor-"))
        monitor = monitor_directory / "artifacts"
        monitor.symlink_to(output)
        (output / "machine.repl").write_text(f'using "{monitor}/stm32f4.repl"\nnvic:\n    systickFrequency: 16000000\nflash:\n    size: 524288\nsram:\n    size: 131072\nheater: Miscellaneous.LED @ gpioPortA 5\ncooler: Miscellaneous.LED @ gpioPortA 6\ngpioPortA:\n    5 -> heater@0\n    6 -> cooler@0\n')
        uart_path = output / "uart.log"
        uart_path.unlink(missing_ok=True)
        script = f'mach create\nmachine LoadPlatformDescription @{monitor}/machine.repl\nsysbus LoadBinary @{monitor}/firmware.bin 0x08000000\nsysbus.cpu PerformanceInMips 100\nsysbus.usart2 CreateFileBackend @{monitor}/uart.log true\n'
        for name in ["board_boot_data", "board_boot_bss"]:
            script += f'sysbus WriteDoubleWord 0x{symbols[name]:x} 0xdeadbeef\n'
        script += "sysbus.cpu Reset\n"
        for sample in range(1, 25):
            duration = "0:00:00.500000" if sample == 1 else "0:00:01.000000"
            script += f'emulation RunFor "{duration}"\nsysbus InfoLog "EMELA_SAMPLE_{sample}"\nsysbus ReadDoubleWord 0x40020014\n'
        script += 'emulation RunFor "0:00:01.500000"\n'
        names = ["board_app_completed", "board_app_result", "board_app_failures", "board_ticks",
                 "board_notifications", "board_rejected_notifications", "board_uart_bytes", "board_boot_data", "board_boot_bss",
                 "board_boot_initialized", "emela_validation_stack_used", "emela_validation_stack_capacity",
                 "emela_validation_stack_guard_ok", "emela_validation_stack_samples"]
        for name in names:
            script += f'sysbus InfoLog "EMELA_METRIC_{name}"\nsysbus ReadDoubleWord 0x{symbols[name]:x}\n'
        script += 'sysbus InfoLog "EMELA_GPIO_FINAL"\nsysbus ReadDoubleWord 0x40020014\nquit\n'
        (output / "run.resc").write_text(script)
        with tempfile.TemporaryDirectory(prefix="renode-tmp-", dir=output) as temporary:
            environment = dict(os.environ, TMPDIR=temporary, XDG_CONFIG_HOME=str(output / "renode-config"))
            code, log, timed_out = run_captured([str(renode), "--disable-xwt", "--console", "--plain", "-e", "include @" + str(monitor / "run.resc")],
                                               cwd=CRATE, env=environment, timeout=600)
        (output / "renode.log").write_text(log)
        if code != 0 or timed_out or re.search(r"\[(?:ERROR|WARNING)\]|There was an error executing command|Unhandled Exception", log):
            raise ValueError(f"Renode failed or reported a warning (exit={code}, timeout={timed_out})")
        values = {name: metric(log, "EMELA_METRIC_" + name) for name in names}
        expected = {"board_app_completed": 1, "board_app_result": 24, "board_app_failures": 0,
                    "board_notifications": 23, "board_rejected_notifications": 0, "board_boot_data": 0x454d4244,
                    "board_boot_bss": 0x4253534f, "board_boot_initialized": 1,
                    "emela_validation_stack_guard_ok": 1}
        for name, value in expected.items():
            if values[name] != value:
                raise ValueError(f"{name}: {values[name]} != {value}")
        if values["board_ticks"] < 23000 or not 0 < values["emela_validation_stack_used"] <= values["emela_validation_stack_capacity"]:
            raise ValueError("timer did not advance or stack observation is invalid")
        uart = uart_path.read_text(encoding="utf-8-sig")
        lines = uart.splitlines()
        if len(lines) != 24 or values["board_uart_bytes"] != len(uart.encode("utf-8")):
            raise ValueError("UART does not contain exactly the 24 expected sample lines")
        modes = ["idle", "idle", "heating", "heating", "heating", "idle", "idle", "cooling",
                 "cooling", "cooling", "idle", "heating", "cooling", "heating", "fault", "idle",
                 "heating", "fault", "heating", "idle", "cooling", "fault", "cooling", "idle"]
        temperatures = [2100, 1900, 1800, 1900, 1999, 2000, 2700, 2800,
                        2700, 2601, 2600, 1800, 2900, 1700, None, 1900,
                        1800, None, 1700, 2000, 2800, None, 2900, 2600]
        observations = []
        for sample, (line, mode) in enumerate(zip(lines, modes), 1):
            match = re.fullmatch(rf"sample={sample} temperature=(\S+) mode={mode}", line)
            if not match:
                raise ValueError(f"unexpected sample {sample}: {line}")
            expected_temperature = temperatures[sample - 1]
            if expected_temperature is None:
                if match[1] != "unavailable":
                    raise ValueError(f"sample {sample} did not report the sensor failure")
            elif not math.isclose(float(match[1]), expected_temperature / 100, rel_tol=0, abs_tol=1e-12):
                raise ValueError(f"sample {sample} temperature does not match sensor input")
            mask = {"heating": 0x20, "cooling": 0x40}.get(mode, 0)
            observed = metric(log, f"EMELA_SAMPLE_{sample}") & 0x60
            if observed != mask:
                raise ValueError(f"sample {sample}: GPIO {observed:#x} != {mask:#x}")
            observations.append({"sample": sample, "mode": mode, "gpioa_actuators": observed})
        if metric(log, "EMELA_GPIO_FINAL") & 0x60:
            raise ValueError("actuators were not stopped")
        report.update({"status": "passed", "metrics": values, "samples": observations,
                       "resources": manifest["resources"], "flash_sha256": manifest["sha256"]["firmware.bin"]})
        print(uart, end="")
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        report.update({"status": "failed", "error": str(error)})
        print(str(error), file=sys.stderr)
    finally:
        atomic_json(output / "report.json", report)
        if monitor_directory:
            shutil.rmtree(monitor_directory)
        lease.close()
    print(f"{report['status'].upper()}: {output / 'report.json'}")
    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    sys.exit(main())
