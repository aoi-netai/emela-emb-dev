#!/usr/bin/env python3
"""Build and observe the ESP32-C3 Renode CPU/MMIO/interrupt probe."""
import argparse
import datetime
from decimal import Decimal
import fcntl
import json
import os
from pathlib import Path
import re
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import time

sys.dont_write_bytecode = True
from setup import (MODEL_COMMIT, MODEL_URL, ROM_SHA256, TOOLCHAIN_SHA256,
                   TOOLCHAIN_VERSION, default_cache, ensure_models, ensure_rom,
                   ensure_toolchain, sha256)

HERE = Path(__file__).resolve().parent
SCALARS = ["probe_status", "probe_failure", "probe_irq_count", "probe_main_count", "probe_gpio_count", "probe_stack_used",
           "probe_stack_guard_ok"]
ARRAYS = ["probe_gpio_trace", "probe_irq_causes", "probe_irq_pcs", "probe_irq_ticks"]
MODEL_FILES = ["interrupt-matrix/ESP32C3_InterruptMatrix.cs", "gpio/ESP32C3_GPIO.cs",
               "systimer/ESP32C3_SysTimer.cs", "uart/ESP32C3_UART.cs"]


def atomic_json(path, data):
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n")
    temporary.replace(path)


def metric(log, label):
    match = re.search(re.escape(label) + r"[^\n]*\n\s*(0x[0-9a-fA-F]+|\d+)", log)
    if not match:
        raise ValueError(f"missing Renode observation: {label}")
    return int(match[1], 0)


def interval(seconds):
    value = Decimal(str(seconds))
    if not value.is_finite() or not Decimal("0") < value <= Decimal("3600"):
        raise ValueError("simulation seconds must be finite and within (0, 3600]")
    return f"0:0:{value:.6f}"


def executable(value, name):
    candidate = value or shutil.which(name)
    if not candidate:
        raise FileNotFoundError(f"{name} was not found; specify --{name} or set {name.upper()}")
    path = Path(candidate).resolve()
    if not path.is_file() or not os.access(path, os.X_OK):
        raise FileNotFoundError(f"executable not found: {path}")
    return path


