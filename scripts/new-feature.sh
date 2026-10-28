#!/usr/bin/env bash
# 按 identity 蓝本生成 features/<name> + <name>_contract 骨架。
set -euo pipefail

NAME="${1:-}"
if [[ -z "$NAME" || ! "$NAME" =~ ^[a-z][a-z0-9_]*$ ]]; then
  echo "usage: just new-feature <snake_case_name>" >&2
  exit 1
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FEAT="$ROOT/features/$NAME"
CONTRACT="$ROOT/features/${NAME}_contract"

if [[ -e "$FEAT" || -e "$CONTRACT" ]]; then
  echo "already exists: $FEAT or $CONTRACT" >&2
  exit 1
fi

mkdir -p "$CONTRACT" "$FEAT/endpoint"

cat >"$CONTRACT/Cargo.toml" <<EOF
[package]
name = "${NAME}_contract"
version = "0.1.0"
edition = "2024"

[lib]
path = "lib.rs"

[lints]
workspace = true

[dependencies]
serde = { workspace = true }
thiserror = { workspace = true }
EOF

cat >"$CONTRACT/lib.rs" <<EOF
//! ${NAME} 跨域契约（错误 / 跨域读模型；勿放 argon2、utoipa、runtime）。
pub mod error;

pub use error::DomainError;
EOF

cat >"$CONTRACT/error.rs" <<'EOF'
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DomainError {
    #[error("not_implemented")]
    NotImplemented,
}
EOF

cat >"$FEAT/Cargo.toml" <<EOF
[package]
name = "${NAME}"
version = "0.1.0"
edition = "2024"

[lib]
path = "lib.rs"

[lints]
workspace = true

[features]
# 单独 cargo check/test -p ${NAME} 时激活 Blob；server 经 workspace default-features=false 引入时不会带上
default = ["dev-blob"]
dev-blob = ["web_state/blob-fs"]

[dependencies]
axum = { workspace = true }
${NAME}_contract = { path = "../${NAME}_contract" }
http_routing = { workspace = true }
modrun = { workspace = true }
serde = { workspace = true }
utoipa = { workspace = true }
utoipa-axum = { workspace = true }
web_error = { workspace = true }
web_state = { workspace = true }
EOF

cat >"$FEAT/lib.rs" <<EOF
//! ${NAME} 业务域。蓝本：features/identity。
mod endpoint;

use http_routing::ProtectedRouter;
use modrun::Module;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub fn module() -> Module {
    Module::builder("${NAME}").provide_group(protected_routes)
}

fn protected_routes() -> ProtectedRouter {
    ProtectedRouter(
        OpenApiRouter::new().routes(routes!(endpoint::health_ping::handler)),
    )
}
EOF

mkdir -p "$FEAT/endpoint"
cat >"$FEAT/endpoint/mod.rs" <<'EOF'
pub mod health_ping;
EOF

cat >"$FEAT/endpoint/health_ping.rs" <<EOF
use axum::Json;
use serde::Serialize;
use utoipa::ToSchema;
use web_error::JsonResponseType;
// handler 统一叫 handler；含副作用时再拆 execute

#[derive(Serialize, ToSchema)]
pub struct PingResponse {
    pub ok: bool,
}

#[utoipa::path(
    get,
    path = "/api/v1/${NAME}/ping",
    operation_id = "${NAME}_ping",
    tag = "${NAME}",
    responses((status = 200, body = PingResponse))
)]
pub async fn handler() -> JsonResponseType<PingResponse> {
    Ok(Json(PingResponse { ok: true }))
}
EOF

echo "created $CONTRACT and $FEAT"
echo "next:"
echo "  1. workspace Cargo.toml members += features/${NAME}, features/${NAME}_contract"
echo "  2. workspace.dependencies: ${NAME} = { path = \"features/${NAME}\", default-features = false }"
echo "  3. bin/server/Cargo.toml dependencies += ${NAME}"
echo "  4. bin/server/main.rs .module(${NAME}::module())"
echo "  5. 补 locale / migration（按需）"
echo "  6. just ai-check ${NAME}"
