"""A Pi session is mirrored into OpenCode, and the OpenCode copy is readable.

Uses the real `opencode import` against an isolated XDG_DATA_HOME, so the
user's OpenCode store is never written. Skipped when the OpenCode CLI is not
installed.
"""
from __future__ import annotations

import importlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

DAEMON_DIR = Path(__file__).resolve().parent.parent
HSCONV = Path(os.environ.get("HSCONV", Path.home() / "bin" / "hsconv"))
OPENCODE = os.environ.get("OPENCODE_BIN") or shutil.which("opencode") or "/var/lib/snapd/snap/bin/opencode"
SRC_ID = "99999999-8888-7777-6666-555555555555"


@unittest.skipUnless(HSCONV.exists(), f"hsconv not built at {HSCONV}")
@unittest.skipUnless(Path(OPENCODE).exists() or shutil.which(OPENCODE), "opencode CLI not found")
class PiToOpenCodeTest(unittest.TestCase):
    def setUp(self) -> None:
        self.sb = Path(tempfile.mkdtemp(prefix="hs-pi2oc-"))
        self.xdg = self.sb / "xdg"
        self.xdg.mkdir()
        (self.sb / "omp").mkdir()
        (self.sb / "claude" / "projects").mkdir(parents=True)
        pi = self.sb / "pi" / "sessions" / "--tmp-x-"
        pi.mkdir(parents=True)
        self.src = pi / f"2026-10-01T10-00-00-000Z_{SRC_ID}.jsonl"
        rows = [
            {"type": "session", "version": 3, "id": SRC_ID, "timestamp": "2026-10-01T10:00:00.000Z",
             "cwd": "/tmp/x", "title": "Pi source title"},
            {"type": "message", "id": "m1", "parentId": None, "message": {"role": "user", "content": "hello from pi"}},
            {"type": "message", "id": "m2", "parentId": "m1",
             "message": {"role": "assistant", "content": [{"type": "text", "text": "hi from pi"}]}},
            {"type": "model_change", "pad": "x" * 600},
        ]
        self.src.write_text("\n".join(json.dumps(r) for r in rows) + "\n", encoding="utf-8")
        self.xdg_db = self.xdg / "opencode" / "opencode.db"
        # Make the isolated store exist, so the daemon can read it.
        subprocess.run([OPENCODE, "session", "list"], capture_output=True, text=True,
                       env=dict(os.environ, XDG_DATA_HOME=str(self.xdg)), timeout=120)
        os.environ.update({
            "HARNESS_SYNC_DIR": str(self.sb / "state"), "OMP_AGENT_DIR": str(self.sb / "omp"),
            "PI_AGENT_DIR": str(self.sb / "pi"), "CLAUDE_CONFIG_DIR": str(self.sb / "claude"),
            "OPENCODE_DB": str(self.xdg_db), "OPENCODE_BIN": OPENCODE, "HSCONV": str(HSCONV),
            "HSLOCK": str(self.sb / "no-hslock"), "HARNESS_SYNC_QUIET": "0",
            "HARNESS_SYNC_MAX_AGE_DAYS": "3650", "XDG_DATA_HOME": str(self.xdg),
        })
        sys.path.insert(0, str(DAEMON_DIR))
        sys.modules.pop("harness_sync", None)
        self.hs = importlib.import_module("harness_sync")

    def tearDown(self) -> None:
        shutil.rmtree(self.sb, ignore_errors=True)
        for k in ("HARNESS_SYNC_DIR", "OMP_AGENT_DIR", "PI_AGENT_DIR", "CLAUDE_CONFIG_DIR", "OPENCODE_DB",
                  "OPENCODE_BIN", "HSCONV", "HSLOCK", "HARNESS_SYNC_QUIET", "HARNESS_SYNC_MAX_AGE_DAYS",
                  "XDG_DATA_HOME"):
            os.environ.pop(k, None)

    def _opencode_sessions(self) -> str:
        r = subprocess.run([OPENCODE, "session", "list"], capture_output=True, text=True,
                           env=dict(os.environ, XDG_DATA_HOME=str(self.xdg)), timeout=120)
        return r.stdout

    def test_pi_session_appears_in_opencode_with_its_title(self) -> None:
        self.hs.one_pass(self.hs.State())
        listing = self._opencode_sessions()
        self.assertIn("Pi source title", listing, f"Pi session not mirrored into OpenCode:\n{listing}")

    def test_mirror_is_not_named_after_a_date(self) -> None:
        self.hs.one_pass(self.hs.State())
        listing = self._opencode_sessions()
        self.assertNotIn("New session - 2026", listing, "date-named mirror in OpenCode")

    def test_mirror_is_listed_once_after_two_passes(self) -> None:
        self.hs.one_pass(self.hs.State())
        self.hs.one_pass(self.hs.State())
        listing = self._opencode_sessions()
        self.assertEqual(listing.count("Pi source title"), 1, "mirror duplicated on re-sync")


if __name__ == "__main__":
    unittest.main()
