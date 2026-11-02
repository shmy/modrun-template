use std::time::Duration;

use authn_kit::{
    access_jti_key, refresh_digest, refresh_key, refresh_key_from_digest, subject_refresh_key,
    used_refresh_key,
};
use cache_kit::Cache;
use identity_contract::IdentityError;
use jwt_kit::TokenHelper;
use rootcause::Result;
use shared_contract::Id;
use uuid::Uuid;

use super::cache_error::map_cache_err;

pub struct TokenPair {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64,
}

async fn del_refresh_by_digest(cache: &Cache, realm: &str, digest: Option<String>) -> Result<()> {
    match digest {
        Some(digest) => cache
            .del(&refresh_key_from_digest(realm, &digest))
            .await
            .map_err(map_cache_err),
        None => Ok(()),
    }
}

pub async fn issue_tokens(
    cache: &Cache,
    token_helper: &TokenHelper,
    user_id: &Id,
) -> Result<TokenPair> {
    let jti = Uuid::new_v4().to_string();
    let refresh_token = Uuid::new_v4().to_string();
    let (access_token, expires_in) = token_helper.encode_access_token(user_id, &jti)?;

    let realm = token_helper.realm();
    let user_refresh_key = subject_refresh_key(realm, user_id);
    let access_ttl = Duration::from_secs(token_helper.access_ttl_secs());
    let refresh_ttl = Duration::from_secs(token_helper.refresh_ttl_secs());

    // 旧 digest 需要先查（串行一轮），其余 3 个键互不依赖，并行写入。
    // 跨键无事务：若部分写入失败可能短暂并存新旧 refresh；下次 login/refresh 会覆盖 subject 指针并清理旧 digest。
    let old_digest = cache
        .get::<String>(&user_refresh_key)
        .await
        .map_err(map_cache_err)?;
    let user_id_str = user_id.to_string();
    let digest = refresh_digest(&refresh_token);
    let clear_old = del_refresh_by_digest(cache, realm, old_digest);
    let refresh_key = refresh_key_from_digest(realm, &digest);
    let jti_key = access_jti_key(realm, user_id);
    let set_refresh = cache.set_ex(&refresh_key, &user_id_str, refresh_ttl);
    let set_subject = cache.set_ex(&user_refresh_key, &digest, refresh_ttl);
    let set_jti = cache.set_ex(&jti_key, &jti, access_ttl);
    let (r0, r1, r2, r3) = tokio::join!(clear_old, set_refresh, set_subject, set_jti);
    [r0, r1, r2, r3]
        .into_iter()
        .try_for_each(|r| r.map_err(map_cache_err))?;

    Ok(TokenPair {
        access_token,
        refresh_token,
        expires_in,
    })
}

/// 原子消费一次 refresh，返回所属账号。
///
/// 调用方**必须在成功签发新令牌后**调用 [`commit_refresh_rotation`]，
/// 否则「已用」标记不会被写入，事后重放将无法被识别。
///
/// 「已用」标记推后到签发之后：若在签发**之前**写入，并发重复提交会在
/// `take` 失败后看到标记并误判为重放，把正在写新令牌的会话一并吊销。
/// 推后之后，并发失败者看不到 `used`（只 401），只有 commit 完成后的
/// 真重放才会命中 `used` 并吊销——**命中即吊销，不再用 inflight 跳过**，
/// 避免 cache 清理失败时留下「只拒绝不吊销」的安全窗口。
pub async fn consume_refresh_token(
    cache: &Cache,
    token_helper: &TokenHelper,
    refresh_token: &str,
) -> Result<Id> {
    let realm = token_helper.realm();
    let digest = refresh_digest(refresh_token);
    let key = refresh_key_from_digest(realm, &digest);

    match cache.take::<Id>(&key).await.map_err(map_cache_err)? {
        Some(user_id) => Ok(user_id),
        None => {
            let Some(user_id) = cache
                .get::<Id>(&used_refresh_key(realm, &digest))
                .await
                .map_err(map_cache_err)?
            else {
                return Err(IdentityError::RefreshTokenInvalid.into());
            };
            tracing::warn!(%user_id, "refresh token reuse detected, revoking all sessions");
            revoke_tokens(cache, token_helper, user_id).await?;
            Err(IdentityError::RefreshTokenInvalid.into())
        }
    }
}

