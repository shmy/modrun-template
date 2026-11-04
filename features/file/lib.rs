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
use http_routing::ProtectedRouter;
#[cfg(feature = "blob-fs")]
use http_routing::PublicRouter;
use modrun::Module;
use utoipa_axum::{
    router::{OpenApiRouter, UtoipaMethodRouterExt},
    routes,
};

pub mod attachment;
pub mod attachment_refs;

mod endpoint;
mod shared;

pub use shared::staging_gc;

/// 单文件最大 2MiB，整体 body 略放大以容纳 boundary 等开销。
const FILE_UPLOAD_BODY_LIMIT: usize = 3 * 1024 * 1024;

pub fn module() -> Module {
    shared::domain_error::register_file_domain_mapper();
    shared::l10n::register_file_locales();
    let module = Module::builder("file").provide_group(protected_routes);
    // COS 等远端后端自行提供公网读；仅 blob-fs 需要本进程挂静态回源路由。
    #[cfg(feature = "blob-fs")]
    let module = module.provide_group(public_routes);
    module
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
/// 不携带 Authorization 头。仅 `blob-fs` 注册（见 `module()`）。
#[cfg(feature = "blob-fs")]
fn public_routes() -> PublicRouter {
    PublicRouter(OpenApiRouter::new().routes(routes!(endpoint::file_serve_static::handler)))
}
