use http_server::PublicRouter;
use modrun::Module;
use utoipa_axum::{router::OpenApiRouter, routes};

mod endpoint;

pub fn module() -> Module {
    Module::builder("health").provide_group(api_routes)
}

fn api_routes() -> PublicRouter {
    PublicRouter(
        OpenApiRouter::new()
            .routes(routes!(endpoint::livez::handler))
            .routes(routes!(endpoint::readyz::handler))
            .routes(routes!(endpoint::healthz::handler)),
    )
}
