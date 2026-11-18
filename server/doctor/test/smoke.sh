#!/usr/bin/env bash
# End-to-end check of the Doctor Worker, run against `npx wrangler dev` (local storage) and
# test/mock.mjs (stand-ins for Claude, GitHub and Discord). Nothing real is called or billed.
#
#   cat > .dev.vars <<'X'
#   ANTHROPIC_API_KEY=test-anthropic-key
#   ADMIN_TOKEN=test-admin
#   DISCORD_WEBHOOK=http://127.0.0.1:8798/discord
#   ANTHROPIC_BASE_URL=http://127.0.0.1:8798
#   SOURCE_BASE_URL=http://127.0.0.1:8798/raw
#   X
#   node test/mock.mjs 8798 &
#   npx wrangler dev --port 8797 &
#   bash test/smoke.sh http://127.0.0.1:8797 test-admin http://127.0.0.1:8798
set -euo pipefail
BASE=$1; ADMIN=$2; MOCK=$3
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; exit 1; }
field() { python3 -c "import json,sys;d=json.load(sys.stdin);print(eval(sys.argv[1]))" "$1"; }
code_of() { curl -s -o /dev/null -w '%{http_code}' "$@"; }
adm() { curl -sS -H "Authorization: Bearer $ADMIN" "$@"; }

BUNDLE='{"schema":1,"tool":"glint-doctor 0.1.0","glint_version":"9.9.9","description":"black screen, mail me at someone@example.com","settings":{"encoder":"auto"},"log_faults":"2026-11-18 10:00:01.000 capture 1: ffmpeg did not start (exit 0x1)","log_tail":"2026-11-18 10:00:00.000 capture: started from 192.168.1.20\n2026-11-18 10:00:01.000 capture 1: ffmpeg did not start (exit 0x1)","extra":"dropped"}'
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
printf '%s' "$BUNDLE" > "$T/bundle.json"
# submit <code> [file]: prints the HTTP status, leaves the answer in $T/out.
submit() { curl -sS -o "$T/out" -w '%{http_code}' -H "Authorization: Bearer $1" -H 'Content-Type: application/json' -H 'Expect:' --data-binary "@${2:-$T/bundle.json}" "$BASE/v1/submit"; }

[ "$(code_of -H 'Authorization: Bearer nope' "$BASE/admin/codes")" = 401 ] && ok "wrong admin key refused" || bad "admin key"
[ "$(submit "not-a-code")" = 401 ] && ok "no code, no upload" || bad "missing code"
[ "$(submit "2222-2222-2222")" = 401 ] && ok "unknown code refused" || bad "unknown code"
[ "$(curl -s "$MOCK/seen" | field 'len(d["claude"])')" = 0 ] && ok "nothing reached Claude without a code" || bad "Claude was called"

CODE=$(adm -H 'Content-Type: application/json' --data-binary '{"label":"smoke test"}' "$BASE/admin/codes" | field 'd["codes"][0]["code"]')
[[ "$CODE" =~ ^[2-9A-HJKMNP-Z]{4}-[2-9A-HJKMNP-Z]{4}-[2-9A-HJKMNP-Z]{4}$ ]] && ok "minted $CODE" || bad "mint gave $CODE"
adm "$BASE/admin/codes" | grep -q "${CODE//-/}" && bad "the list shows a code" || ok "the list never shows a code"

python3 -c 'import json;print(json.dumps({"schema":1,"log_tail":"x"*500000}))' > "$T/big.json"
[ "$(submit "$CODE" "$T/big.json")" = 413 ] && ok "too large refused" || bad "size cap"
echo '{"hello":1}' > "$T/other.json"
[ "$(submit "$CODE" "$T/other.json")" = 400 ] && ok "not a report refused" || bad "shape check"

# Five at once with one code: exactly one may win.
for i in 1 2 3 4 5; do submit "$CODE" > "$T/race$i" 2>/dev/null & done; wait
wins=$(cat "$T"/race* | grep -o 202 | wc -l)
[ "$wins" = 1 ] && ok "five uploads at once, one accepted" || bad "$wins uploads won the race"
[ "$(submit "$CODE")" = 403 ] && grep -q "already been used" "$T/out" && ok "the code is used up" || bad "second use: $(cat "$T/out")"

