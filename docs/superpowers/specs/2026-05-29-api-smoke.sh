#!/usr/bin/env bash
# Manual smoke test for the tmux-style HTTP API.
# Usage:
#   1. Launch the binary in another terminal:  cargo run --release -- --api-addr 127.0.0.1:7878
#   2. Run this script:                         bash docs/superpowers/specs/2026-05-29-api-smoke.sh
#   3. (Override with API=127.0.0.1:7889 bash ... if 7878 is taken)
set -euo pipefail
API=${API:-127.0.0.1:7878}

echo "list tabs"
curl -s "$API/tabs" | jq

echo "create tab"
CREATE=$(curl -sX POST "$API/tabs")
echo "$CREATE" | jq
NEW=$(echo "$CREATE" | jq -r .id)
echo "   created id=$NEW"

echo "active tab"
curl -s "$API/tabs/active" | jq

echo "activate $NEW"
curl -sX POST "$API/tabs/$NEW/activate" | jq

echo "screen capture"
curl -s "$API/tabs/$NEW/screen"

echo "inject 'echo hi' then Enter"
curl -sX POST --data 'echo hi' "$API/tabs/$NEW/input" | jq
curl -sX POST --data 'Enter' "$API/tabs/$NEW/keys" | jq

echo "Ctrl-C"
curl -sX POST --data 'C-c' "$API/tabs/$NEW/keys" | jq

echo "legacy /debug/screen alias"
curl -s "$API/debug/screen"

echo "close tab $NEW"
curl -sX DELETE "$API/tabs/$NEW" | jq

echo "list tabs"
curl -s "$API/tabs" | jq
