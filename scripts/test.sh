#!/usr/bin/env bash
# Run every kind of test. Usage: scripts/test.sh [fast|all]
#   fast: data check, fmt/clippy, Rust unit + API + perf (+ quality if model present), JS tests
#   all (default): fast + build dist/ and smoke-test the real binary
set -euo pipefail
cd "$(dirname "$0")/.."
. "$HOME/.cargo/env" 2>/dev/null || true
MODE=${1:-all}

echo "== data: sections cover every verse"
python3 -I tools/check_sections.py data/sections.json

echo "== rust: fmt + clippy"
cargo fmt --all --check || { echo "run: cargo fmt --all"; exit 1; }
cargo clippy --release --workspace --all-targets -q -- -D warnings

echo "== rust: unit, API, perf, search quality (quality skips without assets/model)"
cargo test --release --workspace 2>&1 | tee /dev/stderr | grep -q "FAILED" && exit 1 || true
cargo test --release --workspace -q >/dev/null 2>&1 || exit 1

echo "== js: frontend helpers"
bun test web/

if [ "$MODE" = all ]; then
  echo "== smoke: built binary"
  scripts/build.sh >/dev/null 2>&1
  scripts/smoke.sh dist/bin/app
fi
echo "ALL TESTS PASSED"
