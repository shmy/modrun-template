use std::sync::LazyLock;
use std::time::Instant;

use axum::{Json, extract::State};
use serde::Serialize;
use utoipa::ToSchema;
use web_state::{HasPool, WebState};

static PROCESS_START: LazyLock<Instant> = LazyLock::new(Instant::now);

#[derive(Serialize, ToSchema)]
pub struct HealthzProcess {
    pub uptime_seconds: u64,
}

#[derive(Serialize, ToSchema)]
pub struct HealthzPostgres {
    pub size: u32,
    pub idle: u32,
    pub max_connections: u32,
    pub is_closed: bool,
}

#[derive(Serialize, ToSchema)]
pub struct HealthzResponse {
    pub process: HealthzProcess,
    pub postgres: HealthzPostgres,
}

#[utoipa::path(
    get,
    path = "/healthz",
    operation_id = "healthz",
    tag = "health",
    responses((status = 200, body = HealthzResponse)),
)]
pub async fn handler(State(state): State<WebState>) -> Json<HealthzResponse> {
    let pool = state.pool();
    let options = pool.options();
    Json(HealthzResponse {
        process: HealthzProcess {
            uptime_seconds: PROCESS_START.elapsed().as_secs(),
        },
        postgres: HealthzPostgres {
            size: pool.size(),
            idle: pool.num_idle() as u32,
            max_connections: options.get_max_connections(),
            is_closed: pool.is_closed(),
        },
    })
}
