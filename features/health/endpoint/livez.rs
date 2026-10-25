use axum::Json;
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub struct LivezResponse {
    /// 进程能处理请求；不访问 DB（适合 K8s liveness）。
    pub status: &'static str,
}

#[utoipa::path(
    get,
    path = "/livez",
    operation_id = "livez",
    tag = "health",
    responses((status = 200, body = LivezResponse)),
)]
pub async fn handler() -> Json<LivezResponse> {
    Json(LivezResponse { status: "ok" })
}
