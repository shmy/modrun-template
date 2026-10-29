use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::Serialize;
use utoipa::ToSchema;
use web_state::{AppState, HasPool};

#[derive(Serialize, ToSchema)]
pub struct ReadyzResponse {
    pub ready: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

async fn check_db(state: &impl HasPool) -> Result<(), &'static str> {
    let mut conn = state.pool().acquire().await.map_err(|_| "postgres_pool")?;
    sqlx::query_scalar!(r#"SELECT 1 as "one!""#)
        .fetch_one(&mut *conn)
        .await
        .map_err(|_| "postgres")?;
    Ok(())
}

#[utoipa::path(
    get,
    path = "/readyz",
    operation_id = "readyz",
    tag = "health",
    responses(
        (status = 200, description = "依赖可用", body = ReadyzResponse),
        (status = 503, description = "依赖不可用", body = ReadyzResponse),
    ),
)]
pub async fn handler(State(state): State<AppState>) -> impl IntoResponse {
    match check_db(&state).await {
        Ok(()) => Json(ReadyzResponse {
            ready: true,
            reason: None,
        })
        .into_response(),
        Err(reason) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ReadyzResponse {
                ready: false,
                reason: Some(reason.into()),
            }),
        )
            .into_response(),
    }
}
