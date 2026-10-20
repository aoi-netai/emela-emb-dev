"""Regression checks for report freshness and Renode metric validation."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from validation_process import prepare_output, run_captured
from run_stm32f4 import CRATE, check_metrics, metric


class ValidationRunnerTests(unittest.TestCase):
    def test_metric_names_do_not_match_a_longer_metric_prefix(self):
        log="INFO EMELA_X_LONG\n0x99\nINFO EMELA_X\n0x2a\n"
        self.assertEqual(metric(log,"EMELA_X"),42)
        with self.assertRaises(ValueError):metric("INFO EMELA_X\nINFO EMELA_Y\n0x42\n","EMELA_X")
        with self.assertRaises(ValueError):metric("INFO EMELA_X\n0x1\nINFO EMELA_X\n0x1\n","EMELA_X")

    def test_gpio_stack_and_missing_runtime_metrics_fail_closed(self):
        values={"gpioa_odr":0xe0,"stack":{"guard_ok":1,"samples":2,"used":32,"painted":512,"capacity":512},"fixture":{}}
        check_metrics("scalar",values)
        valid=json.loads(json.dumps(values));valid["stack"].update(used=512,painted=504);check_metrics("scalar",valid)
        for key,value in [("guard_ok",0),("samples",0),("samples",1),("used",513)]:
            broken=json.loads(json.dumps(values));broken["stack"][key]=value
            with self.assertRaises(ValueError):check_metrics("scalar",broken)
        with self.assertRaises(ValueError):check_metrics("runtime-scalar",values)
        values["gpioa_odr"]=0x100
        with self.assertRaises(ValueError):check_metrics("scalar",values)

    def test_unknown_fixture_cannot_reuse_a_previous_success(self):
        with tempfile.TemporaryDirectory(prefix="emela-runner-regression-") as temporary:
            out=Path(temporary)
            prepare_output(out)
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

    def test_unknown_nonempty_output_is_preserved(self):
        with tempfile.TemporaryDirectory(prefix="emela-output-protection-") as temporary:
            out=Path(temporary);(out/"generated").mkdir();precious=out/"generated/user.txt";precious.write_text("preserve")
            result=subprocess.run([sys.executable,str(CRATE/"tools/run_stm32f4.py"),"scalar","--output",str(out)],capture_output=True,text=True)
            self.assertNotEqual(result.returncode,0);self.assertEqual(precious.read_text(),"preserve")
            self.assertFalse((out/"report.json").exists())

    def test_timeout_terminates_a_nested_process_group(self):
        import time
        with tempfile.TemporaryDirectory(prefix="emela-process-timeout-") as temporary:
            marker=Path(temporary)/"pid"
            code="import subprocess,sys,time; p=subprocess.Popen([sys.executable,'-c','import time;time.sleep(30)'],start_new_session=True);open(sys.argv[1],'w').write(str(p.pid));time.sleep(30)"
            status,_,timed_out=run_captured([sys.executable,"-c",code,str(marker)],cwd=temporary,env=os.environ.copy(),timeout=0.5)
            self.assertTrue(timed_out);self.assertIsNone(status)
            pid=int(marker.read_text());state=Path(f"/proc/{pid}/stat")
            for _ in range(20):
                if not state.exists() or state.read_text().rsplit(")",1)[1].split()[0]=="Z": break
                time.sleep(0.02)
            else:self.fail("timed-out child is still running")


if __name__=="__main__":unittest.main()
