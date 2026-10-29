#!/usr/bin/env bash
# 生产 default（server 的 blob-cos+otlp）下核对 shared_contract 依赖图快照。
# 失败 = 契约又沾上框架/DB，或额外 crate 依赖了 shared_contract（增量编译回潮）。
# 更新快照：just tree-server-bless
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

normalize() {
  # 去掉版本号与绝对 path，保证跨机器可对比
  sed -E 's/ v[^ ]+//g; s/ \([^)]*\)//g'
}

diff_one() {
  local title="$1"
  local cmd="$2"
  local golden="$3"
  local actual
  actual="$(eval "$cmd" | normalize)"
  if ! diff -u "$golden" <(printf '%s\n' "$actual"); then
    echo "FAIL: $title 与 golden 不一致：$golden" >&2
    echo "若有意变更，跑：just tree-server-bless" >&2
    return 1
  fi
  echo "OK: $title"
}

# 不传 --all-features：对齐生产 default（blob-cos + otlp）
diff_one \
  "server ← shared_contract（invert depth 1）" \
  "cargo tree -p server --edges normal -i shared_contract --depth 1 --charset ascii" \
  "$ROOT/bin/server/testdata/shared_contract_invert_depth1.txt"

diff_one \
  "shared_contract 直接依赖（depth 1）" \
  "cargo tree -p shared_contract --edges normal --depth 1 --charset ascii" \
  "$ROOT/bin/server/testdata/shared_contract_deps_depth1.txt"
