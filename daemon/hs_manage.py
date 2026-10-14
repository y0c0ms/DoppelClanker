"""harness-sync management data layer: list and delete session groups.

A group is one source conversation plus every mirror the daemon made of it.
This module is the only place that deletes files or OpenCode sessions for the
TUI. It reuses the daemon's own discovery, title, and state code so the two
cannot disagree about what a session is called or which files belong to it.

Deletion rules:
  * The conversation's lock is claimed for the whole delete (as "tui"), so a
    harness cannot start a turn on it mid-delete. If the lock is held, or the
    lock binary is missing, the delete is refused. Deletion fails closed.
  * File deletes are limited to the session roots of the three file stores.
    Any path outside them is refused.
  * Mirrors go first and the source last. If something fails partway, the
    source still exists and the group can be deleted again.
"""
from __future__ import annotations

import json
import os
import signal
import socket
import subprocess
import time
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path

import harness_sync as hs


class DeleteRefused(RuntimeError):
    """A delete was refused before anything was removed."""


@dataclass
class MirrorRef:
    target: str           # "omp" | "pi" | "claude" | "opencode"
    ref: str              # file path, or session id for opencode
    diverged: bool = False


@dataclass
class Group:
    fmt: str              # "omp" | "pi" | "claude" | "opencode"
    key: str              # file path (file stores) or session id (opencode)
    title: str
    updated: float = 0.0
    mirrors: list[MirrorRef] = field(default_factory=list)
    orphan: bool = False  # in state but no longer on disk

    @property
    def lock_id(self) -> str:
        if self.fmt == "opencode":
            return self.key
        return hs.source_lock_id(hs.Source(fmt=self.fmt, key=self.key))

    @property
    def label(self) -> str:
        return f"{self.fmt}: {self.title}"


def lock_dir() -> Path:
    """Same rule as hslock (lock/src/main.rs lock_dir)."""
    env = os.environ.get("HARNESS_SYNC_LOCK_DIR")
    return Path(env) if env else hs.STATE_DIR / "locks"


def _file_roots() -> list[Path]:
    return [hs.OMP_AGENT / "sessions", hs.PI_AGENT / "sessions", hs.CLAUDE_DIR / "projects"]


def _under_roots(path: Path) -> bool:
    try:
        resolved = path.resolve()
    except OSError:
        return False
    return any(resolved.is_relative_to(root.resolve()) for root in _file_roots() if root.exists())


def list_groups(state: hs.State) -> list[Group]:
    """Every source on disk plus any orphaned source still in state, with the
    mirrors recorded for each. Mirror files are never listed as sources."""
    mirror_refs = {m.path for s in state.sources.values() for m in s.mirrors.values()}
    groups: dict[str, Group] = {}

    def add(fmt: str, key: str, title: str, updated: float) -> None:
        if key in state.mirror_keys or key in mirror_refs:
            return
        groups[key] = Group(fmt=fmt, key=key, title=title or "untitled session", updated=updated)

    for fmt, agent in (("omp", hs.OMP_AGENT), ("pi", hs.PI_AGENT)):
        for p in hs.jsonl_sessions(agent):
            add(fmt, str(p), hs.jsonl_session_meta(p)[2], p.stat().st_mtime)
    for p in hs.claude_sessions():
        meta = hs.claude_session_meta(p)
        add("claude", str(p), meta[2], p.stat().st_mtime)
    if hs.OPENCODE_DB.exists():
        for sid, updated, _directory, title in hs.opencode_sessions():
            add("opencode", sid, title, updated)

    for key, src in state.sources.items():
        if key not in groups:
            # Source recorded in state but gone from disk: keep it visible so
            # its leftover mirrors can still be cleaned up.
            if key in state.mirror_keys:
                continue
            groups[key] = Group(fmt=src.fmt, key=key, title=Path(key).stem if src.fmt != "opencode" else key,
                                updated=src.last_seen_change, orphan=True)
        g = groups[key]
        g.mirrors = [MirrorRef(t, m.path, m.diverged) for t, m in src.mirrors.items()]

    return sorted(groups.values(), key=lambda g: g.updated, reverse=True)


@dataclass
class DuplicateSet:
    """Sessions that look like copies of one conversation. `keep` is the one
    suggested to keep (the most recently updated); `remove` are suggested
    deletions. `reason` says why they matched, so the user can judge."""
    reason: str
    keep: Group
    remove: list[Group]


def _content_fingerprint(path: Path) -> str:
    """Hash of the conversational content, ignoring the header/timestamps
    that differ between copies. Identical messages -> identical value."""
    import hashlib
    import json
    h = hashlib.sha256()
    try:
        with path.open(encoding="utf-8", errors="replace") as fh:
            for line in fh:
                try:
                    rec = json.loads(line)
                except json.JSONDecodeError:
                    continue
                kind = rec.get("type")
                if kind in ("message", "user", "assistant"):
                    msg = rec.get("message") or {}
                    h.update(json.dumps([kind, msg.get("role"), msg.get("content")], sort_keys=True).encode())
    except OSError:
        return ""
    return h.hexdigest()


