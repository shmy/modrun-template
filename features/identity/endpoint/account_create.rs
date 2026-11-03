use crate::entity::Account;
use crate::value_object::{HashedPassword, Password};
use axum::{Json, extract::State};
use blob_kit::Blob;
use http_auth::AuthedAccount;
use http_extractors::ValidJson;
use serde::{Deserialize, Serialize};
use shared_contract::Id;
use sqlx::PgPool;
use utoipa::ToSchema;
use validify::Validify;
use web_error::{JsonResponseType, ProblemDetails};
use web_state::{AppState, HasBlob, HasPool};

use crate::dto::{validate_avatar, validate_phone_number};
use crate::repository;
use crate::shared::avatar_ops;
use identity_contract::IdentityError;
use shared_contract::PhoneNumber;

#[derive(Debug, Deserialize, Validify, ToSchema)]
pub struct CreateAccountRequest {
    #[modify(trim)]
    #[validate(length(min = 1, message = "account_name_too_short"))]
    #[validate(length(max = 64, message = "account_name_too_long"))]
    #[schema(example = "Tom")]
    pub name: String,
    #[modify(trim)]
    #[validate(custom(validate_phone_number))]
    #[schema(value_type = String, example = "13888888888")]
    pub phone: String,
    #[schema(example = "P@ssw0rd1")]
    #[validify]
    pub password: Password,
    /// 可选：上传接口返回的 staging path（或公网 URL）；落库前会转正。
    #[validate(custom(validate_avatar))]
    #[schema(example = "staging/2026/01/abc.webp")]
    pub avatar: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CreateAccountResponse {
    #[schema(value_type = String)]
    pub id: Id,
    pub name: String,
    #[schema(example = "2026/01/abc.webp")]
    pub avatar: Option<String>,
}

#[utoipa::path(
    post,
    path = "/api/v1/accounts",
    operation_id = "account_create",
    tag = "account",
    request_body = CreateAccountRequest,
    responses(
        (status = 200, body = CreateAccountResponse),
        (status = 400, description = "参数校验失败", body = ProblemDetails),
        (status = 401, description = "未认证", body = ProblemDetails),
        (status = 403, description = "调用方不是特权账号", body = ProblemDetails),
        (status = 404, description = "头像 staging 对象不存在", body = ProblemDetails),
        (status = 409, description = "手机号已存在", body = ProblemDetails)
    ),
    security(("bearerAuth" = []))
)]
pub async fn handler(
    AuthedAccount(actor_id): AuthedAccount,
    State(state): State<AppState>,
    ValidJson(request): ValidJson<CreateAccountRequest>,
) -> JsonResponseType<CreateAccountResponse> {
    let response = execute(state.pool(), state.blob(), actor_id, request).await?;
    Ok(Json(response))
}

