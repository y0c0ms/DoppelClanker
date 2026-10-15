"""Rename: each format is renamed, only the name changes, and bad input is refused.

Uses the real hslock binary (HSLOCK) with an isolated lock directory, so the
conversation lock runs for real. Every store lives in a temporary sandbox.
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
import unittest.mock
from pathlib import Path

DAEMON_DIR = Path(__file__).resolve().parent.parent
HSLOCK = Path(os.environ.get("HSLOCK", Path.home() / "bin" / "hslock"))
OPENCODE = shutil.which("opencode")
CLAUDE = shutil.which("claude")
SID = "aaaaaaaa-1111-2222-3333-444444444444"
CID = "bbbbbbbb-1111-2222-3333-444444444444"


def _omp_slot(title: str) -> str:
    s = json.dumps({"type": "title", "v": 1, "title": title, "source": "auto",
                    "updatedAt": "2026-10-01T10:00:00.000Z"})
    return s + " " * (255 - len(s.encode("utf-8")))


@unittest.skipUnless(HSLOCK.exists(), f"hslock not built at {HSLOCK}")
class RenameTest(unittest.TestCase):
    def setUp(self) -> None:
        self.sb = Path(tempfile.mkdtemp(prefix="hs-rename-"))
        self.omp = self.sb / "omp" / "sessions" / "-tmp"
        self.pi = self.sb / "pi" / "sessions" / "-tmp"
        self.claude = self.sb / "claude" / "projects" / "-tmp"
        for d in (self.omp, self.pi, self.claude):
            d.mkdir(parents=True)
        os.environ.update({
            "HARNESS_SYNC_DIR": str(self.sb / "state"), "OMP_AGENT_DIR": str(self.sb / "omp"),
            "PI_AGENT_DIR": str(self.sb / "pi"), "CLAUDE_CONFIG_DIR": str(self.sb / "claude"),
            "OPENCODE_DB": str(self.sb / "none.db"), "HSLOCK": str(HSLOCK),
            "HARNESS_SYNC_LOCK_DIR": str(self.sb / "locks"),
        })
        sys.path.insert(0, str(DAEMON_DIR))
        for m in ("harness_sync", "hs_manage"):
            sys.modules.pop(m, None)
        self.hm = importlib.import_module("hs_manage")

    def tearDown(self) -> None:
        shutil.rmtree(self.sb, ignore_errors=True)
        for k in ("HARNESS_SYNC_DIR", "OMP_AGENT_DIR", "PI_AGENT_DIR", "CLAUDE_CONFIG_DIR",
                  "OPENCODE_DB", "HSLOCK", "HARNESS_SYNC_LOCK_DIR"):
            os.environ.pop(k, None)

    # ------------------------------------------------------------ omp

    def test_omp_slot_is_renamed_and_messages_are_untouched(self) -> None:
        p = self.omp / f"2026-10-01T10-00-00-000Z_{SID}.jsonl"
        body = [{"type": "session", "version": 3, "id": SID, "cwd": "/tmp", "title": "Old"},
                {"type": "message", "id": "m1", "parentId": None,
                 "message": {"role": "user", "content": "keep this exact text"}}]
        p.write_text("\n".join([_omp_slot("Old")] + [json.dumps(x) for x in body]) + "\n", encoding="utf-8")
        before = p.read_text(encoding="utf-8").splitlines()[1:]

        saved = self.hm.rename_group(self.hm.Group(fmt="omp", key=str(p), title="Old"), "  New name  ")

        lines = p.read_text(encoding="utf-8").splitlines()
        self.assertEqual(saved, "New name")
        self.assertEqual(json.loads(lines[0])["title"], "New name")
        self.assertEqual(len(lines[0].encode("utf-8")), 255, "title slot must keep its fixed size")
        self.assertEqual(lines[1:], before, "message records must be byte-identical")

    def test_omp_title_is_what_the_daemon_lists(self) -> None:
        import harness_sync as hs
        p = self.omp / f"2026-10-01T10-00-00-000Z_{SID}.jsonl"
        body = [{"type": "session", "version": 3, "id": SID, "cwd": "/tmp", "title": "Header name"},
                {"type": "message", "id": "m1", "parentId": None, "message": {"role": "user", "content": "x"}}]
        p.write_text("\n".join([_omp_slot("Slot name")] + [json.dumps(x) for x in body]) + "\n", encoding="utf-8")
        self.assertEqual(hs.jsonl_session_meta(p)[2], "Slot name",
                         "the title slot must win over an older header title")
        self.hm.rename_group(self.hm.Group(fmt="omp", key=str(p), title="Slot name"), "Renamed")
        self.assertEqual(hs.jsonl_session_meta(p)[2], "Renamed")

    def test_omp_slot_with_real_padding_is_renamed(self) -> None:
        """Real omp slots carry a large 'pad' field. Keeping it on rewrite made the
        new slot overflow 255 bytes, and the rename was refused on real files.
        The slot here mirrors a real one: padding is most of its 255 bytes."""
        p = self.omp / f"2026-10-01T10-00-00-000Z_{SID}.jsonl"
        head = {"type": "title", "v": 1, "title": "Main agent", "source": "user",
                "updatedAt": "2026-07-21T16:58:21.000Z"}
        # Size the pad the way omp does: whatever brings the JSON to 255 bytes.
        base = json.dumps(dict(head, pad=""))
        slot = json.dumps(dict(head, pad=" " * (255 - len(base.encode("utf-8")))))
        self.assertEqual(len(slot.encode("utf-8")), 255)
        body = [{"type": "session", "version": 3, "id": SID, "cwd": "/tmp", "title": "Main agent"},
                {"type": "message", "id": "m1", "parentId": None,
                 "message": {"role": "user", "content": "real body"}}]
        p.write_text("\n".join([slot] + [json.dumps(x) for x in body]) + "\n", encoding="utf-8")
        before = p.read_text(encoding="utf-8").splitlines()[1:]

        saved = self.hm.rename_group(self.hm.Group(fmt="omp", key=str(p), title="Main agent"), "Renamed real")

        lines = p.read_text(encoding="utf-8").splitlines()
        self.assertEqual(saved, "Renamed real")
        self.assertEqual(json.loads(lines[0])["title"], "Renamed real")
        self.assertEqual(len(lines[0].encode("utf-8")), 255)
        self.assertEqual(lines[1:], before)

    def test_omp_slot_matches_omps_own_layout(self) -> None:
        """omp writes the slot as compact JSON, keys in the order type, v, title,
        source, updatedAt, pad, exactly 255 bytes, pad last. A rename must write
        the same layout, or omp can read it differently."""
        p = self.omp / f"2026-10-01T10-00-00-000Z_{SID}.jsonl"
        base = {"type": "title", "v": 1, "title": "Layout", "source": "auto",
                "updatedAt": "2026-10-01T10:00:00.000Z"}
        empty = json.dumps(dict(base, pad=""), separators=(",", ":"))
        slot = json.dumps(dict(base, pad=" " * (255 - len(empty.encode("utf-8")))), separators=(",", ":"))
        self.assertEqual(len(slot.encode("utf-8")), 255)
        p.write_text(slot + "\n" + json.dumps({"type": "session", "version": 3, "id": SID,
                                                 "cwd": "/tmp", "title": "Layout"}) + "\n", encoding="utf-8")
        self.hm.rename_group(self.hm.Group(fmt="omp", key=str(p), title="Layout"), "Named by user")
        line = p.read_text(encoding="utf-8").split("\n")[0]
        self.assertEqual(len(line.encode("utf-8")), 255)
        self.assertEqual(list(json.loads(line).keys()), ["type", "v", "title", "source", "updatedAt", "pad"])
        self.assertIn('"source":"user"', line, "a rename must mark the title as user-set, as omp does")
        self.assertIn('"title":"Named by user"', line, "omp uses compact JSON; no spaces")

    # ------------------------------------------------------------- pi

    def test_pi_gets_a_new_session_info_name_and_keeps_history(self) -> None:
        p = self.pi / f"2026-10-01T10-00-00-000Z_{SID}.jsonl"
        lines = [{"type": "session", "version": 3, "id": SID, "cwd": "/tmp", "title": "Old"},
                 {"type": "message", "id": "m1", "parentId": None, "message": {"role": "user", "content": "pi text"}}]
        p.write_text("\n".join(json.dumps(x) for x in lines) + "\n", encoding="utf-8")
        originals = p.read_text(encoding="utf-8").splitlines()

        self.hm.rename_group(self.hm.Group(fmt="pi", key=str(p), title="Old"), "Pi renamed")

        after = p.read_text(encoding="utf-8").splitlines()
        self.assertEqual(after[:len(originals)], originals, "existing records must not change")
        self.assertEqual(json.loads(after[-1])["name"], "Pi renamed")

    # --------------------------------------------------------- claude

    def test_claude_gets_an_ai_title_and_keeps_the_transcript(self) -> None:
        p = self.claude / f"{CID}.jsonl"
        p.write_text(json.dumps({"type": "user", "uuid": "u1", "sessionId": CID,
                                 "message": {"role": "user", "content": "claude text"}}) + "\n", encoding="utf-8")
        original = p.read_text(encoding="utf-8")

        self.hm.rename_group(self.hm.Group(fmt="claude", key=str(p), title="?"), "Claude renamed")

        after = p.read_text(encoding="utf-8")
        self.assertTrue(after.startswith(original), "transcript must not be rewritten")
        last = json.loads(after.splitlines()[-1])
        # Claude Code's /rename and -n write custom-title, and its reader prefers it
        # over ai-title. This is a file-format contract only; it does NOT prove that
        # `claude --resume` shows the name. That check is not built yet.
        self.assertEqual(last["type"], "custom-title")
        self.assertEqual(last["customTitle"], "Claude renamed")

    @unittest.skipUnless(CLAUDE, "claude CLI not installed")
    @unittest.skipUnless(sys.platform != "win32", "resume picker check drives a POSIX pty")
    def test_claude_resume_picker_shows_the_renamed_title(self) -> None:
        """Real check: after a rename, `claude --resume` must list the new name.
        A session that already has an ai-title is renamed; the picker must show the
        new name, not the old one. This runs the real Claude Code binary."""
        import pty
        import re
        import select
        import signal

        cfg = self.sb / "claude"  # the same root setUp gives the daemon as CLAUDE_CONFIG_DIR
        ws = self.sb / "ws"
        ws.mkdir()
        (cfg / "projects").mkdir(parents=True, exist_ok=True)
        (cfg / ".claude.json").write_text(json.dumps({"hasCompletedOnboarding": True, "numStartups": 5,
                                                      "projects": {}}), encoding="utf-8")
        key = re.sub(r"[^a-zA-Z0-9]", "-", str(ws.resolve()))
        proj = cfg / "projects" / key
        proj.mkdir(parents=True)
        p = proj / f"{CID}.jsonl"
        p.write_text(
            json.dumps({"type": "user", "uuid": "u1", "sessionId": CID, "cwd": str(ws.resolve()),
                        "timestamp": "2026-10-01T10:00:00.000Z",
                        "message": {"role": "user", "content": "hello rename probe"}}) + "\n"
            + json.dumps({"type": "assistant", "uuid": "a1", "parentUuid": "u1", "sessionId": CID,
                          "cwd": str(ws.resolve()), "timestamp": "2026-10-01T10:00:01.000Z",
                          "message": {"role": "assistant", "content": [{"type": "text", "text": "hi"}]}}) + "\n"
            # A custom-title, as /rename or -n writes. Claude prefers it over ai-title,
            # so a rename that only writes ai-title would not show in the picker.
            + json.dumps({"type": "custom-title", "customTitle": "Old generated name", "sessionId": CID}) + "\n",
            encoding="utf-8")

        self.hm.rename_group(self.hm.Group(fmt="claude", key=str(p), title="Old generated name"),
                             "Renamed for resume")

        env = dict(os.environ, CLAUDE_CONFIG_DIR=str(cfg), TERM="xterm-256color",
                   COLUMNS="200", LINES="60")
        env.pop("ANTHROPIC_API_KEY", None)
        pid, fd = pty.fork()
        if pid == 0:  # child
            os.chdir(str(ws))
            os.execvpe(CLAUDE, [CLAUDE, "--resume"], env)
        found = False
        try:
            buf = b""
            sent_trust = False
            deadline = time.monotonic() + 60
            clean = lambda b: re.sub(rb"\x1b\[[0-9;?>=<]*[A-Za-z]", b"", b).decode("utf-8", "replace")
            while time.monotonic() < deadline:
                r, _, _ = select.select([fd], [], [], 0.5)
                if r:
                    try:
                        buf += os.read(fd, 65536)
                    except OSError:
                        break
                text = clean(buf)
                if not sent_trust and "trust" in text.lower():
                    os.write(fd, b"\r")  # accept the workspace trust prompt
                    sent_trust = True
                    buf = b""
                    continue
                # The TUI drops spaces in its output, so compare with them removed.
                if "Renamed for resume".replace(" ", "") in text.replace(" ", ""):
                    found = True
                    break
            screen = clean(buf)
        finally:
            try:
                os.kill(pid, signal.SIGKILL)
                os.waitpid(pid, 0)
            except (ProcessLookupError, ChildProcessError, OSError):
                pass
        self.assertTrue(found, "claude --resume did not show the renamed title. Last screen:\n"
                        + screen[-800:])
        self.assertNotIn("Old generated name".replace(" ", ""),
                         screen.replace(" ", "").replace("Renamed for resume".replace(" ", ""), ""),
                         "the old name must not be the one shown")

    # --------------------------------------------------------- refusals

    def test_empty_and_control_character_titles_are_refused(self) -> None:
        for bad in ("", "   ", "bad\x07char", "line\nbreak"):
            with self.subTest(bad=bad):
                with self.assertRaises(self.hm.DeleteRefused):
                    self.hm.validate_title(bad)

    def test_title_over_80_characters_is_refused(self) -> None:
        with self.assertRaises(self.hm.DeleteRefused):
            self.hm.validate_title("x" * 81)
        self.assertEqual(self.hm.validate_title("x" * 80), "x" * 80)

    def test_opencode_rename_refuses_unexpected_ids_without_starting_a_server(self) -> None:
        with self.assertRaises(self.hm.DeleteRefused):
            self.hm.rename_group(self.hm.Group(fmt="opencode", key="ses_abc; rm -rf /", title="x"), "New")

    @unittest.skipUnless(OPENCODE, "opencode CLI not installed")
    def test_opencode_session_is_renamed_through_the_api_and_server_is_stopped(self) -> None:
        import subprocess
        env = dict(os.environ, XDG_DATA_HOME=str(self.sb / "oc-data"), XDG_CONFIG_HOME=str(self.sb / "oc-cfg"),
                   XDG_STATE_HOME=str(self.sb / "oc-state"), XDG_CACHE_HOME=str(self.sb / "oc-cache"))
        port = self.hm._free_port()
        base = f"http://127.0.0.1:{port}"
        srv = subprocess.Popen([OPENCODE, "serve", "--port", str(port), "--hostname", "127.0.0.1"],
                               env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                               stderr=subprocess.DEVNULL, start_new_session=True)
        try:
            sid = None
            import time as _t
            deadline = _t.monotonic() + 60
            while sid is None:
                try:
                    created = self.hm._opencode_request(base, "POST", "/session", {"title": "Before"})
                    sid = created["id"]
                except OSError:
                    if _t.monotonic() > deadline:
                        self.fail("opencode serve did not start")
                    _t.sleep(0.25)
            self.hm._stop_server(srv)  # the server must be stopped before rename starts its own
        except BaseException:
            self.hm._stop_server(srv)
            raise

        # Rename runs against an isolated server it starts itself, using the same data dir.
        with unittest.mock.patch.dict(os.environ, env):
            saved = self.hm.rename_group(self.hm.Group(fmt="opencode", key=sid, title="Before"), "  After  ")
        self.assertEqual(saved, "After")

        # Verify with a fresh server on the same data dir, then confirm it is stopped.
        port2 = self.hm._free_port()
        base2 = f"http://127.0.0.1:{port2}"
        srv2 = subprocess.Popen([OPENCODE, "serve", "--port", str(port2), "--hostname", "127.0.0.1"],
                                env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                stderr=subprocess.DEVNULL, start_new_session=True)
        try:
            deadline = _t.monotonic() + 60
            while True:
                try:
                    got = self.hm._opencode_request(base2, "GET", f"/session/{sid}")
                    break
                except OSError:
                    if _t.monotonic() > deadline:
                        self.fail("verification server did not start")
                    _t.sleep(0.25)
            self.assertEqual(got["title"], "After")
            self.hm._opencode_request(base2, "DELETE", f"/session/{sid}")
        finally:
            self.hm._stop_server(srv2)
        self.assertIsNotNone(srv2.poll(), "verification server must be stopped")

    @unittest.skipUnless(OPENCODE, "opencode CLI not installed")
    def test_rename_server_is_stopped_after_a_failed_rename(self) -> None:
        started: list[int] = []
        real_popen = subprocess.Popen

        def tracking_popen(*a, **kw):
            p = real_popen(*a, **kw)
            started.append(p)
            return p

        with unittest.mock.patch.object(self.hm.subprocess, "Popen", side_effect=tracking_popen):
            with self.assertRaises(self.hm.DeleteRefused):
                self.hm._rewrite_opencode_title("ses_doesnotexist123", "x")  # 404 from the server
        self.assertTrue(started, "a server should have been started")
        self.assertIsNotNone(started[-1].poll(), "server must be stopped even when the rename fails")

    def test_source_outside_session_roots_is_refused(self) -> None:
        outside = self.sb / "elsewhere.jsonl"
        outside.write_text('{"type":"user"}\n', encoding="utf-8")
        with self.assertRaises(self.hm.DeleteRefused):
            self.hm.rename_group(self.hm.Group(fmt="pi", key=str(outside), title="x"), "New")
        self.assertEqual(outside.read_text(encoding="utf-8"), '{"type":"user"}\n',
                         "a refused rename must not touch the file")

    def test_refused_rename_leaves_the_lock_released(self) -> None:
        p = self.pi / f"2026-10-01T10-00-00-000Z_{SID}.jsonl"
        p.write_text('{"type":"session","id":"x"}\n{"type":"message","message":{"role":"user","content":"a"}}\n',
                     encoding="utf-8")
        with self.assertRaises(self.hm.DeleteRefused):
            self.hm.rename_group(self.hm.Group(fmt="pi", key=str(p), title="x"), "")
        self.assertFalse((self.sb / "locks" / f"{SID}.lock").exists(),
                         "a refused rename must not leave the conversation locked")


if __name__ == "__main__":
    unittest.main()
