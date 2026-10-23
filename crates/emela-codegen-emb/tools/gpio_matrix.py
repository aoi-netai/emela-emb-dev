#!/usr/bin/env python3
"""Build the GPIO scenario with the normal CLI and run both BSPs in Renode."""
import argparse
import json
import os
from pathlib import Path
import sys

sys.dont_write_bytecode = True
from board_probe_common import atomic_json, claim_output, run

CRATE = Path(__file__).resolve().parents[1]
PROFILES = {"stm32f407vg": ("60", "2", "True"), "esp32c3": ("3", "20", "False")}
CONFIG = """heap_bytes = 4096
string_capacity = 64
peripheral_buffer_capacity = 16
fiber_capacity = 4
frame_capacity = 12
frame_bytes = 1536
task_capacity = 2
event_capacity = 2
deferred_capacity = 1
"""


def validate(args):
    output = args.output.resolve()
    lease = claim_output(output, "gpio-matrix")
    report = {"schema_version": 1, "status": "running", "cases": []}
    atomic_json(output / "report.json", report)
    try:
        source = (CRATE / "examples/support/gpio_interrupt.emel").read_text()
        emela, renode = args.emela.expanduser().resolve(), args.renode.expanduser().resolve()
        if not emela.is_file() or not renode.is_file():
            raise ValueError("--emela and --renode must name existing executables")
        for board in args.boards:
            pin, reserved, analog = PROFILES[board]
            concrete = source.replace("__OUTPUT__", pin).replace("__RESERVED__", reserved).replace("__ANALOG__", analog)
            for optimization in args.optimizations:
                case = output / board / optimization
                project = case / "project"
                (project / "src").mkdir(parents=True, exist_ok=True)
                (project / "src/main.emel").write_text(concrete)
                (project / "embedded.toml").write_text(CONFIG)
                (project / "Pome.toml").write_text(f'[build]\ntarget = "c-embedded"\n[embedded]\nboard = "{board}"\nconfig = "embedded.toml"\noptimization = "{optimization}"\n')
                environment = dict(os.environ, EMELA_POMEPROXY="off", PYTHONDONTWRITEBYTECODE="1")
                for name, command in [
                    ("check", [emela, "check", project]),
                    ("build", [emela, "build", project, "--out-dir", case / "build"]),
                    ("renode", [sys.executable, CRATE / "tools/gpio_probe.py", "--firmware-dir", case / "build/firmware", "--renode", renode, "--output", case / "observed", "--timeout", args.timeout]),
                ]:
                    print(f"{board}/{optimization}: {name}", flush=True)
                    log = run(command, case, name, cwd=CRATE, environment=environment, timeout=args.timeout)
                    if name == "renode":
                        print(log.strip(), flush=True)
                result = json.loads((case / "observed/report.json").read_text())
                if result["status"] != "passed":
                    raise ValueError(f"failed GPIO observation: {board}/{optimization}")
                report["cases"].append(result)
                atomic_json(output / "report.json", report)
        report["status"] = "passed"
    except Exception as error:
        report.update(status="failed", error=str(error))
        raise
    finally:
        atomic_json(output / "report.json", report)
        lease.close()
    print(output / "report.json")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--emela", required=True, type=Path)
    parser.add_argument("--renode", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--boards", nargs="+", choices=PROFILES, default=list(PROFILES))
    parser.add_argument("--optimizations", nargs="+", choices=["O0", "O2"], default=["O0", "O2"])
    parser.add_argument("--timeout", type=float, default=120)
    args = parser.parse_args()
    if not 0 < args.timeout <= 3600:
        parser.error("timeout must be in (0, 3600]")
    try:
        validate(args)
    except (OSError, ValueError) as error:
        print(error, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
