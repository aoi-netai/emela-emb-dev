#!/usr/bin/env python3
"""Run the temperature controller on STM32F446/Renode and retain its observations."""
import argparse
import datetime
import json
import math
import os
from pathlib import Path
import re
import shutil
import sys

HERE = Path(__file__).resolve().parent
CRATE = HERE.parents[2]
sys.path.insert(0, str(CRATE / "tools"))
from run_stm32f4 import Case, STACK_METRICS, atomic_json, metric, renode_interval

COMMON_BOARD = CRATE / "examples/renode-f446"
EXAMPLE = HERE.parent
TEMPERATURES = [2100, 1900, 1800, 1900, 1999, 2000, 2700, 2800, 2700, 2601,
                2600, 1800, 2900, 1700, -(2**31), 1900, 1800, 12501, 1700, 2000,
                2800, -4001, 2900, 2600]
BOARD_METRICS = ["completed", "samples", "processed", "failures", "failure_line",
                 "waits", "ticks", "notifications", "overlap", "uart_bytes", "result"]


def next_mode(mode, temperature):
    if not -4000 <= temperature <= 12500:
        return "fault"
    if temperature <= 1800:
        return "heating"
    if temperature >= 2800:
        return "cooling"
    if mode == "heating" and temperature < 2000:
        return "heating"
    if mode == "cooling" and temperature > 2600:
        return "cooling"
    return "idle"


def signed_int(value):
    return (value + 2**31) % 2**32 - 2**31


def check_observations(values, uart, continuous=False, verify_scenario=False):
    board = values["controller"]
    expected = {"failures": 0, "failure_line": 0, "overlap": 0}
    expected["completed"] = 0 if continuous else 1
    if continuous:
        if board["processed"] < 2 or board["notifications"] < 1 or board["ticks"] < 1:
            raise ValueError("continuous controller did not progress across multiple samples")
        if verify_scenario and (board["processed"] < 24 or board["notifications"] < 23 or board["ticks"] < 23000):
            raise ValueError("continuous controller did not complete the default input cycle")
    else:
        count = board["processed"]
        if count < 1:
            raise ValueError("the controller completed without processing a sample")
        expected.update({"samples": count, "waits": count - 1, "result": count,
                         "notifications": count - 1})
        if count > 1 and board["ticks"] == 0:
            raise ValueError("the controller progressed without any timer interrupt")
    if verify_scenario and not continuous:
        expected.update({"samples": 24, "processed": 24, "waits": 23,
                         "ticks": 23000, "notifications": 23, "result": 24})
    for name, value in expected.items():
        if board[name] != value:
            raise ValueError(f"temperature_{name}: {board[name]} != {value}")
    if not continuous and values["gpioa_odr"] & 0x60:
        raise ValueError("actuators were not both stopped after the finite demo")
    lines = uart.splitlines()
    if len(lines) != board["processed"]:
        raise ValueError(f"UART sample lines {len(lines)} disagree with processed count {board['processed']}")
    samples = []
    mode = "idle"
    for index, line in enumerate(lines, 1):
        match = re.fullmatch(r"sample=(\d+) temperature=(\S+) mode=(\w+)", line)
        if not match or int(match[1]) != index or match[3] not in {"idle", "heating", "cooling", "fault"}:
            raise ValueError(f"unexpected UART sample {index}: {line!r}")
        temperature_c = None if match[2] == "unavailable" else float(match[2])
        if temperature_c is not None and not math.isfinite(temperature_c):
            raise ValueError(f"non-finite temperature at sample {index}")
        if (temperature_c is None) != (match[3] == "fault"):
            raise ValueError(f"fault state disagrees with temperature at sample {index}")
        if verify_scenario:
            raw = TEMPERATURES[(index - 1) % 24]
            mode = next_mode(mode, raw)
            if match[3] != mode:
                raise ValueError(f"sample {index} mode {match[3]} != expected {mode}")
            if -4000 <= raw <= 12500:
                if temperature_c is None or not math.isclose(temperature_c, raw / 100, rel_tol=0, abs_tol=1e-12):
                    raise ValueError(f"unexpected temperature at sample {index}: {match[2]}")
            elif temperature_c is not None:
                raise ValueError(f"sample {index} did not report the sensor failure")
        expected_mask = {"heating": 0x20, "cooling": 0x40}.get(match[3], 0)
        samples.append({"sample": index, "temperature_c": temperature_c,
                        "mode": match[3], "gpioa_actuators": expected_mask})
    if verify_scenario and values["input_history"] != TEMPERATURES:
        raise ValueError("observed sensor input sequence differs from the demo")
    history_count = len(values["gpio_history"])
    for slot in range(min(history_count, len(samples))):
        last = slot + (len(samples) - 1 - slot) // history_count * history_count
        sample = samples[last]
        if values["gpio_history"][slot] != sample["gpioa_actuators"]:
            raise ValueError(f"sample {last + 1} GPIO disagrees with mode {sample['mode']}")
        raw = values["input_history"][slot]
        if sample["temperature_c"] is not None and not math.isclose(sample["temperature_c"], raw / 100, rel_tol=0, abs_tol=1e-12):
            raise ValueError(f"sample {last + 1} UART temperature disagrees with sensor input")
    if any(values["switch_off_history"]):
        raise ValueError("an actuator was still on between switch-off and switch-on")
    if board["uart_bytes"] != len(uart.encode("utf-8")):
        raise ValueError("UART byte count disagrees with captured output")
    stack = values["stack"]
    if stack["guard_ok"] != 1 or stack["samples"] < 2 or not 0 < stack["used"] <= stack["capacity"]:
        raise ValueError(f"invalid observed stack: {stack}")
    return samples