def find_duplicates(groups: list[Group]) -> list[DuplicateSet]:
    """Suggest duplicate sets. Only strong evidence counts:

      * same session id (one conversation stored twice), or
      * identical conversational content (messages match exactly).

    Matching on title or first prompt is deliberately NOT used: generic
    prompts like "hi" repeat across unrelated sessions and would produce
    false suggestions. Nothing is deleted here; the caller decides."""
    from collections import defaultdict

    by_id: dict[str, list[Group]] = defaultdict(list)
    by_content: dict[str, list[Group]] = defaultdict(list)
    for g in groups:
        if g.orphan:
            continue
        by_id[g.lock_id].append(g)
        if g.fmt != "opencode":
            fp = _content_fingerprint(Path(g.key))
            if fp:
                by_content[fp].append(g)

    sets: list[DuplicateSet] = []
    seen: set[str] = set()
    for reason, index in (("same session id", by_id), ("identical content", by_content)):
        for members in index.values():
            if len(members) < 2:
                continue
            members = sorted(members, key=lambda g: g.updated, reverse=True)
            keys = {m.key for m in members}
            if keys & seen:
                continue
            seen |= keys
            sets.append(DuplicateSet(reason=reason, keep=members[0], remove=members[1:]))
    return sets


TRIVIAL_MAX_USER_TURNS = 2       # at most this many real user prompts
TRIVIAL_MAX_TOOL_CALLS = 0       # any tool use means real work was done
TRIVIAL_MAX_BYTES = 10_000       # bigger files hold more than a quick check
TRIVIAL_MIN_IDLE_DAYS = 7        # recent sessions are never suggested


@dataclass
class TrivialSession:
    group: Group
    user_turns: int
    tool_calls: int
    idle_days: float


def _activity(path: Path, fmt: str) -> tuple[int, int]:
    """(real user prompts, tool calls) for an omp or Claude journal."""
    import json
    users = tools = 0
    try:
        with path.open(encoding="utf-8", errors="replace") as fh:
            for line in fh:
                try:
                    rec = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if fmt == "omp" and rec.get("type") == "message":
                    msg = rec.get("message") or {}
                    if msg.get("role") == "user":
                        users += _has_text(msg.get("content"))
                    elif msg.get("role") == "assistant" and isinstance(msg.get("content"), list):
                        tools += sum(1 for b in msg["content"] if isinstance(b, dict) and b.get("type") == "toolCall")
                elif fmt == "claude":
                    if rec.get("type") == "user" and not rec.get("isMeta"):
                        users += _has_text((rec.get("message") or {}).get("content"))
                    elif rec.get("type") == "assistant":
                        c = (rec.get("message") or {}).get("content")
                        if isinstance(c, list):
                            tools += sum(1 for b in c if isinstance(b, dict) and b.get("type") == "tool_use")
    except OSError:
        return 0, 0
    return users, tools


def _has_text(content) -> int:
    """1 if the content holds a real prompt (not a <tag> wrapper), else 0."""
    if isinstance(content, str):
        text = content.strip()
    elif isinstance(content, list):
        text = " ".join(b.get("text", "") for b in content
                        if isinstance(b, dict) and b.get("type") == "text").strip()
    else:
        return 0
    return 1 if text and not text.startswith("<") else 0


def find_trivial(groups: list[Group], now: float | None = None) -> list[TrivialSession]:
    """Sessions that look like throwaway calls: almost no prompts, no tool
    use, small, and not touched for a while. Suggestions only; the caller
    deletes through the normal confirmed path. OpenCode sessions are not
    checked here (their store is SQLite, not a file)."""
    import time
    now = time.time() if now is None else now
    out: list[TrivialSession] = []
    for g in groups:
        if g.orphan or g.fmt == "opencode":
            continue
        p = Path(g.key)
        try:
            st = p.stat()
        except OSError:
            continue
        idle = (now - st.st_mtime) / 86400
        if idle < TRIVIAL_MIN_IDLE_DAYS or st.st_size > TRIVIAL_MAX_BYTES:
            continue
        users, tools = _activity(p, g.fmt)
        if users <= TRIVIAL_MAX_USER_TURNS and tools <= TRIVIAL_MAX_TOOL_CALLS:
            out.append(TrivialSession(g, users, tools, idle))
    return sorted(out, key=lambda t: t.idle_days, reverse=True)


def _remove_lock_alias(mirror_id: str) -> None:
    safe = "".join(c for c in mirror_id if c.isalnum() or c in "-_")
    alias = hs.LOCK_DIR / "map" / safe
    if alias.exists():
        alias.unlink()


