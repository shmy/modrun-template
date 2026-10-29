use axum::{Json, extract::State};
use http_extractors::ValidJson;
use identity_contract::IdentityError;
use rootcause::Report;
use web_error::{JsonResponseType, ProblemDetails, WebError};
use web_state::{HasCache, HasPool, HasTokenHelper, WebState};

use crate::dto::{LoginRequest, TokenResponse};
use crate::repository;
use crate::shared::token_ops;
use crate::value_object::HashedPassword;

#[utoipa::path(
    post,
    path = "/api/v1/identity/login",
    operation_id = "identity_login",
    tag = "identity",
    request_body = LoginRequest,
    responses(
        (status = 200, body = TokenResponse),
        (status = 400, description = "参数校验失败", body = ProblemDetails),
        (status = 401, description = "账号或密码错误", body = ProblemDetails),
        (status = 429, description = "登录过于频繁", body = ProblemDetails)
    )
)]
pub async fn handler(
    State(state): State<WebState>,
    ValidJson(request): ValidJson<LoginRequest>,
) -> JsonResponseType<TokenResponse> {
    let row = match repository::find_auth_by_phone(state.pool(), &request.phone()).await? {
        Some(row) => row,
        None => {
            // 时序一致性：执行等价 argon2，防止通过响应时间差异枚举有效手机号
            HashedPassword::equalize_timing().await;
            #[cfg(feature = "otel")]
            trace_kit::metrics::record_login(false);
            return Err(WebError::from(Report::from(
                IdentityError::AccountInvalidCredentials,
            )));
        }
    };

    if let Err(err) = row.password.verify(&request.password).await {
        #[cfg(feature = "otel")]
        trace_kit::metrics::record_login(false);
        return Err(WebError::from(err));
    }

    let tokens = token_ops::issue_tokens(state.cache(), state.token_helper(), &row.id).await?;
    #[cfg(feature = "otel")]
    trace_kit::metrics::record_login(true);

    Ok(Json(TokenResponse::bearer(
        tokens.access_token,
        tokens.refresh_token,
        tokens.expires_in,
    )))
}
