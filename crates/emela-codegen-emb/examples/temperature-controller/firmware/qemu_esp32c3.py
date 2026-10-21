#!/usr/bin/env python3
"""Verify an ESP32-C3 flash image from a real ROM reset in Espressif QEMU."""
import argparse
import fcntl
import hashlib
import importlib.util
import json
import math
import os
from pathlib import Path
import re
import selectors
import struct
import subprocess
import sys
import tempfile
import time

sys.dont_write_bytecode = True
QEMU_VERSION = "esp_develop_9.2.2_20260417"
METRICS = ["board_app_completed", "board_app_result", "board_failures", "board_ticks",
           "board_notifications", "board_rejected_notifications", "board_uart_bytes", "board_stack_used", "board_stack_guard_ok",
           "board_trap_cause", "board_trap_pc"]


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def atomic_json(path, value):
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n")
    temporary.replace(path)


def elf_symbols(path):
    data = path.read_bytes()
    if data[:7] != b"\x7fELF\x01\x01\x01" or struct.unpack_from("<H", data, 18)[0] != 243:
        raise ValueError("expected a little-endian RISC-V ELF32")
    offset = struct.unpack_from("<I", data, 32)[0]
    entry_size, count = struct.unpack_from("<HH", data, 46)
    if entry_size != 40 or offset + count * entry_size > len(data):
        raise ValueError("invalid ELF section table")
    sections = [struct.unpack_from("<10I", data, offset + index * entry_size) for index in range(count)]
    result = {}
    for section in sections:
        if section[1] != 2:
            continue
        strings = sections[section[6]]
        names = data[strings[4]:strings[4] + strings[5]]
        for at in range(section[4], section[4] + section[5], section[9]):
            name, value, size, info, _, _ = struct.unpack_from("<IIIBBH", data, at)
            end = names.find(b"\0", name)
            if end >= 0:
                result[names[name:end].decode()] = (value, size, info & 15)
    return result


