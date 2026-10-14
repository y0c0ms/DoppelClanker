"""Real handoff test: a LongCat conversation in OpenCode is mirrored to the other
harnesses, continued in one of them, and re-checked.

This runs for real, not as a simulation:
  * a prompt goes to omniroute/longcat-free through the real `opencode run`
  * the daemon exports that session and mirrors it through the real hsconv
  * a turn is then added inside a mirror, the source changes, and the daemon
    runs again; the continued mirror must freeze, not be overwritten

Isolation: every OpenCode call runs with XDG_DATA_HOME pointed at a temporary
directory, so it writes to its own store and never to ~/.local/share/opencode.
The daemon reads the same store through OPENCODE_DB. Nothing touches the
user's real sessions.

Skipped (not failed) when OpenCode or the OmniRoute gateway is unreachable,
so the build still runs offline. A skip is reported as a skip in the output.

Requires: OPENCODE_BIN (or `opencode` on PATH), HSCONV, and OMNIROUTE_API_KEY
in ~/.omp/agent/.env.
"""
from __future__ import annotations

import importlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

DAEMON_DIR = Path(__file__).resolve().parent.parent
HSCONV = Path(os.environ.get("HSCONV", Path.home() / "bin" / "hsconv"))
OPENCODE = os.environ.get("OPENCODE_BIN") or shutil.which("opencode") or "/var/lib/snapd/snap/bin/opencode"
MODEL = "omniroute/longcat-free"
PROMPT = "Reply with exactly the word: pong"


def _env_key() -> str | None:
    """OMNIROUTE_API_KEY from the environment or omp's .env. Never printed."""
    if os.environ.get("OMNIROUTE_API_KEY"):
        return os.environ["OMNIROUTE_API_KEY"]
    env = Path.home() / ".omp" / "agent" / ".env"
    if env.exists():
        for line in env.read_text(encoding="utf-8").splitlines():
            if line.startswith("OMNIROUTE_API_KEY="):
                return line.split("=", 1)[1].strip()
    return None


def _gateway_up() -> bool:
    key = _env_key()
    if not key:
        return False
    try:
        r = subprocess.run(["curl", "-s", "-m", "10", "-o", "/dev/null", "-w", "%{http_code}",
                            "-H", f"Authorization: Bearer {key}",
                            "http://192.168.100.178:30438/v1/models"],
                           capture_output=True, text=True, timeout=20)
        return r.stdout.strip() == "200"
    except (OSError, subprocess.SubprocessError):
        return False


def _opencode_run(xdg: Path, *args: str, timeout: int = 300) -> subprocess.CompletedProcess:
    env = dict(os.environ, XDG_DATA_HOME=str(xdg))
    return subprocess.run([OPENCODE, *args], capture_output=True, text=True, env=env, timeout=timeout)


def _load_daemon(sandbox: Path, xdg: Path):
    os.environ.update({
        "HARNESS_SYNC_DIR": str(sandbox / "state"),
        "OMP_AGENT_DIR": str(sandbox / "omp"),
        "PI_AGENT_DIR": str(sandbox / "pi"),
        "CLAUDE_CONFIG_DIR": str(sandbox / "claude"),
        "OPENCODE_DB": str(xdg / "opencode" / "opencode.db"),
        "OPENCODE_BIN": OPENCODE,
        "HSCONV": str(HSCONV),
        "HSLOCK": str(sandbox / "no-hslock"),
        "HARNESS_SYNC_QUIET": "0",
        "HARNESS_SYNC_MAX_AGE_DAYS": "3650",
    })
    sys.path.insert(0, str(DAEMON_DIR))
    sys.modules.pop("harness_sync", None)
    return importlib.import_module("harness_sync")


