#!/usr/bin/env python3
"""Run the unchanged temperature controller on ESP32-C3/Renode."""
import argparse
import datetime
import fcntl
import importlib.util
import os
from pathlib import Path
import re
import shutil
import struct
import sys
import tempfile

sys.dont_write_bytecode = True
HERE = Path(__file__).resolve().parent
EXAMPLE = HERE.parent
CRATE = EXAMPLE.parents[1]
WORKSPACE = CRATE.parents[1]
PROBE = CRATE / "examples/renode-esp32c3"
sys.path.insert(0, str(PROBE))


def load_module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


probe = load_module("esp32c3_probe_runner", PROBE / "run.py")
scenario = load_module("temperature_scenario", EXAMPLE / "renode/run.py")
from setup import (MODEL_COMMIT, MODEL_URL, TOOLCHAIN_SHA256, TOOLCHAIN_VERSION,
                   default_cache, ensure_models, ensure_toolchain, sha256)

STACK_METRICS = scenario.STACK_METRICS


class TemperatureCase(probe.Case):
    def __init__(self, args):
        self.args = args
        self.out = args.output.resolve()
        self.out.mkdir(parents=True, exist_ok=True)
        self.lock = (self.out / ".run.lock").open("a")
        fcntl.flock(self.lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        marker = self.out / ".emela-temperature-esp32c3-output"
        ownership = "emela-codegen-emb/temperature-controller/esp32c3/v1\n"
        if marker.exists():
            if marker.read_text() != ownership:
                raise ValueError(f"unrecognized output ownership marker: {self.out}")
        elif any(item.name != ".run.lock" for item in self.out.iterdir()):
            raise ValueError(f"output is not an empty or managed directory: {self.out}")
        else:
            marker.write_text(ownership)
        self.process_tmp = self.out / "process-tmp"
        self.process_tmp.mkdir(exist_ok=True)
        for name in ["metrics.json", "resources.json", "uart.log", "firmware.elf", "model.log"]:
            (self.out / name).unlink(missing_ok=True)
        self.report = {"status": "running", "case": "temperature-controller-esp32c3",
                       "optimization": args.optimization, "continuous": args.continuous,
                       "verify_scenario": args.verify_scenario,
                       "started_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
                       "commands": [], "model": {"url": MODEL_URL, "commit": MODEL_COMMIT}}
        self.save()

    def execute(self):
        args = self.args
        if not 0 < args.mips <= 1000 or args.timeout <= 0:
            raise ValueError("mips must be within (0, 1000] and timeout must be positive")
        seconds = args.seconds if args.seconds is not None else (26 if args.continuous else 25)
        duration = probe.interval(seconds)
        renode = probe.executable(args.renode, "renode")
        models = ensure_models(args.cache.resolve(), args.models)
        toolchain = ensure_toolchain(args.cache.resolve(), args.toolchain)
        tools = {name: toolchain / "bin" / ("riscv32-esp-elf-" + name)
                 for name in ["gcc", "nm", "size", "objdump"]}
        if any(not path.is_file() for path in tools.values()):
            raise FileNotFoundError(f"incomplete toolchain: {toolchain}")
        self.report["renode"] = {"executable": str(renode),
                                 "version": self.run("renode-version", [renode, "--version"]).strip()}
        self.report["model"]["checkout"] = str(models)
        gpio_model = PROBE / "models/EmelaESP32C3_GPIO.cs"
        self.report["model"]["local_gpio"] = {"file": str(gpio_model), "sha256": sha256(gpio_model)}
        self.report["toolchain"] = {"directory": str(toolchain), "pinned_version": TOOLCHAIN_VERSION,
                                     "archive_sha256": TOOLCHAIN_SHA256,
                                     "source": "override" if args.toolchain else "verified archive cache",
                                     "compiler": self.run("compiler-version", [tools["gcc"], "--version"]).splitlines()[0]}
        self.generate()
        symbols, sections, sizes = self.build(tools)
        with tempfile.TemporaryDirectory(prefix="emela-temperature-esp32c3-monitor-") as temporary:
            monitor = Path(temporary)
            (monitor / "out").symlink_to(self.out, target_is_directory=True)
            (monitor / "models").symlink_to(models, target_is_directory=True)
            (monitor / "local-models").symlink_to(PROBE / "models", target_is_directory=True)
            self.monitor = monitor
            self.observe(symbols, duration, seconds)
        self.resources(symbols, sections, sizes)

    def generate(self):
        source = self.out / "source"
        source.mkdir(exist_ok=True)
        original = EXAMPLE / "controller.emel"
        shutil.copyfile(original, source / original.name)
        board = EXAMPLE / "boards/esp32c3/board.emel"
        shutil.copyfile(board, source / "board.emel")
        if (source / original.name).read_bytes() != original.read_bytes():
            raise ValueError("application source was changed while selecting the board")
        self.report["source"] = {"controller": str(original), "controller_sha256": sha256(original),
                                  "selected_board": str(board), "board_sha256": sha256(board),
                                  "application_copy_is_identical": True}
        config = EXAMPLE / "boards/esp32c3/embedded.toml"
        self.report["runtime_configuration"] = {
            "file": str(config), "sha256": sha256(config),
            "fixed_capacities": {name: int(value) for name, value in
                                 re.findall(r"(?m)^([a-z_]+)\s*=\s*(\d+)\s*$", config.read_text())}}
        if self.args.emela is not None:
            cli = probe.executable(self.args.emela.expanduser(), "emela")
            self.report["cli"] = {"source": "explicit executable", "executable": str(cli),
                                  "sha256": sha256(cli)}
            self.report["cli"]["version"] = self.run("emela-version", [cli, "--version"], timeout=60).strip()
            command = [cli]
        else:
            self.report["cli"] = {"source": "workspace cargo", "manifest": str(WORKSPACE / "Cargo.toml")}
            command = ["cargo", "run", "--manifest-path", WORKSPACE / "Cargo.toml", "--locked", "-q",
                       "-p", "emela-cli", "--"]
        self.run("generate", [*command, "build", source / original.name,
                               "--target", "c-embedded", "--embedded-config", config,
                               "--out-dir", self.out / "generated"], timeout=600,
                 env={"CARGO_BUILD_JOBS": os.environ.get("CARGO_BUILD_JOBS", "2")})

    def build(self, tools):
        objects = self.out / "objects"
        objects.mkdir(exist_ok=True)
        for stale in objects.iterdir():
            if stale.is_file():
                stale.unlink()
        flags = ["-march=rv32imc_zicsr_zifencei", "-mabi=ilp32", "-" + self.args.optimization,
                 "-g3", "-ffreestanding", "-fno-builtin", "-fno-pic", "-msmall-data-limit=0",
                 "-ffunction-sections", "-fdata-sections", "-std=c11", "-Wall", "-Wextra", "-Werror",
                 "-fstack-usage", "-I" + str(HERE), "-I" + str(self.out / "generated")]
        if self.args.continuous:
            flags.append("-DTEMPERATURE_DEMO_SAMPLES=0")
        sources = [HERE / "board.c", self.out / "generated/emela_embedded.c",
                   HERE / "validation_stack.c", HERE / "startup.S"]
        compiled = []
        for index, source in enumerate(sources):
            obj = objects / (source.name + ".o")
            self.run(f"compile-{index}", [tools["gcc"], *flags, "-c", source, "-o", obj])
            compiled.append(obj)
        elf = self.out / "firmware.elf"
        self.run("link", [tools["gcc"], *flags, "-nostartfiles", "--specs=nosys.specs", "-T", HERE / "firmware.ld",
                          "-Wl,--gc-sections", "-Wl,--no-relax", "-Wl,-Map," + str(self.out / "firmware.map"),
                          *compiled, "-lm", "-lgcc", "-o", elf])
        data = elf.read_bytes()
        if data[:7] != b"\x7fELF\x01\x01\x01" or struct.unpack_from("<H", data, 18)[0] != 243:
            raise ValueError("expected a little-endian RISC-V ELF32 firmware")
        self.report["firmware"] = {"elf": str(elf), "sha256": sha256(elf),
                                    "entry": struct.unpack_from("<I", data, 24)[0],
                                    "sources": {str(path): sha256(path) for path in [*sources, HERE / "firmware.ld"]}}
        size_log = self.run("size", [tools["size"], elf])
        sizes = {name: int(value) for name, value in zip(["text", "data", "bss"], size_log.splitlines()[1].split()[:3])}
        symbols = {}
        nm_log = self.run("symbols", [tools["nm"], "-S", "--defined-only", elf])
        for line in nm_log.splitlines():
            fields = line.split()
            if len(fields) == 4:
                symbols[fields[3]] = {"address": int(fields[0], 16), "size": int(fields[1], 16), "kind": fields[2]}
            elif len(fields) == 3:
                symbols[fields[2]] = {"address": int(fields[0], 16), "size": 0, "kind": fields[1]}
        forbidden = {"malloc", "_malloc_r", "calloc", "_calloc_r", "realloc", "_realloc_r", "free", "_free_r", "sbrk", "_sbrk", "_sbrk_r"}
        linked = sorted(forbidden & set(symbols))
        if linked:
            raise ValueError(f"C library dynamic allocators were linked: {linked}")
        sections = []
        section_log = self.run("sections", [tools["objdump"], "-h", elf])
        lines = section_log.splitlines()
        for index, line in enumerate(lines):
            match = re.match(r"\s*\d+\s+(\S+)\s+([0-9a-fA-F]+)\s+([0-9a-fA-F]+)\s+([0-9a-fA-F]+)\s+", line)
            if match and "ALLOC" in lines[index + 1]:
                sections.append({"name": match[1], "size": int(match[2], 16), "address": int(match[3], 16)})
        return symbols, sections, sizes

    def observe(self, symbols, duration, seconds):
        args = self.args
        names = ["temperature_" + part for part in scenario.BOARD_METRICS]
        names += ["emela_validation_stack_" + part for part in STACK_METRICS]
        if any(symbols.get(name, {}).get("size") != 4 for name in names):
            raise ValueError("missing required 32-bit controller/stack metric")
        histories = {"gpio_history": "temperature_output_masks", "input_history": "temperature_input_centicelsius",
                     "switch_off_history": "temperature_switch_off_masks"}
        history_bytes = symbols.get("temperature_output_masks", {}).get("size", 0)
        if not 0 < history_bytes <= 4096 or history_bytes % 4 or any(symbols.get(name, {}).get("size") != history_bytes for name in histories.values()):
            raise ValueError("missing equal-sized input/GPIO history arrays")
        history_count = history_bytes // 4
        platform = probe.PROBE_PLATFORM.replace("GPIOPort.ESP32C3_GPIO", "GPIOPort.EmelaESP32C3_GPIO")
        mapped_sram = symbols["_stack_top"]["address"] - 0x3FC80000
        if mapped_sram not in [65536, 131072]:
            raise ValueError(f"unexpected ESP32-C3 mapped SRAM slice: {mapped_sram}")
        platform = platform.replace("size: 0x10000", f"size: 0x{mapped_sram:x}")
        platform = platform.replace("gpio: GPIOPort.EmelaESP32C3_GPIO @ sysbus 0x60004000",
                                    "gpio: GPIOPort.EmelaESP32C3_GPIO @ sysbus 0x60004000\n"
                                    "    3 -> heater@0 | observer@0\n    4 -> cooler@0 | observer@1")
        platform += ('\nheater: Miscellaneous.LED @ sysbus\ncooler: Miscellaneous.LED @ sysbus\n'
                     'observer: Miscellaneous.EmelaGPIOObserver @ sysbus\n')
        (self.out / "machine.repl").write_text(platform)
        includes = "\n".join("include @" + str(self.monitor / "models/peripherals" / name)
                             for name in probe.MODEL_FILES if not name.startswith("gpio/"))
        includes += "\ninclude @" + str(self.monitor / "local-models/EmelaESP32C3_GPIO.cs")
        # Separate asynchronous model logging from synchronous observation markers.
        script = f'''logFile @{self.monitor / "out/model.log"}
logLevel 1 file
logLevel 3 console
using sysbus
{includes}
mach create "emela-temperature-esp32c3"
machine LoadPlatformDescription @{self.monitor / "out/machine.repl"}
sysbus LoadELF @{self.monitor / "out/firmware.elf"}
cpu PC 0x{symbols['_start']['address']:x}
cpu MTVEC 0x{symbols['trap_entry']['address'] | 3:x}
sysbus WriteByte 0x20001005 1
sysbus WriteByte 0x20001006 0
sysbus WriteByte 0x20001007 0xff
cpu PerformanceInMips {args.mips}
sysbus.uart0 CreateFileBackend @{self.monitor / "out/uart.log"} true
emulation RunFor "{duration}"
echo "EMELA_GPIO_OUT"
sysbus ReadDoubleWord 0x60004004
echo "EMELA_GPIO_OBSERVER_STATE"
observer State
echo "EMELA_GPIO_OBSERVER_TRANSITIONS"
observer TransitionCount
echo "EMELA_HEATER_LED"
heater State
echo "EMELA_COOLER_LED"
cooler State
'''
        for name in names:
            script += f'echo "EMELA_METRIC_{name}"\nsysbus ReadDoubleWord 0x{symbols[name]["address"]:x}\n'
        for label, symbol in histories.items():
            for index in range(history_count):
                script += f'echo "EMELA_HISTORY_{label}_{index}"\nsysbus ReadDoubleWord 0x{symbols[symbol]["address"] + index * 4:x}\n'
        log, uart = self.emulate(script, seconds)
        model_path = self.out / "model.log"
        if not model_path.is_file():
            raise ValueError("missing separate Renode model log")
        model_log = model_path.read_text(encoding="utf-8-sig")
        self.report["model_log_sha256"] = sha256(model_path)
        warnings = [line for line in model_log.splitlines() if "[WARNING]" in line]
        self.report["renode"]["warnings"] += warnings
        if warnings or re.search(r"\[ERROR\]|Unhandled Exception", model_log):
            raise ValueError("Renode model log reported a warning or error; see model.log")
        raw = {name: probe.metric(log, "EMELA_METRIC_" + name) for name in names}
        values = {"gpio_out": probe.metric(log, "EMELA_GPIO_OUT"),
                  "gpio_observer_state": probe.metric(log, "EMELA_GPIO_OBSERVER_STATE"),
                  "gpio_observer_transitions": probe.metric(log, "EMELA_GPIO_OBSERVER_TRANSITIONS"),
                  "stack": {part: raw["emela_validation_stack_" + part] for part in STACK_METRICS},
                  "controller": {part: raw["temperature_" + part] for part in scenario.BOARD_METRICS}}
        for name, label in [("heater_led", "EMELA_HEATER_LED"), ("cooler_led", "EMELA_COOLER_LED")]:
            match = re.search(re.escape(label) + r"[^\n]*\n\s*(True|False)", log)
            if not match:
                raise ValueError(f"missing external LED observation: {label}")
            values[name] = match[1] == "True"
        for label in histories:
            values[label] = [probe.metric(log, f"EMELA_HISTORY_{label}_{index}") for index in range(history_count)]
        values["input_history"] = [scenario.signed_int(value) for value in values["input_history"]]
        values["controller"]["result"] = scenario.signed_int(values["controller"]["result"])
        samples = scenario.check_observations(values, uart, args.continuous, args.verify_scenario,
                                              heater_mask=0x08, cooler_mask=0x10,
                                              gpio_output_key="gpio_out", sample_gpio_key="gpio_actuators")
        expected_trace = []
        state = 0
        for sample in samples:
            selected = {"heating": 1, "cooling": 2}.get(sample["mode"], 0)
            if state:
                expected_trace.append((0 if state == 1 else 1, 0))
            if selected:
                expected_trace.append((0 if selected == 1 else 1, 1))
            state = selected
        if not args.continuous and state:
            expected_trace.append((0 if state == 1 else 1, 0))
            state = 0
        actual_trace = [(int(pin), int(level)) for pin, level in
                        re.findall(r"EMELA_GPIO_PIN pin=([01]) level=([01])", model_log)]
        if actual_trace != expected_trace:
            raise ValueError(f"external GPIO transition trace disagrees with sample modes: {actual_trace} != {expected_trace}")
        observed_state = 0
        for pin, level in actual_trace:
            observed_state = (observed_state | (1 << pin)) if level else (observed_state & ~(1 << pin))
            if observed_state == 3:
                raise ValueError("external GPIO endpoints were simultaneously active")
        if values["gpio_observer_transitions"] != len(actual_trace) or values["gpio_observer_state"] != state:
            raise ValueError("external GPIO observer counters or final state disagree with transitions")
        if values["gpio_out"] != (((state & 1) << 3) | ((state & 2) << 3)):
            raise ValueError("GPIO output register disagrees with external GPIO endpoints")
        if values["heater_led"] != bool(state & 1) or values["cooler_led"] != bool(state & 2):
            raise ValueError("external LED states disagree with the GPIO observer")
        values["external_gpio_trace"] = [{"pin": pin, "level": level} for pin, level in actual_trace]
        self.report["samples"] = samples
        self.report["metrics"] = values
        probe.atomic_json(self.out / "metrics.json", values)
        self.report["scope"] = {"application_source_unchanged": True,
                                 "interrupt_path": "SYSTIMER target0 -> interrupt matrix source37 -> CLIC line1 -> CPU trap -> ISR -> task notification -> main scheduler",
                                 "gpio": "GPIO3 heater / GPIO4 cooler connected to external Renode LED endpoints",
                                 "mapped_physical_sram_bytes": mapped_sram,
                                 "rom_required": False, "cpu_dispatch_hook": False,
                                 "main_wait": "WFI with MIE masked while checking for pending events; no WfiAsNop override",
                                 "monitor_initialization": "ELF entry PC, trap_entry|3 MTVEC and CLIC line1 attributes before execution"}
        print(uart, end="")

    def resources(self, symbols, sections, sizes):
        address = lambda name: symbols[name]["address"]
        stack = self.report["metrics"]["stack"]
        guard = address("_stack_bottom") - address("_stack_guard_start")
        reserved = address("_stack_top") - address("_stack_guard_start")
        static_ram = sum(section["size"] for section in sections
                         if 0x3FC80000 <= section["address"] < 0x3FD00000 and section["name"] != ".stack")
        code = sum(section["size"] for section in sections if 0x40380000 <= section["address"] < 0x40400000)
        if guard != 256 or reserved != 16384 or stack["capacity"] != reserved - guard:
            raise ValueError("unexpected stack reservation or guard size")
        pools = {name: item["size"] for name, item in symbols.items()
                 if item["kind"] in "bBdD" and (name.startswith("emela_heap") or name.startswith("emela_suspend")
                                                  or name.startswith("emela_runtime") or name.startswith("emela_task")
                                                  or name.startswith("emela_event") or name.startswith("emela_fiber")
                                                  or name.startswith("emela_frame") or name.startswith("emela_deferred")
                                                  or name.startswith("emela_resume"))}
        physical_regions = []
        for section in sections:
            base = 0x40380000 if section["address"] >= 0x40380000 else 0x3FC80000
            start = section["address"] - base
            end = start + section["size"]
            if not 0 <= start <= end <= self.report["scope"]["mapped_physical_sram_bytes"]:
                raise ValueError(f"allocated ELF section is outside the SRAM aliases: {section}")
            if section["size"]:
                physical_regions.append((start, end, section["name"]))
        physical_regions.sort()
        if any(left[1] > right[0] for left, right in zip(physical_regions, physical_regions[1:])):
            raise ValueError("allocated ELF sections overlap in physical SRAM aliases")
        result = {"elf_sections": sizes, "allocated_sections": sections,
                  "mapped_physical_sram_bytes": self.report["scope"]["mapped_physical_sram_bytes"],
                  "code_region_reserved_bytes": address("_iram_end") - address("_iram_start"),
                  "data_region_reserved_bytes": address("_dram_end") - address("_dram_start"),
                  "code_and_rodata_sram_bytes": code, "static_data_sram_bytes": static_ram,
                  "stack_reserved_bytes_including_guard": reserved, "stack_guard_bytes": guard,
                  "stack_usable_bytes": stack["capacity"], "stack_observed_bytes": stack["used"],
                  "stack_guard_ok": stack["guard_ok"] == 1,
                  "physical_sram_allocated_bytes": code + static_ram + reserved,
                  "flash_used_bytes": 0, "flash_boot_supported_by_this_example": False,
                  "c_library_dynamic_allocator_linked": False, "runtime_static_symbols": pools,
                  "limits": "Observed stack applies to this scenario; fixed heap reservation is included in static data, not a measured heap peak."}
        probe.atomic_json(self.out / "resources.json", result)
        self.report["resources"] = result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--cache", type=Path, default=default_cache())
    parser.add_argument("--models", type=Path)
    parser.add_argument("--toolchain", type=Path, default=os.environ.get("RISCV32_ESP_TOOLCHAIN"))
    parser.add_argument("--renode", default=os.environ.get("RENODE"))
    parser.add_argument("--emela", type=Path, help="use an installed or copied CLI instead of cargo run")
    parser.add_argument("--optimization", choices=["O0", "O2"], default="O2")
    parser.add_argument("--continuous", action="store_true")
    parser.add_argument("--verify-scenario", action="store_true")
    parser.add_argument("--seconds", type=float)
    parser.add_argument("--mips", type=int, default=10)
    parser.add_argument("--timeout", type=int, default=600)
    args = parser.parse_args()
    args.case = "temperature-controller"
    args.output = args.output or Path("/tmp/emela-temperature-esp32c3-" + args.optimization + ("-continuous" if args.continuous else ""))
    try:
        case = TemperatureCase(args)
    except (OSError, ValueError) as error:
        print(str(error), file=sys.stderr)
        return 1
    try:
        case.execute()
        case.report["status"] = "passed"
    except Exception as error:
        case.report.update({"status": "failed", "error": str(error)})
        print(str(error), file=sys.stderr)
    finally:
        case.report["finished_at"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
        case.save()
        case.lock.close()
    print(f"{case.report['status'].upper()}: temperature-controller/esp32c3/{args.optimization}: {case.out / 'report.json'}")
    return 0 if case.report["status"] == "passed" else 1


if __name__ == "__main__":
    sys.exit(main())
