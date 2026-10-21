#!/usr/bin/env python3
"""Build and program an STM32F446 Flash image from generated Emela C."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import struct
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
BOARD = "stm32f446"
FLASH = 0x08000000
RAM = 0x20000000


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def atomic_json(path, value):
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")
    temporary.replace(path)


def resolve_tool(name, toolchain=None):
    if toolchain:
        path = Path(toolchain).expanduser().resolve() / "bin" / ("arm-none-eabi-" + name)
        if not path.is_file():
            raise ValueError(f"ARM toolchain executable not found: {path}")
        return str(path)
    path = shutil.which("arm-none-eabi-" + name)
    if not path:
        raise ValueError("arm-none-eabi toolchain not found; set ARM_TOOLCHAIN_PATH or PATH")
    return path


def run(command, cwd, environment, log):
    result = subprocess.run([str(item) for item in command], cwd=cwd, env=environment,
                            text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    log.write(json.dumps([str(item) for item in command]) + "\n" + result.stdout)
    log.flush()
    if result.returncode:
        raise ValueError(f"command failed ({result.returncode}): {command[0]}\n{result.stdout}")
    return result.stdout


def build(args):
    output = args.output_dir.resolve()
    output.mkdir(parents=True, exist_ok=True)
    manifest = output / "firmware.json"
    manifest.unlink(missing_ok=True)
    generated = args.generated_dir.resolve()
    project = args.project_dir.resolve()
    required = [generated / "emela_embedded.c", generated / "emela_embedded_exports.h"]
    sources = [HERE / "runtime.c", HERE / "validation_stack.c", HERE / "startup.S", required[0]]
    sources += [source.resolve() for source in args.source]
    for source in sources + required:
        if not source.is_file():
            raise ValueError(f"build input not found: {source}")
    toolchain = args.toolchain or os.environ.get("ARM_TOOLCHAIN_PATH")
    cc, nm, objcopy, size = [resolve_tool(name, toolchain) for name in ["gcc", "nm", "objcopy", "size"]]
    arch = ["-mcpu=cortex-m4", "-mthumb", "-mfpu=fpv4-sp-d16", "-mfloat-abi=hard"]
    flags = arch + ["-" + args.optimization, "-g3", "-std=c11", "-ffunction-sections",
                    "-fdata-sections", "-Wall", "-Wextra", "-Werror", "-fstack-usage"]
    includes = {HERE, generated, project, *[source.parent for source in sources]}
    flags += ["-I" + str(path) for path in sorted(includes)]
    object_dir = output / "objects"
    object_dir.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="compiler-tmp-", dir=output) as temporary:
        environment = dict(os.environ, TMPDIR=temporary)
        with (output / "build.log").open("w") as log:
            compiler = run([cc, "--version"], project, environment, log).splitlines()[0]
            objects = []
            for index, source in enumerate(sources):
                obj = object_dir / f"{index}-{source.name}.o"
                extra = ["-x", "assembler-with-cpp"] if source.suffix.lower() == ".s" else []
                run([cc, *flags, *extra, "-c", source, "-o", obj], project, environment, log)
                objects.append(obj)
            elf = output / "firmware.elf"
            run([cc, *arch, "-nostartfiles", "--specs=nano.specs", "--specs=nosys.specs",
                 "-T" + str(HERE / "firmware.ld"), "-Wl,--gc-sections", "-Wl,--wrap=main",
                 "-Wl,-Map," + str(output / "firmware.map"), *objects, "-lm", "-o", elf],
                project, environment, log)
            symbols_text = run([nm, "-S", "--defined-only", elf], project, environment, log)
            symbols = {}
            for line in symbols_text.splitlines():
                fields = line.split()
                if len(fields) in [3, 4]:
                    symbols[fields[-1]] = int(fields[0], 16)
            forbidden = re.compile(r"^(?:malloc|calloc|realloc|free|sbrk|_sbrk|_sbrk_r|_malloc_r|_calloc_r|_realloc_r|_free_r)$")
            if any(forbidden.fullmatch(name) for name in symbols):
                raise ValueError("firmware unexpectedly depends on a system allocator")
            raw = output / "firmware.bin"
            run([objcopy, "-O", "binary", elf, raw], project, environment, log)
            size_text = run([size, elf], project, environment, log)
            (output / "size.txt").write_text(size_text)
    image = raw.read_bytes()
    if len(image) < 8 or len(image) > 512 * 1024:
        raise ValueError("invalid STM32 Flash image size")
    initial_sp, reset = struct.unpack_from("<II", image)
    if initial_sp != RAM + 128 * 1024 or not (reset & 1) or not FLASH <= (reset & ~1) < FLASH + len(image):
        raise ValueError("Flash image has an invalid initial SP/reset vector")
    header = elf.read_bytes()[:52]
    if len(header) != 52 or header[:7] != b"\x7fELF\x01\x01\x01" or struct.unpack_from("<H", header, 18)[0] != 40:
        raise ValueError("firmware must be a little-endian 32-bit ARM ELF")
    static_ram = symbols["__emela_static_ram_end__"] - RAM
    stack = symbols["__emela_stack_top__"] - symbols["__emela_stack_bottom__"]
    guard = symbols["__emela_stack_guard_end__"] - symbols["__emela_stack_guard_start__"]
    resources = {"flash_bytes": len(image), "flash_capacity": 512 * 1024,
                 "static_ram_bytes": static_ram, "stack_reserved_bytes": stack,
                 "stack_guard_bytes": guard, "ram_capacity": 128 * 1024,
                 "ram_reserved_bytes": static_ram + stack + guard,
                 "system_allocator": False}
    atomic_json(output / "resources.json", resources)
    artifacts = ["firmware.elf", "firmware.bin", "firmware.map", "resources.json"]
    atomic_json(manifest, {"schema_version": 1, "board": BOARD,
                          "mcu": "STM32F446RE", "boot": "internal-flash-reset-vector",
                          "flash_address": FLASH, "entry_address": reset,
                          "initial_sp": initial_sp, "compiler": compiler,
                          "optimization": args.optimization, "artifacts": artifacts,
                          "sha256": {name: digest(output / name) for name in artifacts},
                          "resources": resources})
    print(json.dumps({"board": BOARD, "manifest": str(manifest), "resources": resources}))


def tcl_word(value):
    replacements = {"\\": "\\\\", '"': '\\"', "$": "\\$", "[": "\\[", "]": "\\]",
                    "\n": "\\n", "\r": "\\r", "\t": "\\t"}
    return '"' + "".join(replacements.get(char, char) for char in str(value)) + '"'


def flash(args):
    output = args.output_dir.resolve()
    manifest = json.loads((output / "firmware.json").read_text())
    if manifest.get("schema_version") != 1 or manifest.get("board") != BOARD:
        raise ValueError("firmware manifest does not match stm32f446")
    for name in ["firmware.elf", "firmware.bin", "firmware.map", "resources.json"]:
        if name not in manifest.get("artifacts", []) or digest(output / name) != manifest.get("sha256", {}).get(name):
            raise ValueError(f"firmware artifact is stale or modified: {name}")
    command = [os.environ.get("OPENOCD", "openocd"), "-f", "interface/stlink.cfg", "-f", "target/stm32f4x.cfg"]
    if args.port:
        command += ["-c", "adapter serial " + tcl_word(args.port)]
    command += ["-c", "program " + tcl_word(output / "firmware.elf") + " verify reset exit"]
    if args.dry_run:
        print(json.dumps({"board": BOARD, "dry_run": True, "argv": command}, ensure_ascii=False))
        return
    subprocess.run(command, check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    build_parser = commands.add_parser("build")
    build_parser.add_argument("--generated-dir", required=True, type=Path)
    build_parser.add_argument("--output-dir", required=True, type=Path)
    build_parser.add_argument("--project-dir", required=True, type=Path)
    build_parser.add_argument("--optimization", choices=["O0", "O2"], default="O2")
    build_parser.add_argument("--source", action="append", type=Path, default=[])
    build_parser.add_argument("--toolchain")
    flash_parser = commands.add_parser("flash")
    flash_parser.add_argument("--output-dir", required=True, type=Path)
    flash_parser.add_argument("--port", help="ST-Link adapter serial number")
    flash_parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()
    try:
        build(args) if args.command == "build" else flash(args)
        return 0
    except (OSError, ValueError, KeyError, struct.error, subprocess.SubprocessError) as error:
        print(str(error), file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