def _delete_mirror(state: hs.State, m: MirrorRef) -> None:
    if m.target == "opencode":
        _delete_opencode_session(m.ref)
        _remove_lock_alias(m.ref)
    else:
        path = Path(m.ref)
        if not _under_roots(path):
            raise DeleteRefused(f"mirror outside session roots, not deleting: {path}")
        if path.exists():
            path.unlink()
        _remove_lock_alias(path.stem.rsplit("_", 1)[-1])
    state.mirror_keys.discard(m.ref)


def _delete_opencode_session(sid: str) -> None:
    if not (sid.startswith("ses_") and sid.replace("_", "").isalnum()):
        raise DeleteRefused(f"refusing to pass unexpected id to opencode: {sid!r}")
    r = hs.run([hs.OPENCODE, "session", "delete", sid])
    if r.returncode != 0:
        raise RuntimeError(f"opencode session delete {sid} failed: {(r.stderr or r.stdout).strip()[-300:]}")


def _claim(group: Group) -> None:
    lock = hs.HSLOCK
    if not Path(lock).exists():
        raise DeleteRefused(f"lock binary not found at {lock}; refusing to delete without the lock")
    r = hs.run([lock, "claim", group.lock_id, "tui", str(os.getpid()), group.key])
    if r.returncode == 2:
        raise DeleteRefused(f"conversation is in use by {r.stdout.strip()}; try again when its turn ends")
    if r.returncode != 0:
        raise DeleteRefused(f"could not claim the conversation lock (rc={r.returncode})")


def _release(group: Group) -> None:
    hs.run([hs.HSLOCK, "release", group.lock_id, "tui", str(os.getpid())])


def delete_group(group: Group) -> list[str]:
    """Delete a source and all its mirors. Returns a log of what was removed."""
    done: list[str] = []
    _claim(group)
    try:
        # Reload state under the lock so a daemon save made since the list was
        # built is not overwritten by stale data.
        state = hs.State()
        for m in group.mirrors:
            _delete_mirror(state, m)
            done.append(f"mirror {m.target}: {m.ref}")
        if group.fmt == "opencode":
            _delete_opencode_session(group.key)
        elif not group.orphan:
            path = Path(group.key)
            if not _under_roots(path):
                raise DeleteRefused(f"source outside session roots, not deleting: {path}")
            if path.exists():
                path.unlink()
        done.append(f"source {group.fmt}: {group.key}")
        state.sources.pop(group.key, None)
        state.mirror_keys.discard(group.key)
        state.save()
    finally:
        _release(group)
    return done


# ------------------------------------------------------------------ rename
#
# Rename changes the name a session is listed under. It writes only the
# title field of the source's own store, never message content, and it runs
# under the same conversation lock as a harness turn, so a rename cannot race
# a write. Mirrors pick the new name up on the daemon's next pass, because the
# name is part of what they are built from.

MAX_TITLE_CHARS = 80


def validate_title(title: str) -> str:
    """Trim and check a new name. Refuses empty, control characters, and
    anything longer than the picker can show."""
    t = (title or "").strip()
    if not t:
        raise DeleteRefused("title is empty")
    if any(ord(c) < 32 or ord(c) == 127 for c in t):
        raise DeleteRefused("title contains control characters")
    if len(t) > MAX_TITLE_CHARS:
        raise DeleteRefused(f"title is longer than {MAX_TITLE_CHARS} characters")
    return t


def _rewrite_omp_title(path: Path, title: str) -> None:
    """omp keeps the name in a fixed 255-byte title slot on line 1. The slot is
    rebuilt with the same size, so the rest of the file is untouched."""
    import json
    lines = path.read_text(encoding="utf-8").split("\n")
    head = json.loads(lines[0]) if lines and lines[0].startswith("{") else {}
    if head.get("type") != "title":
        raise DeleteRefused("omp file has no title slot; not renaming")
    # omp's own layout: compact JSON with the keys in the order type, v, title,
    # source, updatedAt, pad, where `pad` is a string of spaces that brings the
    # whole record to exactly 255 bytes. The padding is INSIDE the JSON, not
    # after it, so the file matches what omp writes.
    order = ["type", "v", "title", "source", "updatedAt"]
    base = {k: head[k] for k in order if k in head}
    base["title"] = title
    base["source"] = "user"
    empty = json.dumps(dict(base, pad=""), ensure_ascii=False, separators=(",", ":"))
    room = 255 - len(empty.encode("utf-8"))
    if room < 0:
        raise DeleteRefused("title is too long for the omp title slot")
    lines[0] = json.dumps(dict(base, pad=" " * room), ensure_ascii=False, separators=(",", ":"))
    if len(lines[0].encode("utf-8")) != 255:
        raise DeleteRefused("could not build a 255-byte title slot")
    _atomic_write(path, "\n".join(lines))


