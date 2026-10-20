#!/usr/bin/env python3
"""Check linked STM32F446 sections, stack layout and optional observed metrics."""
import argparse
from dataclasses import asdict, dataclass
import json
from pathlib import Path
import re
import subprocess
import sys

FLASH_START = 0x08000000
FLASH_SIZE = 512 * 1024
RAM_START = 0x20000000
RAM_SIZE = 128 * 1024


@dataclass
class Section:
    name: str
    size: int
    vma: int
    lma: int
    flags: list


def sections_from_objdump(text):
    result = []
    lines = text.splitlines()
    expression = re.compile(r"^\s*\d+\s+(\S+)\s+([0-9a-fA-F]+)\s+([0-9a-fA-F]+)\s+([0-9a-fA-F]+)\s+[0-9a-fA-F]+\s+2\*\*\d+\s*(.*)$")
    for index, line in enumerate(lines):
        match = expression.match(line)
        if not match:
            continue
        name, size, vma, lma, flags = match.groups()
        if not flags and index + 1 < len(lines):
            flags = lines[index + 1].strip()
        result.append(Section(name, int(size, 16), int(vma, 16), int(lma, 16), [part.strip() for part in flags.split(",")]))
    if not result:
        raise ValueError("objdump did not provide any sections")
    return result


def symbols_from_nm(text):
    result = {}
    for line in text.splitlines():
        fields = line.split()
        if len(fields) >= 3 and re.fullmatch(r"[0-9a-fA-F]+", fields[0]):
            result[fields[-1]] = int(fields[0], 16)
    return result


def union_size(ranges):
    end = None
    size = 0
    for start, stop in sorted(ranges):
        if end is None or start > end:
            size += stop - start
        elif stop > end:
            size += stop - end
        end = max(end or stop, stop)
    return size


def stack_usage(directory):
    frames = []
    for path in sorted(Path(directory).rglob("*.su")):
        for line in path.read_text(encoding="utf-8").splitlines():
            parts = line.split("\t")
            if len(parts) != 3 or not parts[1].isdigit():
                continue
            frames.append({"function": parts[0].rsplit(":", 1)[-1], "bytes": int(parts[1]), "kind": parts[2], "file": str(path)})
    return sorted(frames, key=lambda frame: (-frame["bytes"], frame["function"]))