pub(crate) async fn execute(
    pool: &PgPool,
    blob: &Blob,
    actor_id: Id,
    request: CreateAccountRequest,
) -> rootcause::Result<CreateAccountResponse> {
    repository::require_privileged(pool, actor_id).await?;
    let phone = PhoneNumber::new_unchecked(request.phone);
    // 先查重再 promote，避免手机号冲突时留下已转正孤儿（并发下仍靠 UNIQUE 兜底）。
    if repository::find_auth_by_phone(pool, &phone)
        .await?
        .is_some()
    {
        return Err(IdentityError::AccountDuplicated.into());
    }
    // 哈希放在 commit 前：避免 argon2 失败时留下 pending 正式对象。
    let password = HashedPassword::try_new(&request.password).await?;
    let id = Id::default();
    let avatar = match request
        .avatar
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(submitted) => Some(avatar_ops::commit_avatar(pool, blob, submitted).await?),
        None => None,
    };
    let account = Account {
        id,
        name: request.name,
        phone,
        password,
        privileged: false,
        avatar: avatar.clone(),
        version: 1,
    };
    if let Err(err) = repository::create(pool, &account).await {
        // 任意落库失败：tombstone 本次 commit，交 AttachmentGc。
        if let Some(path) = avatar.as_deref() {
            avatar_ops::detach_avatar(pool, path).await;
        }
        return Err(err);
    }
    if let Some(path) = avatar.as_deref()
        && let Err(err) = avatar_ops::attach_avatar(pool, path).await
    {
        // 账号已存在且 avatar 列已写：清空避免死链，并 tombstone 对象。
        if let Err(clear_err) = repository::set_avatar(pool, id, None).await {
            tracing::warn!(
                error = %clear_err,
                account_id = %id,
                path,
                "create: attach failed and avatar clear failed"
            );
        }
        avatar_ops::detach_avatar(pool, path).await;
        return Err(err);
    }
    Ok(CreateAccountResponse {
        id,
        name: account.name,
        avatar: account.avatar,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests;
    use crate::value_object::Password;
    use migration::run_migrations;
    use std::io::Cursor;

    #[sqlx::test]
    async fn test_create_success(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let blob = Blob::new_for_test().await.expect("test blob");
        let staging = "staging/2026/01/abc.webp";
        blob.write(staging, Cursor::new(b"img"))
            .await
            .expect("write staging");
        let actor = tests::insert_privileged_account(&pool, "13900001200").await;
        let response = execute(
            &pool,
            &blob,
            actor,
            CreateAccountRequest {
                name: "Tom".to_string(),
                phone: "13900001201".to_string(),
                password: Password::new_unchecked("admin123!".to_string()),
                avatar: Some(staging.to_string()),
            },
        )
        .await
        .unwrap();
        assert!(i64::from(response.id) > 0);
        assert_eq!(response.name, "Tom");
        assert_eq!(response.avatar.as_deref(), Some("2026/01/abc.webp"));
        assert!(blob.read("2026/01/abc.webp").await.unwrap().is_some());
        assert!(blob.read(staging).await.unwrap().is_none());
        let rc: i32 = sqlx::query_scalar!(
            r#"SELECT ref_count AS "ref_count!" FROM blob_objects WHERE path = $1"#,
            "2026/01/abc.webp"
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(rc, 1);
    }

    #[sqlx::test]
    async fn test_create_without_avatar(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let blob = Blob::new_for_test().await.expect("test blob");
        let actor = tests::insert_privileged_account(&pool, "13900001202").await;
        let response = execute(
            &pool,
            &blob,
            actor,
            CreateAccountRequest {
                name: "NoAvatar".to_string(),
                phone: "13900001203".to_string(),
                password: Password::new_unchecked("admin123!".to_string()),
                avatar: None,
            },
        )
        .await
        .unwrap();
        assert!(response.avatar.is_none());
    }

    #[sqlx::test]
    async fn test_create_forbidden_for_non_privileged(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let blob = Blob::new_for_test().await.expect("test blob");
        let actor = tests::insert_test_account(&pool, "13900001210").await;
        let err = execute(
            &pool,
            &blob,
            actor,
            CreateAccountRequest {
                name: "Tom".to_string(),
                phone: "13900001211".to_string(),
                password: Password::new_unchecked("admin123!".to_string()),
                avatar: None,
            },
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("account_forbidden"));
    }

    #[sqlx::test]
    async fn test_create_duplicated_phone(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let blob = Blob::new_for_test().await.expect("test blob");
        let actor = tests::insert_privileged_account(&pool, "13900001220").await;
        tests::insert_test_account(&pool, "13900001221").await;
        let err = execute(
            &pool,
            &blob,
            actor,
            CreateAccountRequest {
                name: "Tom".to_string(),
                phone: "13900001221".to_string(),
                password: Password::new_unchecked("admin123!".to_string()),
                avatar: None,
            },
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("account_duplicated"));
    }

    #[sqlx::test]
    async fn test_create_avatar_missing_object(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let blob = Blob::new_for_test().await.expect("test blob");
        let actor = tests::insert_privileged_account(&pool, "13900001230").await;
        let err = execute(
            &pool,
            &blob,
            actor,
            CreateAccountRequest {
                name: "Tom".to_string(),
                phone: "13900001231".to_string(),
                password: Password::new_unchecked("admin123!".to_string()),
                avatar: Some("staging/2026/01/missing.webp".to_string()),
            },
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("account_avatar_not_found"));
    }

    /// create UNIQUE 失败后对已 commit path detach，应进入 tombstone（与 execute 补偿一致）。
    #[sqlx::test]
    async fn test_create_failure_detaches_committed_avatar(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let blob = Blob::new_for_test().await.expect("test blob");
        tests::insert_test_account(&pool, "13900001241").await;

        let staging = "staging/2026/01/dup.webp";
        blob.write(staging, Cursor::new(b"img"))
            .await
            .expect("write");
        let path = avatar_ops::commit_avatar(&pool, &blob, staging)
            .await
            .expect("commit");
        let account = Account {
            id: Id::default(),
            name: "Dup".into(),
            phone: PhoneNumber::new_unchecked("13900001241".into()),
            password: HashedPassword::try_new(&Password::new_unchecked("admin123!".into()))
                .await
                .unwrap(),
            privileged: false,
            avatar: Some(path.clone()),
            version: 1,
        };
        let err = repository::create(&pool, &account).await.expect_err("dup");
        assert!(err.to_string().contains("account_duplicated"));
        // 与 execute 中 create 失败分支相同
        avatar_ops::detach_avatar(&pool, &path).await;
        let row = sqlx::query!(
            r#"SELECT ref_count, tombstoned_at FROM blob_objects WHERE path = $1"#,
            path
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.ref_count, 0);
        assert!(row.tombstoned_at.is_some());
    }
}