class Case:
    def __init__(self, args):
        self.args = args
        self.out = args.output.resolve()
        self.out.mkdir(parents=True, exist_ok=True)
        self.lock = (self.out / ".run.lock").open("a")
        fcntl.flock(self.lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        marker = self.out / ".emela-esp32c3-output"
        ownership = "emela-codegen-emb/renode-esp32c3/v1\n"
        if marker.exists():
            if marker.read_text() != ownership:
                raise ValueError(f"unrecognized output ownership marker: {self.out}")
        elif any(item.name != ".run.lock" for item in self.out.iterdir()):
            raise ValueError(f"output is not an empty or managed directory: {self.out}")
        else:
            marker.write_text(ownership)
        self.process_tmp = self.out / "process-tmp"
        self.process_tmp.mkdir(exist_ok=True)
        for name in ["metrics.json", "uart.log"]:
            (self.out / name).unlink(missing_ok=True)
        self.report = {"status": "running", "case": args.case,
                       "started_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
                       "commands": [], "model": {"url": MODEL_URL, "commit": MODEL_COMMIT}}
        self.save()

    def save(self):
        atomic_json(self.out / "report.json", self.report)

    def run(self, name, command, timeout=300, env=None):
        command = list(map(str, command))
        log_path = self.out / (name + ".log")
        record = {"name": name, "argv": command, "log": str(log_path), "timeout_seconds": timeout}
        self.report["commands"].append(record)
        self.save()
        started = time.monotonic()
        with log_path.open("w") as stream:
            process = subprocess.Popen(command, stdout=stream, stderr=subprocess.STDOUT,
                                       env={**os.environ, "TMPDIR": str(self.process_tmp), **(env or {})}, start_new_session=True)
            try:
                record["returncode"] = process.wait(timeout=timeout)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()
                record["timed_out"] = True
                raise TimeoutError(f"{name} timed out after {timeout}s; see {log_path}")
            finally:
                record["wall_seconds"] = round(time.monotonic() - started, 3)
                self.save()
        result = log_path.read_text(errors="replace")
        if record["returncode"]:
            raise RuntimeError(f"{name} exited {record['returncode']}; see {log_path}")
        return result

    def execute(self):
        args = self.args
        if not 0 < args.mips <= 1000:
            raise ValueError("mips must be within (0, 1000]")
        if args.seconds is not None:
            interval(args.seconds)
        renode = executable(args.renode, "renode")
        models = ensure_models(args.cache.resolve(), args.models)
        self.report["renode"] = {"executable": str(renode), "version": self.run("renode-version", [renode, "--version"]).strip()}
        self.report["model"]["checkout"] = str(models)
        # Monitor commands use space-free aliases, while real output paths can contain spaces.
        with tempfile.TemporaryDirectory(prefix="emela-esp32c3-monitor-") as temporary:
            monitor = Path(temporary)
            (monitor / "out").symlink_to(self.out, target_is_directory=True)
            (monitor / "models").symlink_to(models, target_is_directory=True)
            self.monitor = monitor
            if args.case == "probe":
                self.probe(models)
            else:
                self.hello(models)

    def emulate(self, script, seconds):
        uart_path = self.out / "uart.log"
        uart_path.unlink(missing_ok=True)
        (self.out / "run.resc").write_text(script + "\nquit\n")
        self.report["simulation"] = {"seconds": seconds, "mips": self.args.mips,
                                      "wall_timeout_seconds": self.args.timeout}
        log = self.run("renode", [self.report["renode"]["executable"], "--disable-xwt", "--console", "--plain",
                                  "-e", "include @" + str(self.monitor / "out/run.resc")],
                       timeout=self.args.timeout,
                       env={"XDG_CONFIG_HOME": str(self.out / "renode-config")})
        if re.search(r"There was an error executing command|Unhandled Exception|\[ERROR\]", log):
            raise ValueError("Renode reported a command or runtime error; see renode.log")
        warnings = [line for line in log.splitlines() if "[WARNING]" in line]
        self.report["renode"]["warnings"] = warnings
        allowed_hello_warning = re.compile(
            r"\[WARNING\] rtc: Unhandled (?:read from offset 0x80 \(BiasConf\+0x4\)\."
            r"|write to offset 0x80 \(BiasConf\+0x4\), value 0x(?:0|10000000)\.)$")
        for line in warnings:
            if self.args.case != "hello" or not allowed_hello_warning.search(line):
                raise ValueError(f"unexpected Renode warning: {line}")
        if not uart_path.is_file():
            raise ValueError("Renode did not produce a fresh UART log")
        return log, uart_path.read_text(encoding="utf-8-sig")

    def probe(self, models):
        args = self.args
        seconds = args.seconds if args.seconds is not None else 0.1
        duration = interval(seconds)
        toolchain = ensure_toolchain(args.cache.resolve(), args.toolchain)
        tools = {name: toolchain / "bin" / ("riscv32-esp-elf-" + name) for name in ["gcc", "nm", "size"]}
        if any(not path.is_file() for path in tools.values()):
            raise FileNotFoundError(f"incomplete toolchain: {toolchain}")
        self.report["toolchain"] = {"directory": str(toolchain), "pinned_version": TOOLCHAIN_VERSION,
                                     "archive_sha256": TOOLCHAIN_SHA256,
                                     "compiler": self.run("compiler-version", [tools["gcc"], "--version"]).splitlines()[0],
                                     "source": "override" if args.toolchain else "verified archive cache"}
        objects = self.out / "objects"
        objects.mkdir(exist_ok=True)
        for stale in objects.iterdir():
            if stale.is_file():
                stale.unlink()
        elf = self.out / "firmware.elf"
        elf.unlink(missing_ok=True)
        flags = ["-march=rv32imc_zicsr_zifencei", "-mabi=ilp32", "-" + args.optimization, "-g3", "-ffreestanding",
                 "-fno-builtin", "-fno-pic", "-msmall-data-limit=0", "-ffunction-sections", "-fdata-sections",
                 "-Wall", "-Wextra", "-Werror", "-fstack-usage"]
        sources = [HERE / "probe.c", HERE / "startup.S"]
        compiled = []
        for index, source in enumerate(sources):
            obj = objects / (source.name + ".o")
            self.run(f"compile-{index}", [tools["gcc"], *flags, "-c", source, "-o", obj])
            compiled.append(obj)
        self.run("link", [tools["gcc"], *flags, "-nostdlib", "-nostartfiles", "-T", HERE / "firmware.ld",
                          "-Wl,--gc-sections", "-Wl,--no-relax", "-Wl,-Map," + str(self.out / "firmware.map"),
                          *compiled, "-lgcc", "-o", elf])
        size_lines = self.run("size", [tools["size"], elf]).splitlines()
        sizes = size_lines[1].split()
        self.report["sections"] = {name: int(value) for name, value in zip(["text", "data", "bss"], sizes[:3])}
        symbols = {}
        for line in self.run("symbols", [tools["nm"], "-S", "--defined-only", elf]).splitlines():
            fields = line.split()
            if len(fields) == 4:
                symbols[fields[3]] = (int(fields[0], 16), int(fields[1], 16))
            elif len(fields) == 3:
                symbols[fields[2]] = (int(fields[0], 16), 0)
        for name in SCALARS:
            if symbols.get(name, (0, 0))[1] != 4:
                raise ValueError(f"missing 32-bit probe metric: {name}")
        for name in ARRAYS:
            if symbols.get(name, (0, 0))[1] != 32:
                raise ValueError(f"missing 8-element probe metric: {name}")
        if "trap_entry" not in symbols:
            raise ValueError("missing probe trap_entry symbol")
        data = elf.read_bytes()
        if data[:7] != b"\x7fELF\x01\x01\x01":
            raise ValueError("expected a little-endian ELF32 firmware")
        entry = struct.unpack_from("<I", data, 24)[0]
        self.report["firmware"] = {"elf": str(elf), "sha256": sha256(elf), "entry": entry,
                                    "optimization": args.optimization,
                                    "sources": {p.name: sha256(p) for p in [*sources, HERE / "firmware.ld"]}}
        (self.out / "machine.repl").write_text(PROBE_PLATFORM)
        monitor = self.monitor
        includes = "\n".join("include @" + str(monitor / "models/peripherals" / item) for item in MODEL_FILES)
        script = f'''using sysbus
{includes}
mach create "esp32c3-probe"
machine LoadPlatformDescription @{monitor / "out/machine.repl"}
sysbus LoadELF @{monitor / "out/firmware.elf"}
cpu PC 0x{entry:x}
cpu MTVEC 0x{symbols['trap_entry'][0] | 3:x}
sysbus WriteByte 0x20001005 1
sysbus WriteByte 0x20001006 0
sysbus WriteByte 0x20001007 0xff
cpu PerformanceInMips {args.mips}
sysbus.uart0 CreateFileBackend @{monitor / "out/uart.log"} true
emulation RunFor "{duration}"
'''
        observations = {name: address for name, (address, _) in symbols.items() if name in SCALARS}
        for name in ARRAYS:
            observations.update({f"{name}_{index}": symbols[name][0] + index * 4 for index in range(8)})
        observations["gpio_out"] = 0x60004004
        for name, address in observations.items():
            script += f'sysbus InfoLog "EMELA_METRIC_{name}"\nsysbus ReadDoubleWord 0x{address:x}\n'
        log, uart = self.emulate(script, seconds)
        values = {name: metric(log, "EMELA_METRIC_" + name) for name in observations}
        atomic_json(self.out / "metrics.json", values)
        self.report["metrics"] = values
        expected = {"probe_status": 0x600D, "probe_failure": 0, "probe_irq_count": 4, "probe_main_count": 4, "probe_gpio_count": 8,
                    "probe_stack_guard_ok": 1, "gpio_out": 0}
        expected.update({f"probe_gpio_trace_{i}": 8 if i % 2 == 0 else 0 for i in range(8)})
        expected.update({f"probe_irq_causes_{i}": 0 for i in range(4, 8)})
        for name, value in expected.items():
            if values[name] != value:
                raise ValueError(f"{name}: observed {values[name]:#x}, expected {value:#x}")
        last_tick = 0
        for index in range(4):
            tick = values[f"probe_irq_ticks_{index}"]
            pc = values[f"probe_irq_pcs_{index}"]
            cause = values[f"probe_irq_causes_{index}"]
            # CLIC mcause also carries the saved MPP/MPIE state above the interrupt ID.
            if cause & 0x80000FFF != 0x80000001:
                raise ValueError(f"invalid CLIC interrupt cause at sample {index}: {cause:#x}")
            if tick - last_tick < 160000 or not 0x40380000 <= pc < 0x40384000:
                raise ValueError(f"invalid IRQ timestamp or interrupted PC at sample {index}")
            last_tick = tick
        if not 0 < values["probe_stack_used"] <= 16 * 1024 - 256:
            raise ValueError("invalid observed probe stack usage")
        if uart.count("ESP32C3_PROBE_PASS irq=4 main=4 gpio=8") != 1 or "PROBE_FAIL" in uart:
            raise ValueError("UART did not report one successful probe")
        self.report["scope"] = {"firmware": "freestanding C probe; Emela is not integrated yet",
                                 "interrupt_path": "SYSTIMER target0 -> interrupt matrix source37 -> CLIC line1 -> CPU trap -> firmware ISR -> main",
                                 "gpio": "MMIO output readback and model-internal loopback; no external GPIO endpoint",
                                 "ram_bytes": 65536, "rom_required": False, "cpu_dispatch_hook": False,
                                 "mtvec_workaround": "monitor API sets ELF trap_entry|3 before execution; no PC hook"}
        print(uart, end="")

    def hello(self, models):
        args = self.args
        rom = ensure_rom(args.cache.resolve(), args.rom)
        (self.monitor / "rom.elf").symlink_to(rom)
        monitor = self.monitor
        seconds = args.seconds if args.seconds is not None else 0.05
        duration = interval(seconds)
        script = f'''$base=@{monitor / "models"}
$rom_elf=@{monitor / "rom.elf"}
include @{monitor / "models/hello_world/setup.resc"}
sysbus.uart0 CreateFileBackend @{monitor / "out/uart.log"} true
cpu PerformanceInMips {args.mips}
emulation RunFor "0:0:1"
include @{monitor / "models/tests/clic_setup.resc"}
emulation RunFor "{duration}"
'''
        log, uart = self.emulate(script, 1 + seconds)
        for marker in ["main_task: Calling app_main()", "Hello world!", "silicon revision"]:
            if marker not in uart:
                raise ValueError(f"upstream SDK Hello did not produce {marker!r}")
        self.report["rom"] = {"elf": str(rom), "sha256": ROM_SHA256}
        self.report["scope"] = {"firmware": "upstream prebuilt SDK Hello; not the Emela backend",
                                 "elf_sha256": sha256(models / "hello_world/firmware/hello_world.elf"),
                                 "ram_bytes": 0x78000,
                                 "cpu_dispatch_hook": True,
                                 "limitations": ["upstream ROM-to-firmware fixed-PC hook",
                                                 "upstream fixed interrupt dispatch hook",
                                                 "480 KiB RAM model exceeds physical 400 KiB",
                                                 "23 placeholder peripherals; SDK boot is not peripheral coverage"]}
        print(uart, end="")


PROBE_PLATFORM = '''cpu: CPU.RiscV32 @ sysbus
    cpuType: "rv32imc_zicsr_zifencei"
    privilegedArchitecture: PrivilegedArchitecture.PrivUnratified
    timeProvider: clint
clint: IRQControllers.CoreLevelInterruptor @ sysbus 0x60801000
    frequency: 16000000
clic: IRQControllers.CoreLocalInterruptController @ {
        sysbus <0x20000000, +0x10000>;
        cpu <0x1000, +0x4A1>
    }
    cpu: cpu
    numberOfInterrupts: 32
    machineLevelBits: 4
    supervisorLevelBits: 0
    modeBits: 0
ram: Memory.MappedMemory @ {
        sysbus 0x3FC80000;
        sysbus 0x40380000
    }
    size: 0x10000
uart0: UART.ESP32C3_UART @ sysbus 0x60000000
gpio: GPIOPort.ESP32C3_GPIO @ sysbus 0x60004000
systimer: Timers.ESP32C3_SysTimer @ sysbus 0x60023000
    0 -> intmatrix@37
intmatrix: IRQControllers.ESP32C3_InterruptMatrix @ sysbus 0x600C2000
    [0-31] -> clic@[0-31]
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--case", choices=["probe", "hello"], default="probe")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--cache", type=Path, default=default_cache())
    parser.add_argument("--models", type=Path)
    parser.add_argument("--toolchain", type=Path, default=os.environ.get("RISCV32_ESP_TOOLCHAIN"))
    parser.add_argument("--rom", type=Path)
    parser.add_argument("--renode", default=os.environ.get("RENODE"))
    parser.add_argument("--optimization", choices=["O0", "O2"], default="O2")
    parser.add_argument("--seconds", type=float)
    parser.add_argument("--mips", type=int, default=10)
    parser.add_argument("--timeout", type=int, default=300)
    args = parser.parse_args()
    args.output = args.output or Path(f"/tmp/emela-esp32c3-{args.case}-{args.optimization}")
    try:
        case = Case(args)
    except (OSError, ValueError) as error:
        print(str(error), file=sys.stderr)
        return 1
    try:
        if args.timeout <= 0:
            raise ValueError("timeout must be positive")
        case.execute()
        case.report["status"] = "passed"
    except Exception as error:
        case.report.update({"status": "failed", "error": str(error)})
        print(str(error), file=sys.stderr)
    finally:
        case.report["finished_at"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
        case.save()
        case.lock.close()
    print(f"{case.report['status'].upper()}: esp32c3/{args.case}: {case.out / 'report.json'}")
    return 0 if case.report["status"] == "passed" else 1


if __name__ == "__main__":
    sys.exit(main())
