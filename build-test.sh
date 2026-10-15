#!/bin/sh
# Build and test everything the sync contract depends on.
# 1. hsconv and hslock compile and their Rust tests pass.
# 2. The end-to-end handoff tests run the real daemon and converter in a sandbox.
set -e
ROOT=$(cd "$(dirname "$0")" && pwd)
echo "== lock: build + tests"
(cd "$ROOT/lock" && cargo test --quiet)
echo "== hsconv: build"
(cd "$ROOT/hsconv" && cargo test --release --quiet && cargo build --release --quiet)
echo "== daemon: handoff e2e"
HSCONV="$ROOT/hsconv/target/release/hsconv" python3 -m unittest discover -s "$ROOT/daemon/tests" -v
echo "== all passed"
