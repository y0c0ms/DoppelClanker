"""Adapter conversion tests against the real hsconv binary.

Each test converts a fixture in the real omp journal format and checks the
output in the target harness's format. A conversion must keep both the
prompt and the answer. Fixtures link messages with parentId, as omp does:
a message with no parent link is not reachable from the leaf and is dropped
by design, so fixtures without links would test the wrong thing.

Run: python3 -m unittest discover -s daemon/tests -v
"""
from __future__ import annotations

import glob
import json
import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

HSCONV = Path(os.environ.get("HSCONV", Path.home() / "bin" / "hsconv"))
SESSION = "aaaaaaaa-1111-2222-3333-444444444444"

FIXTURE = [
    {"type": "session", "version": 3, "id": SESSION, "timestamp": "2026-10-01T10:00:00.000Z",
     "cwd": "/tmp/adapt", "title": "Adapter demo"},
    {"type": "message", "id": "m1", "parentId": None,
     "timestamp": "2026-10-01T10:00:01.000Z",
     "message": {"role": "user", "content": "what is 2+2"}},
    {"type": "message", "id": "m2", "parentId": "m1",
     "timestamp": "2026-10-01T10:00:02.000Z",
     "message": {"role": "assistant", "content": [{"type": "text", "text": "4"}]}},
]


@unittest.skipUnless(HSCONV.exists(), f"hsconv not found at {HSCONV}")
class AdapterTest(unittest.TestCase):
    def setUp(self) -> None:
        self.dir = Path(tempfile.mkdtemp(prefix="hs-adapt-"))
        self.src = self.dir / "omp.jsonl"
        self.src.write_text("\n".join(json.dumps(x) for x in FIXTURE) + "\n", encoding="utf-8")

    def tearDown(self) -> None:
        shutil.rmtree(self.dir, ignore_errors=True)

    def _run(self, *args: str) -> subprocess.CompletedProcess:
        return subprocess.run([str(HSCONV), *args], capture_output=True, text=True)

    # --------------------------------------------------------- OpenCode

    def test_omp_to_opencode_keeps_prompt_and_answer(self) -> None:
        out = self.dir / "bundle.json"
        r = self._run("to-opencode", str(self.src), "--out", str(out),
                      "--id", "ses_testadapter000000000000", "--title", "Adapter demo")
        self.assertEqual(r.returncode, 0, r.stderr)
        bundle = json.loads(out.read_text(encoding="utf-8"))
        roles = [m["info"]["role"] for m in bundle["messages"]]
        self.assertEqual(roles, ["user", "assistant"], "both turns must survive")
        self.assertEqual(bundle["info"]["id"], "ses_testadapter000000000000")
        self.assertEqual(bundle["info"]["title"], "Adapter demo")

    def test_opencode_bundle_carries_the_answer_text(self) -> None:
        out = self.dir / "bundle.json"
        self._run("to-opencode", str(self.src), "--out", str(out),
                  "--id", "ses_testadapter000000000000", "--title", "Adapter demo")
        text = out.read_text(encoding="utf-8")
        self.assertIn('"4"', text, "assistant answer missing from OpenCode bundle")
        self.assertIn("what is 2+2", text, "user prompt missing from OpenCode bundle")

    # ------------------------------------------------------------ Claude

    def test_omp_to_claude_keeps_prompt_and_answer(self) -> None:
        out_dir = self.dir / "claude"
        r = self._run("to-claude", str(self.src), "--out-dir", str(out_dir),
                      "--id", "bbbbbbbb-1111-2222-3333-444444444444")
        self.assertEqual(r.returncode, 0, r.stderr)
        files = glob.glob(str(out_dir / "*" / "*.jsonl"))
        self.assertEqual(len(files), 1, f"expected one Claude session file, got {files}")
        rows = [json.loads(line) for line in Path(files[0]).read_text(encoding="utf-8").splitlines() if line.strip()]
        self.assertEqual([r["type"] for r in rows], ["user", "assistant"])
        self.assertEqual(rows[0]["message"]["content"], "what is 2+2")

    def test_claude_keeps_real_timestamps_not_epoch(self) -> None:
        out_dir = self.dir / "claude"
        self._run("to-claude", str(self.src), "--out-dir", str(out_dir),
                  "--id", "bbbbbbbb-1111-2222-3333-444444444444")
        f = glob.glob(str(out_dir / "*" / "*.jsonl"))[0]
        rows = [json.loads(line) for line in Path(f).read_text(encoding="utf-8").splitlines() if line.strip()]
        for r in rows:
            self.assertTrue(r["timestamp"].startswith("2026-10-01"), f"epoch/wrong timestamp: {r['timestamp']}")

    # --------------------------------------------------------- Round trip

    def test_round_trip_omp_to_opencode_and_back_keeps_text(self) -> None:
        bundle = self.dir / "bundle.json"
        self._run("to-opencode", str(self.src), "--out", str(bundle),
                  "--id", "ses_testadapter000000000000", "--title", "Adapter demo")
        data = json.loads(bundle.read_text(encoding="utf-8"))
        texts = []
        for m in data["messages"]:
            for p in m.get("parts", []):
                if p.get("type") == "text":
                    texts.append(p.get("text", ""))
        self.assertIn("what is 2+2", texts, "prompt lost in the OpenCode round trip")
        self.assertIn("4", texts, "answer lost in the OpenCode round trip")

    # ------------------------------------------------------------ Negatives

    def test_unlinked_messages_are_not_silently_invented(self) -> None:
        # A message with no parent link is not on the path to the leaf, so it is
        # not part of the conversation. The converter must not invent it.
        bad = self.dir / "unlinked.jsonl"
        lines = [FIXTURE[0],
                 {"type": "message", "id": "m1", "message": {"role": "user", "content": "orphan"}},
                 {"type": "message", "id": "m2", "parentId": "m1",
                  "message": {"role": "assistant", "content": [{"type": "text", "text": "ok"}]}}]
        bad.write_text("\n".join(json.dumps(x) for x in lines) + "\n", encoding="utf-8")
        out = self.dir / "b.json"
        r = self._run("to-opencode", str(bad), "--out", str(out), "--id", "ses_testadapter000000000000",
                      "--title", "t")
        self.assertEqual(r.returncode, 0, r.stderr)
        data = json.loads(out.read_text(encoding="utf-8"))
        self.assertEqual(len(data["messages"]), 2)

    def test_garbage_input_fails_loudly(self) -> None:
        bad = self.dir / "garbage.jsonl"
        bad.write_text("this is not a journal\n", encoding="utf-8")
        r = self._run("to-opencode", str(bad), "--out", str(self.dir / "x.json"),
                      "--id", "ses_testadapter000000000000", "--title", "t")
        self.assertNotEqual(r.returncode, 0, "non-journal input must not convert silently")


if __name__ == "__main__":
    unittest.main()
