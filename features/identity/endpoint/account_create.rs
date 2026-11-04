use crate::entity::Account;
use crate::value_object::{HashedPassword, Password};
use axum::{Json, extract::State};
use blob_kit::Blob;
use cache_kit::Cache;
use http_auth::AuthedAccount;
use http_extractors::ValidJson;
use serde::{Deserialize, Serialize};
use shared_contract::Id;
use sqlx::PgPool;
use utoipa::ToSchema;
use validify::Validify;
use web_error::{JsonResponseType, ProblemDetails};
use web_state::{AppState, HasBlob, HasCache, HasPool};

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
    let response = execute(state.pool(), state.blob(), state.cache(), actor_id, request).await?;
    Ok(Json(response))
}

pub(crate) async fn execute(
    pool: &PgPool,
    blob: &Blob,
    cache: &Cache,
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
        Some(submitted) => {
            Some(avatar_ops::commit_avatar(pool, blob, cache, actor_id, submitted).await?)
        }
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
    // create + attach 同事务：attach 失败则账号回滚，禁止「错误响应 + 已占手机号」。
    let mut tx = pool.begin().await?;
    if let Err(err) = repository::create_tx(&mut tx, &account).await {
        drop(tx);
        if let Some(path) = avatar.as_deref() {
            avatar_ops::detach_avatar_best_effort(pool, path).await;
        }
        return Err(err);
    }
    if let Some(path) = avatar.as_deref()
        && let Err(err) = avatar_ops::attach_avatar_tx(&mut tx, path).await
    {
        drop(tx);
        avatar_ops::detach_avatar_best_effort(pool, path).await;
        return Err(err);
    }
    tx.commit().await?;
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
    use file::attachment;
    use migration::run_migrations;
    use std::io::Cursor;

    async fn prep_staging(blob: &Blob, cache: &Cache, owner: Id, staging: &str) {
        blob.write(staging, Cursor::new(b"img"))
            .await
            .expect("write staging");
        attachment::claim_staging(cache, staging, owner)
            .await
            .expect("claim");
    }

    #[sqlx::test]
    async fn test_create_success(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let blob = Blob::new_for_test().await.expect("test blob");
        let cache = Cache::try_new_pg(pool.clone()).await.expect("cache");
        let staging = "staging/2026/01/abc.webp";
        let actor = tests::insert_privileged_account(&pool, "13900001200").await;
        prep_staging(&blob, &cache, actor, staging).await;
        let response = execute(
            &pool,
            &blob,
            &cache,
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
        let cache = Cache::try_new_pg(pool.clone()).await.expect("cache");
        let actor = tests::insert_privileged_account(&pool, "13900001202").await;
        let response = execute(
            &pool,
            &blob,
            &cache,
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
        let cache = Cache::try_new_pg(pool.clone()).await.expect("cache");
        let actor = tests::insert_test_account(&pool, "13900001210").await;
        let err = execute(
            &pool,
            &blob,
            &cache,
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
        let cache = Cache::try_new_pg(pool.clone()).await.expect("cache");
        let actor = tests::insert_privileged_account(&pool, "13900001220").await;
        tests::insert_test_account(&pool, "13900001221").await;
        let err = execute(
            &pool,
            &blob,
            &cache,
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
        let cache = Cache::try_new_pg(pool.clone()).await.expect("cache");
        let actor = tests::insert_privileged_account(&pool, "13900001230").await;
        let staging = "staging/2026/01/missing.webp";
        attachment::claim_staging(&cache, staging, actor)
            .await
            .expect("claim");
        let err = execute(
            &pool,
            &blob,
            &cache,
            actor,
            CreateAccountRequest {
                name: "Tom".to_string(),
                phone: "13900001231".to_string(),
                password: Password::new_unchecked("admin123!".to_string()),
                avatar: Some(staging.to_string()),
            },
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("account_avatar_not_found"));
        // 半成功防护：手机号未占用
        assert!(
            repository::find_auth_by_phone(
                &pool,
                &PhoneNumber::new_unchecked("13900001231".into())
            )
            .await
            .unwrap()
            .is_none()
        );
    }

    /// 模拟 execute 在 commit 之后、create_tx UNIQUE 失败：同事务回滚 + best-effort detach。
    #[sqlx::test]
    async fn test_create_tx_unique_failure_detaches_committed_avatar(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let blob = Blob::new_for_test().await.expect("test blob");
        let cache = Cache::try_new_pg(pool.clone()).await.expect("cache");
        tests::insert_test_account(&pool, "13900001241").await;
        let owner = Id::new_unchecked(1);

        let staging = "staging/2026/01/dup.webp";
        prep_staging(&blob, &cache, owner, staging).await;
        let path = avatar_ops::commit_avatar(&pool, &blob, &cache, owner, staging)
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
        // 与 execute 失败分支同序：create_tx 失败 → drop tx → detach_best_effort
        let mut tx = pool.begin().await.expect("begin");
        let err = repository::create_tx(&mut tx, &account)
            .await
            .expect_err("dup");
        assert!(err.to_string().contains("account_duplicated"));
        drop(tx);
        avatar_ops::detach_avatar_best_effort(&pool, &path).await;
        let row = sqlx::query!(
            r#"SELECT ref_count, tombstoned_at FROM blob_objects WHERE path = $1"#,
            path
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.ref_count, 0);
        assert!(row.tombstoned_at.is_some());
        assert!(
            repository::find_auth_by_phone(
                &pool,
                &PhoneNumber::new_unchecked("13900001241".into())
            )
            .await
            .unwrap()
            .is_some(),
            "pre-existing account must remain; no half-created row"
        );
    }

    /// attach 失败时 create 事务回滚：手机号未占用、新 path 已 tombstone。
    #[sqlx::test]
    async fn test_create_attach_failure_rolls_back_account(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let blob = Blob::new_for_test().await.expect("test blob");
        let cache = Cache::try_new_pg(pool.clone()).await.expect("cache");
        let actor = tests::insert_privileged_account(&pool, "13900001250").await;
        let staging = "staging/2026/01/attachfail.webp";
        prep_staging(&blob, &cache, actor, staging).await;
        let path = avatar_ops::commit_avatar(&pool, &blob, &cache, actor, staging)
            .await
            .expect("commit");
        // 拆掉登记，使 attach_tx 失败（模拟 commit 后登记异常）
        sqlx::query!("DELETE FROM blob_objects WHERE path = $1", path)
            .execute(&pool)
            .await
            .unwrap();

        let phone = "13900001251";
        let account = Account {
            id: Id::default(),
            name: "AttachFail".into(),
            phone: PhoneNumber::new_unchecked(phone.into()),
            password: HashedPassword::try_new(&Password::new_unchecked("admin123!".into()))
                .await
                .unwrap(),
            privileged: false,
            avatar: Some(path.clone()),
            version: 1,
        };
        let mut tx = pool.begin().await.expect("begin");
        repository::create_tx(&mut tx, &account)
            .await
            .expect("create");
        let err = avatar_ops::attach_avatar_tx(&mut tx, &path)
            .await
            .expect_err("attach");
        drop(tx);
        avatar_ops::detach_avatar_best_effort(&pool, &path).await;
        assert!(err.to_string().contains("file_attachment_not_registered"));
        assert!(
            repository::find_auth_by_phone(&pool, &PhoneNumber::new_unchecked(phone.into()))
                .await
                .unwrap()
                .is_none(),
            "account must roll back when attach fails in same tx"
        );
    }
}
