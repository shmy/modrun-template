use axum::{Json, extract::State};
use http_auth::AuthedAccount;
use identity_contract::{Account, HashedPassword, Password};
use serde::{Deserialize, Serialize};
use shared_contract::{Id, PhoneNumber};
use sqlx::PgPool;
use utoipa::ToSchema;
use validify::Validify;
use web_error::{JsonResponseType, ProblemDetails};
use web_state::{ValidJson, WebState};

use crate::repository;

#[derive(Debug, Deserialize, Validify, ToSchema)]
pub struct CreateAccountRequest {
    #[modify(trim)]
    #[validate(length(min = 1, message = "account_name_too_short"))]
    #[validate(length(max = 64, message = "account_name_too_long"))]
    #[schema(example = "Tom")]
    pub name: String,
    #[validify]
    pub phone: PhoneNumber,
    #[schema(example = "P@ssw0rd1")]
    #[validify]
    pub password: Password,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CreateAccountResponse {
    pub id: Id,
    pub name: String,
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
        (status = 409, description = "手机号已存在", body = ProblemDetails)
    ),
    security(("bearerAuth" = []))
)]
pub async fn handler(
    AuthedAccount(actor_id): AuthedAccount,
    State(state): State<WebState>,
    ValidJson(request): ValidJson<CreateAccountRequest>,
) -> JsonResponseType<CreateAccountResponse> {
    let response = execute(&state.pg, actor_id, request).await?;
    Ok(Json(response))
}

pub(crate) async fn execute(
    pool: &PgPool,
    actor_id: Id,
    request: CreateAccountRequest,
) -> rootcause::Result<CreateAccountResponse> {
    repository::require_privileged(pool, actor_id).await?;
    let id = Id::default();
    let account = Account {
        id,
        name: request.name,
        phone: request.phone,
        password: HashedPassword::try_new(&request.password).await?,
        privileged: false,
        version: 1,
    };
    repository::create(pool, &account).await?;
    Ok(CreateAccountResponse {
        id,
        name: account.name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests;
    use identity_contract::Password;
    use migration::run_migrations;

    #[sqlx::test]
    async fn test_create_success(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let actor = tests::insert_privileged_account(&pool, "13900001200").await;
        let response = execute(
            &pool,
            actor,
            CreateAccountRequest {
                name: "Tom".to_string(),
                phone: PhoneNumber::try_new("13900001201").unwrap(),
                password: Password::new_unchecked("admin123!".to_string()),
            },
        )
        .await
        .unwrap();
        assert!(i64::from(response.id) > 0);
        assert_eq!(response.name, "Tom");
    }

    #[sqlx::test]
    async fn test_create_forbidden_for_non_privileged(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let actor = tests::insert_test_account(&pool, "13900001210").await;
        let err = execute(
            &pool,
            actor,
            CreateAccountRequest {
                name: "Tom".to_string(),
                phone: PhoneNumber::try_new("13900001211").unwrap(),
                password: Password::new_unchecked("admin123!".to_string()),
            },
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("account_forbidden"));
    }

    #[sqlx::test]
    async fn test_create_duplicated_phone(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let actor = tests::insert_privileged_account(&pool, "13900001220").await;
        tests::insert_test_account(&pool, "13900001221").await;
        let err = execute(
            &pool,
            actor,
            CreateAccountRequest {
                name: "Tom".to_string(),
                phone: PhoneNumber::try_new("13900001221").unwrap(),
                password: Password::new_unchecked("admin123!".to_string()),
            },
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("account_duplicated"));
    }
}
