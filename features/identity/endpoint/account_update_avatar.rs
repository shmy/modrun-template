use axum::{Json, extract::State};
use blob_kit::Blob;
use http_auth::AuthedAccount;
use http_extractors::{ValidJson, ValidPath};
use identity_contract::IdentityError;
use serde::{Deserialize, Serialize};
use shared_contract::Id;
use sqlx::PgPool;
use utoipa::ToSchema;
use validify::Validify;
use web_error::{JsonResponseType, ProblemDetails};
use web_state::{AppState, HasBlob, HasPool};

use crate::dto::{AccountDetail, AccountIdPath, validate_avatar};
use crate::repository;
use crate::shared::avatar_ops;

#[derive(Debug, Deserialize, Validify, ToSchema)]
pub struct UpdateAvatarRequest {
    /// 上传接口返回的 staging path（或公网 URL）；`null` 清空。落库前会转正。
    #[validate(custom(validate_avatar))]
    #[schema(example = "staging/2026/01/abc.webp")]
    pub avatar: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct UpdateAvatarResponse {
    #[schema(value_type = String)]
    pub id: Id,
    #[schema(example = "2026/01/abc.webp")]
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
        (status = 400, description = "参数校验失败或头像 path 非法", body = ProblemDetails),
        (status = 401, description = "未认证", body = ProblemDetails),
        (status = 403, description = "调用方不是特权账号，或目标受保护", body = ProblemDetails),
        (status = 404, description = "账号或不存在的 staging 对象", body = ProblemDetails)
    ),
    security(("bearerAuth" = []))
)]
pub async fn handler(
    AuthedAccount(actor_id): AuthedAccount,
    State(state): State<AppState>,
    ValidPath(path): ValidPath<AccountIdPath>,
    ValidJson(request): ValidJson<UpdateAvatarRequest>,
) -> JsonResponseType<UpdateAvatarResponse> {
    let response = execute(state.pool(), state.blob(), actor_id, path, request).await?;
    Ok(Json(response))
}

pub(crate) async fn execute(
    pool: &PgPool,
    blob: &Blob,
    actor_id: Id,
    path: AccountIdPath,
    request: UpdateAvatarRequest,
) -> rootcause::Result<UpdateAvatarResponse> {
    let Some(before) = repository::require_unprotected_target(pool, actor_id, path.id).await?
    else {
        return Err(IdentityError::AccountNotFound.into());
    };
    let avatar = match request
        .avatar
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(submitted) => Some(avatar_ops::commit_avatar(pool, blob, submitted).await?),
        None => None,
    };
    let account = repository::update_avatar(pool, path.id, avatar.clone()).await?;
    if let Some(path) = avatar.as_deref() {
        avatar_ops::attach_avatar(pool, path).await?;
    }
    if let Some(old) = before.avatar
        && Some(old.as_str()) != avatar.as_deref()
    {
        avatar_ops::detach_avatar(pool, &old).await;
    }
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
    use std::io::Cursor;

    #[sqlx::test]
    async fn test_update_avatar_set_and_clear(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let blob = Blob::new_for_test().await.expect("test blob");
        let actor = tests::insert_privileged_account(&pool, "13900001800").await;
        let account_id = tests::insert_test_account(&pool, "13900001801").await;

        let staging = "staging/2026/01/abc.webp";
        blob.write(staging, Cursor::new(b"img"))
            .await
            .expect("write staging");

        let set = execute(
            &pool,
            &blob,
            actor,
            AccountIdPath { id: account_id },
            UpdateAvatarRequest {
                avatar: Some(staging.to_string()),
            },
        )
        .await
        .unwrap();
        assert_eq!(set.id, account_id);
        assert_eq!(set.avatar.as_deref(), Some("2026/01/abc.webp"));
        assert_eq!(set.version, 2);
        assert!(blob.read("2026/01/abc.webp").await.unwrap().is_some());
        assert!(blob.read(staging).await.unwrap().is_none());

        let cleared = execute(
            &pool,
            &blob,
            actor,
            AccountIdPath { id: account_id },
            UpdateAvatarRequest { avatar: None },
        )
        .await
        .unwrap();
        assert!(cleared.avatar.is_none());
        assert_eq!(cleared.version, 3);
        // 正式对象仍在；登记表已 tombstone，交 AttachmentGc 回收
        let row = sqlx::query!(
            r#"SELECT ref_count, tombstoned_at FROM blob_objects WHERE path = $1"#,
            "2026/01/abc.webp"
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.ref_count, 0);
        assert!(row.tombstoned_at.is_some());
        assert!(blob.read("2026/01/abc.webp").await.unwrap().is_some());
    }

    #[sqlx::test]
    async fn test_update_avatar_protected(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let blob = Blob::new_for_test().await.expect("test blob");
        let actor = tests::insert_privileged_account(&pool, "13900001810").await;
        let err = execute(
            &pool,
            &blob,
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
        let blob = Blob::new_for_test().await.expect("test blob");
        let actor = tests::insert_privileged_account(&pool, "13900001820").await;
        let err = execute(
            &pool,
            &blob,
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

    #[sqlx::test]
    async fn test_update_avatar_invalid_path(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let blob = Blob::new_for_test().await.expect("test blob");
        let actor = tests::insert_privileged_account(&pool, "13900001830").await;
        let account_id = tests::insert_test_account(&pool, "13900001831").await;
        let err = execute(
            &pool,
            &blob,
            actor,
            AccountIdPath { id: account_id },
            UpdateAvatarRequest {
                avatar: Some("2026/01/already.webp".to_string()),
            },
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("account_avatar_invalid"));
    }
}
