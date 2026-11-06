import importlib.machinery
import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent


def load():
    loader = importlib.machinery.SourceFileLoader("nixyon_ci", str(HERE / "nixyon-ci"))
    spec = importlib.util.spec_from_loader("nixyon_ci", loader)
    module = importlib.util.module_from_spec(spec)
    loader.exec_module(module)
    return module


class NixyonCiTest(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.ci = load()
        self.ci.REPORTS = self.tmp / "reports"
        self.ci.LOCK = self.tmp / "heavy.d"
        self.env = {
            "GITHUB_RUN_ID": "77",
            "GITHUB_RUN_ATTEMPT": "2",
            "GITHUB_SHA": "abc123",
            "GITHUB_REF_NAME": "main",
            "GITHUB_ACTOR": "andre",
            "GITHUB_REPOSITORY": "Andre-wb/Nixyon",
            "GITHUB_WORKSPACE": str(self.tmp),
        }
        self.saved_env = dict(os.environ)
        os.environ.update(self.env)

    def tearDown(self):
        os.environ.clear()
        os.environ.update(self.saved_env)

    def read(self, relative):
        return json.loads((self.ci.REPORTS / "77-2" / relative).read_text())

    def test_steps_are_numbered_logged_and_closed_with_their_exit_code(self):
        self.assertEqual(self.ci.step("fmt", "Форматирование", ["sh", "-c", "echo ок"]), 0)
        self.assertEqual(self.ci.step("test", "Тесты", ["sh", "-c", "echo упал; exit 3"]), 3)
        self.assertEqual(self.ci.finish("failure"), 0)
        run = self.read("run.json")
        self.assertEqual(run["run_id"], "77")
        self.assertEqual(run["attempt"], 2)
        self.assertEqual(run["url"], "https://github.com/Andre-wb/Nixyon/actions/runs/77")
        second = self.read("steps/2.json")
        self.assertEqual((second["key"], second["exit_code"]), ("test", 3))
        self.assertIsNotNone(second["finished_at"])
        log = (self.ci.REPORTS / "77-2/steps/2.log").read_text()
        self.assertEqual(log, "упал\n")
        self.assertEqual(self.read("finish.json")["conclusion"], "failure")
        self.assertFalse(self.ci.LOCK.exists())

    def test_missing_program_is_a_failed_step_not_a_crash(self):
        self.assertEqual(self.ci.step("x", "Нет программы", ["нет-такой-программы"]), 127)
        self.assertEqual(self.read("steps/1.json")["exit_code"], 127)

    def test_non_numeric_run_id_is_refused(self):
        os.environ["GITHUB_RUN_ID"] = "../../etc"
        with self.assertRaises(SystemExit):
            self.ci.step("x", "x", ["true"])

    def test_long_log_is_cut_but_the_step_still_passes(self):
        self.ci.MAX_LOG_BYTES = 100
        code = self.ci.step("big", "Большой вывод", [sys.executable, "-c", "print('я' * 500)"])
        self.assertEqual(code, 0)
        log = (self.ci.REPORTS / "77-2/steps/1.log").read_bytes()
        self.assertIn("журнал обрезан".encode(), log)
        self.assertLess(len(log), 200)

    def test_step_waits_for_a_held_lock_and_takes_an_abandoned_one(self):
        self.ci.LOCK.mkdir()
        (self.ci.LOCK / "owner").write_text("сборка APK в хабе")
        done = threading.Event()
        threading.Thread(
            target=lambda: (self.ci.step("a", "A", ["true"]), done.set()), daemon=True
        ).start()
        time.sleep(1.5)
        self.assertFalse(done.is_set())
        old = time.time() - self.ci.STALE_AFTER - 5
        os.utime(self.ci.LOCK / "owner", (old, old))
        self.assertTrue(done.wait(5))
        self.assertFalse(self.ci.LOCK.exists())

    def test_prune_keeps_recent_runs_within_the_limit(self):
        self.ci.KEEP_RUNS = 2
        for run_id, started in (("1", 10), ("2", 20), ("3", 30)):
            root = self.ci.REPORTS / f"{run_id}-1"
            self.ci.write_json(root / "run.json", {"started_at": started})
            (root / "steps").mkdir()
            (root / "steps" / "1.log").write_text("x")
        (self.ci.REPORTS / "not-a-run").mkdir()
        self.ci.prune(40)
        names = sorted(p.name for p in self.ci.REPORTS.iterdir())
        self.assertEqual(names, ["2-1", "3-1", "not-a-run"])
        self.ci.prune(self.ci.KEEP_SECONDS + 25)
        names = sorted(p.name for p in self.ci.REPORTS.iterdir())
        self.assertEqual(names, ["3-1", "not-a-run"])

    def test_cli_rejects_wrong_usage(self):
        result = subprocess.run(
            [sys.executable, str(HERE / "nixyon-ci"), "step", "a"],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(result.returncode, 2)


if __name__ == "__main__":
    unittest.main()
