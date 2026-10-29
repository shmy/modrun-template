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

mkdir -p "$CONTRACT" "$FEAT/endpoint" "$FEAT/shared" \
  "$FEAT/locales/en-US" "$FEAT/locales/zh-CN"

cat >"$CONTRACT/Cargo.toml" <<EOF
[package]
name = "${NAME}_contract"
version = "0.1.0"
edition = "2024"
publish = false

[lib]
path = "lib.rs"

[lints]
workspace = true

[dependencies]
thiserror = { workspace = true }
EOF

cat >"$CONTRACT/lib.rs" <<EOF
//! ${NAME} 跨域契约。
//!
//! **何时放 contract：**
//! - 跨域需要的错误枚举、公开读模型、Id 别名 → 放这里（纯 serde / thiserror）。
//! - 禁止：utoipa / sqlx / axum / validify / argon2 / runtime（见 arch_test）。
//! - 仅本域使用的 DTO / OpenAPI schema → 留在 features/${NAME}/。
//!
//! 参考 identity_contract：当前以错误为主；读模型待跨域消费时再下沉。
mod error;

pub use error::{DomainError, L10nKeyedError};
EOF

cat >"$CONTRACT/error.rs" <<'EOF'
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DomainError {
    #[error("not_implemented")]
    NotImplemented,
}

pub trait L10nKeyedError {
    fn l10n_key(&self) -> &'static str;
}

impl L10nKeyedError for DomainError {
    fn l10n_key(&self) -> &'static str {
        match self {
            Self::NotImplemented => "not_implemented",
        }
    }
}
EOF

cat >"$FEAT/Cargo.toml" <<EOF
[package]
name = "${NAME}"
version = "0.1.0"
edition = "2024"
publish = false

[lib]
path = "lib.rs"

[lints]
workspace = true

[features]
# 无 default；单独 check/test 由 just lint/ai-check 显式传 --features blob-fs
default = []
blob-fs = ["web_state/blob-fs"]
blob-cos = ["web_state/blob-cos"]

[dependencies]
axum = { workspace = true }
${NAME}_contract = { path = "../${NAME}_contract" }
http_routing = { workspace = true }
locale = { workspace = true }
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
mod shared;

use http_routing::ProtectedRouter;
use modrun::Module;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub fn module() -> Module {
    shared::l10n::register_${NAME}_locales();
    // shared::domain_error::register_${NAME}_domain_mapper(); // 有域错误时打开
    Module::builder("${NAME}").provide_group(protected_routes)
}

fn protected_routes() -> ProtectedRouter {
    ProtectedRouter(
        OpenApiRouter::new().routes(routes!(endpoint::health_ping::handler)),
    )
}
EOF

cat >"$FEAT/shared.rs" <<'EOF'
pub mod l10n;
EOF

cat >"$FEAT/shared/l10n.rs" <<EOF
/// 域自注册 FTL；infra/locale 只保留 shared.ftl。
pub fn register_${NAME}_locales() {
    locale::register_ftl("en-US", include_str!("../locales/en-US/${NAME}.ftl"));
    locale::register_ftl("zh-CN", include_str!("../locales/zh-CN/${NAME}.ftl"));
}
EOF

cat >"$FEAT/locales/en-US/${NAME}.ftl" <<EOF
not_implemented = Not implemented
EOF

cat >"$FEAT/locales/zh-CN/${NAME}.ftl" <<EOF
not_implemented = 尚未实现
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
// State 只依赖需要的 trait：web_state::{HasPool, HasBlob, ...}

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

cat <<EOF
created $CONTRACT and $FEAT

清单（何时放 contract / locale / 状态码 / 怎么验证）：
  1. workspace Cargo.toml members += features/${NAME}, features/${NAME}_contract
  2. workspace.dependencies: ${NAME} = { path = "features/${NAME}", default-features = false }
  3. bin/server/Cargo.toml dependencies += ${NAME}
  4. bin/server/main.rs .module(${NAME}::module())  （须在 http_server 之前，以便 register_ftl 生效）
  5. contract：仅跨域错误/读模型；OpenAPI/validify 放 features/${NAME} DTO
  6. locale：已生成 locales/{en-US,zh-CN}/${NAME}.ftl + shared/l10n.rs 自注册
  7. 状态码：遵循 web_error 命名约定（*_not_found→404、*_forbidden→403、*_duplicated→409、
     *_invalid_credentials / *_token_invalid→401）；勿改 infra 白名单
  8. 验证（窄 feature，禁止 --all-features / --workspace 迭代）：
       just lint ${NAME}
       just ai-check ${NAME}
       just check                    # 开发 feature 集
       cargo check -p server         # 生产 default
EOF
