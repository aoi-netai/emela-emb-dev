"""Capture bounded validation commands and terminate their process groups on timeout."""
import json
import os
from pathlib import Path
import signal
import subprocess

OWNER = "emela-codegen-emb STM32F446 validation v1\n"


def prepare_output(path):
    path = Path(path)
    path.mkdir(parents=True, exist_ok=True)
    marker = path / ".emela-stm32f446-owner"
    known = marker.is_file() and marker.read_text() == OWNER
    if not known and any(path.iterdir()):
        # Reports from the earlier runner version identify its existing artifact directories.
        try:
            report = json.loads((path / "report.json").read_text())
            known = report.get("target") == "STM32F446" and report.get("schema_version") == 1 and (
                isinstance(report.get("steps"), list) or (isinstance(report.get("stages"), list) and isinstance(report.get("cases"), list)))
        except (OSError, ValueError, AttributeError):
            known = False
        if not known:
            raise ValueError(f"refusing nonempty output directory without a validation owner marker/report: {path}")
    marker.write_text(OWNER)


def _descendant_groups(pid):
    table = []
    for entry in Path("/proc").iterdir():
        if not entry.name.isdecimal():
            continue
        try:
            fields = (entry / "stat").read_text().rsplit(")", 1)[1].split()
            table.append((int(entry.name), int(fields[1]), int(fields[2])))
        except (OSError, ValueError, IndexError):
            continue
    descendants, groups = {pid}, {pid}
    while True:
        found = {child for child, parent, _ in table if parent in descendants}
        if found <= descendants:
            break
        descendants.update(found)
    groups.update(group for child, _, group in table if child in descendants)
    return groups - {os.getpgrp()}


def run_captured(command, *, cwd, env, timeout):
    process = subprocess.Popen(command, cwd=cwd, env=env, stdout=subprocess.PIPE,
                               stderr=subprocess.STDOUT, text=True, errors="replace", start_new_session=True)
    try:
        output, _ = process.communicate(timeout=timeout)
        return process.returncode, output, False
    except subprocess.TimeoutExpired:
        # Include nested fixture command sessions before the parent can disappear.
        for group in _descendant_groups(process.pid):
            try:
                os.killpg(group, signal.SIGKILL)
            except ProcessLookupError:
                pass
        output, _ = process.communicate()
        return None, output, True
