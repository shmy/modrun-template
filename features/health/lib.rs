use http_routing::ProbeRouter;
use modrun::Module;
use utoipa_axum::{router::OpenApiRouter, routes};

mod endpoint;

pub fn module() -> Module {
    Module::builder("health").provide_group(probe_routes)
}

/// 探针单独成组：`http_server` 会把它们挂在全局限流**之外**，
/// 否则秒级探测会耗光令牌桶并触发误判重启。
fn probe_routes() -> ProbeRouter {
    ProbeRouter(
        OpenApiRouter::new()
            .routes(routes!(endpoint::livez::handler))
            .routes(routes!(endpoint::readyz::handler))
            .routes(routes!(endpoint::healthz::handler)),
    )
}
