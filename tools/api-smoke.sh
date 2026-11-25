#!/usr/bin/env bash
# Manual smoke test for the tmux-style HTTP API.
# Usage:
#   1. Launch with RTERMINAL_WEB_TOKEN set (or --web-token <TOKEN>).
#   2. Run: TOKEN=<same token> bash tools/api-smoke.sh
#   3. Override API=127.0.0.1:7889 if needed.
set -euo pipefail
API=${API:-127.0.0.1:7878}
TOKEN=${TOKEN:-${RTERMINAL_WEB_TOKEN:-}}
if [[ -z "$TOKEN" ]]; then
  echo "set TOKEN or RTERMINAL_WEB_TOKEN before running api-smoke.sh" >&2
  exit 1
fi
AUTH=(-H "X-Rterminal-Token: $TOKEN")

echo "list tabs"
curl -s "${AUTH[@]}" "$API/tabs" | jq

echo "create tab"
CREATE=$(curl -s "${AUTH[@]}" -X POST "$API/tabs")
echo "$CREATE" | jq
NEW=$(echo "$CREATE" | jq -r .id)
echo "   created id=$NEW"

echo "active tab"
curl -s "${AUTH[@]}" "$API/tabs/active" | jq

echo "activate $NEW"
curl -s "${AUTH[@]}" -X POST "$API/tabs/$NEW/activate" | jq

echo "screen capture"
curl -s "${AUTH[@]}" "$API/tabs/$NEW/screen"

echo "inject 'echo hi' then Enter"
curl -s "${AUTH[@]}" -X POST --data 'echo hi' "$API/tabs/$NEW/input" | jq
curl -s "${AUTH[@]}" -X POST --data 'Enter' "$API/tabs/$NEW/keys" | jq

echo "Ctrl-C"
curl -s "${AUTH[@]}" -X POST --data 'C-c' "$API/tabs/$NEW/keys" | jq

echo "legacy /debug/screen alias"
curl -s "${AUTH[@]}" "$API/debug/screen"

echo "close tab $NEW"
curl -s "${AUTH[@]}" -X DELETE "$API/tabs/$NEW" | jq

echo "list tabs"
curl -s "${AUTH[@]}" "$API/tabs" | jq
