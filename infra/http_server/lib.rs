use std::net::SocketAddr;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::{HeaderValue, Method, StatusCode};
use axum::middleware;
use axum::routing::get;
#[cfg(feature = "otel")]
use axum_tracing_opentelemetry::middleware::{OtelAxumLayer, OtelInResponseLayer};
use config::cli::Cli;
use http_auth::middleware::account_auth_middleware;
use locale::middleware::l10n_middleware;
use modrun::{Error, Group, Lifecycle, Module, Result, task_with};
use tokio::net::TcpListener;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::timeout::TimeoutLayer;
use tracing::info;
use utoipa::openapi::OpenApi;
use utoipa_axum::router::OpenApiRouter;
use utoipa_scalar::Scalar;
use utoipa_scalar::Servable as _;
use web_state::WebState;

use axum::http::header::ACCEPT;
use axum::http::header::ACCEPT_LANGUAGE;
use axum::http::header::AUTHORIZATION;
use axum::http::header::CONTENT_TYPE;

pub use http_routing::{ProtectedRouter, PublicRouter, auth_layer, global_layer};

pub fn module() -> Module {
    Module::builder("http_server").invoke(serve)
}

fn serve(
    lc: Lifecycle,
    public: Group<PublicRouter>,
    protected: Group<ProtectedRouter>,
    state: WebState,
    openapi: OpenApi,
    cli: Cli,
) -> Result<()> {
    let global_rate_limit = http_routing::global_layer(&cli).map_err(Error::hook)?;
    let addr = cli.server.listen_addr;
    let serve_addr = addr.to_string();
    let request_timeout = cli.server.request_timeout.duration();
    let body_limit = cli.server.body_limit_bytes;
    let scalar_ui_enabled = cli.server.scalar_ui_enabled && !cli.server.app_env.is_production();

    let mut public_api = OpenApiRouter::new();
    for router in public {
        public_api = public_api.merge(router.0);
    }

    let mut protected_api = OpenApiRouter::new();
    for router in protected {
        protected_api = protected_api.merge(router.0);
    }
    protected_api = protected_api.layer(middleware::from_fn_with_state(
        state.clone(),
        account_auth_middleware,
    ));

    let api = OpenApiRouter::with_openapi(openapi)
        .merge(protected_api)
        .merge(public_api)
        .layer(DefaultBodyLimit::max(body_limit));
    #[cfg(feature = "otel")]
    let api = api
        .layer(OtelInResponseLayer)
        .layer(OtelAxumLayer::default());

    let (api_router, open_api) = api.with_state(state).split_for_parts();

    let mut app = Router::new().merge(api_router);
    app = app.route(
        "/openapi.json",
        get({
            let spec = open_api.clone();
            move || {
                let spec = spec.clone();
                async move { axum::Json(spec) }
            }
        }),
    );

    if scalar_ui_enabled {
        app = app.merge(Scalar::with_url("/scalar", open_api));
    } else if cli.server.scalar_ui_enabled {
        info!("Scalar UI disabled because APP_ENV=production");
    }

    if let Some(cors) = cors_layer(&cli.server.cors_origins) {
        app = app.layer(cors);
    }

    app = app.layer(global_rate_limit);
    app = app.layer(TimeoutLayer::with_status_code(
        StatusCode::REQUEST_TIMEOUT,
        request_timeout,
    ));
    // l10n 必须包在鉴权外层：WebError::into_response 只写入 extensions，
    // 需由 l10n 中间件统一渲染为 ProblemDetails（含鉴权失败）。
    app = app.layer(middleware::from_fn(l10n_middleware));

    lc.append(task_with(
        "http.serve",
        move || async move {
            let listener = TcpListener::bind(&addr)
                .await
                .map_err(|e| Error::io(format!("bind {addr}"), e))?;
            let bound = listener
                .local_addr()
                .map_err(|e| Error::io(format!("local_addr {addr}"), e))?;
            info!("listening on http://{bound}, master: {}", cli.server.master);
            Ok(listener)
        },
        move |listener, stopped| async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(async move {
                stopped.await;
                info!("HTTP server draining connections...");
            })
            .await
            .map_err(|e| Error::io(format!("serve {serve_addr}"), e))?;
            Ok(())
        },
    ))
}

/// `CORS_ORIGINS` 空 → 不加层；非空为逗号分隔 Origin 白名单。
fn cors_layer(origins: &str) -> Option<CorsLayer> {
    let trimmed = origins.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut parsed = Vec::new();
    for origin in trimmed.split(',') {
        let origin = origin.trim();
        if origin.is_empty() {
            continue;
        }
        match origin.parse::<HeaderValue>() {
            Ok(value) => parsed.push(value),
            Err(err) => {
                tracing::warn!(origin, error = %err, "skip invalid CORS origin");
            }
        }
    }
    if parsed.is_empty() {
        return None;
    }
    Some(
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(parsed))
            .allow_methods([
                Method::GET,
                Method::POST,
                Method::PATCH,
                Method::DELETE,
                Method::OPTIONS,
            ])
            .allow_headers([AUTHORIZATION, CONTENT_TYPE, ACCEPT, ACCEPT_LANGUAGE]),
    )
}