class TemperatureCase(Case):
    def execute(self):
        out = self.out
        uart_path = out / "uart.log"
        uart_path.unlink(missing_ok=True)
        base = Path(os.environ.get("STM32_RENODE_ROOT", "/home/aoi_nemui/bareruby/stm32_renode"))
        toolchain = Path(os.environ.get("ARM_TOOLCHAIN_PATH", str(base / ".tools/common/arm/xpack-arm-none-eabi-gcc-13.2.1-1.1")))
        cube = Path(os.environ.get("STM32CUBE_F4", str(base / ".tools/stm32cube/STM32CubeF4-1.28.3")))
        renode = Path(os.environ.get("RENODE", str(base / ".tools/stm32cube/renode-1.16.1/renode")))
        cc, nm, size = [toolchain / "bin" / ("arm-none-eabi-" + name) for name in ["gcc", "nm", "size"]]
        device, cmsis = cube / "Drivers/CMSIS/Device/ST/STM32F4xx", cube / "Drivers/CMSIS/Include"
        startup = device / "Source/Templates/gcc/startup_stm32f446xx.s"
        system = device / "Source/Templates/system_stm32f4xx.c"
        platform = Path(os.environ.get("RENODE_PLATFORM", str(renode.parent / "platforms/cpus/stm32f4.repl")))
        for path in [cc, nm, size, renode, startup, system, platform, HERE / "board.c"]:
            if not path.exists():
                raise FileNotFoundError(f"missing required tool or file: {path}")
        generated = out / "generated"
        self.report["toolchain"] = self.run("compiler-version", [cc, "--version"]).splitlines()[0]
        self.run("generate", ["cargo", "run", "--locked", "-q", "-p", "emela-cli", "--", "build",
                              EXAMPLE / "controller.emel", "--target", "c-embedded",
                              "--embedded-config", EXAMPLE / "embedded.toml", "--out-dir", generated])
        arch = ["-mcpu=cortex-m4", "-mthumb", "-mfpu=fpv4-sp-d16", "-mfloat-abi=hard"]
        flags = arch + ["-" + self.optimization, "-g3", "-ffunction-sections", "-fdata-sections", "-DSTM32F446xx", "-std=c11", "-Wall", "-Wextra", "-Werror", "-fstack-usage"]
        flags += ["-I" + str(path) for path in [HERE, COMMON_BOARD, generated, device / "Include", cmsis]]
        if self.continuous:
            flags.append("-DTEMPERATURE_DEMO_SAMPLES=0")
        objects = []
        for index, source in enumerate([HERE / "board.c", generated / "emela_embedded.c", system, COMMON_BOARD / "validation_stack.c", startup]):
            obj = out / "objects" / (source.name + ".o")
            extra = ["-x", "assembler-with-cpp"] if source == startup else []
            self.run(f"compile-{index}", [cc] + flags + extra + ["-c", source, "-o", obj])
            objects.append(obj)
        elf, mapfile = out / "firmware.elf", out / "firmware.map"
        self.run("link", [cc] + arch + ["-nostartfiles", "--specs=nano.specs", "--specs=nosys.specs", "-T" + str(COMMON_BOARD / "firmware.ld"), "-Wl,--gc-sections", "-Wl,--wrap=main", "-Wl,-Map," + str(mapfile)] + objects + ["-lm", "-o", elf])
        (out / "size.txt").write_text(self.run("size", [size, elf]))
        checker = [sys.executable, CRATE / "tools/check_stm32f4_resources.py", "--elf", elf, "--map", mapfile, "--objects", out / "objects", "--toolchain", toolchain]
        self.run("resources-static", checker + ["--output", out / "resources-static.json"])
        symbols = {}
        for line in self.run("symbols", [nm, "-S", "--defined-only", elf]).splitlines():
            fields = line.split()
            if len(fields) == 4 and fields[2] in "BbDd":
                symbols[fields[3]] = (int(fields[0], 16), int(fields[1], 16))
        names = ["temperature_" + part for part in BOARD_METRICS]
        names += ["emela_validation_stack_" + part for part in STACK_METRICS]
        if any(symbols.get(name, (0, 0))[1] != 4 for name in names):
            raise ValueError("missing required 32-bit controller/stack metric")
        histories = {"gpio_history": "temperature_output_masks", "input_history": "temperature_input_centicelsius",
                     "switch_off_history": "temperature_switch_off_masks"}
        history_bytes = symbols.get("temperature_output_masks", (0, 0))[1]
        if not 0 < history_bytes <= 4096 or history_bytes % 4 or any(symbols.get(name, (0, 0))[1] != history_bytes for name in histories.values()):
            raise ValueError("missing equal-sized input/GPIO history arrays")
        history_count = history_bytes // 4
        repl = "\n".join(line for line in platform.read_text().splitlines() if not re.match(r"\s*ApplySVD\s", line)) + "\n"
        (out / "stm32f4.repl").write_text(repl)
        (out / "machine.repl").write_text(f'using {json.dumps(str(self.monitor / "stm32f4.repl"))}\nnvic:\n    systickFrequency: 16000000\nflash:\n    size: 524288\nsram:\n    size: 131072\nheater: Miscellaneous.LED @ gpioPortA 5\ncooler: Miscellaneous.LED @ gpioPortA 6\ngpioPortA:\n    5 -> heater@0\n    6 -> cooler@0\n')
        duration = os.environ.get("EMELA_EMB_RENODE_RUN_TIME", "26.000000" if self.continuous else "25.000000")
        timeout = int(os.environ.get("EMELA_EMB_RENODE_TIMEOUT", "600"))
        self.report["simulation"] = {"seconds": float(duration), "mips": 100, "systick_hz": 16000000, "wall_timeout_seconds": timeout}
        script = f'mach create\nmachine LoadPlatformDescription @{self.monitor / "machine.repl"}\nsysbus LoadELF @{self.monitor / "firmware.elf"}\nsysbus.cpu VectorTableOffset 0x08000000\nsysbus.cpu PerformanceInMips 100\nsysbus.usart2 CreateFileBackend @{self.monitor / "uart.log"} true\nemulation RunFor "{renode_interval(duration)}"\nsysbus InfoLog "EMELA_GPIOA_ODR"\nsysbus ReadDoubleWord 0x40020014\n'
        for name in names:
            script += f'sysbus InfoLog "EMELA_METRIC_{name}"\nsysbus ReadDoubleWord 0x{symbols[name][0]:08x}\n'
        for label, symbol in histories.items():
            for index in range(history_count):
                script += f'sysbus InfoLog "EMELA_HISTORY_{label}_{index}"\nsysbus ReadDoubleWord 0x{symbols[symbol][0] + index * 4:08x}\n'
        (out / "run.resc").write_text(script + "quit\n")
        log = self.run("renode", [renode, "--disable-xwt", "--console", "--plain", "-e", "include @" + str(self.monitor / "run.resc")], timeout=timeout, env={"XDG_CONFIG_HOME": str(out / "renode-config")})
        if re.search(r"There was an error executing command|Unhandled Exception|\[ERROR\]", log):
            raise ValueError("Renode reported a command or runtime error")
        raw = {name: metric(log, "EMELA_METRIC_" + name) for name in names}
        values = {"gpioa_odr": metric(log, "EMELA_GPIOA_ODR"),
                  "stack": {part: raw["emela_validation_stack_" + part] for part in STACK_METRICS},
                  "controller": {part: raw["temperature_" + part] for part in BOARD_METRICS}}
        for label in histories:
            values[label] = [metric(log, f"EMELA_HISTORY_{label}_{index}") for index in range(history_count)]
        values["input_history"] = [signed_int(value) for value in values["input_history"]]
        values["controller"]["result"] = signed_int(values["controller"]["result"])
        uart = uart_path.read_text(encoding="utf-8-sig")
        atomic_json(out / "metrics.json", values)
        atomic_json(out / "stack-metrics.json", values["stack"])
        self.report["metrics"] = values
        self.report["samples"] = check_observations(values, uart, self.continuous, self.verify_scenario)
        self.run("resources-observed", checker + ["--stack-metrics", out / "stack-metrics.json", "--output", out / "resources.json"])
        self.report["resources"] = json.loads((out / "resources.json").read_text())
        print(uart, end="")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--optimization", choices=["O0", "O2"], default="O2")
    parser.add_argument("--continuous", action="store_true", help="run continuously, observing 26 simulated seconds")
    parser.add_argument("--verify-scenario", action="store_true", help="also require the unchanged default temperatures and hysteresis decisions")
    args = parser.parse_args()
    output = args.output or Path("/tmp/emela-temperature-controller-" + args.optimization + ("-continuous" if args.continuous else ""))
    try:
        case = TemperatureCase(output.resolve(), "temperature-controller", args.optimization)
        case.continuous = args.continuous
        case.verify_scenario = args.verify_scenario
        case.report["continuous"] = args.continuous
        case.report["verify_scenario"] = args.verify_scenario
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
        shutil.rmtree(case.monitor_directory)
    print(f"{case.report['status'].upper()}: temperature-controller/{args.optimization}: {output / 'report.json'}")
    return 0 if case.report["status"] == "passed" else 1


if __name__ == "__main__":
    sys.exit(main())