def _rewrite_pi_title(path: Path, title: str) -> None:
    """Pi reads the name from the last session_info record. Append one; the
    older records stay as history."""
    import json
    import time
    rec = {"type": "session_info", "id": f"rn{time.time_ns():x}"[:12],
           "timestamp": time.strftime("%Y-%m-%dT%H:%M:%S.000Z", time.gmtime()), "name": title}
    with path.open("a", encoding="utf-8") as fh:
        fh.write(json.dumps(rec, ensure_ascii=False) + "\n")


def _rewrite_claude_title(path: Path, title: str) -> None:
    """Write the name the way Claude Code's own /rename and -n do: a
    custom-title record, which Claude prefers over ai-title. Append one; the
    transcript itself is not changed."""
    import json
    import uuid as _uuid
    sid = path.stem
    rec = {"type": "custom-title", "customTitle": title, "sessionId": sid}
    with path.open("a", encoding="utf-8") as fh:
        fh.write(json.dumps(rec, ensure_ascii=False) + "\n")
    _uuid.UUID(sid)  # sanity: Claude session files are named by uuid


def _free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def _opencode_request(base: str, method: str, path: str, body: dict | None = None) -> object:
    data = json.dumps(body).encode("utf-8") if body is not None else None
    req = urllib.request.Request(base + path, data=data, method=method,
                                 headers={"content-type": "application/json"})
    with urllib.request.urlopen(req, timeout=10) as resp:
        raw = resp.read()
    return json.loads(raw) if raw else None


def _stop_server(proc: subprocess.Popen) -> None:
    """Stop the server process group we started, so nothing is left behind."""
    if proc.poll() is not None:
        return
    try:
        os.killpg(proc.pid, signal.SIGTERM)
        proc.wait(timeout=10)
    except subprocess.TimeoutExpired:
        os.killpg(proc.pid, signal.SIGKILL)
        proc.wait(timeout=10)
    except ProcessLookupError:
        pass


def _rewrite_opencode_title(sid: str, title: str) -> None:
    """Rename an OpenCode session through its HTTP API (PATCH /session/{id}).

    The CLI has no rename command and the SQLite store is not written directly.
    A temporary `opencode serve` is started on a free localhost port with its
    own data directory, the title is patched, and the server process group is
    stopped in every case. Nothing is left running afterwards.
    """
    if not (sid.startswith("ses_") and sid.replace("_", "").isalnum()):
        raise DeleteRefused(f"refusing to pass unexpected id to opencode: {sid!r}")
    port = _free_port()
    base = f"http://127.0.0.1:{port}"
    proc = subprocess.Popen(
        [hs.OPENCODE, "serve", "--port", str(port), "--hostname", "127.0.0.1"],
        stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        cwd=str(Path.home()), start_new_session=True,
    )
    try:
        deadline = time.monotonic() + 60
        while True:
            if proc.poll() is not None:
                raise RuntimeError(f"opencode serve exited early (code {proc.returncode})")
            try:
                _opencode_request(base, "GET", "/session")
                break
            except (urllib.error.URLError, ConnectionError, OSError):
                if time.monotonic() > deadline:
                    raise RuntimeError("opencode serve did not become ready within 60s")
                time.sleep(0.25)
        try:
            _opencode_request(base, "PATCH", f"/session/{sid}", {"title": title})
        except urllib.error.HTTPError as e:
            code = e.code
            e.close()
            if code == 404:
                raise DeleteRefused(f"opencode session not found: {sid}") from None
            raise RuntimeError(f"opencode rename failed: HTTP {code}") from None
    finally:
        _stop_server(proc)


def _atomic_write(path: Path, text: str) -> None:
    tmp = path.with_name(path.name + ".rename.tmp")
    tmp.write_text(text, encoding="utf-8", newline="\n")
    os.replace(tmp, path)


def rename_group(group: Group, title: str) -> str:
    """Rename a source conversation. Returns the saved title. Lock-guarded,
    path-guarded, and refused if the source is missing or not a session."""
    new = validate_title(title)
    if group.orphan:
        raise DeleteRefused("source is not on disk; nothing to rename")
    _claim(group)
    try:
        if group.fmt == "opencode":
            _rewrite_opencode_title(group.key, new)
        else:
            path = Path(group.key)
            if not _under_roots(path):
                raise DeleteRefused(f"source outside session roots, not renaming: {path}")
            if not path.exists():
                raise DeleteRefused(f"source no longer exists: {path}")
            if group.fmt == "omp":
                _rewrite_omp_title(path, new)
            elif group.fmt == "pi":
                _rewrite_pi_title(path, new)
            elif group.fmt == "claude":
                _rewrite_claude_title(path, new)
            else:
                raise DeleteRefused(f"unknown format: {group.fmt}")
    finally:
        _release(group)
    return new
