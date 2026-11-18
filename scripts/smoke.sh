#!/usr/bin/env bash
# Smoke test: start a built binary on a free port and hit the real endpoints.
#   scripts/smoke.sh               # tests dist/bin/app (run scripts/build.sh first)
#   scripts/smoke.sh path/to/app   # any build
set -euo pipefail
cd "$(dirname "$0")/.."
BIN=${1:-dist/bin/app}
PORT=${SMOKE_PORT:-$((20000 + RANDOM % 20000))}
LOG=$(mktemp)
PORT=$PORT "$BIN" >"$LOG" 2>&1 &
PID=$!
trap 'kill $PID 2>/dev/null; rm -f "$LOG"' EXIT
URL=http://127.0.0.1:$PORT
for _ in $(seq 1 100); do curl -sf "$URL/healthz" >/dev/null && break; kill -0 $PID 2>/dev/null || break; sleep 0.1; done
kill -0 $PID 2>/dev/null || { echo "FAIL: server exited"; cat "$LOG"; exit 1; }

fail() { echo "FAIL: $*"; echo "--- server log"; cat "$LOG"; exit 1; }
check() { # name, url, jq expression that must be true
  local out; out=$(curl -sf "$URL$2") || fail "$1: request failed"
  echo "$out" | jq -e "$3" >/dev/null || fail "$1: $3 on $(echo "$out" | head -c 300)"
  echo "ok  $1"
}
[ "$(curl -sf "$URL/healthz")" = ok ] || fail healthz
echo "ok  healthz"
html=$(curl -sf "$URL/") && [[ $html == *"Bible"* ]] || fail "index.html"
echo "ok  index.html"
check "info"            "/api/info"                               '.books | length == 4'
check "real model"      "/api/info"                               '.embedder | test("MiniLM")'
check "chapter"         "/api/chapter/MAT/5"                      '.sections[0].title != "" and (.sections | map(.verses | length) | add) == 48'
check "reference"       "/api/search?q=John%203:16"               '.reference.label == "John 3:16"'
check "eye of a needle" "/api/search?q=eye%20of%20the%20needle"   '[.verses[:3][].label] | index("Matthew 19:24") != null'
check "semantic"        "/api/search?q=the%20prodigal%20son"      '.sections[0].label | startswith("Luke 15")'
check "speed"           "/api/search?q=love%20your%20neighbour"  '.took_ms < 500'
echo "smoke test passed ($BIN)"
