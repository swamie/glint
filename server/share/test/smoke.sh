#!/usr/bin/env bash
# End-to-end check of the share Worker, run against `npx wrangler dev` (local R2).
#   echo 'UPLOAD_TOKEN=test-key' > .dev.vars && npx wrangler dev --port 8799 &
#   bash test/smoke.sh http://127.0.0.1:8799 test-key some-video.mp4
# Uploads the video in 5 MiB pieces (Glint uses 16 MiB), then checks the share page,
# Range requests, HEAD, the poster, usage, a wrong key, and deletion.
set -euo pipefail
BASE=$1; KEY=$2; VIDEO=$3
PART=$((5 * 1024 * 1024))
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; exit 1; }
api() { curl -sS -H "Authorization: Bearer $KEY" -H "Expect:" "$@"; }

code=$(curl -s -o /dev/null -w '%{http_code}' -H "Authorization: Bearer nope" "$BASE/api/usage")
[ "$code" = 401 ] && ok "wrong key refused" || bad "wrong key gave $code"

clips() { api "$BASE/api/usage" | python3 -c 'import json,sys;print(json.load(sys.stdin)["clips"])'; }
BEFORE=$(clips)
SIZE=$(stat -c %s "$VIDEO")
start=$(api -H 'Content-Type: application/json' --data-binary "{\"title\":\"Clutch <1v3> & \\\"ace\\\" 🎯\",\"uploader\":\"Rubust\",\"game\":\"Minecraft\",\"width\":1280,\"height\":720,\"duration\":20,\"size\":$SIZE}" "$BASE/api/uploads")
ID=$(echo "$start" | python3 -c 'import json,sys;print(json.load(sys.stdin)["id"])')
UP=$(echo "$start" | python3 -c 'import json,sys,urllib.parse;print(urllib.parse.quote(json.load(sys.stdin)["uploadId"],safe=""))')
ok "started $ID"

[ "$(curl -s -o /dev/null -w '%{http_code}' "$BASE/c/$ID")" = 404 ] && ok "pending clip hidden" || bad "pending clip shown"

split -b $PART -d -a 3 "$VIDEO" "$T/p"
parts="["; n=0
for f in "$T"/p*; do
  n=$((n + 1))
  etag=$(api -X PUT -H 'Content-Type: application/octet-stream' -T "$f" "$BASE/api/uploads/$ID/parts/$n?uploadId=$UP" | python3 -c 'import json,sys;print(json.load(sys.stdin)["etag"])')
  parts+="{\"partNumber\":$n,\"etag\":\"$etag\"},"
done
parts="${parts%,}]"
ok "$n pieces sent"

ffmpeg -loglevel error -y -ss 5 -i "$VIDEO" -frames:v 1 -q:v 3 "$T/poster.jpg"
api -X PUT -H 'Content-Type: image/jpeg' -T "$T/poster.jpg" "$BASE/api/uploads/$ID/poster?uploadId=$UP" >/dev/null && ok "poster sent"

done_=$(api -H 'Content-Type: application/json' --data-binary "{\"parts\":$parts}" "$BASE/api/uploads/$ID/complete?uploadId=$UP")
URL=$(echo "$done_" | python3 -c 'import json,sys;print(json.load(sys.stdin)["url"])')
[ "$URL" = "$BASE/c/$ID" ] && ok "link $URL" || bad "complete said $done_"

page=$(curl -sS "$URL")
grep -q '<meta name="theme-color" content="#3B82F6">' <<<"$page" && ok "blue theme-color" || bad "theme-color"
grep -q 'og:title" content="Clutch &lt;1v3&gt; &amp; &quot;ace&quot; 🎯"' <<<"$page" && ok "title escaped" || bad "title"
grep -q 'og:description" content="Rubust · Minecraft"' <<<"$page" && ok "uploader · game line" || bad "description"
grep -q "og:video\" content=\"$BASE/v/$ID.mp4\"" <<<"$page" && ok "og:video" || bad "og:video"
grep -q 'twitter:player:stream" content=' <<<"$page" && ok "twitter player stream" || bad "twitter"

curl -sS "$BASE/v/$ID.mp4" -o "$T/back.mp4"
cmp -s "$T/back.mp4" "$VIDEO" && ok "video comes back byte for byte" || bad "video differs"
h=$(curl -sS -D - -o "$T/r" -H 'Range: bytes=100-199' "$BASE/v/$ID.mp4")
grep -qi "^HTTP/1.1 206" <<<"$h" && grep -qi "content-range: bytes 100-199/$SIZE" <<<"$h" && [ "$(stat -c %s "$T/r")" = 100 ] && ok "Range 206" || bad "range: $h"
h=$(curl -sS -D - -o /dev/null -H 'Range: bytes=-50' "$BASE/v/$ID.mp4")
grep -qi "content-range: bytes $((SIZE - 50))-$((SIZE - 1))/$SIZE" <<<"$h" && ok "suffix Range" || bad "suffix: $h"
h=$(curl -sS -I "$BASE/v/$ID.mp4")
grep -qi "content-length: $SIZE" <<<"$h" && grep -qi "content-type: video/mp4" <<<"$h" && grep -qi "accept-ranges: bytes" <<<"$h" && ok "HEAD" || bad "head: $h"
[ "$(curl -s -o /dev/null -w '%{content_type}' "$BASE/t/$ID.jpg")" = image/jpeg ] && ok "poster served" || bad "poster"

[ "$(clips)" = $((BEFORE + 1)) ] && ok "usage counts it" || bad "usage"
api -X DELETE "$BASE/api/clips/$ID" >/dev/null
[ "$(curl -s -o /dev/null -w '%{http_code}' "$URL")" = 404 ] && [ "$(curl -s -o /dev/null -w '%{http_code}' "$BASE/v/$ID.mp4")" = 404 ] && ok "deleted" || bad "still there after delete"
[ "$(clips)" = "$BEFORE" ] && ok "usage drops after delete" || bad "usage after delete"
echo "all passed"
