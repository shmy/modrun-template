use axum::{Json, extract::State};
use http_auth::AuthedAccount;
use http_extractors::ValidPath;
use shared_contract::Id;
use sqlx::PgPool;
use web_error::{JsonResponseType, ProblemDetails};
use web_state::{HasPool, WebState};

use crate::dto::{AccountDetail, AccountIdPath};
use crate::repository;

#[utoipa::path(
    get,
    path = "/api/v1/accounts/{id}",
    operation_id = "account_get",
    tag = "account",
    params(AccountIdPath),
    responses(
        (status = 200, body = AccountDetail),
        (status = 401, description = "未认证", body = ProblemDetails),
        (status = 403, description = "调用方不是特权账号", body = ProblemDetails),
        (status = 404, description = "账号不存在", body = ProblemDetails)
    ),
    security(("bearerAuth" = []))
)]
pub async fn handler(
    AuthedAccount(actor_id): AuthedAccount,
    State(state): State<WebState>,
    ValidPath(path): ValidPath<AccountIdPath>,
) -> JsonResponseType<AccountDetail> {
    let response = execute(state.pool(), actor_id, path).await?;
    Ok(Json(response))
}

pub(crate) async fn execute(
    pool: &PgPool,
    actor_id: Id,
    path: AccountIdPath,
) -> rootcause::Result<AccountDetail> {
    repository::require_privileged(pool, actor_id).await?;
    let account = repository::find_by_id(pool, path.id).await?;
    Ok(account.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests;
    use migration::run_migrations;
    use shared_contract::Id;

    #[sqlx::test]
    async fn test_get_success(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let actor = tests::insert_privileged_account(&pool, "13900001900").await;
        let account_id = tests::insert_test_account(&pool, "13900001901").await;
        let response = execute(&pool, actor, AccountIdPath { id: account_id })
            .await
            .unwrap();
        assert_eq!(response.id, account_id);
        assert!(response.name.starts_with("test-"));
        assert_eq!(response.phone.as_str(), "13900001901");
        assert!(!response.privileged);
        assert_eq!(response.version, 1);
    }

    #[sqlx::test]
    async fn test_get_not_found(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let actor = tests::insert_privileged_account(&pool, "13900001902").await;
        let err = execute(
            &pool,
            actor,
            AccountIdPath {
                id: Id::new_unchecked(999),
            },
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("account_not_found"));
    }

    #[sqlx::test]
    async fn test_get_forbidden_for_non_privileged(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let actor = tests::insert_test_account(&pool, "13900001910").await;
        let target = tests::insert_test_account(&pool, "13900001911").await;
        let err = execute(&pool, actor, AccountIdPath { id: target })
            .await
            .unwrap_err();
        assert!(err.to_string().contains("account_forbidden"));
    }
}
