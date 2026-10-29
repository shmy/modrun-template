use axum::{Json, extract::State};
use http_auth::AuthedAccount;
use http_extractors::{ValidJson, ValidPath};
use identity_contract::IdentityError;
use serde::Deserialize;
use shared_contract::Id;
use sqlx::PgPool;
use utoipa::ToSchema;
use validify::Validify;
use web_error::{JsonResponseType, ProblemDetails};
use web_state::{HasPool, WebState};

use crate::dto::{AccountDetail, AccountIdPath, validate_phone_number};
use crate::repository;
use shared_contract::PhoneNumber;

#[derive(Debug, Deserialize, Validify, ToSchema)]
pub struct UpdateAccountRequest {
    #[modify(trim)]
    #[validate(length(min = 1, message = "account_name_too_short"))]
    #[validate(length(max = 64, message = "account_name_too_long"))]
    #[schema(example = "Tom")]
    pub name: String,
    #[modify(trim)]
    #[validate(custom(validate_phone_number))]
    #[schema(value_type = String, example = "13888888888")]
    pub phone: String,
    #[schema(example = 1)]
    pub version: i64,
}

#[utoipa::path(
    patch,
    path = "/api/v1/accounts/{id}",
    operation_id = "account_update",
    tag = "account",
    params(AccountIdPath),
    request_body = UpdateAccountRequest,
    responses(
        (status = 200, body = AccountDetail),
        (status = 400, description = "参数校验失败", body = ProblemDetails),
        (status = 401, description = "未认证", body = ProblemDetails),
        (status = 403, description = "调用方不是特权账号，或目标受保护", body = ProblemDetails),
        (status = 404, description = "账号不存在", body = ProblemDetails),
        (status = 409, description = "乐观锁版本冲突", body = ProblemDetails)
    ),
    security(("bearerAuth" = []))
)]
pub async fn handler(
    AuthedAccount(actor_id): AuthedAccount,
    State(state): State<WebState>,
    ValidPath(path): ValidPath<AccountIdPath>,
    ValidJson(request): ValidJson<UpdateAccountRequest>,
) -> JsonResponseType<AccountDetail> {
    let response = execute(state.pool(), actor_id, path, request).await?;
    Ok(Json(response))
}

pub(crate) async fn execute(
    pool: &PgPool,
    actor_id: Id,
    path: AccountIdPath,
    request: UpdateAccountRequest,
) -> rootcause::Result<AccountDetail> {
    let Some(before) = repository::require_unprotected_target(pool, actor_id, path.id).await?
    else {
        return Err(IdentityError::AccountNotFound.into());
    };
    let mut account = before;
    account.name = request.name;
    account.phone = PhoneNumber::new_unchecked(request.phone);
    account.version = request.version;
    let account = repository::update(pool, &account).await?;
    Ok(account.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests;
    use migration::run_migrations;

    #[sqlx::test]
    async fn test_update_success(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let actor = tests::insert_privileged_account(&pool, "13900002100").await;
        let account_id = tests::insert_test_account(&pool, "13900002101").await;
        let response = execute(
            &pool,
            actor,
            AccountIdPath { id: account_id },
            UpdateAccountRequest {
                name: "UpdatedName".to_string(),
                phone: "13900002102".to_string(),
                version: 1,
            },
        )
        .await
        .unwrap();
        assert_eq!(response.id, account_id);
        assert_eq!(response.name, "UpdatedName");
        assert_eq!(response.phone.as_str(), "13900002102");
        assert_eq!(response.version, 2);
    }

    #[sqlx::test]
    async fn test_update_not_found(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let actor = tests::insert_privileged_account(&pool, "13900002104").await;
        let err = execute(
            &pool,
            actor,
            AccountIdPath {
                id: Id::new_unchecked(999),
            },
            UpdateAccountRequest {
                name: "N".to_string(),
                phone: "13900002103".to_string(),
                version: 1,
            },
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("account_not_found"));
    }

    #[sqlx::test]
    async fn test_update_version_conflict(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let actor = tests::insert_privileged_account(&pool, "13900002120").await;
        let account_id = tests::insert_test_account(&pool, "13900002121").await;
        execute(
            &pool,
            actor,
            AccountIdPath { id: account_id },
            UpdateAccountRequest {
                name: "First".to_string(),
                phone: "13900002122".to_string(),
                version: 1,
            },
        )
        .await
        .unwrap();
        let err = execute(
            &pool,
            actor,
            AccountIdPath { id: account_id },
            UpdateAccountRequest {
                name: "Second".to_string(),
                phone: "13900002123".to_string(),
                version: 1,
            },
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("account_version_conflict"));
    }

    #[sqlx::test]
    async fn test_update_protected(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let actor = tests::insert_privileged_account(&pool, "13900002130").await;
        let err = execute(
            &pool,
            actor,
            AccountIdPath { id: actor },
            UpdateAccountRequest {
                name: "Nope".to_string(),
                phone: "13900002132".to_string(),
                version: 1,
            },
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("account_protected"));
    }
}
