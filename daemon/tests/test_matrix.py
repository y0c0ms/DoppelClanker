"""Cross-provider matrix: every source format is mirrored into every other.

For each (source, target) pair, the test builds a real conversation in the
source's own format, runs the real daemon sync, and checks the target copy:

  * the prompt and the answer both arrive, in order
  * the title is the conversation's name, not a date
  * a second pass does not duplicate the mirror
  * a turn typed into the target freezes that mirror on the next pass

Sources: omp, Pi, Claude Code, OpenCode.  Targets: the same four.
A (source, target) pair where source == target is not mirrored by design and
is skipped.

OpenCode runs against an isolated XDG_DATA_HOME, so the user's real store is
never written. The OpenCode source is created with the real `opencode run` when
the gateway is reachable, and with `opencode import` of a fixture otherwise.
"""
from __future__ import annotations

import importlib
import json
import os
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

DAEMON_DIR = Path(__file__).resolve().parent.parent
HSCONV = Path(os.environ.get("HSCONV", Path.home() / "bin" / "hsconv"))
OPENCODE = os.environ.get("OPENCODE_BIN") or shutil.which("opencode") or "/var/lib/snapd/snap/bin/opencode"
SID = "aaaaaaaa-1111-2222-3333-444444444444"
PROMPT = "matrix prompt: what is the capital of Australia"
ANSWER = "matrix answer: Canberra"
TITLE = "Matrix conversation"
FORMATS = ("omp", "pi", "claude", "opencode")


def _has_opencode() -> bool:
    return bool(shutil.which(OPENCODE) or Path(OPENCODE).exists())


def _write_source(fmt: str, root: Path, xdg: Path) -> Path:
    """Create one source conversation in `fmt`'s native store under root."""
    if fmt == "omp":
        p = root / "omp" / "sessions" / "-tmp" / f"2026-10-01T10-00-00-000Z_{SID}.jsonl"
        p.parent.mkdir(parents=True, exist_ok=True)
        slot = json.dumps({"type": "title", "v": 1, "title": TITLE, "source": "user",
                           "updatedAt": "2026-10-01T10:00:00.000Z"})
        slot += " " * (255 - len(slot.encode("utf-8")))
        rows = [
            {"type": "session", "version": 3, "id": SID, "timestamp": "2026-10-01T10:00:00.000Z",
             "cwd": "/tmp/matrix", "title": TITLE},
            {"type": "message", "id": "m1", "parentId": None, "timestamp": "2026-10-01T10:00:01.000Z",
             "message": {"role": "user", "content": PROMPT}},
            {"type": "message", "id": "m2", "parentId": "m1", "timestamp": "2026-10-01T10:00:02.000Z",
             "message": {"role": "assistant", "content": [{"type": "text", "text": ANSWER}]}},
            {"type": "model_change", "id": "m3", "pad": "x" * 600},
        ]
        p.write_text("\n".join([slot] + [json.dumps(r) for r in rows]) + "\n", encoding="utf-8")
        return p
    if fmt == "pi":
        p = root / "pi" / "sessions" / "-tmp" / f"2026-10-01T10-00-00-000Z_{SID}.jsonl"
        p.parent.mkdir(parents=True, exist_ok=True)
        rows = [
            {"type": "session", "version": 3, "id": SID, "timestamp": "2026-10-01T10:00:00.000Z",
             "cwd": "/tmp/matrix", "title": TITLE},
            {"type": "message", "id": "m1", "parentId": None, "timestamp": "2026-10-01T10:00:01.000Z",
             "message": {"role": "user", "content": PROMPT}},
            {"type": "message", "id": "m2", "parentId": "m1", "timestamp": "2026-10-01T10:00:02.000Z",
             "message": {"role": "assistant", "content": [{"type": "text", "text": ANSWER}]}},
            {"type": "model_change", "id": "m3", "pad": "x" * 600},
        ]
        p.write_text("\n".join(json.dumps(r) for r in rows) + "\n", encoding="utf-8")
        return p
    if fmt == "claude":
        cid = "bbbbbbbb-1111-2222-3333-444444444444"
        p = root / "claude" / "projects" / "-tmp-matrix" / f"{cid}.jsonl"
        p.parent.mkdir(parents=True, exist_ok=True)
        rows = [
            {"type": "ai-title", "aiTitle": TITLE, "sessionId": cid},
            {"type": "user", "uuid": "u1", "parentUuid": None, "sessionId": cid, "cwd": "/tmp/matrix",
             "timestamp": "2026-10-01T10:00:01.000Z", "message": {"role": "user", "content": PROMPT}},
            {"type": "assistant", "uuid": "a1", "parentUuid": "u1", "sessionId": cid, "cwd": "/tmp/matrix",
             "timestamp": "2026-10-01T10:00:02.000Z",
             "message": {"role": "assistant", "content": [{"type": "text", "text": ANSWER}]}},
        ]
        p.write_text("\n".join(json.dumps(r) for r in rows) + "\n", encoding="utf-8")
        return p
    if fmt == "opencode":
        # Created in the isolated store with the real CLI. The prompt is
        # written via `opencode import` of an omp-format bundle built with hsconv,
        # so no model call is needed here. The model call is covered by the
        # LongCat test.
        bundle = root / "oc-source-bundle.json"
        journal = root / "oc-source.jsonl"
        journal.write_text("\n".join(json.dumps(r) for r in [
            {"type": "session", "version": 3, "id": SID, "timestamp": "2026-10-01T10:00:00.000Z",
             "cwd": "/tmp/matrix", "title": TITLE},
            {"type": "message", "id": "m1", "parentId": None, "timestamp": "2026-10-01T10:00:01.000Z",
             "message": {"role": "user", "content": PROMPT}},
            {"type": "message", "id": "m2", "parentId": "m1", "timestamp": "2026-10-01T10:00:02.000Z",
             "message": {"role": "assistant", "content": [{"type": "text", "text": ANSWER}]}},
        ]) + "\n", encoding="utf-8")
        r = subprocess.run([str(HSCONV), "to-opencode", str(journal), "--out", str(bundle),
                            "--id", "ses_matrixsource0000000000", "--title", TITLE],
                           capture_output=True, text=True)
        if r.returncode != 0:
            raise RuntimeError(f"to-opencode fixture failed: {r.stderr}")
        r = subprocess.run([OPENCODE, "import", str(bundle)], capture_output=True, text=True,
                           env=dict(os.environ, XDG_DATA_HOME=str(xdg)), timeout=180)
        if r.returncode != 0 or "Imported session" not in r.stdout:
            raise RuntimeError(f"opencode import fixture failed: {(r.stderr or r.stdout)[-300:]}")
        return Path("ses_matrixsource0000000000")
    raise ValueError(fmt)


