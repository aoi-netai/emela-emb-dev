"""Regression checks for report freshness and Renode metric validation."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from run_stm32f4 import CRATE, check_metrics, metric


class ValidationRunnerTests(unittest.TestCase):
    def test_metric_names_do_not_match_a_longer_metric_prefix(self):
        log="INFO EMELA_X_LONG\n0x99\nINFO EMELA_X\n0x2a\n"
        self.assertEqual(metric(log,"EMELA_X"),42)
        with self.assertRaises(ValueError):metric("INFO EMELA_X\nINFO EMELA_Y\n0x42\n","EMELA_X")
        with self.assertRaises(ValueError):metric("INFO EMELA_X\n0x1\nINFO EMELA_X\n0x1\n","EMELA_X")

    def test_gpio_stack_and_missing_runtime_metrics_fail_closed(self):
        values={"gpioa_odr":0xe0,"stack":{"guard_ok":1,"samples":1,"used":32,"painted":512,"capacity":512},"fixture":{}}
        check_metrics("scalar",values)
        for key,value in [("guard_ok",0),("samples",0),("used",513)]:
            broken=json.loads(json.dumps(values));broken["stack"][key]=value
            with self.assertRaises(ValueError):check_metrics("scalar",broken)
        with self.assertRaises(ValueError):check_metrics("runtime-scalar",values)
        values["gpioa_odr"]=0x100
        with self.assertRaises(ValueError):check_metrics("scalar",values)

    def test_unknown_fixture_cannot_reuse_a_previous_success(self):
        with tempfile.TemporaryDirectory(prefix="emela-runner-regression-") as temporary:
            out=Path(temporary)
            (out/"report.json").write_text('{"status":"passed"}')
            (out/"firmware.elf").write_text("stale")
            result=subprocess.run([sys.executable,str(CRATE/"tools/run_stm32f4.py"),"unknown","--output",str(out)],capture_output=True,text=True)
            self.assertNotEqual(result.returncode,0)
            self.assertEqual(json.loads((out/"report.json").read_text())["status"],"failed")
            self.assertFalse((out/"firmware.elf").exists())

    def test_missing_tool_is_an_explicit_failed_report(self):
        with tempfile.TemporaryDirectory(prefix="emela-runner-regression-") as temporary:
            out=Path(temporary)
            result=subprocess.run([sys.executable,str(CRATE/"tools/run_stm32f4.py"),"scalar","--output",str(out)],env=dict(os.environ,ARM_TOOLCHAIN_PATH=str(out/"missing")),capture_output=True,text=True)
            report=json.loads((out/"report.json").read_text())
            self.assertNotEqual(result.returncode,0)
            self.assertEqual(report["status"],"failed")
            self.assertIn("missing required tool",report["error"])


if __name__=="__main__":unittest.main()
