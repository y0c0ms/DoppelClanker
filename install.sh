#!/bin/sh
# Install the harness-sync package: three binaries and the Claude hook.
#
#   sh install.sh                     install into ~/.local (bin in ~/.local/bin)
#   sh install.sh --prefix DIR        install under DIR
#   sh install.sh --uninstall         remove exactly what install added
#   sh install.sh --no-service        do not register the daemon to start at login
#
# Installed:
#   <prefix>/bin/hslock         lock: claim, release, check, sweep, hook
#   <prefix>/bin/hsconv         converter between harness formats
#   <prefix>/bin/harness-sync   sync daemon (--watch) and manage TUI (--tui)
#   <prefix>/share/harness-sync/hooks/   hook files for each harness
#   <prefix>/share/harness-sync/MANIFEST final list of what was installed
#
# Needs: cargo (Rust stable). Nothing else. No Python, no Node.
set -eu

ROOT=$(cd "$(dirname "$0")" && pwd)
PREFIX="${HOME}/.local"
UNINSTALL=0
SERVICE=1

while [ $# -gt 0 ]; do
  case "$1" in
    --prefix) PREFIX="$2"; shift 2 ;;
    --uninstall) UNINSTALL=1; shift ;;
    --no-service) SERVICE=0; shift ;;
    -h|--help) sed -n '2,15p' "$0"; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
done

BIN="$PREFIX/bin"
SHARE="$PREFIX/share/harness-sync"
MANIFEST="$SHARE/MANIFEST"

if [ "$UNINSTALL" = 1 ]; then
  # Stop the daemon service first, if one was registered.
  if [ -f "$SHARE/service-id" ]; then
    sh "$SHARE/service-stop" 2>/dev/null || true
  fi
  if [ -f "$MANIFEST" ]; then
    while IFS= read -r f; do
      [ -n "$f" ] && rm -f "$f"
    done < "$MANIFEST"
  fi
  rm -rf "$SHARE"
  echo "removed harness-sync from $PREFIX"
  exit 0
fi

command -v cargo >/dev/null 2>&1 || { echo "cargo is required (install Rust stable)" >&2; exit 1; }

echo "== building"
(cd "$ROOT/lock" && cargo build --release --quiet)
(cd "$ROOT/hsconv" && cargo build --release --quiet)
(cd "$ROOT/daemon-rs" && cargo build --release --quiet)

mkdir -p "$BIN" "$SHARE/hooks"
: > "$MANIFEST.tmp"

install_file() {
  # install_file <src> <dst> ; records dst in the manifest
  cp "$1" "$2"
  chmod 755 "$2" 2>/dev/null || true
  echo "$2" >> "$MANIFEST.tmp"
}

echo "== installing to $PREFIX"
install_file "$ROOT/lock/target/release/hslock" "$BIN/hslock"
install_file "$ROOT/hsconv/target/release/hsconv" "$BIN/hsconv"
install_file "$ROOT/daemon-rs/target/release/harness-sync" "$BIN/harness-sync"
for h in "$ROOT"/hooks/*; do
  [ -f "$h" ] || continue
  install_file "$h" "$SHARE/hooks/$(basename "$h")"
done

if [ "$SERVICE" = 1 ]; then
  # Start the daemon at login, per OS. Each service is recorded in the manifest so
  # --uninstall removes it. Windows is handled by install.ps1 (per-user, no admin).
  case "$(uname -s)" in
    Linux)
      if command -v systemctl >/dev/null 2>&1 && [ "$PREFIX" = "${HOME}/.local" ]; then
        UNIT="${HOME}/.config/systemd/user/harness-sync.service"
        mkdir -p "$(dirname "$UNIT")"
        cat > "$UNIT" <<EOF2
[Unit]
Description=harness-sync: keep coding-agent sessions in step across harnesses

[Service]
ExecStart=$BIN/harness-sync --watch
Restart=on-failure

[Install]
WantedBy=default.target
EOF2
        echo "$UNIT" >> "$MANIFEST.tmp"
        systemctl --user daemon-reload 2>/dev/null || true
        systemctl --user enable --now harness-sync.service 2>/dev/null || echo "note: run 'systemctl --user enable --now harness-sync' to start at login"
        printf '#!/bin/sh\nsystemctl --user disable --now harness-sync.service 2>/dev/null || true\n' > "$SHARE/service-stop"
        echo "$SHARE/service-stop" >> "$MANIFEST.tmp"
        echo "harness-sync.service" > "$SHARE/service-id"
        echo "$SHARE/service-id" >> "$MANIFEST.tmp"
      else
        echo "note: no login service registered. Run '$BIN/harness-sync --watch' to sync."
      fi
      ;;
    Darwin)
      PLIST="${HOME}/Library/LaunchAgents/com.harnesssync.watch.plist"
      mkdir -p "$(dirname "$PLIST")"
      cat > "$PLIST" <<EOF3
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.harnesssync.watch</string>
  <key>ProgramArguments</key>
  <array>
    <string>$BIN/harness-sync</string>
    <string>--watch</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>${HOME}/Library/Logs/harness-sync.log</string>
  <key>StandardErrorPath</key><string>${HOME}/Library/Logs/harness-sync.log</string>
</dict>
</plist>
EOF3
      echo "$PLIST" >> "$MANIFEST.tmp"
      launchctl bootout "gui/$(id -u)" "$PLIST" 2>/dev/null || true
      launchctl bootstrap "gui/$(id -u)" "$PLIST" 2>/dev/null || echo "note: run 'launchctl bootstrap gui/$(id -u) $PLIST' to start at login"
      printf '#!/bin/sh\nlaunchctl bootout "gui/$(id -u)" "%s" 2>/dev/null || true\n' "$PLIST" > "$SHARE/service-stop"
      echo "$SHARE/service-stop" >> "$MANIFEST.tmp"
      echo "com.harnesssync.watch" > "$SHARE/service-id"
      echo "$SHARE/service-id" >> "$MANIFEST.tmp"
      ;;
    *)
      echo "note: no login service for this system. Run '$BIN/harness-sync --watch' to sync."
      ;;
  esac
fi

echo "$MANIFEST" >> "$MANIFEST.tmp"
mv "$MANIFEST.tmp" "$MANIFEST"
echo "== installed. Run: $BIN/harness-sync --once"
