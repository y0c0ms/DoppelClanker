# DoppelClanker

Keep several AI coding harnesses working on the same conversation.

You can start a conversation in oh-my-pi (omp), continue it in Pi, pick it up in OpenCode, and then resume it in Claude Code. DoppelClanker keeps each harness's copy of the conversation in step with the others, and makes sure only one harness writes to a conversation at a time.

## How it works

Three parts cooperate:

1. **A canonical journal.** omp and Pi session files are the source of truth. Every other harness gets a converted copy, called a *mirror*. Mirrors are derived data: deleting one and letting the daemon rebuild it is safe.
2. **A converter (`hsconv`, Rust).** Converts between the omp/Pi journal format and the OpenCode and Claude Code formats. Images are inlined when moved to Pi. Reasoning content is kept for omp, Pi and OpenCode, and dropped for Claude Code, which signs its reasoning blocks and cannot accept ours.
3. **A lock (`hslock`, Rust) and a sync daemon (Python).** A conversation is locked while a harness is taking a turn. The lock is claimed when the turn starts and released when it ends, so an idle conversation holds nothing. The daemon rebuilds mirrors when their sources change and skips any conversation that is currently locked.

The lock is enforced inside each harness:

| Harness | Hook |
|---|---|
| omp and Pi | `hooks/harness-sync-lock.ts`, a shared extension. Claims on `turn_start`, releases on `turn_end` and session lifecycle events. |
| Claude Code | `hooks/claude-hook.sh`, wired to `UserPromptSubmit` (claim, block on refusal) and `Stop` / `SessionEnd` (release). |
| OpenCode | A plugin that claims on `chat.message` and releases on `session.idle` and `session.deleted`. |

A refused claim blocks the prompt and names the holder, so two harnesses never write the same conversation at once.

## Layout

```
lock/      hslock: claim / release / check / resolve / sweep / list
hsconv/    hsconv: inspect / ids / to-pi / to-omp / to-opencode / to-claude
hooks/     per-harness lock hooks (omp+Pi extension, Claude Code shell hook)
daemon/    harness_sync.py: the sync daemon
```

## Requirements

- Rust (stable) to build `hslock` and `hsconv`.
- Python 3.11+ for the daemon (developed and tested on 3.13).
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
| `SMIGRATE` | `Scripts/smigrate.exe` beside the Python interpreter | Path to session-migrate, used for Claude Code sources |

## Usage

```sh
python daemon/harness_sync.py --once      # run one sync pass and exit
python daemon/harness_sync.py -v          # run continuously, verbose
python daemon/harness_sync.py --status    # show current state
```

Inspect the lock and converter directly:

```sh
hslock list                    # held locks
hslock check <session-id>      # exit 0 if free, non-zero if held
hsconv inspect <file.jsonl>    # summarise a journal
hsconv ids <file.jsonl>        # list session ids in a journal
```

## Caveats

- **Windows-tested.** Developed and run on Windows. Paths use Windows conventions, and the daemon starts subprocesses with no console window.
- **Claude Code sources** are read through [session-migrate](https://github.com/xhluca/session-migrate) (`smigrate`). `hsconv` handles only omp and Pi sources; the daemon routes Claude-as-source through `smigrate`.
- **A crashed harness can leave a lock.** Locks record the harness pid and are swept on daemon start. Run `hslock sweep` to clear stale locks manually.
- **Lock failure policy.** If `hslock` is missing, the hooks fail open with a notice. The daemon's divergence guard is the backstop.

## Acknowledgements

Claude Code session conversion builds on the approach in session-migrate.

## License

MIT. See [LICENSE](LICENSE).
