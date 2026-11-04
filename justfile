# 本地开发 feature 集：cache-pg + blob-fs + job-queue-pg（无 otlp）。
# AI 迭代默认只用 `just lint <crate>` / `just ai-check <crate>`；禁止默认 `cargo test --workspace` / `--all-features`。
dev_features := "cache-pg,blob-fs,job-queue-pg"

# 日常开发：blob-fs + 无 otlp（对象落 BLOB_FILE_ROOT）。对齐生产 COS/OTLP 请用 `cargo run -p server` / `just build`。
dev:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo watch -q -c -x "run -p server --no-default-features --features {{dev_features}}" -w bin -w features -w infra -w crates

# 与 just dev 相同 feature 集的快速类型检查（非生产 default）
check:
    cargo check -p server --no-default-features --features {{dev_features}}

# 生产 default（blob-cos + otlp）依赖图快照门禁；失败=契约/引用面回潮
# 与 arch_test::server_shared_contract_tree_matches_golden 同源（经 arch_test 进 ai-check / pre_commit / CI）
tree-server:
    @bash scripts/check-server-tree.sh

# 有意变更依赖图后刷新 golden（须审 diff 再提交）
tree-server-bless:
    #!/usr/bin/env bash
    set -euo pipefail
    normalize() { sed -E 's/ v[^ ]+//g; s/ \([^)]*\)//g'; }
    cargo tree -p server --edges normal -i shared_contract --depth 1 --charset ascii --color never \
      | normalize > bin/server/testdata/shared_contract_invert_depth1.txt
    cargo tree -p shared_contract --edges normal --depth 1 --charset ascii --color never \
      | normalize > bin/server/testdata/shared_contract_deps_depth1.txt
    echo "blessed bin/server/testdata/shared_contract_*.txt"
    just tree-server

build:
    cargo build --package server --release --locked
    ls -lh target/release

build_release_ci:
    cargo build --package server --profile release-ci --locked
    ls -lh target/release-ci

build_linux_x86_64_gnu:
    cargo build --package server --release --target x86_64-unknown-linux-gnu --locked
    ls -lh target/x86_64-unknown-linux-gnu/release

build_linux_x86_64_gnu_ci:
    cargo build --package server --profile release-ci --target x86_64-unknown-linux-gnu --locked
    ls -lh target/x86_64-unknown-linux-gnu/release-ci

# 业务迁移 + apalis schema。空库须 SQLX_OFFLINE：apalis-postgres 宏要用包内 .sqlx
# 才能在 `apalis.jobs` 尚未创建时编过 migrator；跑完 setup 后即可正常在线校验。
sqlx_up:
    #!/usr/bin/env bash
    set -euo pipefail
    set -a && [ -f .env ] && source .env
    SQLX_OFFLINE=true cargo run --package migrator

db-up:
    docker compose up -d

e2e:
    hurl --test --variables-file e2e/env e2e/identity.hurl

e2e_file:
    hurl --test --variables-file e2e/env e2e/file.hurl

who-uses symbol:
    @rg -l --type rust '{{symbol}}' features crates infra bin | sort

# 生成 features/<name> + <name>_contract 骨架（仍需手改 workspace / main.rs）
new-feature name:
    @bash scripts/new-feature.sh {{name}}

# 负载锚点：服务需已启动；N=次数（默认 20）
load-baseline:
    @bash scripts/load-baseline.sh

# 单独 check 时需要额外 --features 的 crate（WebState 字段按 feature 门控）：
# - blob 组：用到 HasBlob / Blob 参数
# - queue 组：用到 HasJobs / JobBus（scheduler 的 HasJobBus 读 WebState.jobs）
# health / http_server / http_routing / http_auth 只用 pool+cache，无需额外 feature。
crate_needs_blob := "web_state identity file"
crate_needs_queue := "web_state scheduler"

# 快门（迭代中小步）：格式 + clippy（clippy 覆盖类型检查），秒级反馈
lint crate:
    #!/usr/bin/env bash
    set -euo pipefail
    set -a && [ -f .env ] && source .env
    feats=""
    case " {{crate_needs_blob}} " in *" {{crate}} "*) feats="blob-fs" ;; esac
    case " {{crate_needs_queue}} " in *" {{crate}} "*) feats="${feats:+$feats,}job-queue-pg" ;; esac
    extra=""
    [ -n "$feats" ] && extra="--features $feats"
    cargo fmt --all -- --check
    cargo clippy -p {{crate}} $extra --all-targets