@unittest.skipUnless(HSCONV.exists(), f"hsconv not built at {HSCONV}")
@unittest.skipUnless(Path(OPENCODE).exists() or shutil.which(OPENCODE), "opencode CLI not found")
@unittest.skipUnless(_gateway_up(), "OmniRoute gateway unreachable or no OMNIROUTE_API_KEY")
class LongCatHandoffTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.sandbox = Path(tempfile.mkdtemp(prefix="hs-longcat-"))
        cls.xdg = cls.sandbox / "xdg"
        cls.xdg.mkdir()
        (cls.sandbox / "pi" / "sessions").mkdir(parents=True)
        (cls.sandbox / "claude" / "projects").mkdir(parents=True)
        (cls.sandbox / "omp").mkdir()

        # 1. A real prompt to the real free model, in the isolated store.
        r = _opencode_run(cls.xdg, "run", "-m", MODEL, PROMPT)
        if r.returncode != 0 or "pong" not in r.stdout.lower():
            raise unittest.SkipTest(f"longcat run did not answer pong: rc={r.returncode} {r.stdout[-200:]}")

        import sqlite3
        db = cls.xdg / "opencode" / "opencode.db"
        con = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
        try:
            row = con.execute(
                "select s.id from session s join message m on m.session_id = s.id "
                "join part p on p.message_id = m.id where p.data like ? "
                "order by s.time_created desc limit 1", (f"%{PROMPT}%",)).fetchone()
        finally:
            con.close()
        if not row:
            raise unittest.SkipTest("prompt session not found in the isolated store")
        cls.session_id = row[0]
        cls.hs = _load_daemon(cls.sandbox, cls.xdg)

    @classmethod
    def tearDownClass(cls) -> None:
        shutil.rmtree(cls.sandbox, ignore_errors=True)
        for k in ("HARNESS_SYNC_DIR", "OMP_AGENT_DIR", "PI_AGENT_DIR", "CLAUDE_CONFIG_DIR", "OPENCODE_DB",
                  "OPENCODE_BIN", "HSCONV", "HSLOCK", "HARNESS_SYNC_QUIET", "HARNESS_SYNC_MAX_AGE_DAYS"):
            os.environ.pop(k, None)

    def _pass(self) -> None:
        self.hs.one_pass(self.hs.State())

    def _pi_mirror(self) -> Path | None:
        files = sorted((self.sandbox / "pi" / "sessions").rglob("*.jsonl"))
        return files[0] if files else None

    # ------------------------------------------------------------- steps

    def test_1_longcat_session_is_mirrored_to_pi(self) -> None:
        self._pass()
        mirror = self._pi_mirror()
        self.assertIsNotNone(mirror, "no Pi mirror of the LongCat session")
        text = mirror.read_text(encoding="utf-8")
        self.assertIn(PROMPT, text, "prompt missing from Pi mirror")
        self.assertIn('"pong"', text, "LongCat answer missing from Pi mirror")

    def test_2_mirror_title_is_the_prompt_not_a_date(self) -> None:
        self._pass()
        head = json.loads(self._pi_mirror().read_text(encoding="utf-8").splitlines()[0])
        title = head.get("title", "")
        self.assertNotRegex(title, r"\d{4}-\d{2}-\d{2}", f"date in mirror title: {title!r}")
        self.assertNotIn("New session", title)

    def test_3_continue_in_mirror_then_source_change_freezes_it(self) -> None:
        self._pass()
        mirror = self._pi_mirror()
        # The user continues inside Pi.
        with mirror.open("a", encoding="utf-8") as fh:
            fh.write(json.dumps({"type": "message", "id": f"pi{time.time_ns()}",
                                 "message": {"role": "user", "content": "typed inside pi"}}) + "\n")
        after_pi_turn = mirror.read_text(encoding="utf-8")
        # The source changes in OpenCode (another real turn).
        r = _opencode_run(self.xdg, "run", "-s", self.session_id, "-m", MODEL, "Reply with exactly the word: ping")
        self.assertEqual(r.returncode, 0, r.stderr)
        self._pass()
        self.assertEqual(mirror.read_text(encoding="utf-8"), after_pi_turn,
                         "continued Pi mirror was overwritten by a source change")
        state = self.hs.State()
        self.assertTrue(state.sources[self.session_id].mirrors["pi"].diverged)


if __name__ == "__main__":
    unittest.main()
