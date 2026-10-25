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

pub async fn consume_refresh_token(
    cache: &Cache,
    token_helper: &TokenHelper,
    refresh_token: &str,
) -> Result<Id> {
    let realm = token_helper.realm();
    let digest = refresh_digest(refresh_token);
    let key = refresh_key_from_digest(realm, &digest);

    match cache.take::<Id>(&key).await.map_err(map_cache_err)? {
        Some(user_id) => {
            cache
                .set_ex(
                    &used_refresh_key(realm, &digest),
                    &user_id,
                    Duration::from_secs(token_helper.refresh_ttl_secs()),
                )
                .await
                .map_err(map_cache_err)?;
            Ok(user_id)
        }
        None => {
            if let Some(user_id) = cache
                .get::<Id>(&used_refresh_key(realm, &digest))
                .await
                .map_err(map_cache_err)?
            {
                tracing::warn!(%user_id, "refresh token reuse detected, revoking all sessions");
                revoke_tokens(cache, token_helper, user_id).await?;
            }
            Err(IdentityError::RefreshTokenInvalid.into())
        }
    }
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

        let err = consume_refresh_token(&cache, &token_helper, &tokens_a.refresh_token)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("refresh_token_invalid"));

        let refresh_err = consume_refresh_token(&cache, &token_helper, &tokens_b.refresh_token)
            .await
            .unwrap_err();
        assert!(refresh_err.to_string().contains("refresh_token_invalid"));
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
