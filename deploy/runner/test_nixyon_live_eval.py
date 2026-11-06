import importlib.machinery
import importlib.util
import json
import shutil
import struct
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
DRIVER = HERE / "nixyon-live-eval"

# Подменная программа eval: отмечает старт и конец в журнале, на каждое сочетание пишет
# дамп и строку итога, как eval --no-truth --combo; на записи broken падает после первого.
FAKE_EVAL = """#!/usr/bin/env python3
import json, os, sys, time
args = sys.argv[1:]
dump = args[args.index("--dump") + 1]
threads = args[args.index("--model-threads") + 1]
jobs = args[args.index("--embed-jobs") + 1]
asr = args[args.index("--asr-jobs") + 1]
combos = [args[i + 1] for i, a in enumerate(args) if a == "--combo"]
name = args[-1]
log = os.environ["FAKE_LOG"]
with open(log, "a") as f:
    f.write(f"start {name} {threads} {jobs} {asr} {time.time()}\\n")
data = bytearray(8 * 1024 * 1024)
time.sleep(0.4)
for n, combo in enumerate(combos):
    label = combo.split("=")[0]
    if name == "broken" and n == 1:
        print("ошибка: блок не разобран", file=sys.stderr)
        sys.exit(3)
    os.makedirs(os.path.join(dump, label), exist_ok=True)
    with open(os.path.join(dump, label, name + ".json"), "w") as f:
        json.dump({"combo": combo, "flags": args}, f)
    print(f"{name} {label}: звук 10.0 с, обработка 1.0 с, RTF 0.100, спикеров 2 (крупных от 5 с: 1)")
with open(log, "a") as f:
    f.write(f"end {name} {threads} {jobs} {asr} {time.time()}\\n")
"""


def load():
    loader = importlib.machinery.SourceFileLoader("nixyon_live_eval", str(DRIVER))
    spec = importlib.util.spec_from_loader("nixyon_live_eval", loader)
    module = importlib.util.module_from_spec(spec)
    loader.exec_module(module)
    return module


def wav(path: Path, seconds: int) -> None:
    """WAV в 1 байт в секунду: длинная по заголовку запись занимает мало места."""
    fmt = struct.pack("<HHIIHH", 1, 1, 1, 1, 1, 8)
    data = b"\0" * seconds
    body = b"WAVE" + b"fmt " + struct.pack("<I", len(fmt)) + fmt
    body += b"LIST" + struct.pack("<I", 3) + b"abc\0"
    body += b"data" + struct.pack("<I", len(data)) + data
    path.write_bytes(b"RIFF" + struct.pack("<I", len(body)) + body)


