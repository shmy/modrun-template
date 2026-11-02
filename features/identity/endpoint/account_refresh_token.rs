use axum::{Json, extract::State};
use http_extractors::ValidJson;
use web_error::{JsonResponseType, ProblemDetails};
use web_state::{AppState, HasAuthCaps};

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
    State(state): State<AppState>,
    ValidJson(request): ValidJson<RefreshRequest>,
) -> JsonResponseType<TokenResponse> {
    run(&state, request).await
}

async fn run(state: &impl HasAuthCaps, request: RefreshRequest) -> JsonResponseType<TokenResponse> {
    let account_id = token_ops::consume_refresh_token(
        state.cache(),
        state.token_helper(),
        &request.refresh_token,
    )
    .await?;
    match token_ops::issue_tokens(state.cache(), state.token_helper(), &account_id).await {
        Ok(tokens) => {
            // 签发成功后才写「已用」标记：提前写会让并发重复提交被误判为重放，
            // 连带吊销本次刚建立的会话。
            //
            // commit 失败（「已用」未写入）绝不能对客户端返回 200：否则旧 refresh
            // 重放只会 401、不会走 revoke。fail-closed：吊销新会话 + 归还旧 refresh
            // + 清 inflight，客户端可带着原 token 重试。
            if let Err(err) = token_ops::commit_refresh_rotation(
                state.cache(),
                state.token_helper(),
                account_id,
                &request.refresh_token,
            )
            .await
            {
                tracing::error!(error = %err, "failed to commit refresh rotation");
                if let Err(abort_err) = token_ops::abort_refresh_rotation(
                    state.cache(),
                    state.token_helper(),
                    account_id,
                    &request.refresh_token,
                )
                .await
                {
                    tracing::error!(
                        error = %abort_err,
                        "failed to abort refresh rotation after commit failure"
                    );
                }
                return Err(err.into());
            }
            Ok(Json(TokenResponse::bearer(
                tokens.access_token,
                tokens.refresh_token,
                tokens.expires_in,
            )))
        }
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
