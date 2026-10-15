"""Title rules: a mirror name must never carry a date that is not the user's.

Covers O-penCode's default 'New session - <timestamp>' names, which must be
treated as untitled, and keeps real titles unchanged.
"""
from __future__ import annotations

import os
import sys
import unittest
from pathlib import Path

DAEMON_DIR = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(DAEMON_DIR))
os.environ.setdefault("HARNESS_SYNC_DIR", "/tmp/hs-title-test-state")
import harness_sync as hs  # noqa: E402


class OpenCodeTitleTest(unittest.TestCase):
    def test_default_timestamp_title_is_untitled(self) -> None:
        self.assertEqual(hs.clean_opencode_title("New session - 2026-10-14T16:52:39.241Z"), "")

    def test_default_title_with_other_precision_is_untitled(self) -> None:
        self.assertEqual(hs.clean_opencode_title("New session - 2026-09-21T16:29:28.123Z"), "")

    def test_real_title_is_kept(self) -> None:
        self.assertEqual(hs.clean_opencode_title("Research harness stores"), "Research harness stores")

    def test_title_that_mentions_a_date_is_kept(self) -> None:
        # Only the exact O-penCode default pattern is stripped. A user title
        # that happens to contain a date must survive.
        self.assertEqual(hs.clean_opencode_title("Plan for 2026-10-14"), "Plan for 2026-10-14")

    def test_missing_title_is_empty(self) -> None:
        self.assertEqual(hs.clean_opencode_title(None), "")
        self.assertEqual(hs.clean_opencode_title("   "), "")


class ClaudeTitleTest(unittest.TestCase):
    def test_ai_title_beats_first_prompt(self) -> None:
        import json
        import tempfile
        d = Path(tempfile.mkdtemp())
        sid = "ec447302-155c-4280-9c34-2b5122fcd4a4"
        p = d / f"{sid}.jsonl"
        rows = [
            {"type": "user", "isMeta": False, "message": {"role": "user", "content": "This session is being continued from a previous conversation"}},
            {"type": "ai-title", "aiTitle": "Configure homelab MCP servers", "sessionId": sid},
            {"type": "user", "message": {"role": "user", "content": "hi there"}},
        ]
        p.write_text("\n".join(json.dumps(r) for r in rows) + "\n", encoding="utf-8")
        _, _, title, has_user = hs.claude_session_meta(p)
        self.assertEqual(title, "Configure homelab MCP servers")
        self.assertTrue(has_user)

    def test_continuation_banner_is_never_the_title(self) -> None:
        import json
        import tempfile
        d = Path(tempfile.mkdtemp())
        sid = "660c446d-5cc4-4508-92ca-5e725c622147"
        p = d / f"{sid}.jsonl"
        rows = [
            {"type": "user", "message": {"role": "user", "content": "This session is being continued from a previous conversation"}},
            {"type": "user", "message": {"role": "user", "content": "hi"}},
        ]
        p.write_text("\n".join(json.dumps(r) for r in rows) + "\n", encoding="utf-8")
        _, _, title, _ = hs.claude_session_meta(p)
        self.assertNotIn("continued", title)
        self.assertEqual(title, "hi")


if __name__ == "__main__":
    unittest.main()
