dev:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo watch -q -c -x "run -p server" -w bin -w features -w infra -w crates

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

ai-check crate:
    #!/usr/bin/env bash
    set -euo pipefail
    set -a && [ -f .env ] && source .env
    echo "=== Architecture test ==="
    cargo test -p server arch_test --quiet 2>&1 | tail -3
    echo "=== Format check ==="
    cargo fmt --all -- --check 2>&1 | tail -3
    echo "=== Check {{crate}} ==="
    cargo check -p {{crate}} --message-format=short 2>&1 | tail -5
    echo "=== Test {{crate}} ==="
    cargo test -p {{crate}} --quiet 2>&1 | tail -10
    echo "--- Done ---"

pre_commit:
    #!/usr/bin/env bash
    set -euo pipefail
    set -a && [ -f .env ] && source .env
    cargo machete
    cargo fixrs
    cargo fmt --all
    cargo clippy --workspace --all-targets --all-features
    cargo test -p server arch_test
    
test:
    cargo test --workspace

test-features:
    #!/usr/bin/env bash
    set -euo pipefail
    set -a && [ -f .env ] && source .env
    echo "=== cache_kit redb ==="
    cargo test -p cache_kit --features redb --quiet
    echo "=== cache_kit redis (testcontainers) ==="
    cargo test -p cache_kit --features redis --quiet
    echo "=== blob_kit test-utils ==="
    cargo test -p blob_kit --features test-utils --quiet
    echo "=== file ==="
    cargo test -p file --quiet
    echo "=== job_queue_kit sqlite ==="
    cargo test -p job_queue_kit --no-default-features --features sqlite --quiet
    echo "--- Done ---"