# 全门（端点完成 / 提交前）：先快门，再架构测试 + 定向测试
ai-check crate: (lint crate)
    #!/usr/bin/env bash
    set -euo pipefail
    set -a && [ -f .env ] && source .env
    feats=""
    case " {{crate_needs_blob}} " in *" {{crate}} "*) feats="blob-fs" ;; esac
    case " {{crate_needs_queue}} " in *" {{crate}} "*) feats="${feats:+$feats,}job-queue-pg" ;; esac
    extra=""
    [ -n "$feats" ] && extra="--features $feats"
    echo "=== Architecture test ==="
    cargo test -p server arch_test --quiet 2>&1 | tail -5
    echo "=== Test {{crate}} ==="
    cargo test -p {{crate}} $extra --quiet 2>&1 | tail -10
    echo "--- Done ---"

# 与 CI lint / pre_commit 同口径：全后端覆盖，但 job-queue pg/sqlite 分趟
# （apalis-postgres / apalis-sqlite 的 sqlx 宏不能共用 DATABASE_URL）。
clippy_workspace:
    #!/usr/bin/env bash
    set -euo pipefail
    set -a && [ -f .env ] && source .env
    # 主趟：其余 crate 仍 --all-features；排除会让 pg/sqlite 冲突或需显式后端的根包。
    # scheduler 也排除：它 `--all-features` 会同时打开 job-queue-pg + job-queue-sqlite，
    # 把两个 apalis 后端编进同一图（apalis 宏共用 DATABASE_URL，必炸）。
    cargo clippy --workspace --all-targets --all-features \
        --exclude job_queue_kit \
        --exclude job_queue \
        --exclude web_state \
        --exclude scheduler \
        --exclude server
    # 上述根包：显式开全后端，但 job-queue 只开 pg（DATABASE_URL=PG + apalis schema）。
    # web_state 必须同时给出 blob 与队列后端（字段按 feature 门控）；分两趟避免 pg/sqlite 冲突。
    cargo clippy -p web_state --all-targets --features "job-queue-pg,blob-fs,blob-cos"
    SQLX_OFFLINE=true cargo clippy -p web_state --all-targets \
        --no-default-features --features "job-queue-sqlite,blob-fs,blob-cos"
    cargo clippy -p scheduler --all-targets --features "job-queue-pg"
    SQLX_OFFLINE=true cargo clippy -p scheduler --all-targets \
        --no-default-features --features "job-queue-sqlite"
    cargo clippy -p job_queue --all-targets --features pg
    cargo clippy -p job_queue_kit --all-targets --features pg
    cargo clippy -p server --all-targets --no-default-features \
        --features "cache-pg,cache-redb,cache-redis,blob-cos,blob-fs,job-queue-pg,otlp"
    # sqlite 队列：只用 apalis-sqlite 包内自带 .sqlx，不碰本仓库 query! 在线校验。
    SQLX_OFFLINE=true cargo clippy -p job_queue_kit --all-targets \
        --no-default-features --features sqlite
    SQLX_OFFLINE=true cargo clippy -p job_queue --all-targets \
        --no-default-features --features sqlite

pre_commit:
    #!/usr/bin/env bash
    set -euo pipefail
    set -a && [ -f .env ] && source .env
    cargo machete
    cargo fixrs
    cargo fmt --all
    just clippy_workspace
    # 供应链门禁：advisories/bans/licenses/sources 四项（advisory DB 需网络，约数秒）
    cargo deny check
    cargo test -p server arch_test
    
test:
    # 全量；迭代中请用 just ai-check <crate>，勿默认 --workspace / --all-features
    cargo test --workspace

test-features:
    #!/usr/bin/env bash
    set -euo pipefail
    set -a && [ -f .env ] && source .env
    echo "=== cache_kit redb ==="
    cargo test -p cache_kit --features redb --quiet
    echo "=== cache_kit redis (testcontainers, 需 Docker) ==="
    cargo test -p cache_kit --features test-redis --quiet
    echo "=== blob_kit test-utils ==="
    cargo test -p blob_kit --features test-utils --quiet
    echo "=== file ==="
    cargo test -p file --features blob-fs --quiet
    echo "=== job_queue_kit sqlite ==="
    cargo test -p job_queue_kit --no-default-features --features sqlite --quiet
    echo "--- Done ---"
