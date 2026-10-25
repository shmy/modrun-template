#!/usr/bin/env bash
# 可复现负载锚点：login（argon2）与 account_search（需 privileged token）。
# 依赖：curl、jq；服务已启动；变量见 e2e/env 或环境。
set -euo pipefail

BASE_URL="${BASE_URL:-http://127.0.0.1:8080}"
PHONE="${SEED_ADMIN_PHONE:-13888888888}"
PASSWORD="${SEED_ADMIN_PASSWORD:-admin123!}"
N="${N:-20}"

echo "== login x${N} against ${BASE_URL} =="
ok=0
fail=0
for i in $(seq 1 "$N"); do
  code=$(curl -sS -o /tmp/modrun-login.json -w '%{http_code}' \
    -X POST "${BASE_URL}/api/v1/identity/login" \
    -H 'content-type: application/json' \
    -d "{\"phone\":\"${PHONE}\",\"password\":\"${PASSWORD}\"}")
  if [[ "$code" == "200" ]]; then
    ok=$((ok + 1))
  else
    fail=$((fail + 1))
    echo "login #$i -> $code $(head -c 120 /tmp/modrun-login.json)"
  fi
done
echo "login ok=$ok fail=$fail"

TOKEN=$(jq -r '.access_token // empty' /tmp/modrun-login.json)
if [[ -z "$TOKEN" ]]; then
  echo "no access_token; skip account_search" >&2
  exit 1
fi

echo "== account_search x${N} =="
ok=0
fail=0
for i in $(seq 1 "$N"); do
  code=$(curl -sS -o /tmp/modrun-search.json -w '%{http_code}' \
    "${BASE_URL}/api/v1/accounts?limit=10" \
    -H "authorization: Bearer ${TOKEN}")
  if [[ "$code" == "200" ]]; then
    ok=$((ok + 1))
  else
    fail=$((fail + 1))
    echo "search #$i -> $code $(head -c 120 /tmp/modrun-search.json)"
  fi
done
echo "search ok=$ok fail=$fail"
