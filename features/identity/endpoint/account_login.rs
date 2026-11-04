use axum::{Json, extract::State};
use http_extractors::ValidJson;
use identity_contract::{IdentityError, L10nKeyedError};
use web_error::{JsonResponseType, ProblemDetails, WebError};
use web_state::{AppState, HasSession};

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
    State(state): State<AppState>,
    ValidJson(request): ValidJson<LoginRequest>,
) -> JsonResponseType<TokenResponse> {
    run(&state, request).await
}

async fn run(state: &impl HasSession, request: LoginRequest) -> JsonResponseType<TokenResponse> {
    let row = match repository::find_auth_by_phone(state.pool(), &request.phone()).await? {
        Some(row) => row,
        None => {
            // 时序一致性：执行等价 argon2，防止通过响应时间差异枚举有效手机号
            HashedPassword::equalize_timing().await;
            #[cfg(feature = "otel")]
            trace_kit::metrics::record_login(false);
            // 直接构造 401 key，不经 `Report` → mapper：登录是本域已知的拒绝路径，
            // 不该依赖 `module()` 是否已注册 mapper。
            return Err(WebError::L10n(
                IdentityError::AccountInvalidCredentials
                    .l10n_key()
                    .to_string(),
            ));
        }
    };

    if let Err(err) = row.password.verify(&request.password).await {
        // 统一映射为 `account_invalid_credentials`（401）：若透出 `account_password_incorrect`，
        // 攻击者可区分"手机号存在但密码错"与"手机号不存在"，绕过 `equalize_timing`
        // 抹平的时间侧信道，从响应体上重新枚举有效手机号。
        let _ = err;
        #[cfg(feature = "otel")]
        trace_kit::metrics::record_login(false);
        return Err(WebError::L10n(
            IdentityError::AccountInvalidCredentials
                .l10n_key()
                .to_string(),
        ));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests;
    use crate::value_object::Password;
    use axum::http::StatusCode;
    use cache_kit::Cache;
    use jwt_kit::TokenHelper;
    use migration::run_migrations;
    use sqlx::PgPool;
    use web_state::{HasCache, HasPool, HasTokenHelper};

    struct SessionCtx {
        pool: PgPool,
        cache: Cache,
        token_helper: TokenHelper,
    }

    impl HasPool for SessionCtx {
        fn pool(&self) -> &PgPool {
            &self.pool
        }
    }

    impl HasCache for SessionCtx {
        fn cache(&self) -> &Cache {
            &self.cache
        }
    }

    impl HasTokenHelper for SessionCtx {
        fn token_helper(&self) -> &TokenHelper {
            &self.token_helper
        }
    }

    async fn session_ctx(pool: PgPool) -> SessionCtx {
        let cache = Cache::try_new_pg(pool.clone()).await.expect("cache");
        SessionCtx {
            pool,
            cache,
            token_helper: tests::test_token_helper(),
        }
    }

    /// 错误密码与「手机号不存在」都必须是 `account_invalid_credentials`，
    /// 不得透出 `account_password_incorrect`（否则可枚举有效手机号）。
    #[sqlx::test]
    async fn wrong_password_maps_to_invalid_credentials(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        tests::insert_test_account(&pool, "13900001801").await;
        let ctx = session_ctx(pool).await;

        let err = run(
            &ctx,
            LoginRequest {
                phone: "13900001801".to_string(),
                password: Password::new_unchecked("wrong1234".to_string()),
            },
        )
        .await
        .expect_err("wrong password must fail");

        assert_eq!(err.error_code(), "account_invalid_credentials");
        assert_eq!(err.status_code(), StatusCode::UNAUTHORIZED);
        assert!(
            !err.error_code().contains("password_incorrect"),
            "must not leak account_password_incorrect"
        );
    }

    #[sqlx::test]
    async fn unknown_phone_maps_to_invalid_credentials(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let ctx = session_ctx(pool).await;

        let err = run(
            &ctx,
            LoginRequest {
                phone: "13900001899".to_string(),
                password: Password::new_unchecked("test1234".to_string()),
            },
        )
        .await
        .expect_err("unknown phone must fail");

        assert_eq!(err.error_code(), "account_invalid_credentials");
        assert_eq!(err.status_code(), StatusCode::UNAUTHORIZED);
        assert!(
            !err.error_code().contains("password_incorrect"),
            "must not leak account_password_incorrect"
        );
    }
}