def validate(sections, symbols, frames, *, flash_budget=FLASH_SIZE, ram_budget=RAM_SIZE,
             stack_budget=16384, max_frame_bytes=None, observed=None):
    errors = []
    ram = []
    static_ram = []
    flash = []
    loaded_flash = []
    flash_reservations = []
    allocated = []
    section_report = []
    for section in sections:
        if "ALLOC" not in section.flags or not section.size:
            continue
        start, end = section.vma, section.vma + section.size
        allocated.append((start, end, section.name))
        item = asdict(section)
        item["noload"] = "LOAD" not in section.flags
        section_report.append(item)
        if FLASH_START <= start < FLASH_START + FLASH_SIZE:
            flash.append((start, end))
            flash_reservations.append((start, end, section.name))
            if end > FLASH_START + FLASH_SIZE:
                errors.append(f"{section.name} exceeds physical Flash")
        elif RAM_START <= start < RAM_START + RAM_SIZE:
            ram.append((start, end))
            if section.name not in [".emela_stack", ".emela_stack_guard"]:
                static_ram.append((start, end))
            if end > RAM_START + RAM_SIZE:
                errors.append(f"{section.name} exceeds physical RAM")
        else:
            errors.append(f"{section.name} is outside STM32F446 Flash/RAM")
        if "LOAD" in section.flags and "CONTENTS" in section.flags:
            load_start, load_end = section.lma, section.lma + section.size
            if FLASH_START <= load_start < FLASH_START + FLASH_SIZE:
                loaded_flash.append((load_start, load_end))
                flash.append((load_start, load_end))
                if (load_start, load_end, section.name) not in flash_reservations:
                    flash_reservations.append((load_start, load_end, section.name))
                if load_end > FLASH_START + FLASH_SIZE:
                    errors.append(f"{section.name} load image exceeds physical Flash")
            else:
                errors.append(f"{section.name} load image is not in Flash")
    previous = None
    for start, end, name in sorted(allocated):
        if previous and start < previous[1]:
            errors.append(f"allocated sections overlap: {previous[2]} and {name}")
        if previous is None or end > previous[1]:
            previous = (start, end, name)
    previous = None
    for start, end, name in sorted(flash_reservations):
        if previous and start < previous[1]:
            errors.append(f"Flash load/reserved ranges overlap: {previous[2]} and {name}")
        if previous is None or end > previous[1]:
            previous = (start, end, name)

    required = ["__emela_stack_top__", "__emela_stack_bottom__", "__emela_stack_guard_start__", "__emela_stack_guard_end__", "__emela_static_ram_end__", "_estack"]
    for name in required:
        if name not in symbols:
            errors.append(f"missing linker symbol {name}")
    capacity = guard_bytes = gap = 0
    if all(name in symbols for name in required):
        top, bottom = symbols[required[0]], symbols[required[1]]
        guard_start, guard_end = symbols[required[2]], symbols[required[3]]
        capacity = top - bottom
        guard_bytes = guard_end - guard_start
        gap = guard_start - symbols["__emela_static_ram_end__"]
        if top != RAM_START + RAM_SIZE or symbols["_estack"] != top:
            errors.append("MSP initial value and stack top must equal physical SRAM end")
        if bottom != guard_end or capacity <= 0 or capacity % 8 or bottom % 8:
            errors.append("invalid descending MSP stack boundaries")
        if guard_bytes < 32 or guard_bytes % 8 or guard_start < RAM_START:
            errors.append("invalid MSP low guard boundaries")
        if gap < 0:
            errors.append("static RAM overlaps MSP guard/stack")
        if capacity > stack_budget:
            errors.append(f"MSP reservation {capacity} exceeds stack budget {stack_budget}")
        names = {section.name: section for section in sections}
        for name, start, size in [(".emela_stack", bottom, capacity), (".emela_stack_guard", guard_start, guard_bytes)]:
            section = names.get(name)
            if section is None or section.vma != start or section.size != size or "ALLOC" not in section.flags or "LOAD" in section.flags:
                errors.append(f"{name} does not match its NOLOAD linker reservation")

    flash_span = max((end - FLASH_START for _, end in flash), default=0)
    ram_bytes = union_size(ram)
    if flash_span > flash_budget:
        errors.append(f"Flash image span {flash_span} exceeds budget {flash_budget}")
    if ram_bytes > ram_budget:
        errors.append(f"RAM allocated bytes {ram_bytes} exceeds budget {ram_budget}")
    allocator_pattern = re.compile(r"^(?:_*|__wrap_|__real_)(malloc|calloc|realloc|free|memalign|aligned_alloc|sbrk|valloc|pvalloc)(?:_|$)")
    allocators = sorted(name for name in symbols if allocator_pattern.search(name) or name == "cfree")
    if allocators:
        errors.append("linked dynamic C allocator symbols: " + ", ".join(allocators))
    if not frames:
        errors.append("no GCC .su stack usage records found")
    dynamic_frames = [frame for frame in frames if "dynamic" in frame["kind"] and "bounded" not in [part.strip() for part in frame["kind"].split(",")]]
    if dynamic_frames:
        errors.append("unbounded dynamic GCC stack frames found")
    largest = max((frame["bytes"] for frame in frames), default=0)
    if capacity > 0 and largest > capacity:
        errors.append(f"individual GCC frame {largest} exceeds linked MSP capacity {capacity}")
    if max_frame_bytes is not None and largest > max_frame_bytes:
        errors.append(f"individual GCC frame {largest} exceeds budget {max_frame_bytes}")

    stack_observed = None
    if observed is not None and not isinstance(observed, dict):
        errors.append("stack metrics must be a JSON object")
        observed = None
    if observed is not None:
        values = {name.removeprefix("emela_validation_stack_"): value for name, value in observed.items()}
        keys = ["used", "capacity", "painted", "guard_ok", "samples", "initial_sp", "min_sp"]
        missing = [name for name in keys if name not in values or type(values[name]) is not int]
        if missing:
            errors.append("missing/integer stack metrics: " + ", ".join(missing))
        else:
            stack_observed = {name: values[name] for name in keys}
            if values["guard_ok"] != 1:
                errors.append("observed MSP low guard was modified")
            if values["samples"] < 2:
                errors.append("no final workload stack sample")
            if values["capacity"] != capacity or not 0 <= values["used"] <= capacity:
                errors.append("observed MSP use exceeds or disagrees with linker reservation")
            bottom, top = symbols.get("__emela_stack_bottom__", 0), symbols.get("__emela_stack_top__", 0)
            if not bottom <= values["min_sp"] <= values["initial_sp"] <= top:
                errors.append("observed MSP address is outside reserved stack")
            if values["painted"] != values["initial_sp"] - bottom:
                errors.append("painted stack extent disagrees with initial MSP")
    return {
        "target": "STM32F446", "ok": not errors, "errors": errors,
        "flash": {"physical_bytes": FLASH_SIZE, "budget_bytes": flash_budget, "allocated_union_bytes": union_size(flash), "loaded_union_bytes": union_size(loaded_flash), "image_span_bytes": flash_span},
        "ram": {"physical_bytes": RAM_SIZE, "budget_bytes": ram_budget, "allocated_union_bytes": ram_bytes, "stack_reserved_bytes": capacity, "guard_reserved_bytes": guard_bytes, "static_allocated_bytes": union_size(static_ram), "unallocated_gap_bytes": gap},
        "stack": {"budget_bytes": stack_budget, "largest_individual_frame_bytes": largest, "individual_frames": frames, "observed": stack_observed, "whole_call_stack_bound_proved": False},
        "linked_dynamic_allocators": allocators, "sections": section_report,
    }


