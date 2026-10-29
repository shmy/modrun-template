use axum::{Json, extract::State};
use http_auth::AuthedAccount;
use web_error::{JsonResponseType, ProblemDetails};
use web_state::{AppState, HasPool};

use crate::dto::AccountPublic;
use crate::repository;

#[utoipa::path(
    get,
    path = "/api/v1/profile/current",
    operation_id = "profile_current",
    tag = "identity",
    responses(
        (status = 200, body = AccountPublic),
        (status = 401, description = "未认证", body = ProblemDetails),
        (status = 404, description = "账号不存在", body = ProblemDetails)
    ),
    security(("bearerAuth" = []))
)]
pub async fn handler(
    AuthedAccount(account_id): AuthedAccount,
    State(state): State<AppState>,
) -> JsonResponseType<AccountPublic> {
    let account = repository::find_by_id(state.pool(), account_id).await?;
    Ok(Json(account.into()))
}
