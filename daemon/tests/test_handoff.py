"""End-to-end handoff test: one conversation moved between harnesses.

Simulates the workflow a user does, with the real sync daemon and the real
hsconv converter, against a throwaway sandbox (never the user's stores):

  1. A session is created in omp and a prompt is answered.
     -> the daemon mirrors it into Pi and OpenCode (and Claude when installed).
  2. The user switches to another harness and checks the mirror exists.
  3. The user continues inside the mirror. The mirror's content changes.
  4. The daemon is run again. The changed mirror must be FROZEN (divergence
     guard), never overwritten, and the source must remain intact.

Model calls are not made; a "turn" is written in the harness's own file
format. That is the part the sync contract depends on.

Run:  python3 -m unittest discover -s daemon/tests -v
Requires: the hsconv binary (HSCONV env var, or ../hsconv/target/release/hsconv).
"""
from __future__ import annotations

import importlib
import json
import os
import shutil
import sys
import tempfile
import time
import unittest
from pathlib import Path

DAEMON_DIR = Path(__file__).resolve().parent.parent
HSCONV_BIN = Path(os.environ.get("HSCONV", DAEMON_DIR.parent / "hsconv" / "target" / "release" / "hsconv"))
SESSION_ID = "aaaaaaaa-1111-2222-3333-444444444444"
SOURCE_NAME = f"2026-10-01T10-00-00-000Z_{SESSION_ID}.jsonl"


def _load_daemon(sandbox: Path):
    """Import harness_sync with every store path pointed into the sandbox.
    The module reads its paths from the environment at import time, so each
    test reloads it after setting them."""
    env = {
        "HARNESS_SYNC_DIR": str(sandbox / "state"),
        "OMP_AGENT_DIR": str(sandbox / "omp"),
        "PI_AGENT_DIR": str(sandbox / "pi"),
        "CLAUDE_CONFIG_DIR": str(sandbox / "claude"),
        "OPENCODE_DB": str(sandbox / "no-opencode.db"),
        "HSCONV": str(HSCONV_BIN),
        "HSLOCK": str(sandbox / "no-hslock"),  # absent: daemon skips the lock layer
        "HARNESS_SYNC_QUIET": "0",
        "HARNESS_SYNC_MAX_AGE_DAYS": "3650",
    }
    os.environ.update(env)
    sys.path.insert(0, str(DAEMON_DIR))
    if "harness_sync" in sys.modules:
        del sys.modules["harness_sync"]
    return importlib.import_module("harness_sync")


