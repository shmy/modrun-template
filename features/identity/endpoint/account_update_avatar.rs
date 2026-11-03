use axum::{Json, extract::State};
use http_auth::AuthedAccount;
use http_extractors::{ValidJson, ValidPath};
use identity_contract::IdentityError;
use serde::{Deserialize, Serialize};
use shared_contract::Id;
use sqlx::PgPool;
use utoipa::ToSchema;
use validify::Validify;
use web_error::{JsonResponseType, ProblemDetails};
use web_state::{AppState, HasPool};

use crate::dto::{AccountDetail, AccountIdPath, validate_avatar};
use crate::repository;

#[derive(Debug, Deserialize, Validify, ToSchema)]
pub struct UpdateAvatarRequest {
    /// 头像对象路径；`null` 清空。
    #[validate(custom(validate_avatar))]
    #[schema(example = "staging/2026/01/abc.webp")]
    pub avatar: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct UpdateAvatarResponse {
    #[schema(value_type = String)]
    pub id: Id,
    pub avatar: Option<String>,
    pub version: i64,
}

#[utoipa::path(
    patch,
    path = "/api/v1/accounts/{id}/avatar",
    operation_id = "account_update_avatar",
    tag = "account",
    params(AccountIdPath),
    request_body = UpdateAvatarRequest,
    responses(
        (status = 200, body = UpdateAvatarResponse),
        (status = 400, description = "参数校验失败", body = ProblemDetails),
        (status = 401, description = "未认证", body = ProblemDetails),
        (status = 403, description = "调用方不是特权账号，或目标受保护", body = ProblemDetails),
        (status = 404, description = "账号不存在", body = ProblemDetails)
    ),
    security(("bearerAuth" = []))
)]
pub async fn handler(
    AuthedAccount(actor_id): AuthedAccount,
    State(state): State<AppState>,
    ValidPath(path): ValidPath<AccountIdPath>,
    ValidJson(request): ValidJson<UpdateAvatarRequest>,
) -> JsonResponseType<UpdateAvatarResponse> {
    let response = execute(state.pool(), actor_id, path, request).await?;
    Ok(Json(response))
}

pub(crate) async fn execute(
    pool: &PgPool,
    actor_id: Id,
    path: AccountIdPath,
    request: UpdateAvatarRequest,
) -> rootcause::Result<UpdateAvatarResponse> {
    repository::require_unprotected_target(pool, actor_id, path.id)
        .await?
        .ok_or(IdentityError::AccountNotFound)?;
    let avatar = request
        .avatar
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let account = repository::update_avatar(pool, path.id, avatar).await?;
    let detail = AccountDetail::from(account);
    Ok(UpdateAvatarResponse {
        id: detail.id,
        avatar: detail.avatar,
        version: detail.version,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests;
    use migration::run_migrations;

    #[sqlx::test]
    async fn test_update_avatar_set_and_clear(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let actor = tests::insert_privileged_account(&pool, "13900001800").await;
        let account_id = tests::insert_test_account(&pool, "13900001801").await;

        let set = execute(
            &pool,
            actor,
            AccountIdPath { id: account_id },
            UpdateAvatarRequest {
                avatar: Some("staging/2026/01/abc.webp".to_string()),
            },
        )
        .await
        .unwrap();
        assert_eq!(set.id, account_id);
        assert_eq!(set.avatar.as_deref(), Some("staging/2026/01/abc.webp"));
        assert_eq!(set.version, 2);

        let cleared = execute(
            &pool,
            actor,
            AccountIdPath { id: account_id },
            UpdateAvatarRequest { avatar: None },
        )
        .await
        .unwrap();
        assert!(cleared.avatar.is_none());
        assert_eq!(cleared.version, 3);
    }

    #[sqlx::test]
    async fn test_update_avatar_protected(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let actor = tests::insert_privileged_account(&pool, "13900001810").await;
        let err = execute(
            &pool,
            actor,
            AccountIdPath { id: actor },
            UpdateAvatarRequest {
                avatar: Some("staging/2026/01/abc.webp".to_string()),
            },
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("account_protected"));
    }

    #[sqlx::test]
    async fn test_update_avatar_not_found(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let actor = tests::insert_privileged_account(&pool, "13900001820").await;
        let err = execute(
            &pool,
            actor,
            AccountIdPath {
                id: Id::new_unchecked(999),
            },
            UpdateAvatarRequest {
                avatar: Some("staging/2026/01/abc.webp".to_string()),
            },
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("account_not_found"));
    }
}
