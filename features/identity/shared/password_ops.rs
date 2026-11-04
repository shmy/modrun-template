//! 改密写库与会话吊销的固定顺序。

use cache_kit::Cache;
use jwt_kit::TokenHelper;
use rootcause::Result;
use shared_contract::Id;
use sqlx::PgPool;

use crate::repository;
use crate::shared::token_ops;
use crate::value_object::HashedPassword;

/// 先写库再吊销会话。
///
/// 反过来的话 `update_password` 失败（并发删号 / DB 抖动）会出现
/// 「会话已被踢下线但密码没改」。写库成功后再 revoke，最坏只是旧会话多存活
/// 一次往返（此时旧密码已失效，无法再用旧密码换新会话）。
pub async fn commit_password_change(
    pool: &PgPool,
    cache: &Cache,
    token_helper: &TokenHelper,
    user_id: Id,
    new_hashed: &HashedPassword,
) -> Result<()> {
    repository::update_password(pool, user_id, new_hashed).await?;
    token_ops::revoke_tokens(cache, token_helper, user_id).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests;
    use crate::value_object::Password;
    use migration::run_migrations;

    /// 写库失败（行已不存在）时不得吊销会话。
    ///
    /// 直接打 `commit_password_change`：绕过 endpoint 里先于写库的授权检查，
    /// 才能覆盖「require 已通过、随后 update 失败」的竞态窗口。
    #[sqlx::test]
    async fn write_failure_does_not_revoke_sessions(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let cache = Cache::try_new_pg(pool.clone()).await.unwrap();
        let token_helper = tests::test_token_helper();
        let account_id = tests::insert_test_account(&pool, "13900001740").await;
        let tokens = token_ops::issue_tokens(&cache, &token_helper, &account_id)
            .await
            .unwrap();
        let new_hashed = HashedPassword::try_new(&Password::new_unchecked("reset1234".to_string()))
            .await
            .unwrap();

        sqlx::query!("DELETE FROM accounts WHERE id = $1", *account_id)
            .execute(&pool)
            .await
            .unwrap();

        let err = commit_password_change(&pool, &cache, &token_helper, account_id, &new_hashed)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("account_not_found"), "{err}");

        token_ops::consume_refresh_token(&cache, &token_helper, &tokens.refresh_token)
            .await
            .expect("refresh must survive a failed password write");
    }
}
