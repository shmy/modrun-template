use axum::{Json, extract::State};
use cache_kit::Cache;
use http_auth::AuthedAccount;
use http_extractors::ValidPath;
use jwt_kit::TokenHelper;
use serde::Serialize;
use shared_contract::Id;
use sqlx::PgPool;
use utoipa::ToSchema;
use web_error::{JsonResponseType, ProblemDetails};
use web_state::{AppState, HasSession};

use crate::dto::AccountIdPath;
use crate::repository;
use crate::shared::{avatar_ops, token_ops};

#[derive(Debug, Serialize, ToSchema)]
pub struct DeleteAccountResponse {
    #[schema(value_type = String)]
    pub id: Id,
    #[schema(example = true)]
    pub deleted: bool,
}

#[utoipa::path(
    delete,
    path = "/api/v1/accounts/{id}",
    operation_id = "account_delete",
    tag = "account",
    params(AccountIdPath),
    responses(
        (status = 200, description = "删除成功（目标不存在时也返回 deleted: true，幂等）", body = DeleteAccountResponse),
        (status = 401, description = "未认证", body = ProblemDetails),
        (status = 403, description = "调用方不是特权账号，或目标受保护", body = ProblemDetails)
    ),
    security(("bearerAuth" = []))
)]
pub async fn handler(
    AuthedAccount(actor_id): AuthedAccount,
    State(state): State<AppState>,
    ValidPath(path): ValidPath<AccountIdPath>,
) -> JsonResponseType<DeleteAccountResponse> {
    run(&state, actor_id, path).await
}

async fn run(
    state: &impl HasSession,
    actor_id: Id,
    path: AccountIdPath,
) -> JsonResponseType<DeleteAccountResponse> {
    let response = execute(
        state.pool(),
        state.cache(),
        state.token_helper(),
        actor_id,
        path,
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
) -> rootcause::Result<DeleteAccountResponse> {
    let before = repository::require_unprotected_target(pool, actor_id, path.id).await?;
    // detach 与 DELETE 同事务：禁止「先删行 → ref_count≥1 且安全网查不到 → 永久泄漏」。
    // revoke 放在 commit 之后：避免 tx 失败时会话已吊销、账号仍在。
    let mut tx = pool.begin().await?;
    if let Some(account) = before.as_ref()
        && let Some(avatar) = account.avatar.as_deref()
    {
        avatar_ops::detach_avatar_tx_for_delete(&mut tx, avatar).await?;
    }
    repository::delete_tx(&mut tx, path.id).await?;
    tx.commit().await?;
    token_ops::revoke_tokens(cache, token_helper, path.id).await?;
    Ok(DeleteAccountResponse {
        id: path.id,
        deleted: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests;
    use blob_kit::Blob;
    use file::attachment;
    use migration::run_migrations;
    use std::io::Cursor;

    #[sqlx::test]
    async fn test_delete_success(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let cache = Cache::try_new_pg(pool.clone()).await.unwrap();
        let token_helper = tests::test_token_helper();
        let actor = tests::insert_privileged_account(&pool, "13900001800").await;
        let account_id = tests::insert_test_account(&pool, "13900001801").await;
        let response = execute(
            &pool,
            &cache,
            &token_helper,
            actor,
            AccountIdPath { id: account_id },
        )
        .await
        .unwrap();
        assert_eq!(response.id, account_id);
        assert!(response.deleted);
        let err = repository::find_by_id(&pool, account_id).await.unwrap_err();
        assert!(err.to_string().contains("account_not_found"));
    }

    #[sqlx::test]
    async fn test_delete_detaches_avatar(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let cache = Cache::try_new_pg(pool.clone()).await.unwrap();
        let token_helper = tests::test_token_helper();
        let blob = Blob::new_for_test().await.expect("test blob");
        let actor = tests::insert_privileged_account(&pool, "13900001803").await;
        let account_id = tests::insert_test_account(&pool, "13900001804").await;
        let staging = "staging/2026/01/del.webp";
        blob.write(staging, Cursor::new(b"img"))
            .await
            .expect("write");
        attachment::claim_staging(&cache, staging, actor)
            .await
            .expect("claim");
        let path = avatar_ops::commit_avatar(&pool, &blob, &cache, actor, staging)
            .await
            .expect("commit");
        avatar_ops::attach_avatar(&pool, &path)
            .await
            .expect("attach");
        repository::set_avatar(&pool, account_id, Some(path.clone()))
            .await
            .expect("set avatar");

        execute(
            &pool,
            &cache,
            &token_helper,
            actor,
            AccountIdPath { id: account_id },
        )
        .await
        .unwrap();
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
            repository::find_optional_by_id(&pool, account_id)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[sqlx::test]
    async fn test_delete_idempotent(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let cache = Cache::try_new_pg(pool.clone()).await.unwrap();
        let token_helper = tests::test_token_helper();
        let actor = tests::insert_privileged_account(&pool, "13900001810").await;
        let account_id = tests::insert_test_account(&pool, "13900001802").await;
        execute(
            &pool,
            &cache,
            &token_helper,
            actor,
            AccountIdPath { id: account_id },
        )
        .await
        .unwrap();
        let response = execute(
            &pool,
            &cache,
            &token_helper,
            actor,
            AccountIdPath { id: account_id },
        )
        .await
        .unwrap();
        assert!(response.deleted);
    }

    #[sqlx::test]
    async fn test_delete_protected(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let cache = Cache::try_new_pg(pool.clone()).await.unwrap();
        let token_helper = tests::test_token_helper();
        let actor = tests::insert_privileged_account(&pool, "13900001820").await;
        let err = execute(
            &pool,
            &cache,
            &token_helper,
            actor,
            AccountIdPath { id: actor },
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("account_protected"));
    }

    #[sqlx::test]
    async fn test_delete_revokes_tokens(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let cache = Cache::try_new_pg(pool.clone()).await.unwrap();
        let token_helper = tests::test_token_helper();
        let actor = tests::insert_privileged_account(&pool, "13900001821").await;
        let account_id = tests::insert_test_account(&pool, "13900001822").await;
        let tokens = token_ops::issue_tokens(&cache, &token_helper, &account_id)
            .await
            .unwrap();
        execute(
            &pool,
            &cache,
            &token_helper,
            actor,
            AccountIdPath { id: account_id },
        )
        .await
        .unwrap();
        let err = token_ops::consume_refresh_token(&cache, &token_helper, &tokens.refresh_token)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("refresh_token_invalid"));
    }

    /// 业务列有 avatar 但 blob_objects 无登记：不得卡死删号。
    #[sqlx::test]
    async fn test_delete_unregistered_avatar_still_succeeds(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let cache = Cache::try_new_pg(pool.clone()).await.unwrap();
        let token_helper = tests::test_token_helper();
        let actor = tests::insert_privileged_account(&pool, "13900001830").await;
        let account_id = tests::insert_test_account(&pool, "13900001831").await;
        repository::set_avatar(&pool, account_id, Some("2026/01/orphan.webp".into()))
            .await
            .expect("dirty avatar");
        let tokens = token_ops::issue_tokens(&cache, &token_helper, &account_id)
            .await
            .unwrap();

        execute(
            &pool,
            &cache,
            &token_helper,
            actor,
            AccountIdPath { id: account_id },
        )
        .await
        .expect("delete must succeed despite missing registration");
        assert!(
            repository::find_optional_by_id(&pool, account_id)
                .await
                .unwrap()
                .is_none()
        );
        let err = token_ops::consume_refresh_token(&cache, &token_helper, &tokens.refresh_token)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("refresh_token_invalid"));
    }
}
