"""harness-sync: keep coding-agent sessions available in every installed harness.

Watches the native session stores of omp, Pi and OpenCode. When a session
changes, it is converted with `session-migrate` and installed into the other
harnesses as a mirror, so it can be resumed there without typing anything.

Design (every point below was measured, not assumed):

* Mirrors carry a deterministic id derived from the source id, so an updated
  source always replaces the same mirror instead of creating duplicates.
  - OpenCode `import` upserts by session id (same id twice -> one row).
  - Pi/omp: session-migrate refuses to overwrite, so we import into a scratch
    home and atomically replace the mirror file ourselves.
* Mirrors are never treated as sources (loop prevention): the daemon keeps a
  state file mapping every mirror path/id back to its origin, and ignores
  change events on anything it created. A mirror is also marked with a title
  prefix so it is recognisable in the pickers.
* omp sessions need two fix-ups before session-migrate accepts them on
  Windows: `bashExecution` records without `content` are removed and their
  children re-parented; the blob store is exposed next to the sanitized copy
  through a directory junction (no symlink privilege on this box).
* OpenCode has no file to watch; its store is SQLite. We poll `session.time_updated`
  and export with the CLI (which prints a banner before the JSON).
* Debounce: a session that is actively being written changes every few
  seconds. We wait for QUIET_SECONDS of silence before converting, and we
  skip conversion if the content hash is unchanged since the last run.

Usage:
    python harness_sync.py            # run forever
    python harness_sync.py --once     # one pass, then exit
    python harness_sync.py --status   # show what is mirrored where
"""
from __future__ import annotations

import argparse
import hashlib
import json
import logging
import os
import re
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import time
import uuid
from dataclasses import dataclass, field
from pathlib import Path

HOME = Path.home()
STATE_DIR = Path(os.environ.get("HARNESS_SYNC_DIR", HOME / ".local" / "share" / "harness-sync"))
STATE_FILE = STATE_DIR / "state.json"
LOG_FILE = STATE_DIR / "harness-sync.log"
SCRATCH = STATE_DIR / "scratch"

OMP_AGENT = Path(os.environ.get("OMP_AGENT_DIR", HOME / ".omp" / "agent"))
PI_AGENT = Path(os.environ.get("PI_AGENT_DIR", HOME / ".pi" / "agent"))
OPENCODE_DB = Path(os.environ.get("OPENCODE_DB", HOME / ".local" / "share" / "opencode" / "opencode.db"))
CLAUDE_DIR = Path(os.environ.get("CLAUDE_CONFIG_DIR", HOME / ".claude"))

SMIGRATE = os.environ.get("SMIGRATE", str(Path(sys.executable).parent / "Scripts" / "smigrate.exe"))
OPENCODE = os.environ.get("OPENCODE_BIN", shutil.which("opencode") or str(HOME / ".bun" / "bin" / "opencode.exe"))

POLL_SECONDS = float(os.environ.get("HARNESS_SYNC_POLL", "5"))
QUIET_SECONDS = float(os.environ.get("HARNESS_SYNC_QUIET", "20"))
# Only sessions changed within this window are mirrored. Keeps the first pass
# bounded (199 omp sessions / 1.5 GB here) and the pickers free of stale copies.
MAX_AGE_DAYS = float(os.environ.get("HARNESS_SYNC_MAX_AGE_DAYS", "14"))
MIRROR_PREFIX = "[mirror] "
NAMESPACE = uuid.UUID("5e55a0b1-7c3e-4d2a-9f1b-0a12ab34cd56")  # stable, project-local

log = logging.getLogger("harness-sync")


# ----------------------------------------------------------------- state

@dataclass
class Mirror:
    target: str              # "pi" | "omp" | "opencode"
    path: str                # file path (pi/omp) or session id (opencode)
    source_hash: str         # content hash of the source at last successful sync
    installed_hash: str = ""  # fingerprint of the mirror itself right after install
    diverged: bool = False   # a harness continued the conversation inside this mirror


@dataclass
class Source:
    fmt: str                 # "omp" | "pi" | "opencode"
    key: str                 # file path (omp/pi) or session id (opencode)
    last_seen_change: float = 0.0
    last_hash: str = ""
    last_stamp: str = ""   # size:mtime_ns of the file when last_hash was computed
    mirrors: dict[str, Mirror] = field(default_factory=dict)   # target -> Mirror
    failures: int = 0


