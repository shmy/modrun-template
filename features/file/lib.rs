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
    Module::builder("file").provide_group(protected_routes)
}

fn protected_routes() -> ProtectedRouter {
    ProtectedRouter(
        OpenApiRouter::new().routes(
            routes!(endpoint::file_upload_image::handler)
                .layer(DefaultBodyLimit::max(FILE_UPLOAD_BODY_LIMIT)),
        ),
    )
}
