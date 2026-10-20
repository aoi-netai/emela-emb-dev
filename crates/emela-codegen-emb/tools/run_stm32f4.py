#!/usr/bin/env python3
"""Build one isolated STM32F446 fixture and validate its Renode observations."""
import argparse
import datetime
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import time

CRATE = Path(__file__).resolve().parents[1]
ROOT = CRATE.parents[1]
BOARD = CRATE / "examples/renode-f446"
STACK_METRICS = ["used", "capacity", "painted", "guard_ok", "samples", "initial_sp", "min_sp"]
RUNTIME_MODES = ["scalar", "gc", "string", "gc-string"]
NEGATIVE_MODES = ["string", "heap", "fiber", "frame", "task", "event"]


def atomic_json(path, value):
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n")
    temporary.replace(path)


def metric(log, label):
    lines = log.splitlines()
    positions = [i for i, line in enumerate(lines) if re.search(re.escape(label) + r"(?![A-Za-z0-9_])", line)]
    if len(positions) != 1:
        raise ValueError(f"expected one {label} label, found {len(positions)}")
    for line in lines[positions[0] + 1:]:
        if "EMELA_" in line:
            break
        if re.fullmatch(r"\s*0x[0-9a-fA-F]+\s*", line):
            return int(line, 16)
    raise ValueError(f"missing value after {label}")


def check_metrics(fixture, values):
    if values["gpioa_odr"] != 0xe0:
        raise ValueError(f"unexpected GPIOA_ODR: {values['gpioa_odr']:#x}")
    stack = values["stack"]
    if stack["guard_ok"] != 1 or stack["samples"] < 2 or not 0 < stack["used"] <= stack["painted"] <= stack["capacity"]:
        raise ValueError(f"invalid stack observation: {stack}")
    if fixture == "completion":
        got = values["fixture"]
        if got["baseline"] == 0 or got["iterations"] != 1000 or got["producers"] != 1 or got["debugs"] != 1:
            raise ValueError(f"invalid completion metrics: {got}")
    elif fixture.startswith("negative-"):
        got = values["fixture"]
        if got["stops"] != 1 or got["observed"] != got["expected"]:
            raise ValueError(f"invalid expected-defect metrics: {got}")
    elif fixture.startswith("runtime-"):
        got = values["fixture"]
        for name, expected in [("iterations", 1000), ("failures", 0), ("heap_after", 0)]:
            if got.get(name) != expected:
                raise ValueError(f"invalid runtime {name}: {got.get(name)} (expected {expected})")
        expected = {"cleanup_iterations":1000,"cancelled_cleanups":1000,"stopped_pending":1000,"pools_clean":1,"sanity_checks":5,"notifications":4998,"isr_calls":6997,"rejected_notifications":1999,"board_rejected_leases":999,"error_produced":500,"fp_context_checks":6997,"irq_during_scheduler":1}
        managed = fixture in ["runtime-string","runtime-gc-string"]
        expected.update({"error_released":1499 if managed else 1749,"success_produced":2248 if managed else 0,"success_released":2498 if managed else 0})
        for name, count in expected.items():
            if got.get(name) != count:
                raise ValueError(f"runtime {name}: {got.get(name)} != {count}")
        for name in ["main_callbacks","old_generation_checks"]:
            if got.get(name,0)==0: raise ValueError(f"runtime did not observe {name}")
        if got.get("children_peak", 0) > 3:
            raise ValueError("runtime child high-water mark exceeded 3")


