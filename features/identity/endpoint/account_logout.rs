use axum::{Json, extract::State};
use http_auth::AuthedAccount;
use shared_contract::Id;
use web_error::{JsonResponseType, ProblemDetails};
use web_state::{AppState, HasAuthCaps};

use crate::dto::LogoutResponse;
use crate::shared::token_ops;

#[utoipa::path(
    post,
    path = "/api/v1/identity/logout",
    operation_id = "identity_logout",
    tag = "identity",
    responses(
        (status = 200, body = LogoutResponse),
        (status = 401, description = "未认证", body = ProblemDetails)
    ),
    security(("bearerAuth" = []))
)]
pub async fn handler(
    AuthedAccount(account_id): AuthedAccount,
    State(state): State<AppState>,
) -> JsonResponseType<LogoutResponse> {
    run(&state, account_id).await
}

async fn run(state: &impl HasAuthCaps, account_id: Id) -> JsonResponseType<LogoutResponse> {
    token_ops::revoke_tokens(state.cache(), state.token_helper(), account_id).await?;
    Ok(Json(LogoutResponse { logged_out: true }))
}
