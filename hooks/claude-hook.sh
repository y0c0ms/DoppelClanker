#!/bin/sh
# harness-sync lock hook for Claude Code. Runs under bash (Claude Code
# invokes command hooks through /usr/bin/bash on Windows), reads the hook
# payload on stdin, and calls hslock with Claude's own session id. The
# daemon's alias map turns a mirror id into its source id inside hslock.
#
#   UserPromptSubmit  -> claim; on refusal emit {"decision":"block"} so the
#                        prompt is not sent and the user sees who holds it.
#   Stop / SessionEnd -> release.
#
# Fail open: if hslock is missing, print a notice as additionalContext and
# let the prompt through. The daemon's divergence guard is the backstop.
#
# Claude Code exports CLAUDE_PID (its own Win32 pid) and CLAUDE_CODE_SESSION_ID
# to hooks, so the lock is tied to the real process and dies with it if
# Claude is killed mid-turn. ($PPID is useless here: it is an MSYS pid of a
# per-hook bash.)

EVENT="$1"
# Find the lock binary: an explicit HSLOCK, then PATH (hslock or hslock.exe), then the
# per-user default. The installer puts hslock on PATH, so no path is hardcoded here.
if [ -n "${HSLOCK:-}" ]; then
  HS="$HSLOCK"
elif command -v hslock >/dev/null 2>&1; then
  HS="$(command -v hslock)"
elif command -v hslock.exe >/dev/null 2>&1; then
  HS="$(command -v hslock.exe)"
else
  HS="$HOME/bin/hslock"
fi
IN=$(cat)
SID=${CLAUDE_CODE_SESSION_ID:-$(printf '%s' "$IN" | sed -n 's/.*"session_id"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -1)}
PID=${CLAUDE_PID:-$PPID}
[ -z "$SID" ] && exit 0

if [ ! -x "$HS" ]; then
  if [ "$EVENT" = "submit" ]; then
    printf '{"hookSpecificOutput":{"hookEventName":"UserPromptSubmit","additionalContext":"harness-sync: lock binary missing (hslock is not on PATH); this turn is unprotected (cd ~/.local/share/harness-sync/lock && cargo build --release && cp target/release/hslock.exe ~/bin/)"}}\n'
  fi
  exit 0
fi

case "$EVENT" in
  submit)
    OUT=$("$HS" claim "$SID" claude "$PID" "$SID" 2>/dev/null); RC=$?
    if [ "$RC" = 2 ]; then
      WHO=${OUT%% *}; WPID=${OUT#* }
      printf '{"decision":"block","reason":"This conversation is being written by %s (pid %s). Finish the turn there first; this prompt was not sent."}\n' "$WHO" "$WPID"
    fi
    ;;
  stop)
    "$HS" release "$SID" claude "$PID" >/dev/null 2>&1
    ;;
esac
exit 0