class Case:
    def __init__(self, out, fixture, optimization):
        self.out, self.fixture, self.optimization = out, fixture, optimization
        self.monitor_directory = Path(tempfile.mkdtemp(prefix="emela-renode-path-", dir="/tmp"))
        self.monitor = self.monitor_directory / "artifacts"
        self.monitor.symlink_to(out)
        out.mkdir(parents=True, exist_ok=True)
        self.report = {"schema_version": 1, "target": "STM32F446", "fixture": fixture, "optimization": optimization,
                       "status": "running", "started_at": datetime.datetime.now(datetime.timezone.utc).isoformat(), "steps": []}
        self.save()
        for name in ["firmware.elf", "firmware.map", "renode.log", "resources.json", "resources-static.json", "metrics.json", "size.txt"]:
            (out / name).unlink(missing_ok=True)
        for name in ["objects", "generated", "tmp", "renode-config"]:
            path = out / name
            if path.exists():
                shutil.rmtree(path)
            path.mkdir()

    def save(self):
        atomic_json(self.out / "report.json", self.report)

    def run(self, name, command, timeout=600, env=None):
        step = {"name": name, "command": [str(x) for x in command], "status": "running"}
        self.report["steps"].append(step)
        self.save()
        start = time.monotonic()
        environment = os.environ.copy()
        environment["CARGO_BUILD_JOBS"] = environment.get("CARGO_BUILD_JOBS", "2")
        temporary = self.out / "tmp" / name
        temporary.mkdir()
        environment["TMPDIR"] = str(temporary)
        if env:
            environment.update(env)
        try:
            completed = subprocess.run(command, cwd=ROOT, env=environment, text=True, errors="replace", stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=timeout)
            output, code = completed.stdout, completed.returncode
        except OSError as error:
            output, code = str(error), None
            step["error"] = str(error)
        except subprocess.TimeoutExpired as error:
            output = error.stdout or ""
            if isinstance(output, bytes):
                output = output.decode(errors="replace")
            code = None
            step["timeout_seconds"] = timeout
        (self.out / (name + ".log")).write_text(output)
        step.update({"status": "passed" if code == 0 else "failed", "exit_code": code,
                     "elapsed_seconds": round(time.monotonic() - start, 3), "log": name + ".log"})
        self.save()
        if code != 0:
            raise RuntimeError(f"{name} failed (exit={code}):\n{output[-8000:]}")
        return output

    def execute(self):
        fixture = self.fixture
        mode = None
        if fixture == "scalar":
            example, board, duration, mips = "renode_smoke", "board.c", "0.500000", 1
        elif fixture == "completion":
            example, board, duration, mips = "renode_completion", "completion_board.c", "0.500000", 100
        elif fixture.startswith("runtime-") and fixture[8:] in RUNTIME_MODES:
            example, board, duration, mips, mode = "renode_runtime", "runtime_board.c", "10.000000", 100, fixture[8:]
        elif fixture.startswith("negative-") and fixture[9:] in NEGATIVE_MODES:
            example, board, duration, mips, mode = "renode_negative", "negative_board.c", "0.020000", 10, fixture[9:]
        else:
            raise ValueError(f"unknown STM32F446 fixture: {fixture}")
        if self.optimization not in ["O0", "O2"]:
            raise ValueError(f"unknown optimization: {self.optimization}")
        base = Path(os.environ.get("STM32_RENODE_ROOT", "/home/aoi_nemui/bareruby/stm32_renode"))
        toolchain = Path(os.environ.get("ARM_TOOLCHAIN_PATH", str(base / ".tools/common/arm/xpack-arm-none-eabi-gcc-13.2.1-1.1")))
        cube = Path(os.environ.get("STM32CUBE_F4", str(base / ".tools/stm32cube/STM32CubeF4-1.28.3")))
        renode = Path(os.environ.get("RENODE", str(base / ".tools/stm32cube/renode-1.16.1/renode")))
        cc, nm, size = [toolchain / "bin" / ("arm-none-eabi-" + name) for name in ["gcc", "nm", "size"]]
        device, cmsis = cube / "Drivers/CMSIS/Device/ST/STM32F4xx", cube / "Drivers/CMSIS/Include"
        startup = device / "Source/Templates/gcc/startup_stm32f446xx.s"
        system = device / "Source/Templates/system_stm32f4xx.c"
        platform = Path(os.environ.get("RENODE_PLATFORM", str(renode.parent / "platforms/cpus/stm32f4.repl")))
        for path in [cc, nm, size, renode, startup, system, platform, BOARD / board, BOARD / "validation_stack.c"]:
            if not path.exists():
                raise FileNotFoundError(f"missing required tool or file: {path}")
        generated = self.out / "generated"
        self.report["toolchain"] = self.run("compiler-version", [cc, "--version"]).splitlines()[0]
        self.run("generate", ["cargo", "run", "--locked", "-q", "-p", "emela-codegen-emb", "--example", example, "--", str(generated)] + ([mode] if mode else []))
        arch = ["-mcpu=cortex-m4", "-mthumb", "-mfpu=fpv4-sp-d16", "-mfloat-abi=hard"]
        flags = arch + ["-" + self.optimization, "-g3", "-ffunction-sections", "-fdata-sections", "-DSTM32F446xx", "-std=c11", "-Wall", "-Wextra", "-Werror", "-fstack-usage"]
        flags += ["-I" + str(p) for p in [BOARD, generated, device / "Include", cmsis]]
        objects = []
        for index, source in enumerate([BOARD / board, generated / "emela_embedded.c", system, BOARD / "validation_stack.c", startup]):
            obj = self.out / "objects" / (source.name + ".o")
            extra = ["-x", "assembler-with-cpp"] if source == startup else []
            self.run(f"compile-{index}", [cc] + flags + extra + ["-c", source, "-o", obj])
            objects.append(obj)
        elf, mapfile = self.out / "firmware.elf", self.out / "firmware.map"
        self.run("link", [cc] + arch + ["-nostartfiles", "--specs=nano.specs", "--specs=nosys.specs", "-T" + str(BOARD / "firmware.ld"), "-Wl,--gc-sections", "-Wl,--wrap=main", "-Wl,-Map," + str(mapfile)] + objects + ["-lm", "-o", elf])
        (self.out / "size.txt").write_text(self.run("size", [size, elf]))
        checker = [sys.executable, CRATE / "tools/check_stm32f4_resources.py", "--elf", elf, "--map", mapfile, "--objects", self.out / "objects", "--toolchain", toolchain]
        self.run("resources-static", checker + ["--output", self.out / "resources-static.json"])
        symbols = {}
        for line in self.run("symbols", [nm, "-S", "--defined-only", elf]).splitlines():
            fields = line.split()
            if len(fields) == 4 and fields[2] in "BbDd" and int(fields[1], 16) == 4:
                symbols[fields[3]] = int(fields[0], 16)
        prefix = "emela_completion_" if fixture == "completion" else "emela_runtime_" if fixture.startswith("runtime-") else "emela_negative_" if fixture.startswith("negative-") else None
        selected = {name: address for name, address in symbols.items() if (prefix and name.startswith(prefix)) or name in ["emela_validation_stack_" + part for part in STACK_METRICS]}
        for name in ["emela_validation_stack_" + part for part in STACK_METRICS]:
            if name not in selected:
                raise ValueError(f"missing required metric symbol: {name}")
        repl = "\n".join(line for line in platform.read_text().splitlines() if not re.match(r"\s*ApplySVD\s", line)) + "\n"
        (self.out / "stm32f4.repl").write_text(repl)
        (self.out / "machine.repl").write_text(f'using {json.dumps(str(self.monitor / "stm32f4.repl"))}\ndwt: Miscellaneous.DWT @ sysbus 0xE0001000\n    frequency: 84000000\nnvic:\n    systickFrequency: 84000000\nsyscfg: Miscellaneous.STM32_SYSCFG @ sysbus 0x40013800\nflash:\n    size: 524288\nsram:\n    size: 131072\nled: Miscellaneous.LED @ gpioPortA 5\ngpioPortA:\n    5 -> led@0\n')
        duration = os.environ.get("EMELA_EMB_RENODE_RUN_TIME", duration)
        if not re.fullmatch(r"\d+\.\d+", duration):
            raise ValueError("EMELA_EMB_RENODE_RUN_TIME must be decimal seconds")
        self.report["simulation"] = {"seconds":float(duration),"mips":mips,"wall_timeout_seconds":int(os.environ.get("EMELA_EMB_RENODE_TIMEOUT","180"))}
        script = f'mach create\nmachine LoadPlatformDescription @{self.monitor / "machine.repl"}\nsysbus LoadELF @{self.monitor / "firmware.elf"}\nsysbus.cpu VectorTableOffset 0x08000000\nsysbus.cpu PerformanceInMips {mips}\nemulation RunFor "0:00:{duration}"\nsysbus InfoLog "EMELA_GPIOA_ODR"\nsysbus ReadDoubleWord 0x40020014\n'
        for name, address in sorted(selected.items()):
            script += f'sysbus InfoLog "EMELA_METRIC_{name}"\nsysbus ReadDoubleWord 0x{address:08x}\n'
        (self.out / "run.resc").write_text(script + "quit\n")
        log = self.run("renode", [renode, "--disable-xwt", "--console", "--plain", "-e", "include @" + str(self.monitor / "run.resc")], timeout=int(os.environ.get("EMELA_EMB_RENODE_TIMEOUT", "180")), env={"XDG_CONFIG_HOME": str(self.out / "renode-config")})
        if re.search(r"There was an error executing command|Unhandled Exception|\[ERROR\]", log):
            raise ValueError("Renode reported a command or runtime error")
        raw = {name: metric(log, "EMELA_METRIC_" + name) for name in selected}
        values = {"gpioa_odr": metric(log, "EMELA_GPIOA_ODR"), "stack": {name: raw["emela_validation_stack_" + name] for name in STACK_METRICS}, "fixture": {name[len(prefix):]: value for name, value in raw.items() if prefix and name.startswith(prefix)}}
        atomic_json(self.out / "metrics.json", values)
        atomic_json(self.out / "stack-metrics.json", values["stack"])
        self.report["metrics"] = values
        check_metrics(fixture, values)
        self.run("resources-observed", checker + ["--stack-metrics", self.out / "stack-metrics.json", "--output", self.out / "resources.json"])
        self.report["resources"] = json.loads((self.out / "resources.json").read_text())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("fixture", nargs="?", default="scalar")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--optimization", default=os.environ.get("EMELA_EMB_OPT_LEVEL", "O2"))
    args = parser.parse_args()
    output = args.output or Path(os.environ.get("EMELA_EMB_RENODE_OUT", "/tmp/emela-codegen-emb-renode-" + args.fixture))
    case = Case(output.resolve(), args.fixture, args.optimization)
    try:
        case.execute()
        case.report["status"] = "passed"
    except Exception as error:
        case.report.update({"status": "failed", "error": str(error)})
        print(str(error), file=sys.stderr)
    finally:
        case.report["finished_at"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
        case.save()
        shutil.rmtree(case.monitor_directory)
    print(f"{case.report['status'].upper()}: {args.fixture}/{args.optimization}: {output / 'report.json'}")
    return 0 if case.report["status"] == "passed" else 1


if __name__ == "__main__":
    sys.exit(main())
