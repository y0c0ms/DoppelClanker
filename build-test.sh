#!/bin/sh
# Build and test everything the sync contract depends on. No Python on the build path.
# 1. sesync-lock compiles and its Rust tests pass.
# 2. sesync-conv compiles and its Rust tests pass.
# 3. The sesync daemon (daemon-rs) compiles and its tests pass, including the
#    end-to-end tests that run the real sesync-conv, opencode and claude CLIs in a sandbox.
set -e
ROOT=$(cd "$(dirname "$0")" && pwd)
echo "== lock: build + tests"
(cd "$ROOT/sesync-lock" && cargo test --quiet)
echo "== sesync-conv: build + tests"
(cd "$ROOT/sesync-conv" && cargo test --release --quiet && cargo build --release --quiet)
echo "== daemon-rs: build + tests"
(cd "$ROOT/sesync" && cargo build --quiet && cargo test --quiet)
echo "== team-server: build + tests"
(cd "$ROOT/sesync-team" && cargo test --quiet)
echo "== no python on the build path"
# Check only command lines: skip comments and echo text. A python3 invocation
# would start with the command word, not inside a string.
if grep -vE '^[[:space:]]*(#|echo )' "$0" | grep -qE '(^|[[:space:];&|])python3?([[:space:]]|$)'; then
  echo "python is invoked by build-test.sh" >&2
  exit 1
fi
echo "== all passed"