class State:
    def __init__(self) -> None:
        self.sources: dict[str, Source] = {}
        self.mirror_keys: set[str] = set()        # every path/id we created
        self.load()

    def load(self) -> None:
        if not STATE_FILE.exists():
            return
        raw = json.loads(STATE_FILE.read_text(encoding="utf-8"))
        for k, v in raw.get("sources", {}).items():
            m = {t: Mirror(**mv) for t, mv in v.pop("mirrors", {}).items()}
            self.sources[k] = Source(**v, mirrors=m)
        self.mirror_keys = set(raw.get("mirror_keys", []))

    def save(self) -> None:
        STATE_DIR.mkdir(parents=True, exist_ok=True)
        data = {
            "sources": {k: {**{f: getattr(s, f) for f in ("fmt", "key", "last_seen_change", "last_hash", "last_stamp", "failures")},
                            "mirrors": {t: m.__dict__ for t, m in s.mirrors.items()}}
                        for k, s in self.sources.items()},
            "mirror_keys": sorted(self.mirror_keys),
        }
        tmp = STATE_FILE.with_suffix(".tmp")
        tmp.write_text(json.dumps(data, indent=1), encoding="utf-8")
        os.replace(tmp, STATE_FILE)


# ------------------------------------------------------------ discovery

OMP_PI_NAME = re.compile(r"^(?P<ts>\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}-\d{3}Z)_(?P<id>[0-9a-f-]{36})\.jsonl$")


def jsonl_sessions(agent_dir: Path) -> list[Path]:
    root = agent_dir / "sessions"
    if not root.is_dir():
        return []
    return [p for p in root.glob("*/*.jsonl") if OMP_PI_NAME.match(p.name)]


def jsonl_session_meta(path: Path) -> tuple[str, str, str]:
    """(session_id, cwd, title) from the header records of an omp/pi journal."""
    sid = OMP_PI_NAME.match(path.name)["id"]
    cwd, title = "", ""
    with path.open("r", encoding="utf-8", errors="replace") as fh:
        for i, line in enumerate(fh):
            if i > 3:
                break
            try:
                rec = json.loads(line)
            except json.JSONDecodeError:
                continue
            if rec.get("type") == "session":
                cwd = rec.get("cwd", "") or cwd
                title = rec.get("title") or title
            elif rec.get("type") == "title":
                title = (rec.get("title") or "").strip() or title
    return sid, cwd, title


CLAUDE_NAME = re.compile(r"^(?P<id>[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})\.jsonl$")


def claude_sessions() -> list[Path]:
    root = CLAUDE_DIR / "projects"
    if not root.is_dir():
        return []
    return [p for p in root.glob("*/*.jsonl") if CLAUDE_NAME.match(p.name)]


def claude_session_meta(path: Path) -> tuple[str, str, str, bool]:
    """(session_id, cwd, title, has_user_turn). Title is the first user prompt;
    Claude Code has no title record. Sessions with no user turn (warm-up files
    Claude Code creates for every launch) are not resumable and are skipped."""
    sid = CLAUDE_NAME.match(path.name)["id"]
    cwd, title, has_user = "", "", False
    with path.open("r", encoding="utf-8", errors="replace") as fh:
        for line in fh:
            try:
                rec = json.loads(line)
            except json.JSONDecodeError:
                continue
            cwd = cwd or rec.get("cwd", "")
            if rec.get("type") == "user" and not rec.get("isMeta"):
                msg = rec.get("message") or {}
                content = msg.get("content")
                text = content if isinstance(content, str) else next(
                    (b.get("text", "") for b in content if isinstance(b, dict) and b.get("type") == "text"), "") if isinstance(content, list) else ""
                if text and not text.startswith("<"):
                    has_user = True
                    if not title:
                        title = text.strip().split("\n", 1)[0][:80]
            if title and cwd and has_user:
                break
    return sid, cwd, title, has_user


def opencode_sessions() -> list[tuple[str, float, str, str]]:
    """(id, time_updated, directory, title) for every OpenCode session."""
    if not OPENCODE_DB.exists():
        return []
    con = sqlite3.connect(f"file:{OPENCODE_DB.as_posix()}?mode=ro", uri=True, timeout=5)
    try:
        return [(r[0], float(r[1]) / 1000.0, r[2], r[3]) for r in
                con.execute("select id, time_updated, directory, title from session where time_archived is null")]
    finally:
        con.close()


# ------------------------------------------------------------- helpers