def temperature_expectations():
    path = Path(__file__).resolve().parent.parent / "renode/run.py"
    spec = importlib.util.spec_from_file_location("emela_temperature_expectations", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.TEMPERATURES, module.next_mode


def capture_gpio(monitor, uart, observations):
    before = uart.read_bytes() if uart.exists() else b""
    mask = monitor.word(0x60004004) & 0x18
    ticks = monitor.word(observations["ticks_address"])
    after = uart.read_bytes() if uart.exists() else b""
    if before != after or not before.endswith(b"\n"):
        return
    samples = re.findall(rb"(?m)^sample=(\d+) temperature=\S+ mode=(\w+)\r?$", before)
    if samples:
        observations["samples"].append({"sample": int(samples[-1][0]), "mode": samples[-1][1].decode(),
                                         "gpio_mask": mask, "ticks": ticks})


def verify_temperature(uart, report):
    temperatures, next_mode = temperature_expectations()
    lines = [line for line in uart.splitlines() if line.startswith("sample=")]
    if len(lines) != len(temperatures):
        raise ValueError(f"expected 24 temperature sample lines, observed {len(lines)}")
    mode = "idle"
    expected_masks = {}
    for index, (line, raw) in enumerate(zip(lines, temperatures), 1):
        match = re.fullmatch(r"sample=(\d+) temperature=(\S+) mode=(\w+)", line)
        mode = next_mode(mode, raw)
        if not match or int(match[1]) != index or match[3] != mode:
            raise ValueError(f"unexpected temperature sample {index}: {line!r}")
        if -4000 <= raw <= 12500:
            actual = float(match[2])
            if not math.isfinite(actual) or not math.isclose(actual, raw / 100, rel_tol=0, abs_tol=1e-12):
                raise ValueError(f"unexpected temperature at sample {index}")
        elif match[2] != "unavailable":
            raise ValueError(f"sample {index} did not report sensor failure")
        expected_masks[index] = {"heating": 8, "cooling": 16}.get(mode, 0)
    captured = ("\n".join(lines) + "\n").encode()
    if report["metrics"]["board_uart_bytes"] != len(captured):
        raise ValueError("board UART byte count disagrees with the temperature output")
    report["temperature_samples"] = len(lines)
    # This pinned QEMU GPIO model implements strap reads only and ignores all writes.
    # Retain raw observations without presenting zero-valued reads as output verification.
    report["gpio_verification"] = {
        "status": "unavailable",
        "reason": "The fixed QEMU GPIO model ignores writes and returns zero except for the strap register",
        "source": "https://github.com/espressif/qemu/blob/esp-develop-9.2.2-20260417/hw/gpio/esp32_gpio.c",
        "expected_masks_from_uart": expected_masks,
        "verified_samples": [],
    }


class Monitor:
    def __init__(self, process, trace, timeout):
        self.process = process
        self.trace = trace
        self.timeout = timeout
        self.selector = selectors.DefaultSelector()
        self.selector.register(process.stdout, selectors.EVENT_READ)
        self.buffer = b""
        self.read()
        self.command("qmp_capabilities")

    def read(self):
        deadline = time.monotonic() + self.timeout
        while b"\n" not in self.buffer:
            if not self.selector.select(max(0, deadline - time.monotonic())):
                raise TimeoutError("QEMU monitor did not respond")
            chunk = os.read(self.process.stdout.fileno(), 65536)
            if not chunk:
                raise ValueError("QEMU exited before completing the flash scenario")
            self.buffer += chunk
        line, self.buffer = self.buffer.split(b"\n", 1)
        value = json.loads(line)
        self.trace.append({"response": value})
        return value

    def command(self, name, arguments=None):
        request = {"execute": name}
        if arguments is not None:
            request["arguments"] = arguments
        self.trace.append({"request": request})
        self.process.stdin.write(json.dumps(request).encode() + b"\n")
        self.process.stdin.flush()
        while True:
            response = self.read()
            if "error" in response:
                raise ValueError(f"QEMU monitor: {response['error']}")
            if "return" in response:
                return response["return"]

    def word(self, address):
        value = self.command("human-monitor-command", {"command-line": f"xp /1wx 0x{address:x}"})
        match = re.search(r":\s*(0x[0-9a-f]+)", value)
        if not match:
            raise ValueError(f"invalid QEMU memory observation: {value!r}")
        return int(match[1], 16)


def verify(args, report, output):
    firmware = args.firmware_dir.resolve()
    manifest = json.loads((firmware / "firmware.json").read_text())
    if manifest.get("board") != "esp32c3" or manifest.get("flash_address") != 0:
        raise ValueError("expected an ESP32-C3 image at flash offset 0x0")
    for name in ["firmware.elf", "firmware.bin", "firmware.map", "resources.json"]:
        if manifest.get("sha256", {}).get(name) != sha256(firmware / name):
            raise ValueError(f"firmware artifact hash mismatch: {name}")
    symbols = elf_symbols(firmware / "firmware.elf")
    for name in METRICS:
        if name not in symbols or symbols[name][1:] != (4, 1):
            raise ValueError(f"missing 32-bit board metric: {name}")
    version = subprocess.run([str(args.qemu), "--version"], text=True, capture_output=True, check=True).stdout.strip()
    if QEMU_VERSION not in version:
        raise ValueError(f"expected the fixed Espressif QEMU version {QEMU_VERSION}; found {version!r}")
    image = (firmware / "firmware.bin").read_bytes()
    if len(image) > 4 * 1024 * 1024:
        raise ValueError("firmware image exceeds 4 MB flash")
    (output / "flash.bin").write_bytes(image + b"\xff" * (4 * 1024 * 1024 - len(image)))
    uart = output / "uart.log"
    uart.unlink(missing_ok=True)
    command = [str(args.qemu.resolve()), "-machine", "esp32c3", "-display", "none", "-monitor", "none",
               "-serial", "file:" + str(uart), "-qmp", "stdio", "-drive",
               "file=" + str(output / "flash.bin").replace(",", ",,") + ",if=mtd,format=raw", "-no-reboot"]
    report.update({"qemu": version, "argv": command, "firmware": str(firmware),
                   "firmware_image_sha256": sha256(firmware / "firmware.bin"),
                   "boot": "built-in ROM reset -> flash offset 0x0 -> RAM segments -> firmware entry",
                   "resources": manifest["resources"], "monitor_trace": []})
    started = time.monotonic()
    with tempfile.TemporaryDirectory(prefix="emela-c3-qemu-", dir=output) as temporary, (output / "qemu.log").open("w") as log:
        process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log,
                                   env={**os.environ, "TMPDIR": temporary})
        try:
            monitor = Monitor(process, report["monitor_trace"], 10)
            gpio_observations = {"ticks_address": symbols["board_ticks"][0], "samples": []}
            while monitor.word(symbols["board_app_completed"][0]) == 0:
                if monitor.word(symbols["board_failures"][0]):
                    raise ValueError("the application reported a board defect")
                if time.monotonic() - started > args.timeout:
                    raise TimeoutError("the application did not complete before the timeout")
                if args.verify_temperature:
                    capture_gpio(monitor, uart, gpio_observations)
                time.sleep(0.1)
            monitor.command("stop")
            if args.verify_temperature:
                capture_gpio(monitor, uart, gpio_observations)
                report["gpio_observations"] = gpio_observations["samples"]
            metrics = {name: monitor.word(symbols[name][0]) for name in METRICS}
            metrics["board_app_result"] = struct.unpack("<i", struct.pack("<I", metrics["board_app_result"]))[0]
            metrics["gpio_out"] = monitor.word(0x60004004)
            registers = monitor.command("human-monitor-command", {"command-line": "info registers"})
            report["metrics"] = metrics
            report["registers"] = registers
            checks = {"board_app_completed": 1, "board_app_result": args.expect_result,
                      "board_failures": 0, "board_stack_guard_ok": 1, "board_rejected_notifications": 0}
            if args.expect_ticks is not None:
                checks["board_ticks"] = args.expect_ticks
            if args.expect_notifications is not None:
                checks["board_notifications"] = args.expect_notifications
            for name, expected in checks.items():
                if metrics[name] != expected:
                    raise ValueError(f"{name}: observed {metrics[name]}, expected {expected}")
            if not 0 < metrics["board_stack_used"] <= 16384 - 256:
                raise ValueError("observed stack use is outside its reservation")
            vector = re.search(r"\bmtvec\s+([0-9a-f]+)", registers)
            if not vector or int(vector[1], 16) != (symbols["_vectors"][0] | 1):
                raise ValueError("the CPU did not install the hardware-mode-1 vector table")
            if metrics["board_notifications"] and metrics["board_trap_cause"] != 0x80000001:
                raise ValueError("clock completion did not pass through the real ESP32-C3 interrupt")
            monitor.command("quit")
            process.wait(timeout=5)
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
    text = uart.read_text(errors="replace")
    if "SPI_FAST_FLASH_BOOT" not in text or "load:0x40380000" not in text or "entry 0x40380000" not in text:
        raise ValueError("the UART log does not contain the ROM flash load sequence")
    if text.count("POWERON") != 1 or text.count("entry 0x40380000") != 1:
        raise ValueError("the application reset unexpectedly")
    if args.verify_temperature:
        verify_temperature(text, report)
    report["resources"]["observed_stack_bytes"] = report["metrics"]["board_stack_used"]
    report["wall_seconds"] = round(time.monotonic() - started, 3)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--firmware-dir", type=Path, required=True)
    parser.add_argument("--qemu", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--expect-result", type=int, default=24)
    parser.add_argument("--expect-ticks", type=int)
    parser.add_argument("--expect-notifications", type=int)
    parser.add_argument("--verify-temperature", action="store_true")
    parser.add_argument("--timeout", type=float, default=60)
    args = parser.parse_args()
    if not 0 < args.timeout <= 3600:
        parser.error("timeout must be within (0, 3600]")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    lock = (output / ".run.lock").open("a")
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        parser.error("another QEMU verification owns the output directory")
    marker = output / ".emela-qemu-esp32c3"
    ownership = "emela-qemu-esp32c3/v1\n"
    if marker.exists() and marker.read_text() != ownership:
        parser.error("unrecognized output ownership marker")
    if not marker.exists() and any(path.name != ".run.lock" for path in output.iterdir()):
        parser.error("output is not an empty or managed directory")
    marker.write_text(ownership)
    report = {"status": "running"}
    atomic_json(output / "report.json", report)
    try:
        verify(args, report, output)
    except (OSError, ValueError, KeyError, struct.error, subprocess.SubprocessError, TimeoutError) as error:
        report.update({"status": "failed", "error": str(error)})
        atomic_json(output / "report.json", report)
        print(str(error), file=sys.stderr)
        return 1
    report["status"] = "passed"
    atomic_json(output / "report.json", report)
    print(f"ESP32-C3 ROM/Flash boot verified; report: {output / 'report.json'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
