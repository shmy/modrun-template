use crate::value_object::{HashedPassword, Password};
use axum::{Json, extract::State};
use cache_kit::Cache;
use http_auth::AuthedAccount;
use http_extractors::{ValidJson, ValidPath};
use identity_contract::IdentityError;
use jwt_kit::TokenHelper;
use serde::{Deserialize, Serialize};
use shared_contract::Id;
use sqlx::PgPool;
use utoipa::ToSchema;
use validify::Validify;
use web_error::{JsonResponseType, ProblemDetails};
use web_state::{HasCache, HasPool, HasTokenHelper, WebState};

use crate::dto::AccountIdPath;
use crate::repository;
use crate::shared::token_ops;

#[derive(Debug, Deserialize, Validify, ToSchema)]
pub struct ResetPasswordRequest {
    #[validify]
    pub new_password: Password,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ResetPasswordResponse {
    #[schema(example = true)]
    pub updated: bool,
}

#[utoipa::path(
    patch,
    path = "/api/v1/accounts/{id}/password",
    operation_id = "account_reset_password",
    tag = "account",
    params(AccountIdPath),
    request_body = ResetPasswordRequest,
    responses(
        (status = 200, body = ResetPasswordResponse),
        (status = 400, description = "参数校验失败", body = ProblemDetails),
        (status = 401, description = "未认证", body = ProblemDetails),
        (status = 403, description = "调用方不是特权账号，或目标受保护", body = ProblemDetails),
        (status = 404, description = "账号不存在", body = ProblemDetails)
    ),
    security(("bearerAuth" = []))
)]
pub async fn handler(
    AuthedAccount(actor_id): AuthedAccount,
    State(state): State<WebState>,
    ValidPath(path): ValidPath<AccountIdPath>,
    ValidJson(request): ValidJson<ResetPasswordRequest>,
) -> JsonResponseType<ResetPasswordResponse> {
    let response = execute(
        state.pool(),
        state.cache(),
        state.token_helper(),
        actor_id,
        path,
        request,
    )
    .await?;
    Ok(Json(response))
}

pub(crate) async fn execute(
    pool: &PgPool,
    cache: &Cache,
    token_helper: &TokenHelper,
    actor_id: Id,
    path: AccountIdPath,
    request: ResetPasswordRequest,
) -> rootcause::Result<ResetPasswordResponse> {
    repository::require_unprotected_target(pool, actor_id, path.id)
        .await?
        .ok_or(IdentityError::AccountNotFound)?;
    let new_hashed = HashedPassword::try_new(&request.new_password).await?;
    token_ops::revoke_tokens(cache, token_helper, path.id).await?;
    repository::update_password(pool, path.id, &new_hashed).await?;
    Ok(ResetPasswordResponse { updated: true })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests;
    use migration::run_migrations;

    #[sqlx::test]
    async fn test_reset_password_success(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let cache = Cache::try_new_pg(pool.clone()).await.unwrap();
        let token_helper = tests::test_token_helper();
        let actor = tests::insert_privileged_account(&pool, "13900001710").await;
        let account_id = tests::insert_test_account(&pool, "13900001711").await;
        let response = execute(
            &pool,
            &cache,
            &token_helper,
            actor,
            AccountIdPath { id: account_id },
            ResetPasswordRequest {
                new_password: Password::new_unchecked("reset1234".to_string()),
            },
        )
        .await
        .unwrap();
        assert!(response.updated);
        let hashed = repository::get_password_hash(&pool, account_id)
            .await
            .unwrap();
        assert!(hashed.verify_sync("reset1234").is_ok());
        assert!(hashed.verify_sync("test1234").is_err());
    }

    #[sqlx::test]
    async fn test_reset_password_protected(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let cache = Cache::try_new_pg(pool.clone()).await.unwrap();
        let token_helper = tests::test_token_helper();
        let actor = tests::insert_privileged_account(&pool, "13900001720").await;
        let err = execute(
            &pool,
            &cache,
            &token_helper,
            actor,
            AccountIdPath { id: actor },
            ResetPasswordRequest {
                new_password: Password::new_unchecked("reset1234".to_string()),
            },
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("account_protected"));
    }

    #[sqlx::test]
    async fn test_reset_password_revokes_tokens(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let cache = Cache::try_new_pg(pool.clone()).await.unwrap();
        let token_helper = tests::test_token_helper();
        let actor = tests::insert_privileged_account(&pool, "13900001721").await;
        let account_id = tests::insert_test_account(&pool, "13900001722").await;
        let tokens = token_ops::issue_tokens(&cache, &token_helper, &account_id)
            .await
            .unwrap();
        execute(
            &pool,
            &cache,
            &token_helper,
            actor,
            AccountIdPath { id: account_id },
            ResetPasswordRequest {
                new_password: Password::new_unchecked("reset1234".to_string()),
            },
        )
        .await
        .unwrap();
        let err = token_ops::consume_refresh_token(&cache, &token_helper, &tokens.refresh_token)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("refresh_token_invalid"));
    }
}
