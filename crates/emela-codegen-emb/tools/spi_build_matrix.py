#!/usr/bin/env python3
"""Check and cross-build the SPI example with both bundled BSPs (no flashing)."""
import argparse
import json
import os
from pathlib import Path
import sys

sys.dont_write_bytecode = True
from board_probe_common import atomic_json, claim_output, run

ROOT = Path(__file__).resolve().parents[3]
CS_PINS = {"stm32f407vg": (16, 17), "stm32f412zg": (16, 17),
           "stm32f429zi": (16, 17), "stm32f446re": (16, 17), "esp32c3": (2, 3)}


def validate(args):
    output = args.output.expanduser().resolve()
    lease = claim_output(output, "spi-build-matrix")
    report = {"schema_version": 1, "status": "running", "cases": []}
    example = ROOT / "examples/spi_devices"
    try:
        emela = args.emela.expanduser().resolve()
        if not emela.is_file():
            raise ValueError("--emela must name an existing executable")
        for board in args.boards:
            for optimization in args.optimizations:
                case = output / board / optimization
                project = case / "project"
                (project / "src").mkdir(parents=True, exist_ok=True)
                source = (example / "src/main.emel").read_text()
                first_cs, second_cs = CS_PINS[board]
                (project / "src/main.emel").write_text(source.replace("GpioPin(16)", f"GpioPin({first_cs})").replace("GpioPin(17)", f"GpioPin({second_cs})"))
                (project / "embedded.toml").write_text((example / "embedded.toml").read_text())
                (project / "Pome.toml").write_text(f'[build]\ntarget = "c-embedded"\n[embedded]\nboard = "{board}"\nconfig = "embedded.toml"\noptimization = "{optimization}"\n')
                environment = dict(os.environ, EMELA_POMEPROXY="off", PYTHONDONTWRITEBYTECODE="1")
                for name, command in [
                    ("check", [emela, "check", project]),
                    ("build", [emela, "build", project, "--out-dir", case / "build"]),
                ]:
                    print(f"{board}/{optimization}: {name}", flush=True)
                    run(command, case, name, cwd=ROOT, environment=environment, timeout=args.timeout)
                firmware = json.loads((case / "build/firmware/firmware.json").read_text())
                report["cases"].append({"board": board, "optimization": optimization,
                    "resources": firmware["resources"], "sha256": firmware["sha256"]})
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
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--boards", nargs="+", choices=CS_PINS, default=list(CS_PINS))
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