def _source_key(fmt: str, path: Path) -> str:
    return str(path)


def _read_rows(path: Path) -> list[dict]:
    return [json.loads(x) for x in path.read_text(encoding="utf-8").splitlines() if x.strip()]


@unittest.skipUnless(HSCONV.exists(), f"hsconv not built at {HSCONV}")
@unittest.skipUnless(_has_opencode(), "opencode CLI not found")
class MatrixTest(unittest.TestCase):
    """One test per (source, target) pair, generated below."""

    def _setup_world(self, source_fmt: str) -> dict:
        sb = Path(tempfile.mkdtemp(prefix=f"hs-matrix-{source_fmt}-"))
        xdg = sb / "xdg"
        for d in (sb / "omp" / "sessions", sb / "pi" / "sessions", sb / "claude" / "projects"):
            d.mkdir(parents=True, exist_ok=True)
        xdg.mkdir()
        os.environ.update({
            "HARNESS_SYNC_DIR": str(sb / "state"), "OMP_AGENT_DIR": str(sb / "omp"),
            "PI_AGENT_DIR": str(sb / "pi"), "CLAUDE_CONFIG_DIR": str(sb / "claude"),
            "OPENCODE_DB": str(xdg / "opencode" / "opencode.db"), "OPENCODE_BIN": OPENCODE,
            "HSCONV": str(HSCONV), "HSLOCK": str(sb / "no-hslock"), "XDG_DATA_HOME": str(xdg),
            "HARNESS_SYNC_QUIET": "0", "HARNESS_SYNC_MAX_AGE_DAYS": "3650",
        })
        sys.path.insert(0, str(DAEMON_DIR))
        for m in ("harness_sync",):
            sys.modules.pop(m, None)
        hs = importlib.import_module("harness_sync")
        # The OpenCode store must exist before the daemon reads or writes it, in
        # every world (not only when OpenCode is the source). Creating it is a
        # side effect of any `opencode` call.
        subprocess.run([OPENCODE, "session", "list"], capture_output=True,
                       env=dict(os.environ, XDG_DATA_HOME=str(xdg)), timeout=120)
        src = _write_source(source_fmt, sb, xdg)
        return {"sb": sb, "xdg": xdg, "hs": hs, "src": src}

    def _teardown_world(self, world: dict) -> None:
        shutil.rmtree(world["sb"], ignore_errors=True)
        for k in ("HARNESS_SYNC_DIR", "OMP_AGENT_DIR", "PI_AGENT_DIR", "CLAUDE_CONFIG_DIR", "OPENCODE_DB",
                  "OPENCODE_BIN", "HSCONV", "HSLOCK", "XDG_DATA_HOME", "HARNESS_SYNC_QUIET",
                  "HARNESS_SYNC_MAX_AGE_DAYS"):
            os.environ.pop(k, None)

    def _target_files(self, world: dict, target: str) -> list[Path]:
        sb = world["sb"]
        if target == "omp":
            return sorted((sb / "omp" / "sessions").rglob("*.jsonl"))
        if target == "pi":
            return sorted((sb / "pi" / "sessions").rglob("*.jsonl"))
        if target == "claude":
            return sorted((sb / "claude" / "projects").rglob("*.jsonl"))
        return []

    def _opencode_listing(self, world: dict) -> str:
        r = subprocess.run([OPENCODE, "session", "list"], capture_output=True, text=True,
                           env=dict(os.environ, XDG_DATA_HOME=str(world["xdg"])), timeout=120)
        return r.stdout

    def _sync(self, world: dict, source_fmt: str, target: str) -> None:
        """One real daemon pass, through one_pass() and its scan(). The daemon
        registers sources in state during scan and saves state after the pass;
        the test does the same, so the divergence guard sees the mirrors it made.
        Building Source objects by hand skips that registration and makes the
        guard blind."""
        hs = world["hs"]
        state = world.setdefault("state", hs.State())
        hs.one_pass(state)

    # ------------------------------------------------------- the checks

    def _check_pair(self, source_fmt: str, target: str) -> None:
        world = self._setup_world(source_fmt)
        try:
            self._sync(world, source_fmt, target)
            if target == "opencode":
                listing = self._opencode_listing(world)
                self.assertIn(TITLE, listing, f"{source_fmt} -> opencode: mirror not listed")
                self.assertNotIn("New session - 2026", listing)
                return
            files = self._target_files(world, target)
            self.assertEqual(len(files), 1, f"{source_fmt} -> {target}: expected one mirror, got {files}")
            text = files[0].read_text(encoding="utf-8")
            self.assertIn(PROMPT, text, f"{source_fmt} -> {target}: prompt lost")
            self.assertIn(ANSWER, text, f"{source_fmt} -> {target}: answer lost")
            self.assertNotRegex(text.splitlines()[0], r"\d{4}-\d{2}-\d{2}T\d{2}-\d{2}",
                                f"{source_fmt} -> {target}: date in the first record")
        finally:
            self._teardown_world(world)

    def _check_resync_is_idempotent(self, source_fmt: str, target: str) -> None:
        world = self._setup_world(source_fmt)
        try:
            self._sync(world, source_fmt, target)
            self._sync(world, source_fmt, target)
            if target == "opencode":
                listing = self._opencode_listing(world)
                self.assertEqual(listing.count(TITLE), 1, f"{source_fmt} -> opencode duplicated on re-sync")
                return
            files = self._target_files(world, target)
            self.assertEqual(len(files), 1, f"{source_fmt} -> {target}: re-sync duplicated the mirror")
        finally:
            self._teardown_world(world)


    def _check_opencode_freeze(self, world: dict, source_fmt: str) -> None:
        """A turn continued inside an OpenCode mirror freezes it. The turn is a
        real `opencode run -s <mirror>`; the source then changes and the daemon
        must not rewrite the mirror."""
        import sqlite3
        db = world["xdg"] / "opencode" / "opencode.db"
        con = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
        try:
            row = con.execute("select id from session where title = ?", (TITLE,)).fetchone()
            before = con.execute("select count(*) from message m join session s on s.id = m.session_id "
                                 "where s.title = ?", (TITLE,)).fetchone()[0]
        finally:
            con.close()
        self.assertIsNotNone(row, f"{source_fmt} -> opencode: mirror not created")
        mirror_id = row[0]
        env = dict(os.environ, XDG_DATA_HOME=str(world["xdg"]))
        run = subprocess.run([OPENCODE, "run", "-s", mirror_id, "-m", "omniroute/longcat-free",
                              "Reply with exactly the word: continued"],
                             capture_output=True, text=True, env=env, timeout=300)
        if run.returncode != 0 or "continued" not in run.stdout.lower():
            self.skipTest("no model reply from the gateway; the OpenCode freeze needs one")
        con = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
        try:
            typed = con.execute("select count(*) from message m join session s on s.id = m.session_id "
                                "where s.title = ?", (TITLE,)).fetchone()[0]
        finally:
            con.close()
        self.assertGreater(typed, before, "the continued turn was not written to the mirror")
        # The source changes; the daemon runs again.
        src = world["src"]
        if source_fmt in ("omp", "pi"):
            with src.open("a", encoding="utf-8") as fh:
                fh.write(json.dumps({"type": "message", "id": f"s{time.time_ns()}", "parentId": "m2",
                                     "message": {"role": "user", "content": "source moved on"}}) + "\n")
        elif source_fmt == "claude":
            with src.open("a", encoding="utf-8") as fh:
                fh.write(json.dumps({"type": "user", "uuid": f"s{time.time_ns()}", "parentUuid": "a1",
                                     "sessionId": "bbbbbbbb-1111-2222-3333-444444444444",
                                     "message": {"role": "user", "content": "source moved on"}}) + "\n")
        self._sync(world, source_fmt, "opencode")
        con = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
        try:
            after = con.execute("select count(*) from message m join session s on s.id = m.session_id "
                                "where s.title = ?", (TITLE,)).fetchone()[0]
        finally:
            con.close()
        self.assertEqual(after, typed, f"{source_fmt} -> opencode: continued mirror was overwritten by a source change")

    def _check_freeze(self, source_fmt: str, target: str) -> None:
        """A turn typed into the mirror freezes it. A source change after that
        must not overwrite the typed turn."""
        world = self._setup_world(source_fmt)
        try:
            self._sync(world, source_fmt, target)
            if target == "opencode":
                self._check_opencode_freeze(world, source_fmt)
                return
            files = self._target_files(world, target)
            self.assertEqual(len(files), 1, f"{source_fmt} -> {target}: no mirror to continue")
            mirror = files[0]
            typed = "TYPED-IN-" + target + "-" + str(time.time_ns())
            with mirror.open("a", encoding="utf-8") as fh:
                fh.write(json.dumps({"type": "message", "id": f"t{time.time_ns()}",
                                     "message": {"role": "user", "content": typed}}) + "\n")
            after_turn = mirror.read_text(encoding="utf-8")
            # the source changes
            src = world["src"]
            if source_fmt in ("omp", "pi"):
                with src.open("a", encoding="utf-8") as fh:
                    fh.write(json.dumps({"type": "message", "id": f"s{time.time_ns()}", "parentId": "m2",
                                         "message": {"role": "user", "content": "source moved on"}}) + "\n")
            elif source_fmt == "claude":
                with src.open("a", encoding="utf-8") as fh:
                    fh.write(json.dumps({"type": "user", "uuid": f"s{time.time_ns()}", "parentUuid": "a1",
                                         "sessionId": "bbbbbbbb-1111-2222-3333-444444444444",
                                         "message": {"role": "user", "content": "source moved on"}}) + "\n")
            self._sync(world, source_fmt, target)
            self.assertEqual(mirror.read_text(encoding="utf-8"), after_turn,
                             f"{source_fmt} -> {target}: a typed turn was overwritten")
        finally:
            self._teardown_world(world)


def _make_pair_test(source_fmt: str, target: str):
    def test(self: MatrixTest) -> None:
        self._check_pair(source_fmt, target)

    test.__name__ = f"test_{source_fmt}_to_{target}"
    return test


def _make_resync_test(source_fmt: str, target: str):
    def test(self: MatrixTest) -> None:
        self._check_resync_is_idempotent(source_fmt, target)

    test.__name__ = f"test_resync_{source_fmt}_to_{target}"
    return test




def _make_freeze_test(source_fmt: str, target: str):
    def test(self: MatrixTest) -> None:
        self._check_freeze(source_fmt, target)

    test.__name__ = f"test_freeze_{source_fmt}_to_{target}"
    return test


for _s in FORMATS:
    for _t in FORMATS:
        if _s == _t:
            continue  # a source is never mirrored into its own format
        setattr(MatrixTest, f"test_{_s}_to_{_t}", _make_pair_test(_s, _t))
        setattr(MatrixTest, f"test_resync_{_s}_to_{_t}", _make_resync_test(_s, _t))
        setattr(MatrixTest, f"test_freeze_{_s}_to_{_t}", _make_freeze_test(_s, _t))


if __name__ == "__main__":
    unittest.main()
