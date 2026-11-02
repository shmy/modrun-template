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

pub use http_routing::{ProbeRouter, ProtectedRouter, PublicRouter, auth_layer, global_layer};

pub fn module() -> Module {
    Module::builder("http_server").invoke(serve)
}

fn serve(
    lc: Lifecycle,
    public: Group<PublicRouter>,
    protected: Group<ProtectedRouter>,
    probes: Group<ProbeRouter>,
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

    // 全局 body 上限：对 `body_limit` 留出 multipart 边界的余量。
    //
    // 不能统一压到 `body_limit`（默认 2MiB）：文件上传路由自带更宽松的
    // `DefaultBodyLimit`（见 `file::FILE_UPLOAD_BODY_LIMIT`），外层若先压到 2MiB，
    // 内层 3MiB 永远不生效，且 multipart boundary 开销会被误判为超限。
    // JSON 接口的实际约束远低于此，真正的字段级校验在下游 extractor。
    let api = OpenApiRouter::with_openapi(openapi)
        .merge(protected_api)
        .merge(public_api)
        .layer(DefaultBodyLimit::max(
            body_limit.saturating_add(MULTIPART_OVERHEAD_BYTES),
        ));
    #[cfg(feature = "otel")]
    let api = api
        .layer(OtelInResponseLayer)
        .layer(OtelAxumLayer::default());

    let (api_router, mut open_api) = api.with_state(state.clone()).split_for_parts();

    // 探针单独组装后合并，且**不套全局限流**（见 `ProbeRouter` 文档）。
    let mut probe_api = OpenApiRouter::new();
    for router in probes {
        probe_api = probe_api.merge(router.0);
    }
    let (probe_router, probe_openapi) = probe_api.with_state(state).split_for_parts();
    // 探针路径也要出现在 OpenAPI 契约里。
    open_api.merge(probe_openapi);

    let mut app = Router::new().merge(probe_router);
    // 限流只作用于业务路由：探针（`probe_router`）已在此之外合并，
    // 避免秒级 liveness/readiness 探测耗尽令牌桶并触发误判重启。
    let busy = api_router
        .layer(global_rate_limit)
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            request_timeout,
        ));
    app = app.merge(busy);
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

    app = finish_layers(app, &cli.server.cors_origins);

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

/// 最外层两层：`l10n`（内）→ `CORS`（外）。
///
/// **顺序不可交换**：`l10n_middleware` 会把 `WebError` extension 渲染成全新的
/// `ProblemDetails` 响应体（丢弃原响应的头）。若 CORS 在它内层，错误响应就带不上
/// `access-control-*`，浏览器端只能看到 `Failed to fetch` 而拿不到 `error_code`。
/// 因此 CORS 必须是最后（最外）一层，才能给 l10n 重建的响应补头。
fn finish_layers(app: Router, cors_origins: &str) -> Router {
    // l10n 必须包在鉴权外层：WebError::into_response 只写入 extensions，
    // 需由 l10n 中间件统一渲染为 ProblemDetails（含鉴权失败）。
    let app = app.layer(middleware::from_fn(l10n_middleware));
    match cors_layer(cors_origins) {
        Some(cors) => app.layer(cors),
        None => app,
    }
}

/// multipart 边界的粗放余量：给上传路由留出 `DefaultBodyLimit` 覆盖空间。
const MULTIPART_OVERHEAD_BYTES: usize = 2 * 1024 * 1024;

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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use ::web_error::{InvalidParamsKind, WebError};
    use axum::body::Body;
    use axum::body::to_bytes;
    use axum::http::Request;
    use axum::http::header;
    use std::result;
    use tower::ServiceExt as _;

    const ORIGIN: &str = "http://app.example.com";

    async fn boom() -> result::Result<&'static str, WebError> {
        Err(WebError::L10n("account_not_found".to_string()))
    }

    async fn bad_params() -> result::Result<&'static str, WebError> {
        Err(WebError::InvalidParams {
            kind: InvalidParamsKind::Body,
            key: "json_body_missing_field".to_string(),
            field: Some("phone".to_string()),
        })
    }

    async fn ok() -> &'static str {
        "ok"
    }

    /// 错误响应必须经 l10n 渲染后仍带上 CORS 头，否则浏览器拿不到 error_code。
    #[tokio::test]
    async fn error_responses_keep_cors_headers() {
        let app = finish_layers(
            Router::new()
                .route("/domain", get(boom))
                .route("/params", get(bad_params))
                .route("/ok", get(ok)),
            ORIGIN,
        );

        for (path, expected_status) in [
            ("/domain", StatusCode::NOT_FOUND),
            ("/params", StatusCode::BAD_REQUEST),
            ("/ok", StatusCode::OK),
        ] {
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .header("origin", ORIGIN)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                resp.status(),
                expected_status,
                "unexpected status for {path}"
            );
            assert_eq!(
                resp.headers()
                    .get("access-control-allow-origin")
                    .and_then(|v| v.to_str().ok()),
                Some(ORIGIN),
                "missing CORS header on {path} (status {expected_status})"
            );
        }
    }

    /// 错误响应仍须是 problem+json 且带 error_code（l10n 渲染未被 CORS 顺序改动破坏）。
    #[tokio::test]
    async fn error_responses_are_problem_json_after_cors_outermost() {
        let app = finish_layers(Router::new().route("/domain", get(boom)), ORIGIN);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/domain")
                    .header("origin", ORIGIN)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            resp.headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some("application/problem+json")
        );
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            json.get("error_code").and_then(|v| v.as_str()),
            Some("account_not_found")
        );
    }

    /// CORS_ORIGINS 为空时不加层（纯 API / 反代场景）。
    #[tokio::test]
    async fn empty_cors_origins_adds_no_layer() {
        let app = finish_layers(Router::new().route("/ok", get(ok)), "");
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/ok")
                    .header("origin", ORIGIN)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.headers().get("access-control-allow-origin").is_none(),
            "no CORS layer expected when CORS_ORIGINS is empty"
        );
    }
}
