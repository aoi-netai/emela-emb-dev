#!/usr/bin/env python3
"""Run the host checks and the STM32F446 acceptance matrix with one command."""
import argparse
import datetime
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

from validation_process import prepare_output, run_captured
from run_stm32f4 import CRATE, ROOT, NEGATIVE_MODES, RUNTIME_MODES, atomic_json

FIXTURES = ["scalar", "completion"] + ["runtime-" + mode for mode in RUNTIME_MODES] + ["negative-" + mode for mode in NEGATIVE_MODES]


def execute(report, out, label, command, environment, timeout):
    stage = {"name": label, "command": [str(value) for value in command], "status": "running"}
    report["stages"].append(stage)
    atomic_json(out / "report.json", report)
    temporary = Path(tempfile.mkdtemp(prefix=label + "-", dir=out / "tmp"))
    env = dict(environment, TMPDIR=str(temporary))
    started = time.monotonic()
    try:
        code, content, timed_out = run_captured(command,cwd=ROOT,env=env,timeout=timeout)
        if timed_out: stage["timeout_seconds"] = timeout
    except OSError as error:
        content, code = str(error), None
        stage["error"] = str(error)
    (out / (label + ".log")).write_text(content)
    stage.update({"status": "passed" if code == 0 else "failed", "exit_code": code, "elapsed_seconds": round(time.monotonic() - started, 3), "log": label + ".log"})
    atomic_json(out / "report.json", report)
    print(f"{stage['status'].upper()}: {label}", flush=True)
    if code != 0:
        print(content[-6000:], file=sys.stderr)
    return stage


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=Path("/tmp/emela-stm32f4-acceptance"))
    parser.add_argument("--optimizations", nargs="+", choices=["O0", "O2"], default=["O0", "O2"])
    parser.add_argument("--fixtures", nargs="+", choices=FIXTURES, default=FIXTURES)
    parser.add_argument("--skip-host", action="store_true", help="Run only the selected target cases; this is a partial acceptance run.")
    parser.add_argument("--timeout", type=int, default=900, help="Wall-clock limit per host/build/case stage.")
    args = parser.parse_args()
    out = args.output.resolve()
    try:
        prepare_output(out)
    except (OSError,ValueError) as error:
        print(str(error),file=sys.stderr)
        return 1
    (out / "tmp").mkdir(exist_ok=True)
    report = {"schema_version": 1, "target": "STM32F446", "status": "running", "started_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
              "scope": "full" if not args.skip_host and set(args.fixtures) == set(FIXTURES) and set(args.optimizations) == {"O0", "O2"} else "partial", "stages": [], "cases": []}
    atomic_json(out / "report.json", report)
    environment = os.environ.copy()
    environment["CARGO_BUILD_JOBS"] = "2"
    environment["CARGO_TARGET_DIR"] = environment.get("CARGO_TARGET_DIR", str(ROOT / "target"))
    version = subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT, capture_output=True, text=True, check=True).stdout.strip()
    report["revision"] = version
    report["working_tree_dirty"] = bool(subprocess.run(["git", "status", "--porcelain"], cwd=ROOT, capture_output=True, text=True, check=True).stdout.strip())
    host = [
        ("format", ["cargo", "fmt", "-p", "emela-codegen-emb", "--check"]),
        ("host-suite", ["cargo", "test", "--locked", "-p", "emela-codegen-emb"]),
        ("flow-inventory", ["cargo", "run", "--locked", "-q", "-p", "emela-codegen-emb", "--example", "check_flow_coverage"]),
        ("runner-checks", [sys.executable, "-m", "unittest", "discover", "-s", str(CRATE / "tools"), "-p", "test_*.py"]),
    ]
    if not args.skip_host:
        for label, command in host:
            stage = execute(report, out, label, command, environment, args.timeout)
            if label == "flow-inventory" and stage["status"] == "passed":
                counts = {}
                for line in (out / "flow-inventory.log").read_text().splitlines():
                    if ": " in line:
                        name, value = line.rsplit(": ", 1)
                        if value.isdecimal():
                            counts[name] = int(value)
                stage["inventory"] = counts
                stage["total"] = sum(counts.values())
                if stage["total"] != 149:
                    stage.update(status="failed", error="expected 149 inventoried enum variants")
                atomic_json(out / "report.json", report)
    for optimization in args.optimizations:
        for fixture in args.fixtures:
            label = f"{fixture}-{optimization}"
            directory = out / label
            command = [sys.executable, CRATE / "tools/run_stm32f4.py", fixture, "--output", directory, "--optimization", optimization]
            stage = execute(report, out, label, command, environment, args.timeout)
            case_report = directory / "report.json"
            case = {"fixture": fixture, "optimization": optimization, "status": "failed"}
            if stage["exit_code"] == 0 and case_report.exists():
                case = json.loads(case_report.read_text())
            elif case_report.exists():
                case["error"] = json.loads(case_report.read_text()).get("error", "case process failed")
            report["cases"].append(case)
            atomic_json(out / "report.json", report)
    complete = len(report["cases"]) == len(args.fixtures) * len(args.optimizations)
    report["status"] = "passed" if complete and all(stage["status"] == "passed" for stage in report["stages"]) and all(case["status"] == "passed" for case in report["cases"]) else "failed"
    report["finished_at"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
    atomic_json(out / "report.json", report)
    print(f"{report['status'].upper()}: {report['scope']} STM32F446 report: {out / 'report.json'}")
    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    sys.exit(main())