class LiveEvalTest(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.records = self.tmp / "live"
        self.records.mkdir()
        self.out = self.tmp / "out"
        self.eval = self.tmp / "eval"
        self.eval.write_text(FAKE_EVAL)
        self.eval.chmod(0o755)
        self.log = self.tmp / "eval.log"

    def tearDown(self):
        shutil.rmtree(self.tmp)

    def run_driver(self, *names):
        return subprocess.run(
            [
                sys.executable,
                str(DRIVER),
                "--eval",
                str(self.eval),
                "--models",
                str(self.tmp / "models"),
                "--records",
                str(self.records),
                "--out",
                str(self.out),
                "--",
                *names,
            ],
            capture_output=True,
            text=True,
            env={"FAKE_LOG": str(self.log), "PATH": "/usr/bin:/bin"},
            timeout=120,
        )

    def events(self):
        return [line.split() for line in self.log.read_text().splitlines()]

    def test_duration_comes_from_the_wav_header(self):
        wav(self.records / "a.wav", 42)
        self.assertEqual(load().wav_seconds(self.records / "a.wav"), 42)
        (self.records / "b.wav").write_bytes(b"not a wav")
        with self.assertRaises(ValueError):
            load().wav_seconds(self.records / "b.wav")

    def test_one_process_per_record_long_first_with_more_embedding_jobs(self):
        driver = load()
        for name, seconds in [("s1", 60), ("s2", 120), ("s3", 90), ("s4", 30)]:
            wav(self.records / f"{name}.wav", seconds)
        wav(self.records / "long.wav", driver.LONG_SEC + 1)
        done = self.run_driver("s1", "long", "s2", "s3", "s4")
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        events = self.events()
        starts = [e for e in events if e[0] == "start"]
        self.assertEqual([e[1] for e in starts].count("long"), 1, "одна запись, один процесс")
        self.assertEqual(len(starts), 5)
        launched = [l.split("]")[0][1:] for l in done.stdout.splitlines() if "] старт" in l]
        self.assertEqual(launched[0], "long", "длинная запись первой")
        self.assertEqual(launched[-1], "s4", "самая короткая последней")
        threads = {e[1]: (int(e[2]), int(e[3]), int(e[4])) for e in starts}
        self.assertEqual(threads["long"], driver.LONG_THREADS)
        self.assertEqual(threads["s1"], driver.SHORT_THREADS)
        running = most = 0
        for e in sorted(events, key=lambda e: float(e[5])):
            running += 1 if e[0] == "start" else -1
            most = max(most, running)
        self.assertEqual(most, driver.PARALLEL)
        self.assertIn("ядер занято в среднем", done.stdout)

    def test_dumps_of_every_combo_and_one_run_line_per_record_with_memory_and_cpu(self):
        wav(self.records / "talk.wav", 30)
        done = self.run_driver("talk")
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        combos = {}
        for combo in "ABCD":
            combos[combo] = json.loads((self.out / combo / "talk.json").read_text())["combo"]
        self.assertEqual(combos["B"], "B=redimnet2,pyannote,0.7")
        self.assertEqual(combos["C"], "C=campplus,sortformer-fp32,0.6")
        self.assertEqual(combos["D"], "D=campplus,sortformer-int8,0.6")
        flags = json.loads((self.out / "A" / "talk.json").read_text())["flags"]
        self.assertIn("--no-truth", flags)
        self.assertEqual(flags[flags.index("--dump") + 1], str(self.out))
        runs = [json.loads(l) for l in (self.out / "runs.jsonl").read_text().splitlines()]
        self.assertEqual(len(runs), 1)
        r = runs[0]
        self.assertEqual(r["exit_code"], 0)
        self.assertGreater(r["peak_rss_mb"], 8)
        self.assertGreaterEqual(r["cpu_sec"], 0)
        self.assertEqual(sorted(r["summaries"]), list("ABCD"))
        self.assertIn("спикеров 2", r["summaries"]["B"])
        self.assertEqual(r["dumps"]["C"], "C/talk.json")
        self.assertEqual(sorted(json.loads((self.out / "combos.json").read_text())), list("ABCD"))
        self.assertIn("[talk] talk B: звук", done.stdout)

    def test_a_failed_run_fails_the_driver_but_the_others_finish(self):
        wav(self.records / "broken.wav", 30)
        wav(self.records / "fine.wav", 30)
        done = self.run_driver("broken", "fine")
        self.assertEqual(done.returncode, 1)
        runs = {
            r["record"]: r
            for r in map(json.loads, (self.out / "runs.jsonl").read_text().splitlines())
        }
        self.assertEqual(runs["broken"]["exit_code"], 3)
        self.assertEqual(sorted(runs["broken"]["summaries"]), ["A"])
        self.assertEqual(runs["fine"]["exit_code"], 0)
        self.assertIn("[broken] ошибка: блок не разобран", done.stdout)

    def test_bad_names_links_and_missing_records_are_refused_before_any_run(self):
        wav(self.records / "ok.wav", 30)
        outside = self.tmp / "secret.wav"
        wav(outside, 30)
        (self.records / "link.wav").symlink_to(outside)
        for names in (["../secret"], [".hidden"], ["missing"], ["link"], ["ok", "ok"]):
            done = self.run_driver(*names)
            self.assertEqual(done.returncode, 2, names)
        self.assertFalse(self.log.exists(), "eval не запускался")


if __name__ == "__main__":
    unittest.main()
