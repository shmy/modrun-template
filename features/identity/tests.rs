use crate::value_object::HashedPassword;
use jwt_kit::{TokenHelper, TokenRealm};
use secrecy::SecretString;
use shared_contract::Id;
use sqlx::PgPool;

pub fn test_token_helper() -> TokenHelper {
    TokenHelper::new(
        TokenRealm::Account,
        SecretString::from("test-jwt-secret-at-least-32-bytes!!"),
        60,
        3600,
        "modrun-template",
        "account",
    )
}

pub async fn insert_test_account(pool: &PgPool, phone: &str) -> Id {
    insert_test_account_with(pool, phone, false).await
}

pub async fn insert_privileged_account(pool: &PgPool, phone: &str) -> Id {
    insert_test_account_with(pool, phone, true).await
}

async fn insert_test_account_with(pool: &PgPool, phone: &str, privileged: bool) -> Id {
    let id = Id::default();
    let password = HashedPassword::try_new_sync("test1234").expect("hash password");
    sqlx::query!(
        r#"
            INSERT INTO accounts (id, name, phone, password, privileged, version)
            VALUES ($1, $2, $3, $4, $5, 1)
            "#,
        *id,
        format!("test-{phone}"),
        phone,
        password.as_str(),
        privileged
    )
    .execute(pool)
    .await
    .expect("insert test account");
    id
}