/// 签发成功后收尾：写入「已用」标记。
///
/// 必须在 [`issue_tokens`] 成功之后调用——此时新 refresh 已经落库，
/// 旧 digest 再被使用才是真正的重放。
///
/// **失败语义**：写入失败时返回 `Err`（调用方应 [`abort_refresh_rotation`]，
/// 绝不能对客户端返回 200——否则重放检测永久失效）。
pub async fn commit_refresh_rotation(
    cache: &Cache,
    token_helper: &TokenHelper,
    user_id: Id,
    consumed_refresh_token: &str,
) -> Result<()> {
    let realm = token_helper.realm();
    let digest = refresh_digest(consumed_refresh_token);
    let used_key = used_refresh_key(realm, &digest);
    cache
        .set_ex(
            &used_key,
            &user_id,
            Duration::from_secs(token_helper.refresh_ttl_secs()),
        )
        .await
        .map_err(map_cache_err)?;
    Ok(())
}

pub async fn restore_refresh_token(
    cache: &Cache,
    token_helper: &TokenHelper,
    refresh_token: &str,
    user_id: &Id,
) -> Result<()> {
    let realm = token_helper.realm();
    cache
        .set_ex(
            &refresh_key(realm, refresh_token),
            &user_id.to_string(),
            Duration::from_secs(token_helper.refresh_ttl_secs()),
        )
        .await
        .map_err(map_cache_err)?;
    Ok(())
}

/// `commit_refresh_rotation` 失败后的 fail-closed 收尾：
/// 1. 吊销刚签发的新会话（避免无 `used` 标记的「幽灵会话」）
/// 2. 归还已 `take` 的旧 refresh，让客户端可重试
///
/// 两步各自尽力：`revoke` 失败仍尝试 `restore`，避免落到「新旧皆无」。
pub async fn abort_refresh_rotation(
    cache: &Cache,
    token_helper: &TokenHelper,
    user_id: Id,
    consumed_refresh_token: &str,
) -> Result<()> {
    let revoke_result = revoke_tokens(cache, token_helper, user_id).await;
    let restore_result =
        restore_refresh_token(cache, token_helper, consumed_refresh_token, &user_id).await;
    match (revoke_result, restore_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(revoke_err), Ok(())) => Err(revoke_err),
        (Ok(()), Err(restore_err)) => Err(restore_err),
        (Err(revoke_err), Err(restore_err)) => {
            tracing::error!(
                revoke_error = %revoke_err,
                restore_error = %restore_err,
                %user_id,
                "both revoke and restore failed during abort_refresh_rotation"
            );
            Err(revoke_err)
        }
    }
}

