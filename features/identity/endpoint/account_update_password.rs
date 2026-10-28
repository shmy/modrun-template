use crate::value_object::HashedPassword;
use axum::{Json, extract::State};
use cache_kit::Cache;
use http_auth::AuthedAccount;
use http_extractors::ValidJson;
use jwt_kit::TokenHelper;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validify::Validify;
use web_error::{JsonResponseType, ProblemDetails};
use web_state::WebState;

use crate::repository;
use crate::shared::token_ops;

use crate::value_object::Password;
use shared_contract::Id;
use sqlx::PgPool;

#[derive(Debug, Deserialize, Validify, ToSchema)]
pub struct UpdatePasswordRequest {
    #[validify]
    pub old_password: Password,
    #[validify]
    pub new_password: Password,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct UpdatePasswordResponse {
    #[schema(example = true)]
    pub updated: bool,
}

#[utoipa::path(
    patch,
    path = "/api/v1/identity/password",
    operation_id = "identity_update_password",
    tag = "identity",
    request_body = UpdatePasswordRequest,
    responses(
        (status = 200, body = UpdatePasswordResponse),
        (status = 400, description = "参数校验失败或旧密码错误", body = ProblemDetails),
        (status = 401, description = "未认证", body = ProblemDetails),
        (status = 404, description = "账号不存在", body = ProblemDetails)
    ),
    security(("bearerAuth" = []))
)]
pub async fn handler(
    AuthedAccount(account_id): AuthedAccount,
    State(state): State<WebState>,
    ValidJson(request): ValidJson<UpdatePasswordRequest>,
) -> JsonResponseType<UpdatePasswordResponse> {
    let response = execute(
        &state.pg,
        &state.cache,
        &state.token_helper,
        account_id,
        request,
    )
    .await?;
    Ok(Json(response))
}

pub(crate) async fn execute(
    pool: &PgPool,
    cache: &Cache,
    token_helper: &TokenHelper,
    account_id: Id,
    request: UpdatePasswordRequest,
) -> rootcause::Result<UpdatePasswordResponse> {
    let old_hashed = repository::get_password_hash(pool, account_id).await?;
    old_hashed.verify(&request.old_password).await?;
    let new_hashed = HashedPassword::try_new(&request.new_password).await?;
    token_ops::revoke_tokens(cache, token_helper, account_id).await?;
    repository::update_password(pool, account_id, &new_hashed).await?;
    Ok(UpdatePasswordResponse { updated: true })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests;
    use crate::value_object::Password;
    use migration::run_migrations;

    #[sqlx::test]
    async fn test_update_password_success(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let cache = Cache::try_new_pg(pool.clone()).await.unwrap();
        let token_helper = tests::test_token_helper();
        let account_id = tests::insert_test_account(&pool, "13900001701").await;
        let response = execute(
            &pool,
            &cache,
            &token_helper,
            account_id,
            UpdatePasswordRequest {
                old_password: Password::new_unchecked("test1234".to_string()),
                new_password: Password::new_unchecked("newpass1234".to_string()),
            },
        )
        .await
        .unwrap();
        assert!(response.updated);
        let hashed = repository::get_password_hash(&pool, account_id)
            .await
            .unwrap();
        assert!(hashed.verify_sync("newpass1234").is_ok());
        assert!(hashed.verify_sync("test1234").is_err());
    }

    #[sqlx::test]
    async fn test_update_password_incorrect_old_password(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let cache = Cache::try_new_pg(pool.clone()).await.unwrap();
        let token_helper = tests::test_token_helper();
        let account_id = tests::insert_test_account(&pool, "13900001702").await;
        let err = execute(
            &pool,
            &cache,
            &token_helper,
            account_id,
            UpdatePasswordRequest {
                old_password: Password::new_unchecked("wrong1234".to_string()),
                new_password: Password::new_unchecked("newpass1234".to_string()),
            },
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("account_password_incorrect"));
    }

    #[sqlx::test]
    async fn test_update_password_revokes_tokens(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let cache = Cache::try_new_pg(pool.clone()).await.unwrap();
        let token_helper = tests::test_token_helper();
        let account_id = tests::insert_test_account(&pool, "13900001703").await;
        let tokens = token_ops::issue_tokens(&cache, &token_helper, &account_id)
            .await
            .unwrap();
        execute(
            &pool,
            &cache,
            &token_helper,
            account_id,
            UpdatePasswordRequest {
                old_password: Password::new_unchecked("test1234".to_string()),
                new_password: Password::new_unchecked("newpass1234".to_string()),
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
