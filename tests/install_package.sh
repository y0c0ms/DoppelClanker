#!/bin/sh
# Install-and-run check for the package. Installs into a throwaway prefix, runs the
# installed binaries, then uninstalls and confirms nothing is left behind.
# Exit 0 only if every step passes.
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
PREFIX=$(mktemp -d /tmp/hs-pkg-XXXX)
trap 'rm -rf "$PREFIX"' EXIT

echo "== install into $PREFIX"
sh "$ROOT/install.sh" --prefix "$PREFIX" --no-service > /dev/null

for b in hslock hsconv harness-sync; do
  [ -x "$PREFIX/bin/$b" ] || { echo "missing $b"; exit 1; }
done
echo "== installed binaries run"
"$PREFIX/bin/hslock" list > /dev/null
"$PREFIX/bin/hsconv" > /dev/null 2>&1 || true
HARNESS_SYNC_DIR="$PREFIX/state" OMP_AGENT_DIR="$PREFIX/omp" PI_AGENT_DIR="$PREFIX/pi" \
  CLAUDE_CONFIG_DIR="$PREFIX/claude" OPENCODE_DB="$PREFIX/none.db" \
  "$PREFIX/bin/harness-sync" --status > /dev/null

echo "== claude hook installed"
[ -f "$PREFIX/share/harness-sync/hooks/claude-hook.sh" ] || { echo "hook missing"; exit 1; }

echo "== claude hook runs the installed hslock (exercises the hook's own lookup)"
# A stub that records it was called. The hook must reach the installed hslock by name.
LOG="$PREFIX/hook-calls.log"
cat > "$PREFIX/bin/hslock" <<STUB
#!/bin/sh
echo "\$1" >> "$LOG"
exit 0
STUB
chmod 755 "$PREFIX/bin/hslock"
echo '{}' | PATH="$PREFIX/bin:/usr/bin:/bin" CLAUDE_CODE_SESSION_ID=aaaaaaaa-1111-2222-3333-444444444444 CLAUDE_PID=4242 \
  sh "$PREFIX/share/harness-sync/hooks/claude-hook.sh" submit
grep -q "claim" "$LOG" || { echo "hook did not call the installed hslock"; exit 1; }
echo "hook called hslock: $(head -1 "$LOG")"

echo "== uninstall"
sh "$ROOT/install.sh" --uninstall --prefix "$PREFIX" > /dev/null
for b in hslock hsconv harness-sync; do
  [ ! -e "$PREFIX/bin/$b" ] || { echo "$b left behind"; exit 1; }
done
[ ! -e "$PREFIX/share/harness-sync" ] || { echo "share dir left behind"; exit 1; }
echo "== install/run/uninstall passed"
