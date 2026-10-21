#!/usr/bin/env python3
"""Build the shared temperature example and audit every Renode 1.16.1 F4 platform."""
import argparse
import concurrent.futures
import datetime
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

from run_stm32f4 import atomic_json
from stm32f4_flash import CRATE, PLATFORMS, inventory
from validation_process import run_captured

EXAMPLE = CRATE / "examples/temperature-controller"
ROOT = CRATE.parents[1]
OWNER = "emela-codegen-emb STM32F4 platform matrix v1\n"


def run(command, output, name, *, cwd, environment, timeout=600):
    with tempfile.TemporaryDirectory(prefix=name + "-", dir=output) as temporary:
        code, text, timed_out = run_captured([str(item) for item in command], cwd=cwd,
                                            env=dict(environment, TMPDIR=temporary), timeout=timeout)
    (output / (name + ".log")).write_text(text)
    if code != 0 or timed_out:
        raise ValueError(f"{name} failed (exit={code}, timeout={timed_out}): {text[-4000:]}")
    return text


def copy_project(destination, board, optimization):
    destination.mkdir(parents=True, exist_ok=True)
    (destination / "src").mkdir(exist_ok=True)
    inputs = {
        "src/main.emel": EXAMPLE / "firmware/src/main.emel",
        "controller.emel": EXAMPLE / "controller.emel",
        "board.emel": EXAMPLE / "boards" / board / "board.emel",
        "embedded.toml": EXAMPLE / "firmware/embedded.toml",
        "sensor.c": EXAMPLE / "firmware/sensor.c",
    }
    for name, source in inputs.items():
        shutil.copyfile(source, destination / name)
    (destination / "Pome.toml").write_text(
        '[build]\ntarget = "c-embedded"\n\n[embedded]\n'
        f'board = "{board}"\nconfig = "embedded.toml"\nentry = "main"\n'
        f'sources = ["sensor.c"]\noptimization = "{optimization}"\n\n'
        '[embedded.modules]\nController = "controller.emel"\nBoard = "board.emel"\n')
    return {name: hashlib.sha256((destination / name).read_bytes()).hexdigest() for name in inputs}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--renode", type=Path, default=os.environ.get("RENODE"))
    parser.add_argument("--toolchain", type=Path, default=os.environ.get("ARM_TOOLCHAIN_PATH"))
    parser.add_argument("--emela", type=Path, help="use an installed or copied CLI instead of building the workspace CLI")
    parser.add_argument("--platform", choices=PLATFORMS, action="append", help="restrict the run; the report lists the actual selected scope")
    parser.add_argument("--optimization", choices=["O0", "O2"], action="append", help="default: both O0 and O2")
    parser.add_argument("--jobs", type=int, choices=[1, 2], default=2)
    parser.add_argument("--timeout", type=int, default=600)
    args = parser.parse_args()
    if not args.renode:
        parser.error("set RENODE or pass --renode")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    marker = output / ".emela-stm32f4-matrix-owner"
    if any(output.iterdir()) and not (marker.is_file() and marker.read_text() == OWNER):
        parser.error("refusing nonempty output directory without the matrix owner marker")
    lease = (output / ".emela-stm32f4-matrix-lock").open("a")
    try:
        fcntl.flock(lease, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        lease.close()
        parser.error("output directory is already in use")
    marker.write_text(OWNER)
    selected = list(dict.fromkeys(args.platform or PLATFORMS))
    optimizations = list(dict.fromkeys(args.optimization or ["O0", "O2"]))
    report = {"schema_version": 1, "status": "running", "renode_version": "1.16.1",
              "started_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
              "platforms": selected, "optimizations": optimizations, "builds": [], "cases": [],
              "source_controller_sha256": hashlib.sha256((EXAMPLE / "controller.emel").read_bytes()).hexdigest()}
    atomic_json(output / "report.json", report)
    try:
        renode = args.renode.expanduser().resolve()
        report["platform_source_sha256"] = inventory(renode)
        environment = dict(os.environ, CARGO_BUILD_JOBS=os.environ.get("CARGO_BUILD_JOBS", "2"))
        if args.toolchain:
            environment["ARM_TOOLCHAIN_PATH"] = str(args.toolchain.expanduser().resolve())
        emela = args.emela.expanduser().resolve() if args.emela else ROOT / "target/debug/emela"
        if not args.emela:
            run(["cargo", "build", "--locked", "-q", "-p", "emela-cli"], output, "build-cli", cwd=ROOT, environment=environment)
        report["cli"] = {"path": str(emela), "sha256": hashlib.sha256(emela.read_bytes()).hexdigest()}
        for optimization in optimizations:
            for board in dict.fromkeys(PLATFORMS[platform][0] for platform in selected):
                build_output = output / "builds" / board / optimization
                build_output.mkdir(parents=True, exist_ok=True)
                project = build_output / "standalone project"
                input_hashes = copy_project(project, board, optimization)
                if input_hashes["controller.emel"] != report["source_controller_sha256"]:
                    raise ValueError("controller was modified during project copying")
                generated = build_output / "artifacts"
                run([emela, "check", project, "--board", board], build_output, "check", cwd=project, environment=environment)
                run([emela, "build", project, "--board", board, "--out-dir", generated], build_output, "build", cwd=project, environment=environment)
                flash = run([emela, "flash", project, "--board", board, "--out-dir", generated, "--dry-run"], build_output, "flash-dry-run", cwd=project, environment=environment)
                if '"dry_run": true' not in flash:
                    raise ValueError("CLI flash did not produce an explicit dry run")
                manifest = json.loads((generated / "firmware/firmware.json").read_text())
                report["builds"].append({"board": board, "optimization": optimization, "status": "passed",
                                         "input_sha256": input_hashes, "resources": manifest["resources"],
                                         "firmware_directory": str(generated / "firmware")})
                atomic_json(output / "report.json", report)
        def observe(platform, optimization):
            board = PLATFORMS[platform][0]
            case_output = output / "cases" / platform / optimization
            case_output.mkdir(parents=True, exist_ok=True)
            command = [sys.executable, CRATE / "tools/stm32f4_flash.py", "--firmware-dir", output / "builds" / board / optimization / "artifacts/firmware",
                       "--output", case_output / "observations", "--platform", platform,
                       "--renode", renode, "--timeout", str(args.timeout)]
            if args.toolchain:
                command += ["--toolchain", args.toolchain]
            try:
                run(command, case_output, "observe", cwd=ROOT, environment=environment, timeout=args.timeout + 30)
                item = json.loads((case_output / "observations/report.json").read_text())
                if item.get("status") != "passed":
                    raise ValueError("observation did not produce a passed report")
                item["optimization"] = optimization
                return item
            except (OSError, ValueError) as error:
                return {"platform": platform, "optimization": optimization, "board": board, "status": "failed", "error": str(error)}
        with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as pool:
            futures = [pool.submit(observe, platform, optimization) for platform in selected for optimization in optimizations]
            for future in concurrent.futures.as_completed(futures):
                case = future.result()
                report["cases"].append(case)
                atomic_json(output / "report.json", report)
                print(f"{case['status'].upper()}: {case['platform']}/{case['optimization']}", flush=True)
        report["cases"].sort(key=lambda item: (selected.index(item["platform"]), optimizations.index(item["optimization"])))
        report["status"] = "passed" if all(item["status"] == "passed" for item in report["cases"]) else "failed"
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        report.update({"status": "failed", "error": str(error)})
        print(str(error), file=sys.stderr)
    finally:
        report["finished_at"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
        atomic_json(output / "report.json", report)
        lease.close()
    print(f"{report['status'].upper()}: {output / 'report.json'}")
    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    sys.exit(main())