TICKET=$(adm "$BASE/admin/reports" | field 'd["reports"][0]["ticket"]')
for i in $(seq 1 30); do
  st=$(curl -sS "$BASE/v1/status/$TICKET" | field 'd["status"]'); [ "$st" = done ] && break; sleep 1
done
[ "$st" = done ] && ok "analysed (ticket $TICKET)" || bad "status stayed $st"
summary=$(curl -sS "$BASE/v1/status/$TICKET" | field 'd["user_summary"]')
[[ "$summary" == *"graphics driver"* && "$summary" != *"http"* ]] && ok "the user gets the summary, without the link" || bad "summary: $summary"
[ "$(code_of "$BASE/v1/status/000000000000000000000000")" = 404 ] && ok "unknown ticket 404" || bad "status of unknown ticket"
[ "$(code_of "$BASE/admin/reports/$TICKET")" = 401 ] && ok "the report needs the admin key" || bad "report readable without key"

adm "$BASE/admin/reports/$TICKET?format=md" > "$T/report.md"
grep -q "## Patch 1: src-tauri/src/recorder.rs" "$T/report.md" && grep -q "smoke test" "$T/report.md" && ok "report as Markdown" || bad "markdown"
adm "$BASE/admin/reports/$TICKET?include=bundle" > "$T/full.json"
grep -q "someone@example.com\|192.168.1.20\|dropped" "$T/full.json" && bad "the stored diagnostics kept personal data" || ok "stored diagnostics scrubbed again, unknown sections dropped"

curl -sS "$MOCK/seen" > "$T/seen.json"
c() { field "$1" < "$T/seen.json"; }
[ "$(c 'len(d["claude"])')" = 1 ] && ok "Claude called once" || bad "Claude calls: $(c 'len(d["claude"])')"
[ "$(c 'd["claude"][0]["key"]')" = test-anthropic-key ] && [ "$(c 'd["claude"][0]["version"]')" = 2026-10-21 ] && ok "x-api-key and anthropic-version sent" || bad "headers"
[ "$(c 'd["claude"][0]["model"]')" = claude-opus-5-5 ] && [ "$(c 'd["claude"][0]["format"]')" = json_schema ] && [ "$(c 'd["claude"][0]["effort"]')" = high ] && ok "model, effort, structured output" || bad "request body"
[ "$(c '"ref=\"v9.9.9\"" in d["claude"][0]["prompt"] and "recorder.rs" in d["claude"][0]["prompt"]')" = True ] && ok "source of the reported version attached" || bad "source"
[ "$(c '"someone@example.com" in d["claude"][0]["prompt"]')" = False ] && ok "no e-mail address in the prompt" || bad "prompt leaks"
[ "$(c 'len(d["discord"])')" = 1 ] && [ "$(c '"smoke test" in d["discord"][0] and "glint-doctor-" in d["discord"][0]')" = True ] && ok "posted to Discord with the report attached" || bad "discord"

adm -X POST "$BASE/admin/reports/$TICKET/retry" >/dev/null; sleep 4
[ "$(curl -sS "$MOCK/seen" | field 'len(d["claude"])')" = 2 ] && ok "retry analyses the stored diagnostics again" || bad "retry"

C2=$(adm -H 'Content-Type: application/json' --data-binary '{"label":"to revoke","hours":1}' "$BASE/admin/codes" | field 'd["codes"][0]["code"]')
adm -X DELETE "$BASE/admin/codes/${C2: -4}" >/dev/null
[ "$(submit "$C2")" = 403 ] && ok "a withdrawn code is refused" || bad "revoke"
adm -X DELETE "$BASE/admin/reports/$TICKET" >/dev/null
[ "$(code_of -H "Authorization: Bearer $ADMIN" "$BASE/admin/reports/$TICKET")" = 404 ] && ok "deleted" || bad "delete"
echo "all passed"