def tool(directory, name):
    directory = Path(directory)
    for candidate in [directory / "bin" / f"arm-none-eabi-{name}", directory / f"arm-none-eabi-{name}"]:
        if candidate.is_file():
            return str(candidate)
    raise ValueError(f"missing tool arm-none-eabi-{name} below {directory}")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--elf", type=Path, required=True)
    parser.add_argument("--map", type=Path, required=True)
    parser.add_argument("--objects", type=Path, required=True)
    parser.add_argument("--toolchain", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--flash-budget", type=int, default=FLASH_SIZE)
    parser.add_argument("--ram-budget", type=int, default=RAM_SIZE)
    parser.add_argument("--stack-budget", type=int, default=16384)
    parser.add_argument("--max-frame-bytes", type=int)
    parser.add_argument("--stack-metrics", type=Path)
    args = parser.parse_args(argv)
    try:
        if not args.map.read_text(encoding="utf-8").strip():
            raise ValueError("linker map is empty")
        sections = sections_from_objdump(subprocess.check_output([tool(args.toolchain, "objdump"), "-h", str(args.elf)], text=True))
        symbols = symbols_from_nm(subprocess.check_output([tool(args.toolchain, "nm"), "--defined-only", str(args.elf)], text=True))
        observed = json.loads(args.stack_metrics.read_text(encoding="utf-8")) if args.stack_metrics else None
        report = validate(sections, symbols, stack_usage(args.objects), flash_budget=args.flash_budget, ram_budget=args.ram_budget, stack_budget=args.stack_budget, max_frame_bytes=args.max_frame_bytes, observed=observed)
        report["inputs"] = {"elf": str(args.elf), "map": str(args.map), "objects": str(args.objects)}
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        report = {"target": "STM32F446", "ok": False, "errors": [str(error)]}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    if not report["ok"]:
        for error in report["errors"]:
            print("FAIL: " + error, file=sys.stderr)
        return 1
    print(f"PASS: Flash span={report['flash']['image_span_bytes']} RAM allocated={report['ram']['allocated_union_bytes']} MSP reserved={report['ram']['stack_reserved_bytes']} guard={report['ram']['guard_reserved_bytes']}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