def file_hash(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def mirror_uuid(source_key: str, target: str) -> str:
    return str(uuid.uuid5(NAMESPACE, f"{source_key}|{target}"))


def mirror_opencode_id(source_key: str) -> str:
    # OpenCode ids: "ses_" + 12 hex (time) + 14 base62. Deterministic, valid shape.
    d = hashlib.sha256(f"{source_key}|opencode".encode()).hexdigest()
    alphabet = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz"
    tail = "".join(alphabet[int(d[i:i + 2], 16) % 62] for i in range(12, 40, 2))
    return "ses_" + d[:12] + tail


# The daemon runs under pythonw.exe, which has no console. Without this flag
# Windows allocates a visible console window for every console child
# (smigrate, opencode, cmd.exe) and the desktop flickers on each sync.
_NO_WINDOW = getattr(subprocess, "CREATE_NO_WINDOW", 0)


def run(cmd: list[str], **kw) -> subprocess.CompletedProcess:
    log.debug("run: %s", " ".join(cmd))
    return subprocess.run(cmd, capture_output=True, text=True, encoding="utf-8", errors="replace",
                          creationflags=_NO_WINDOW, **kw)


def junction(link: Path, target: Path) -> None:
    if link.exists():
        return
    link.parent.mkdir(parents=True, exist_ok=True)
    r = run(["cmd.exe", "/c", "mklink", "/J", str(link), str(target)])
    if r.returncode != 0:
        raise RuntimeError(f"junction failed: {r.stderr.strip() or r.stdout.strip()}")


# ---------------------------------------------------- source preparation

def sanitize_omp(src: Path, dst: Path) -> int:
    return sanitize_journal(src, dst, drop_roles={"bashExecution"}, only_without_content=True)


def sanitize_journal(src: Path, dst: Path, drop_roles: set[str], only_without_content: bool = False) -> int:
    """Copy an omp/pi journal dropping message records whose role is in
    drop_roles (optionally only when they lack a content field), re-parenting
    their children so the id/parentId chain stays intact. Returns the number
    of dropped records."""
    recs = []
    with src.open("r", encoding="utf-8") as fh:
        for line in fh:
            line = line.strip()
            if line:
                recs.append(json.loads(line))
    drop = {r["id"] for r in recs
            if r.get("type") == "message"
            and (r.get("message") or {}).get("role") in drop_roles
            and (not only_without_content or "content" not in r["message"])}
    parent = {r["id"]: r.get("parentId") for r in recs if "id" in r}

    def resolve(pid):
        while pid in drop:
            pid = parent.get(pid)
        return pid

    with dst.open("w", encoding="utf-8", newline="\n") as fh:
        for r in recs:
            if r.get("id") in drop:
                continue
            if r.get("parentId") in drop:
                r["parentId"] = resolve(r["parentId"])
            fh.write(json.dumps(r, ensure_ascii=False) + "\n")
    return len(drop)


def sanitize_claude(src: Path, dst: Path) -> int:
    """Claude Code can emit two compact_boundary records whose logicalParentUuid
    links point into each other's segment (seen 2026-10-12 in a 41 MB session
    with 8 compactions). session-migrate follows logicalParentUuid as ancestry
    and rejects the file as an 'ancestry cycle'. Claude Code itself does not
    walk those links for resume, so the file is fine for it. Fix: walk the
    active chain exactly as session-migrate does; at the first revisit, clear
    logicalParentUuid on the boundary that closes the loop. That turns the
    boundary into a plain root: everything after it is kept, the pre-compaction
    history it pointed at is dropped (which is what compaction meant anyway).
    Returns the number of links cut; 0 means src is clean and dst is unused."""
    recs = []
    with src.open("r", encoding="utf-8", errors="replace") as fh:
        for line in fh:
            line = line.strip()
            if line:
                try:
                    recs.append(json.loads(line))
                except json.JSONDecodeError:
                    pass
    by = {r["uuid"]: r for r in recs if r.get("uuid")}
    leaf = next((r.get("leafUuid") for r in reversed(recs)
                 if r.get("type") == "last-prompt" and r.get("leafUuid")), None)
    if not leaf:
        msgs = [r for r in recs if r.get("type") in ("user", "assistant")]
        leaf = msgs[-1]["uuid"] if msgs else None
    cut = 0
    while leaf:
        seen: set[str] = set()
        cur = leaf
        closing = None
        while cur:
            if cur in seen:
                break
            seen.add(cur)
            r = by.get(cur)
            if r is None:
                break
            nxt = r.get("parentUuid")
            if not nxt and r.get("type") == "system" and r.get("subtype") == "compact_boundary":
                nxt = r.get("logicalParentUuid")
                if nxt in seen:
                    closing = r
            cur = nxt
        if closing is None:
            break
        closing["logicalParentUuid"] = None
        cut += 1
    if cut:
        with dst.open("w", encoding="utf-8", newline="\n") as fh:
            for r in recs:
                fh.write(json.dumps(r, ensure_ascii=False) + "\n")
    return cut


def prepare_source(fmt: str, key: str, scratch: Path) -> tuple[Path, str]:
    """Return (path session-migrate should read, cwd) for a source."""
    if fmt in ("omp", "pi") and Path(HSCONV).exists():
        src = Path(key)
        _, cwd, _ = jsonl_session_meta(src)
        return src, cwd
    if fmt == "omp":
        src = Path(key)
        bucket = scratch / "src" / ".omp" / "agent" / "sessions" / src.parent.name
        bucket.mkdir(parents=True, exist_ok=True)
        dst = bucket / src.name
        dropped = sanitize_omp(src, dst)
        if dropped:
            log.info("omp: dropped %d content-less bashExecution record(s)", dropped)
        junction(scratch / "src" / ".omp" / "agent" / "blobs", OMP_AGENT / "blobs")
        _, cwd, _ = jsonl_session_meta(src)
        return dst, cwd
    if fmt == "pi":
        src = Path(key)
        _, cwd, _ = jsonl_session_meta(src)
        dst = scratch / "src" / src.name
        dst.parent.mkdir(parents=True, exist_ok=True)
        dropped = sanitize_journal(src, dst, drop_roles={"system"})
        if dropped:
            log.info("pi: dropped %d system-prompt record(s) (pi >= 1.1 persists them; converter pin predates that)", dropped)
        return (dst if dropped else src), cwd
    if fmt == "claude":
        src = Path(key)
        _, cwd, _, _ = claude_session_meta(src)
        dst = scratch / "src" / src.name
        dst.parent.mkdir(parents=True, exist_ok=True)
        cut = sanitize_claude(src, dst)
        if cut:
            log.info("claude: broke %d compact-boundary cycle(s); history before the loop is dropped", cut)
        return (dst if cut else src), cwd
    if fmt == "opencode":
        r = run([OPENCODE, "export", key])
        if r.returncode != 0:
            raise RuntimeError(f"opencode export failed: {r.stderr.strip()[:300]}")
        out = r.stdout
        start = out.find("{")
        if start < 0:
            raise RuntimeError("opencode export produced no JSON")
        bundle = scratch / "src" / f"{key}.json"
        bundle.parent.mkdir(parents=True, exist_ok=True)
        bundle.write_text(out[start:], encoding="utf-8")
        info = json.loads(out[start:])["info"]
        return bundle, info.get("directory", "")
    raise ValueError(fmt)


# -------------------------------------------------------- installation

def set_jsonl_title(path: Path, title: str, fmt: str) -> None:
    """Prefix the picker title so a mirror is recognisable. omp uses a fixed
    256-byte slot on line 1; pi uses session_info / header title."""
    lines = path.read_text(encoding="utf-8").split("\n")
    if fmt == "omp" and lines and '"type"' in lines[0] and '"title"' in lines[0] and json.loads(lines[0]).get("type") == "title":
        rec = json.loads(lines[0])
        rec["title"] = (MIRROR_PREFIX + (rec.get("title") or title)).strip()
        raw = json.dumps(rec, ensure_ascii=False)
        pad = 256 - len(raw.encode("utf-8")) - 1
        if pad < 0:  # title too long for the slot; truncate
            rec["title"] = rec["title"][: max(0, len(rec["title"]) + pad)]
            raw = json.dumps(rec, ensure_ascii=False)
            pad = 256 - len(raw.encode("utf-8")) - 1
        lines[0] = raw + " " * pad
    elif fmt == "pi":
        for i, line in enumerate(lines[:3]):
            if '"type": "session"' in line or line.startswith('{"type":"session"'):
                rec = json.loads(line)
                rec["title"] = MIRROR_PREFIX + (rec.get("title") or title)
                lines[i] = json.dumps(rec, ensure_ascii=False)
                break
    path.write_text("\n".join(lines), encoding="utf-8", newline="\n")


HSCONV = os.environ.get("HSCONV", str(HOME / "bin" / "hsconv.exe"))


def _journal_source(src_fmt: str) -> bool:
    """omp and Pi journals are converted natively by hsconv (Rust). Claude
    sources still go through session-migrate until that adapter is ported."""
    return src_fmt in ("omp", "pi") and Path(HSCONV).exists()


def install_jsonl(target: str, src_path: Path, src_fmt: str, cwd: str, mirror_id: str, title: str) -> Path:
    """Write the mirror into the real store, replacing any previous mirror
    with the same id.
    pi/omp: <agent>/sessions/<bucket>/<ts>_<uuid>.jsonl
    claude: <~/.claude>/projects/<bucket>/<uuid>.jsonl"""
    if _journal_source(src_fmt):
        return _install_jsonl_hsconv(target, src_path, cwd, mirror_id, title)
    with tempfile.TemporaryDirectory(dir=SCRATCH, prefix=f"{target}-") as tmp:
        home = Path(tmp) / "home"
        r = run([SMIGRATE, "import", str(src_path), "--format", src_fmt, "--to", target,
                 "--home", str(home), "--cwd", cwd or str(HOME), "--session-id", mirror_id])
        if r.returncode != 0:
            raise RuntimeError(f"session-migrate -> {target} failed: {r.stderr.strip()[-400:]}")
        sub = "projects" if target == "claude" else "sessions"
        produced = list((home / sub).glob("*/*.jsonl"))
        if len(produced) != 1:
            raise RuntimeError(f"expected one output file, got {produced}")
        out = produced[0]
        if target != "claude":
            set_jsonl_title(out, title, target)
        root = {"pi": PI_AGENT, "omp": OMP_AGENT, "claude": CLAUDE_DIR}[target]
        dest_dir = root / sub / out.parent.name
        dest_dir.mkdir(parents=True, exist_ok=True)
        for old in dest_dir.glob(f"*{mirror_id}.jsonl"):   # ts prefix may differ
            old.unlink()
        dest = dest_dir / out.name
        tmp_dest = dest.with_suffix(".jsonl.tmp")
        shutil.copyfile(out, tmp_dest)
        os.replace(tmp_dest, dest)
        return dest


def _install_jsonl_hsconv(target: str, src_path: Path, cwd: str, mirror_id: str, title: str) -> Path:
    """hsconv writes straight into the store (atomic rename inside), with the
    deterministic mirror id, so a re-run replaces the same file."""
    root = {"pi": PI_AGENT, "omp": OMP_AGENT, "claude": CLAUDE_DIR}[target]
    mtitle = MIRROR_PREFIX + title
    if target == "claude":
        cmd = [HSCONV, "to-claude", str(src_path), "--out-dir", str(root / "projects"), "--id", mirror_id]
    elif target == "pi":
        cmd = [HSCONV, "to-pi", str(src_path), "--out-dir", str(root / "sessions"), "--id", mirror_id, "--title", mtitle]
    else:
        cmd = [HSCONV, "to-omp", str(src_path), "--out-dir", str(root / "sessions"), "--home", str(HOME), "--id", mirror_id, "--title", mtitle]
    r = run(cmd)
    if r.returncode != 0:
        raise RuntimeError(f"hsconv -> {target} failed: {r.stderr.strip()[-400:]}")
    dest = Path(r.stdout.strip().splitlines()[-1])
    # an older mirror of the same id under a different timestamp prefix
    for old in dest.parent.glob(f"*{mirror_id}.jsonl"):
        if old != dest:
            old.unlink()
    return dest


def install_opencode(src_path: Path, src_fmt: str, cwd: str, mirror_id: str, title: str) -> str:
    with tempfile.TemporaryDirectory(dir=SCRATCH, prefix="oc-") as tmp:
        if _journal_source(src_fmt):
            bundle = Path(tmp) / "bundle.json"
            r = run([HSCONV, "to-opencode", str(src_path), "--out", str(bundle), "--id", mirror_id, "--title", MIRROR_PREFIX + title])
            if r.returncode != 0:
                raise RuntimeError(f"hsconv -> opencode failed: {r.stderr.strip()[-400:]}")
            r = run([OPENCODE, "import", str(bundle)])
            if r.returncode != 0 or "Imported session" not in r.stdout:
                raise RuntimeError(f"opencode import failed: {(r.stderr or r.stdout).strip()[-400:]}")
            return mirror_id
        bundle = Path(tmp) / "bundle.json"
        r = run([SMIGRATE, "convert", str(src_path), "--format", src_fmt, "--to", "opencode",
                 "--output", str(bundle), "--cwd", cwd or str(HOME)])
        if r.returncode != 0:
            raise RuntimeError(f"session-migrate -> opencode failed: {r.stderr.strip()[-400:]}")
        data = json.loads(bundle.read_text(encoding="utf-8"))
        data["info"]["id"] = mirror_id
        data["info"]["title"] = MIRROR_PREFIX + (data["info"].get("title") or title)
        for m in data.get("messages", []):
            if isinstance(m, dict):
                if "sessionID" in m:
                    m["sessionID"] = mirror_id
                info = m.get("info")
                if isinstance(info, dict) and "sessionID" in info:
                    info["sessionID"] = mirror_id
                for p in m.get("parts", []) or []:
                    if isinstance(p, dict) and "sessionID" in p:
                        p["sessionID"] = mirror_id
        bundle.write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")
        r = run([OPENCODE, "import", str(bundle)])
        if r.returncode != 0 or "Imported session" not in r.stdout:
            raise RuntimeError(f"opencode import failed: {(r.stderr or r.stdout).strip()[-400:]}")
        return mirror_id


# ---------------------------------------------------------- lock aliases

LOCK_DIR = STATE_DIR / "locks"


def source_lock_id(src: Source) -> str:
    """The id a conversation is locked under: its SOURCE session id. For
    jsonl sources that is the uuid in the filename; for OpenCode the row id."""
    if src.fmt == "opencode":
        return src.key
    name = Path(src.key).name
    m = OMP_PI_NAME.match(name) or CLAUDE_NAME.match(name)
    return m["id"] if m else name


def write_lock_alias(mirror_id: str, source_id: str) -> None:
    """locks/map/<mirror-id> contains the source id. A hook running inside a
    mirror reads this one small file to find which lock to check/claim."""
    d = LOCK_DIR / "map"
    d.mkdir(parents=True, exist_ok=True)
    safe = "".join(c for c in mirror_id if c.isalnum() or c in "-_")
    tmp = d / f"{safe}.tmp"
    tmp.write_text(source_id + "\n", encoding="utf-8")
    os.replace(tmp, d / safe)


# --------------------------------------------------------------- sync

TARGETS = ("omp", "pi", "opencode", "claude")


def mirror_fingerprint(target: str, ref: str) -> str:
    """Cheap identity of a mirror's current content: file size+mtime for
    journals, message count + time_updated for OpenCode rows."""
    if target == "opencode":
        con = sqlite3.connect(f"file:{OPENCODE_DB.as_posix()}?mode=ro", uri=True, timeout=5)
        try:
            row = con.execute("select time_updated,(select count(*) from message m where m.session_id=s.id) "
                              "from session s where id=?", (ref,)).fetchone()
        finally:
            con.close()
        return f"{row[0]}:{row[1]}" if row else "missing"
    p = Path(ref)
    if not p.exists():
        return "missing"
    st = p.stat()
    return f"{st.st_size}:{st.st_mtime_ns}"


def source_label(src: Source) -> str:
    return src.key if src.fmt == "opencode" else Path(src.key).name


HSLOCK = os.environ.get("HSLOCK", str(HOME / "bin" / "hslock.exe"))


def lock_holder(conversation_id: str) -> str | None:
    """'harness pid' if a live process holds the conversation, else None.
    Missing binary -> None (the daemon never blocks on the lock layer)."""
    if not Path(HSLOCK).exists():
        return None
    r = run([HSLOCK, "check", conversation_id])
    return r.stdout.strip() if r.returncode == 2 else None


def sync_source(state: State, src: Source, title: str, installed: set[str]) -> None:
    """Mirror one source. The daemon CLAIMS the conversation for the duration,
    exactly like a harness turn: a prompt submitted in any harness while we
    are rewriting its mirror is refused with "being written by daemon", which
    closes the window between "turn ended" and "mirror up to date" (Q2 b:
    the re-mirror gap). If a harness already holds it, we skip and retry."""
    conv = source_lock_id(src)
    if Path(HSLOCK).exists():
        r = run([HSLOCK, "claim", conv, "daemon", str(os.getpid()), src.key])
        if r.returncode == 2:
            log.info("skip %s: conversation held by %s", source_label(src), r.stdout.strip())
            src.last_hash = ""
            return
    try:
        _sync_source_locked(state, src, title, installed)
    finally:
        if Path(HSLOCK).exists():
            run([HSLOCK, "release", conv, "daemon", str(os.getpid())])


def _sync_source_locked(state: State, src: Source, title: str, installed: set[str]) -> None:
    scratch = SCRATCH / hashlib.sha1(src.key.encode()).hexdigest()[:12]
    scratch.mkdir(parents=True, exist_ok=True)
    prepared, cwd = prepare_source(src.fmt, src.key, scratch)
    for target in TARGETS:
        if target == src.fmt or target not in installed:
            continue
        existing = src.mirrors.get(target)
        if existing and existing.source_hash == src.last_hash:
            continue
        # DIVERGENCE GUARD. If a harness continued the conversation inside the
        # mirror, its content no longer matches what we installed. Overwriting
        # it would destroy those turns, so the mirror is frozen instead: it
        # becomes an independent conversation and is never refreshed again.
        if existing and not existing.diverged:
            if not existing.installed_hash:
                # Mirror from before fingerprints existed: we cannot know whether a
                # harness wrote into it. Never overwrite; adopt it as-is and freeze.
                existing.diverged = True
                log.warning("mirror %s of %s predates fingerprinting; frozen to be safe "
                            "(delete it to get a fresh mirror)", existing.path, source_label(src))
            elif mirror_fingerprint(target, existing.path) != existing.installed_hash:
                existing.diverged = True
                log.warning("mirror %s of %s was continued in %s; frozen, no longer refreshed "
                            "(the two conversations have diverged)", existing.path, source_label(src), target)
        if existing and existing.diverged:
            continue
        try:
            if target == "opencode":
                mid = mirror_opencode_id(src.key)
                install_opencode(prepared, src.fmt, cwd, mid, title)
                ref = mid
            else:
                mid = mirror_uuid(src.key, target)
                ref = str(install_jsonl(target, prepared, src.fmt, cwd, mid, title))
            src.mirrors[target] = Mirror(target, ref, src.last_hash, installed_hash=mirror_fingerprint(target, ref))
            state.mirror_keys.add(ref)
            write_lock_alias(mid, source_lock_id(src))
            log.info("synced %s:%s -> %s (%s)", src.fmt, source_label(src), target, title[:50])
        except Exception as exc:  # keep going with other targets
            src.failures += 1
            log.error("sync %s -> %s failed: %s", src.key, target, exc)
    cleanup_scratch(scratch)


def cleanup_scratch(scratch: Path) -> None:
    """Remove the per-source scratch dir. The blob junction inside it must be
    unlinked with rmdir first: rmtree would either fail on it or, worse,
    descend through it into the real blob store."""
    junction_path = scratch / "src" / ".omp" / "agent" / "blobs"
    if junction_path.is_dir() and junction_path.is_junction():
        os.rmdir(junction_path)
    shutil.rmtree(scratch, ignore_errors=True)

def _mirror_ids(state: State) -> set[str]:
    return {m.path for s in state.sources.values() for m in s.mirrors.values()}


def _scan_journals(state: State, now: float, fmt: str, paths: list[Path], installed: dict[str, bool],
                   meta, changed: dict[str, tuple[Source, str]]) -> None:
    """Shared change detection for file-based stores (omp, pi, claude).
    `meta(path) -> (title, resumable)`."""
    mirrors = _mirror_ids(state)
    want = sum(1 for t in TARGETS if t != fmt and installed[t])
    for path in paths:
        key = str(path)
        if key in state.mirror_keys or key in mirrors:
            continue
        st = path.stat()
        if st.st_size < 512 or now - st.st_mtime > MAX_AGE_DAYS * 86400:
            continue
        src = state.sources.setdefault(key, Source(fmt=fmt, key=key))
        if st.st_mtime > src.last_seen_change:
            src.last_seen_change = st.st_mtime
        if now - src.last_seen_change < QUIET_SECONDS:
            continue  # still being written
        # Cheap pre-check: if size+mtime are what we hashed last time, the
        # content is unchanged; do not re-read a 130 MB journal every 5 s.
        stamp = f"{st.st_size}:{st.st_mtime_ns}"
        if stamp == src.last_stamp and src.last_hash:
            h = src.last_hash
        else:
            h = file_hash(path)
            src.last_stamp = stamp
        live = {t: m for t, m in src.mirrors.items() if not m.diverged}
        # Up to date when every *live* mirror carries this hash. Frozen mirrors
        # never count against us, otherwise a frozen target makes the source
        # look perpetually behind and we re-sync every pass.
        if h == src.last_hash and all(m.source_hash == h for m in live.values()) and len(src.mirrors) >= want:
            continue
        title, resumable = meta(path)
        if not resumable:
            continue
        src.last_hash = h
        changed[key] = (src, title or path.stem)


def scan(state: State, now: float) -> dict[str, tuple[Source, str]]:
    """Return sources that changed, keyed by source key, with their title."""
    changed: dict[str, tuple[Source, str]] = {}
    installed = {"omp": OMP_AGENT.is_dir(), "pi": PI_AGENT.is_dir(),
                 "opencode": OPENCODE_DB.exists(), "claude": (CLAUDE_DIR / "projects").is_dir()}

    for fmt, agent in (("omp", OMP_AGENT), ("pi", PI_AGENT)):
        if installed[fmt]:
            _scan_journals(state, now, fmt, jsonl_sessions(agent), installed,
                           lambda p: (jsonl_session_meta(p)[2], True), changed)
    if installed["claude"]:
        _scan_journals(state, now, "claude", claude_sessions(), installed,
                       lambda p: (lambda m: (m[2], m[3]))(claude_session_meta(p)), changed)

    if installed["opencode"]:
        for sid, updated, directory, title in opencode_sessions():
            if sid in state.mirror_keys or title.startswith(MIRROR_PREFIX) or now - updated > MAX_AGE_DAYS * 86400:
                continue
            src = state.sources.setdefault(sid, Source(fmt="opencode", key=sid))
            if updated > src.last_seen_change:
                src.last_seen_change = updated
            if now - src.last_seen_change < QUIET_SECONDS:
                continue
            h = f"{updated:.3f}"
            if h == src.last_hash and len(src.mirrors) == sum(1 for t in TARGETS if t != "opencode" and installed[t]):
                continue
            src.last_hash = h
            changed[sid] = (src, title or sid)

    state._installed = {k for k, v in installed.items() if v}  # type: ignore[attr-defined]
    return changed


def one_pass(state: State) -> int:
    now = time.time()
    changed = scan(state, now)
    for key, (src, title) in changed.items():
        try:
            sync_source(state, src, title, state._installed)  # type: ignore[attr-defined]
        except Exception as exc:
            src.failures += 1
            log.error("prepare %s failed: %s", key, exc)
    if changed:
        state.save()
    return len(changed)


def status(state: State) -> None:
    for key, src in sorted(state.sources.items(), key=lambda kv: kv[1].last_seen_change, reverse=True):
        name = Path(key).name if src.fmt != "opencode" else key
        print(f"{src.fmt:9s} {name}")
        for t, m in src.mirrors.items():
            print(f"    -> {t:9s} {Path(m.path).name if t != 'opencode' else m.path}")
        if src.failures:
            print(f"    failures: {src.failures}")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    ap.add_argument("--once", action="store_true")
    ap.add_argument("--status", action="store_true")
    ap.add_argument("-v", "--verbose", action="store_true")
    args = ap.parse_args()

    STATE_DIR.mkdir(parents=True, exist_ok=True)
    SCRATCH.mkdir(parents=True, exist_ok=True)
    logging.basicConfig(
        level=logging.DEBUG if args.verbose else logging.INFO,
        format="%(asctime)s %(levelname)s %(message)s",
        handlers=[logging.FileHandler(LOG_FILE, encoding="utf-8"), logging.StreamHandler(sys.stdout)],
    )
    state = State()
    if args.status:
        status(state)
        return
    if not Path(HSCONV).exists() and not Path(SMIGRATE).exists():
        sys.exit(f"neither hsconv ({HSCONV}) nor smigrate ({SMIGRATE}) found")
    log.info("converter: %s for omp/pi sources, %s for claude sources",
             "hsconv" if Path(HSCONV).exists() else "session-migrate", "session-migrate" if Path(SMIGRATE).exists() else "none")
    log.info("harness-sync starting (poll=%ss quiet=%ss) omp=%s pi=%s opencode=%s claude=%s",
             POLL_SECONDS, QUIET_SECONDS, OMP_AGENT.is_dir(), PI_AGENT.is_dir(), OPENCODE_DB.exists(),
             (CLAUDE_DIR / "projects").is_dir())
    if Path(HSLOCK).exists():
        swept = run([HSLOCK, "sweep"]).stdout.strip()
        if swept and swept != "0":
            log.info("swept %s stale lock(s) left by dead processes", swept)
    while True:
        n = one_pass(state)
        if args.once:
            log.info("one pass done, %d source(s) synced", n)
            return
        time.sleep(POLL_SECONDS)


if __name__ == "__main__":
    main()