pub async fn revoke_tokens(cache: &Cache, token_helper: &TokenHelper, user_id: Id) -> Result<()> {
    let realm = token_helper.realm();
    let user_refresh_key = subject_refresh_key(realm, user_id);

    // `take` 同时读取并删除 subject 指针（原子）；两个删除互不依赖，并行执行。
    let old_digest = cache
        .take::<String>(&user_refresh_key)
        .await
        .map_err(map_cache_err)?;
    let clear_old = del_refresh_by_digest(cache, realm, old_digest);
    let jti_key = access_jti_key(realm, user_id);
    let clear_jti = cache.del(&jti_key);
    let (r0, r1) = tokio::join!(clear_old, clear_jti);
    [r0, r1]
        .into_iter()
        .try_for_each(|r| r.map_err(map_cache_err))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests;
    use migration::run_migrations;

    #[sqlx::test]
    async fn revoke_drops_refresh_and_jti(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let cache = Cache::try_new_pg(pool).await.unwrap();
        let token_helper = tests::test_token_helper();
        let user_id = Id::default();
        let tokens = issue_tokens(&cache, &token_helper, &user_id).await.unwrap();
        revoke_tokens(&cache, &token_helper, user_id).await.unwrap();
        let err = consume_refresh_token(&cache, &token_helper, &tokens.refresh_token)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("refresh_token_invalid"));
        let jti = cache
            .get::<String>(&access_jti_key(token_helper.realm(), user_id))
            .await
            .unwrap();
        assert!(jti.is_none());
    }

    #[sqlx::test]
    async fn restore_refresh_allows_retry_after_consume(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let cache = Cache::try_new_pg(pool).await.unwrap();
        let token_helper = tests::test_token_helper();
        let user_id = Id::default();
        let tokens = issue_tokens(&cache, &token_helper, &user_id).await.unwrap();
        let consumed = consume_refresh_token(&cache, &token_helper, &tokens.refresh_token)
            .await
            .unwrap();
        restore_refresh_token(&cache, &token_helper, &tokens.refresh_token, &consumed)
            .await
            .unwrap();
        let again = consume_refresh_token(&cache, &token_helper, &tokens.refresh_token)
            .await
            .unwrap();
        assert_eq!(consumed, again);
    }

    #[sqlx::test]
    async fn concurrent_refresh_only_one_succeeds(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let cache = Cache::try_new_pg(pool).await.unwrap();
        let token_helper = tests::test_token_helper();
        let tokens = issue_tokens(&cache, &token_helper, &Id::default())
            .await
            .unwrap();
        let token = tokens.refresh_token.clone();
        let (r1, r2) = tokio::join!(
            consume_refresh_token(&cache, &token_helper, &token),
            consume_refresh_token(&cache, &token_helper, &token),
        );
        let ok_count = [r1.is_ok(), r2.is_ok()]
            .into_iter()
            .filter(|ok| *ok)
            .count();
        assert_eq!(ok_count, 1);
    }

    #[sqlx::test]
    async fn refresh_reuse_revokes_rotated_session(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let cache = Cache::try_new_pg(pool).await.unwrap();
        let token_helper = tests::test_token_helper();
        let user_id = Id::default();
        let tokens_a = issue_tokens(&cache, &token_helper, &user_id).await.unwrap();

        let consumed_id = consume_refresh_token(&cache, &token_helper, &tokens_a.refresh_token)
            .await
            .unwrap();
        let tokens_b = issue_tokens(&cache, &token_helper, &consumed_id)
            .await
            .unwrap();
        commit_refresh_rotation(&cache, &token_helper, consumed_id, &tokens_a.refresh_token)
            .await
            .unwrap();

        let err = consume_refresh_token(&cache, &token_helper, &tokens_a.refresh_token)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("refresh_token_invalid"));

        let refresh_err = consume_refresh_token(&cache, &token_helper, &tokens_b.refresh_token)
            .await
            .unwrap_err();
        assert!(
            refresh_err.to_string().contains("refresh_token_invalid"),
            "replay should revoke the rotated session"
        );
    }

    /// commit 失败后的 abort：新会话作废，旧 refresh 可重试。
    #[sqlx::test]
    async fn abort_after_commit_failure_restores_old_refresh(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let cache = Cache::try_new_pg(pool).await.unwrap();
        let token_helper = tests::test_token_helper();
        let user_id = Id::default();
        let tokens_a = issue_tokens(&cache, &token_helper, &user_id).await.unwrap();

        let consumed = consume_refresh_token(&cache, &token_helper, &tokens_a.refresh_token)
            .await
            .unwrap();
        let tokens_b = issue_tokens(&cache, &token_helper, &consumed)
            .await
            .unwrap();

        abort_refresh_rotation(&cache, &token_helper, consumed, &tokens_a.refresh_token)
            .await
            .unwrap();

        let revived = consume_refresh_token(&cache, &token_helper, &tokens_a.refresh_token)
            .await
            .expect("old refresh must be restorable after abort");
        assert_eq!(revived, consumed);

        let new_dead = consume_refresh_token(&cache, &token_helper, &tokens_b.refresh_token)
            .await
            .unwrap_err();
        assert!(
            new_dead.to_string().contains("refresh_token_invalid"),
            "aborted new session must be revoked"
        );
    }

    /// abort 在 revoke 已生效的前提下，restore 必须仍被执行（分步失败时也不能只 revoke）。
    ///
    /// 这里用「revoke 后再 abort」模拟：第二次 revoke 是幂等的，restore 仍要把旧
    /// refresh 写回——覆盖「两步都要尽力」的契约。
    #[sqlx::test]
    async fn abort_still_restores_after_prior_revoke(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let cache = Cache::try_new_pg(pool).await.unwrap();
        let token_helper = tests::test_token_helper();
        let user_id = Id::default();
        let tokens_a = issue_tokens(&cache, &token_helper, &user_id).await.unwrap();
        let consumed = consume_refresh_token(&cache, &token_helper, &tokens_a.refresh_token)
            .await
            .unwrap();
        let _tokens_b = issue_tokens(&cache, &token_helper, &consumed)
            .await
            .unwrap();

        revoke_tokens(&cache, &token_helper, consumed)
            .await
            .expect("pre-revoke");
        abort_refresh_rotation(&cache, &token_helper, consumed, &tokens_a.refresh_token)
            .await
            .expect("abort must succeed even if revoke already happened");

        consume_refresh_token(&cache, &token_helper, &tokens_a.refresh_token)
            .await
            .expect("restore must run after revoke in abort");
    }

    /// 并发重复提交（客户端多标签页 / 网络重试）不得吊销正在轮换的会话。
    ///
    /// 回归保护：修复前 `used` 标记在签发**之前**写入，第二个请求会把它当成重放，
    /// 触发 `revoke_tokens` 把第一个请求刚签发的新会话一起踢掉。
    #[sqlx::test]
    async fn concurrent_refresh_does_not_revoke_winner(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let cache = Cache::try_new_pg(pool).await.unwrap();
        let token_helper = tests::test_token_helper();
        let user_id = Id::default();
        let tokens = issue_tokens(&cache, &token_helper, &user_id).await.unwrap();
        let token = tokens.refresh_token.clone();

        let (r1, r2) = tokio::join!(
            consume_refresh_token(&cache, &token_helper, &token),
            consume_refresh_token(&cache, &token_helper, &token),
        );
        let winner = match (r1, r2) {
            (Ok(id), _) | (_, Ok(id)) => id,
            (Err(e), Err(_)) => panic!("both concurrent consumes failed: {e}"),
        };

        let new_tokens = issue_tokens(&cache, &token_helper, &winner).await.unwrap();
        commit_refresh_rotation(&cache, &token_helper, winner, &token)
            .await
            .unwrap();

        let still_alive =
            consume_refresh_token(&cache, &token_helper, &new_tokens.refresh_token).await;
        assert!(
            still_alive.is_ok(),
            "winner's freshly issued refresh must survive a concurrent duplicate"
        );
    }

    #[sqlx::test]
    async fn cache_keys_do_not_store_raw_refresh_token(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("run migrations");
        let cache = Cache::try_new_pg(pool.clone()).await.unwrap();
        let token_helper = tests::test_token_helper();
        let tokens = issue_tokens(&cache, &token_helper, &Id::default())
            .await
            .unwrap();
        let rows = sqlx::query_as::<_, (String, String)>("SELECT key, value FROM _pg_caches")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert!(!rows.is_empty());
        for (key, value) in rows {
            assert!(
                !key.contains(&tokens.refresh_token),
                "cache key leaked refresh token: {key}"
            );
            assert!(
                !value.contains(&tokens.refresh_token),
                "cache value leaked refresh token: {value}"
            );
        }
    }
}
