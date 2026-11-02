#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )
)]

use axum::extract::DefaultBodyLimit;
use http_routing::{ProtectedRouter, PublicRouter};
use modrun::Module;
use utoipa_axum::{
    router::{OpenApiRouter, UtoipaMethodRouterExt},
    routes,
};

mod endpoint;
mod shared;

/// 单文件最大 2MiB，整体 body 略放大以容纳 boundary 等开销。
const FILE_UPLOAD_BODY_LIMIT: usize = 3 * 1024 * 1024;

pub fn module() -> Module {
    shared::domain_error::register_file_domain_mapper();
    shared::l10n::register_file_locales();
    Module::builder("file")
        .provide_group(protected_routes)
        .provide_group(public_routes)
}

fn protected_routes() -> ProtectedRouter {
    ProtectedRouter(
        OpenApiRouter::new().routes(
            routes!(endpoint::file_upload_image::handler)
                .layer(DefaultBodyLimit::max(FILE_UPLOAD_BODY_LIMIT)),
        ),
    )
}

/// 对象读取必须公开：上传接口返回的 URL 会被 `<img>` / CDN 直接访问，
/// 不携带 Authorization 头。
fn public_routes() -> PublicRouter {
    PublicRouter(OpenApiRouter::new().routes(routes!(endpoint::file_serve_static::handler)))
}
