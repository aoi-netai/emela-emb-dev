#!/usr/bin/env python3
"""Record and verify firmware inputs without executing a shell."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import sys


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def inside(path, parent):
    return path == parent or parent in path.parents


def snapshot(spec, artifacts=True):
    output = Path(spec["output"]).resolve()
    paths = set()
    root = Path(spec["source_root"]).resolve()
    # Include the module inventory so newly added conflicting modules invalidate a build.
    if spec["single_file"]:
        candidates = root.iterdir()
    else:
        candidates = walk_inputs(root, output)
    for path in candidates:
        if path.is_file() and path.suffix in (".emel", ".c", ".h", ".S", ".toml"):
            paths.add(path.resolve())
    for name in spec["files"]:
        path = Path(name).resolve()
        if not path.is_file():
            raise ValueError(f"build input is missing: {path}; run emela build")
        paths.add(path)
        if path.suffix == ".c":
            paths.update(header.resolve() for header in walk_inputs(path.parent, output) if header.suffix == ".h")
    if artifacts:
        paths.update((output / "bsp" / name).resolve() for name in spec["bsp_files"])
        paths.update(path.resolve() for path in (output / "generated").iterdir() if path.is_file() and path.suffix in (".c", ".h"))
        metadata = firmware(spec)
        paths.add((output / "firmware/firmware.json").resolve())
        paths.update((output / "firmware" / name).resolve() for name in metadata["artifacts"])
    return {str(path): digest(path) for path in sorted(paths)}


def walk_inputs(root, output):
    for directory, children, files in os.walk(root):
        children[:] = [name for name in children if name not in (".git", "target", "__pycache__") and not inside((Path(directory) / name).resolve(), output)]
        for name in files:
            yield Path(directory) / name


def firmware(spec):
    output = Path(spec["output"]) / "firmware"
    metadata = json.loads((output / "firmware.json").read_text())
    if metadata.get("board") != spec["board"]:
        raise ValueError("firmware board differs from the selected board; run emela build")
    if metadata.get("schema_version") != 1:
        raise ValueError("unsupported board firmware metadata version")
    names = metadata.get("artifacts")
    if not isinstance(names, list) or not {"firmware.elf", "firmware.bin", "firmware.map"}.issubset(names):
        raise ValueError("firmware metadata is missing required artifacts")
    for name in names:
        path = Path(name)
        if path.is_absolute() or ".." in path.parts or not (output / path).is_file():
            raise ValueError(f"invalid firmware artifact: {name}")
    return metadata


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("action", choices=("prepare", "seal", "verify"))
    parser.add_argument("--spec")
    parser.add_argument("--expected")
    parser.add_argument("--manifest", required=True)
    parser.add_argument("--prepared")
    args = parser.parse_args()
    manifest = Path(args.manifest)
    if args.action == "prepare":
        spec = json.loads(args.expected)
        manifest.write_text(json.dumps({"spec": spec, "sha256": snapshot(spec, artifacts=False)}, indent=2) + "\n")
    elif args.action == "seal":
        spec = json.loads(Path(args.spec).read_text())
        prepared = json.loads(Path(args.prepared).read_text())
        if prepared != {"spec": spec, "sha256": snapshot(spec, artifacts=False)}:
            raise ValueError("project inputs changed during the build; run emela build again")
        firmware(spec)
        state = {"schema_version": 1, "spec": spec, "sha256": snapshot(spec)}
        manifest.write_text(json.dumps(state, indent=2) + "\n")
    else:
        state = json.loads(manifest.read_text())
        expected = json.loads(args.expected)
        if state.get("schema_version") != 1 or state.get("spec") != expected:
            raise ValueError("selected project or board settings differ from the successful build; run emela build")
        firmware(expected)
        if state["sha256"] != snapshot(expected):
            raise ValueError("firmware, BSP, generated C, or project inputs changed; run emela build")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(f"error: {error}", file=sys.stderr)
        sys.exit(1)
