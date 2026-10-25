use axum::{Json, extract::State};
use http_auth::AuthedAccount;
use identity_contract::IdentityError;
use serde::Deserialize;
use shared_contract::{CursorPagingQuery, CursorPagingResult, Id};
use sqlx::PgPool;
use utoipa::IntoParams;
use validify::Validify;
use web_error::{JsonResponseType, ProblemDetails};
use web_state::{ValidQuery, WebState};

use crate::dto::AccountPublic;
use crate::repository;

/// 搜索关键字 3–64 字符；过短无法有效利用 pg_trgm，过长拖垮 ILIKE。
const MIN_SEARCH_Q_LEN: usize = 3;
const MAX_SEARCH_Q_LEN: usize = 64;

fn prepare_search_q(q: Option<String>) -> rootcause::Result<Option<String>> {
    let q = q.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    if let Some(q) = q.as_deref() {
        let len = q.chars().count();
        if len < MIN_SEARCH_Q_LEN {
            return Err(IdentityError::AccountSearchQTooShort.into());
        }
        if len > MAX_SEARCH_Q_LEN {
            return Err(IdentityError::AccountSearchQTooLong.into());
        }
    }
    Ok(q)
}

#[derive(Debug, Deserialize, Validify, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct SearchAccountQuery {
    #[serde(flatten)]
    #[param(inline)]
    pub paging: CursorPagingQuery,
    #[param(example = "test")]
    #[serde(default)]
    pub q: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/accounts",
    operation_id = "account_search",
    tag = "account",
    params(SearchAccountQuery),
    responses(
        (status = 200, body = CursorPagingResult<AccountPublic>),
        (status = 400, description = "查询参数无效", body = ProblemDetails),
        (status = 401, description = "未认证", body = ProblemDetails),
        (status = 403, description = "调用方不是特权账号", body = ProblemDetails)
    ),
    security(("bearerAuth" = []))
)]
pub async fn handler(
    AuthedAccount(actor_id): AuthedAccount,
    State(state): State<WebState>,
    ValidQuery(query): ValidQuery<SearchAccountQuery>,
) -> JsonResponseType<CursorPagingResult<AccountPublic>> {
    let response = execute(&state.pg, actor_id, query).await?;
    Ok(Json(response))
}

pub(crate) async fn execute(
    pool: &PgPool,
    actor_id: Id,
    query: SearchAccountQuery,
) -> rootcause::Result<CursorPagingResult<AccountPublic>> {
    repository::require_privileged(pool, actor_id).await?;
    let q = prepare_search_q(query.q)?;
    let (items, next_cursor) = repository::search(pool, &query.paging, q.as_deref()).await?;
    Ok(CursorPagingResult {
        items: items.into_iter().map(AccountPublic::from).collect(),
        next_cursor,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests;
    use identity_contract::HashedPassword;
    use migration::run_migrations;
    use serde_json::json;
    use shared_contract::Id as SharedContractId;

    #[sqlx::test]
    async fn test_search_by_phone(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let actor = tests::insert_privileged_account(&pool, "13900002000").await;
        tests::insert_test_account(&pool, "13900002001").await;
        let query: SearchAccountQuery = serde_json::from_value(json!({
            "q": "13900002001"
        }))
        .unwrap();
        let result = execute(&pool, actor, query).await.unwrap();
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].phone.as_str(), "13900002001");
        assert!(result.next_cursor.is_none());
    }

    #[sqlx::test]
    async fn test_search_forbidden_for_non_privileged(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let actor = tests::insert_test_account(&pool, "13900002010").await;
        let query: SearchAccountQuery = serde_json::from_value(json!({})).unwrap();
        let err = execute(&pool, actor, query).await.unwrap_err();
        assert!(err.to_string().contains("account_forbidden"));
    }

    #[sqlx::test]
    async fn test_search_cursor_pagination(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let actor = tests::insert_privileged_account(&pool, "13900002200").await;
        tests::insert_test_account(&pool, "13900002201").await;
        tests::insert_test_account(&pool, "13900002202").await;
        tests::insert_test_account(&pool, "13900002203").await;

        let page1 = execute(
            &pool,
            actor,
            serde_json::from_value(json!({ "limit": 2 })).unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(page1.items.len(), 2);
        assert!(page1.next_cursor.is_some());

        let cursor = page1.next_cursor.unwrap();
        let page2 = execute(
            &pool,
            actor,
            serde_json::from_value(json!({
                "limit": 2,
                "cursor": cursor.to_string()
            }))
            .unwrap(),
        )
        .await
        .unwrap();
        assert!(!page2.items.is_empty());
        assert!(page2.next_cursor.is_none());
    }

    #[sqlx::test]
    async fn test_search_empty_q_is_unfiltered(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let actor = tests::insert_privileged_account(&pool, "13900002300").await;
        tests::insert_test_account(&pool, "13900002301").await;
        let empty = execute(
            &pool,
            actor,
            serde_json::from_value(json!({ "q": "" })).unwrap(),
        )
        .await
        .unwrap();
        let none = execute(&pool, actor, serde_json::from_value(json!({})).unwrap())
            .await
            .unwrap();
        assert_eq!(empty.items.len(), none.items.len());
        assert!(empty.items.len() >= 2);
    }

    #[sqlx::test]
    async fn test_search_q_too_short(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let actor = tests::insert_privileged_account(&pool, "13900002510").await;
        let query: SearchAccountQuery = serde_json::from_value(json!({ "q": "ab" })).unwrap();
        let err = execute(&pool, actor, query).await.unwrap_err();
        assert!(err.to_string().contains("account_search_q_too_short"));
    }

    #[sqlx::test]
    async fn test_search_q_too_long(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let actor = tests::insert_privileged_account(&pool, "13900002500").await;
        let q = "a".repeat(65);
        let query: SearchAccountQuery = serde_json::from_value(json!({ "q": q })).unwrap();
        let err = execute(&pool, actor, query).await.unwrap_err();
        assert!(err.to_string().contains("account_search_q_too_long"));
    }

    #[sqlx::test]
    async fn test_search_like_metacharacters_are_literal(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let actor = tests::insert_privileged_account(&pool, "13900002400").await;
        let named_id = SharedContractId::default();
        let password = HashedPassword::try_new_sync("test1234").unwrap();
        sqlx::query!(
            r#"
            INSERT INTO accounts (id, name, phone, password, privileged, version)
            VALUES ($1, $2, $3, $4, FALSE, 1)
            "#,
            *named_id,
            "100%match",
            "13900002401",
            password.as_str()
        )
        .execute(&pool)
        .await
        .unwrap();

        let percent = execute(
            &pool,
            actor,
            serde_json::from_value(json!({ "q": "100" })).unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(percent.items.len(), 1);
        assert_eq!(percent.items[0].id, named_id);

        let underscore = execute(
            &pool,
            actor,
            serde_json::from_value(json!({ "q": "match" })).unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(underscore.items.len(), 1);
        assert_eq!(underscore.items[0].id, named_id);
    }
}
