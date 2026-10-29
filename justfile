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

# 生产 default（blob-cos + otlp）依赖图轻量快照，便于发现契约/infra 回潮
tree-server:
    cargo tree -p server --edges normal -i shared_contract
    @echo "---"
    cargo tree -p server --edges normal --depth 1

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

sqlx_up:
    cargo run --package migrator

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

# 需要显式 blob-fs 才能编过 WebState/Blob 的 crate（无 default feature）
crate_needs_blob := "web_state identity file health http_server http_auth http_routing scheduler"

# 快门（迭代中小步）：格式 + clippy（clippy 覆盖类型检查），秒级反馈
lint crate:
    #!/usr/bin/env bash
    set -euo pipefail
    set -a && [ -f .env ] && source .env
    extra=""
    case " {{crate_needs_blob}} " in
      *" {{crate}} "*) extra="--features blob-fs" ;;
    esac
    cargo fmt --all -- --check
    cargo clippy -p {{crate}} $extra --all-targets

# 全门（端点完成 / 提交前）：先快门，再架构测试 + 定向测试
ai-check crate: (lint crate)
    #!/usr/bin/env bash
    set -euo pipefail
    set -a && [ -f .env ] && source .env
    extra=""
    case " {{crate_needs_blob}} " in
      *" {{crate}} "*) extra="--features blob-fs" ;;
    esac
    echo "=== Architecture test ==="
    cargo test -p server arch_test --quiet 2>&1 | tail -3
    echo "=== Test {{crate}} ==="
    cargo test -p {{crate}} $extra --quiet 2>&1 | tail -10
    echo "--- Done ---"

pre_commit:
    #!/usr/bin/env bash
    set -euo pipefail
    set -a && [ -f .env ] && source .env
    cargo machete
    cargo fixrs
    cargo fmt --all
    # --all-features 会 unification 进全部后端（含 blob-fs）；质量门保留，与 just build 的轻量图分离
    cargo clippy --workspace --all-targets --all-features
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
