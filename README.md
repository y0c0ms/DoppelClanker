# DoppelClanker

Keep several AI coding harnesses working on the same conversation.

You can start a conversation in oh-my-pi (omp), continue it in Pi, pick it up in OpenCode, and then resume it in Claude Code. DoppelClanker keeps each harness's copy of the conversation in step with the others, and makes sure only one harness writes to a conversation at a time.

## How it works

Three parts cooperate:

1. **A canonical journal.** omp and Pi session files are the source of truth. Every other harness gets a converted copy, called a *mirror*. Mirrors are derived data: deleting one and letting the daemon rebuild it is safe.
2. **A converter (`hsconv`, Rust).** Converts between the omp/Pi journal format and the OpenCode and Claude Code formats. Images are inlined when moved to Pi. Reasoning content is kept for omp, Pi and OpenCode, and dropped for Claude Code, which signs its reasoning blocks and cannot accept ours.
3. **A lock (`hslock`, Rust) and a sync daemon (`harness-sync`, Rust).** A conversation is locked while a harness is taking a turn. The lock is claimed when the turn starts and released when it ends, so an idle conversation holds nothing. The daemon rebuilds mirrors when their sources change and skips any conversation that is currently locked.

The lock is enforced inside each harness:

| Harness | Hook |
|---|---|
| omp and Pi | `hooks/omp-lock-shim.ts`, a thin omp extension that forwards events to `hslock hook`. Claims on `turn_start`, releases on `turn_end` and session lifecycle events. |
| Claude Code | `hooks/claude-hook.sh`, wired to `UserPromptSubmit` (claim, block on refusal) and `Stop` / `SessionEnd` (release). |
| OpenCode | A plugin that claims on `chat.message` and releases on `session.idle` and `session.deleted`. |

A refused claim blocks the prompt and names the holder, so two harnesses never write the same conversation at once.

## Layout

```
lock/      hslock: claim / release / check / resolve / sweep / list
hsconv/    hsconv: inspect / ids / to-pi / to-omp / to-opencode / to-claude
hooks/     per-harness lock hooks (omp+Pi extension, Claude Code shell hook)
daemon-rs/ harness-sync: the sync daemon and manage TUI (Rust)
```

## Requirements

- Rust (stable) to build `hslock` and `hsconv`.
- The harnesses you want to sync: omp, Pi, OpenCode, Claude Code.

## Build

```sh
cargo build --release --manifest-path lock/Cargo.toml
cargo build --release --manifest-path hsconv/Cargo.toml
```

Copy the two binaries somewhere on your `PATH` (for example `~/bin/`).

## Configuration

The daemon reads these environment variables. All are optional.

| Variable | Default | Purpose |
|---|---|---|
| `HARNESS_SYNC_DIR` | `~/.local/share/harness-sync` | State, locks, logs |
| `OMP_AGENT_DIR` | `~/.omp/agent` | omp data |
| `PI_AGENT_DIR` | `~/.pi/agent` | Pi data |
| `OPENCODE_DB` | `~/.local/share/opencode/opencode.db` | OpenCode database |
| `HSCONV` | `~/bin/hsconv.exe` | Path to the converter |

## Usage

```sh
harness-sync --once      # run one sync pass and exit
harness-sync --watch     # run continuously (HARNESS_SYNC_POLL seconds apart)
harness-sync --status    # show current state
harness-sync --tui       # manage sessions: list, rename, delete
```

Inspect the lock and converter directly:

```sh
hslock list                    # held locks
hslock check <session-id>      # exit 0 if free, non-zero if held
hsconv inspect <file.jsonl>    # summarise a journal
hsconv ids <file.jsonl>        # list session ids in a journal
```

## Caveats

- **Platform status.** Developed on Windows and tested on Linux. The Rust daemon has not been run on Windows or macOS yet (see the packaging issues).
- **Claude Code sources** are converted by `hsconv` (`from-claude` and `to-claude`). The Python session-migrate dependency is no longer used.
- **A crashed harness can leave a lock.** Locks record the harness pid and are swept when the daemon starts. Run `hslock sweep` to clear stale locks manually.
- **Lock failure policy.** If `hslock` is missing, the hooks fail open with a notice. The daemon's divergence guard is the backstop.

## Acknowledgements

Session conversion in `hsconv` is inspired by
[session-migrate](https://github.com/xhluca/session-migrate) by Xing Han Lu (xhluca), MIT licence.
The ideas taken from it are:

- one shared event model (message, tool call, tool result, thinking, compaction) that every
  harness is read into and written out of, with calls and results linked by id;
- the native tool-call and thinking shapes it documents for Claude Code, Pi, omp and OpenCode;
- its conversion rules: pair every tool result with its call, never move signed thinking, count
  every dropped item, and never modify the source session.

No session-migrate code is included in DoppelClanker; `hsconv` is a separate Rust
implementation. Research notes are in [docs/research/session-fidelity.md](docs/research/session-fidelity.md).

## License

MIT. See [LICENSE](LICENSE).