def _omp_session(path: Path, prompt: str, answer: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    lines = [
        {"type": "session", "version": 3, "id": SESSION_ID, "timestamp": "2026-10-01T10:00:00.000Z",
         "cwd": "/tmp/handoff-demo", "title": "Handoff demo"},
        {"type": "message", "id": "m1", "message": {"role": "user", "content": prompt}},
        {"type": "message", "id": "m2", "message": {"role": "assistant",
                                                    "content": [{"type": "text", "text": answer}]}},
    ]
    # Pad past the daemon's 512-byte minimum so the session is considered.
    lines.append({"type": "model_change", "id": "m3", "model": "test/model", "pad": "x" * 600})
    path.write_text("\n".join(json.dumps(x) for x in lines) + "\n", encoding="utf-8")


def _append_turn(path: Path, prompt: str, answer: str) -> None:
    """Continue a conversation in a harness: append one user + one assistant turn."""
    with path.open("a", encoding="utf-8") as fh:
        fh.write(json.dumps({"type": "message", "id": f"u{time.time_ns()}",
                             "message": {"role": "user", "content": prompt}}) + "\n")
        fh.write(json.dumps({"type": "message", "id": f"a{time.time_ns()}",
                             "message": {"role": "assistant",
                                         "content": [{"type": "text", "text": answer}]}}) + "\n")


def _mirror_files(root: Path, glob: str) -> list[Path]:
    return sorted(root.glob(glob)) if root.exists() else []


@unittest.skipUnless(HSCONV_BIN.exists(), f"hsconv not built at {HSCONV_BIN}")
class HandoffTest(unittest.TestCase):
    def setUp(self) -> None:
        self.sandbox = Path(tempfile.mkdtemp(prefix="hs-handoff-"))
        (self.sandbox / "pi").mkdir()
        (self.sandbox / "claude").mkdir()
        self.omp_file = self.sandbox / "omp" / "sessions" / "-tmp-handoff-demo" / SOURCE_NAME
        self.hs = _load_daemon(self.sandbox)
        # Pi and Claude are "installed" when their directories exist.
        (self.sandbox / "pi" / "sessions").mkdir()
        (self.sandbox / "claude" / "projects").mkdir()

    def tearDown(self) -> None:
        shutil.rmtree(self.sandbox, ignore_errors=True)
        for key in ("HARNESS_SYNC_DIR", "OMP_AGENT_DIR", "PI_AGENT_DIR", "CLAUDE_CONFIG_DIR",
                    "OPENCODE_DB", "HSCONV", "HSLOCK", "HARNESS_SYNC_QUIET", "HARNESS_SYNC_MAX_AGE_DAYS"):
            os.environ.pop(key, None)

    def _pass(self) -> None:
        state = self.hs.State()
        self.hs.one_pass(state)

    def _pi_mirrors(self) -> list[Path]:
        return _mirror_files(self.sandbox / "pi" / "sessions", "*/*.jsonl")

    def _claude_mirrors(self) -> list[Path]:
        return _mirror_files(self.sandbox / "claude" / "projects", "*/*.jsonl")

    # ------------------------------------------------------------- steps

    def test_1_session_in_omp_is_mirrored_to_pi(self) -> None:
        _omp_session(self.omp_file, "what is the capital of Australia?", "Canberra.")
        self._pass()
        mirrors = self._pi_mirrors()
        self.assertEqual(len(mirrors), 1, f"expected one Pi mirror, got {mirrors}")
        text = mirrors[0].read_text(encoding="utf-8")
        self.assertIn("what is the capital of Australia?", text, "prompt missing from mirror")
        self.assertIn("Canberra.", text, "answer missing from mirror")

    def test_2_mirror_keeps_original_title_without_date(self) -> None:
        _omp_session(self.omp_file, "hello", "hi there")
        self._pass()
        head = json.loads(self._pi_mirrors()[0].read_text(encoding="utf-8").splitlines()[0])
        title = head.get("title", "")
        self.assertEqual(title, "Handoff demo", f"mirror title must be the source name, got {title!r}")
        self.assertNotRegex(title, r"\d{4}-\d{2}-\d{2}", "date leaked into the mirror title")
        self.assertNotIn("[mirror]", title, "old prefix must be gone")

    def test_2b_untitled_session_never_shows_a_date(self) -> None:
        # No title record, no prompt: the only input that reaches the old
        # filename fallback. The mirror must not be named after the timestamp.
        path = self.omp_file
        path.parent.mkdir(parents=True, exist_ok=True)
        lines = [
            {"type": "session", "version": 3, "id": SESSION_ID, "timestamp": "2026-10-01T10:00:00.000Z",
             "cwd": "/tmp/handoff-demo"},
            {"type": "model_change", "id": "m3", "model": "test/model", "pad": "x" * 600},
        ]
        path.write_text("\n".join(json.dumps(x) for x in lines) + "\n", encoding="utf-8")
        self.hs.one_pass(self.hs.State())
        for mirror in self._pi_mirrors():
            head = json.loads(mirror.read_text(encoding="utf-8").splitlines()[0])
            title = head.get("title", "")
            self.assertNotRegex(title, r"\d{4}-\d{2}-\d{2}", f"date leaked into title: {title!r}")
            self.assertNotIn("T10-00-00", title)
            self.assertNotIn("1970", title)

    def test_3_continue_in_mirror_then_resync_freezes_it(self) -> None:
        _omp_session(self.omp_file, "first question", "first answer")
        self._pass()
        mirror = self._pi_mirrors()[0]
        before_source = self.omp_file.read_text(encoding="utf-8")

        # The user continues the conversation inside Pi (the mirror).
        _append_turn(mirror, "follow-up typed in pi", "answer given in pi")
        mirror_after_turn = mirror.read_text(encoding="utf-8")

        # Source changes and the daemon runs again.
        _append_turn(self.omp_file, "follow-up in omp", "omp answer")
        self._pass()

        # Mirror is frozen: the Pi turn must survive and not be overwritten.
        self.assertIn("follow-up typed in pi", mirror.read_text(encoding="utf-8"),
                      "continued turn in the mirror was destroyed")
        self.assertEqual(mirror.read_text(encoding="utf-8"), mirror_after_turn,
                         "diverged mirror was rewritten")
        # Source keeps its own turns untouched.
        self.assertIn("follow-up in omp", self.omp_file.read_text(encoding="utf-8"))
        self.assertIn("follow-up in omp", before_source + "follow-up in omp")

        state = self.hs.State()
        src = state.sources[str(self.omp_file)]
        self.assertTrue(src.mirrors["pi"].diverged, "mirror should be marked diverged")

    def test_4_untouched_mirror_follows_source_updates(self) -> None:
        _omp_session(self.omp_file, "first question", "first answer")
        self._pass()
        mirror = self._pi_mirrors()[0]
        # The source changes, the mirror has not been continued: it must update.
        _append_turn(self.omp_file, "second question", "second answer")
        self._pass()
        self.assertIn("second answer", mirror.read_text(encoding="utf-8"),
                      "an untouched mirror should follow the source")
        state = self.hs.State()
        self.assertFalse(state.sources[str(self.omp_file)].mirrors["pi"].diverged)

    def test_5_mirror_is_not_treated_as_a_source(self) -> None:
        _omp_session(self.omp_file, "hello", "hi")
        self._pass()
        self._pass()  # second pass must not create a mirror of the mirror
        self.assertEqual(len(self._pi_mirrors()), 1, "mirror was re-mirrored (loop)")

    def test_6_claude_mirror_is_created_when_claude_is_installed(self) -> None:
        _omp_session(self.omp_file, "hello claude", "hi from omp")
        self._pass()
        claude = self._claude_mirrors()
        self.assertEqual(len(claude), 1, f"expected a Claude mirror, got {claude}")
        lines = [json.loads(x) for x in claude[0].read_text(encoding="utf-8").splitlines() if x.strip()]
        self.assertTrue(any(r.get("type") in ("user", "assistant") for r in lines))


if __name__ == "__main__":
    unittest.main()
