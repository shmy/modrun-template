use axum::{Json, extract::State};
use http_extractors::ValidJson;
use web_error::{JsonResponseType, ProblemDetails};
use web_state::{HasCache, HasTokenHelper, WebState};

use crate::dto::{RefreshRequest, TokenResponse};
use crate::shared::token_ops;

#[utoipa::path(
    post,
    path = "/api/v1/identity/refresh",
    operation_id = "identity_refresh",
    tag = "identity",
    request_body = RefreshRequest,
    responses(
        (status = 200, body = TokenResponse),
        (status = 400, description = "参数校验失败", body = ProblemDetails),
        (status = 401, description = "刷新令牌无效", body = ProblemDetails),
        (status = 429, description = "刷新过于频繁", body = ProblemDetails)
    )
)]
pub async fn handler(
    State(state): State<WebState>,
    ValidJson(request): ValidJson<RefreshRequest>,
) -> JsonResponseType<TokenResponse> {
    let account_id = token_ops::consume_refresh_token(
        state.cache(),
        state.token_helper(),
        &request.refresh_token,
    )
    .await?;
    match token_ops::issue_tokens(state.cache(), state.token_helper(), &account_id).await {
        Ok(tokens) => Ok(Json(TokenResponse::bearer(
            tokens.access_token,
            tokens.refresh_token,
            tokens.expires_in,
        ))),
        Err(err) => {
            if let Err(restore_err) = token_ops::restore_refresh_token(
                state.cache(),
                state.token_helper(),
                &request.refresh_token,
                &account_id,
            )
            .await
            {
                tracing::error!(error = %restore_err, "failed to restore refresh token after issue failure");
            }
            Err(err.into())
        }
    }
}
