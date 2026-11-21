#!/usr/bin/env bash
# Runs the Worker's tests: the unit tests, then the smoke test against `wrangler dev` with
# test/mock.mjs. Fresh local storage each time; nothing real is called. From server/doctor:
#   npm install --no-save wrangler   (once; node_modules is git-ignored)
#   bash test/run.sh
set -euo pipefail
cd "$(dirname "$0")/.."
node --test test/*.test.mjs

PORT=${PORT:-8797}; MOCK_PORT=${MOCK_PORT:-8798}; ADMIN=test-admin-0123456789abcdef
STATE=$(mktemp -d)
LOG="$STATE/wrangler.log"
# wrangler dev starts child processes of its own (workerd): stop the whole tree at the end.
killtree() { local c; for c in $(pgrep -P "$1" 2>/dev/null); do killtree "$c"; done; kill "$1" 2>/dev/null || true; }
cleanup() { kill "$MOCK_PID" 2>/dev/null || true; killtree "$DEV_PID"; wait 2>/dev/null || true; rm -rf "$STATE"; }
trap cleanup EXIT

node test/mock.mjs "$MOCK_PORT" > "$STATE/mock.log" 2>&1 & MOCK_PID=$!
npx wrangler dev --port "$PORT" --ip 127.0.0.1 --persist-to "$STATE/data" --show-interactive-dev-session=false \
  --var "ANTHROPIC_API_KEY:test-anthropic-key" --var "ADMIN_TOKEN:$ADMIN" \
  --var "DISCORD_WEBHOOK:http://127.0.0.1:$MOCK_PORT/discord" \
  --var "ANTHROPIC_BASE_URL:http://127.0.0.1:$MOCK_PORT" --var "SOURCE_BASE_URL:http://127.0.0.1:$MOCK_PORT/raw" \
  > "$LOG" 2>&1 & DEV_PID=$!
for i in $(seq 1 60); do
  curl -s -o /dev/null "http://127.0.0.1:$PORT/" && break
  kill -0 "$DEV_PID" 2>/dev/null || { cat "$LOG"; exit 1; }
  sleep 1
done
bash test/smoke.sh "http://127.0.0.1:$PORT" "$ADMIN" "http://127.0.0.1:$MOCK_PORT" || { echo "--- wrangler dev log"; tail -40 "$LOG"; exit 1; }
