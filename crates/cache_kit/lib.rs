//! 统一缓存后端：`Cache` 枚举 + 方法门面。
//!
//! 编译期按 feature 装配（pg 可与 redb/redis 并存，唯 redb+redis 互斥；`infra/cache` 组装处选择变体）：
//! - `pg`（**默认**）：PostgreSQL `_pg_caches`
//! - `redb`：嵌入式 redb
//! - `redis`：bb8 + Redis

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )
)]

#[cfg(feature = "pg")]
mod pg;
#[cfg(feature = "redb")]
mod redb;
#[cfg(feature = "redis")]
mod redis;

#[cfg(feature = "redis")]
pub use bb8_redis::RedisConnectionManager;
#[cfg(feature = "redis")]
pub use bb8_redis::bb8::Pool;

use std::time::Duration;

use rootcause::Result;
use serde::{Serialize, de::DeserializeOwned};

#[cfg(feature = "pg")]
pub use pg::PgCache;
#[cfg(feature = "redb")]
pub use redb::RedbCache;
#[cfg(feature = "redis")]
pub use redis::RedisCache;

#[cfg(not(any(feature = "pg", feature = "redb", feature = "redis")))]
compile_error!("cache_kit requires feature \"pg\", \"redb\" or \"redis\"");

#[derive(Clone)]
pub enum Cache {
    #[cfg(feature = "pg")]
    Pg(PgCache),
    #[cfg(feature = "redb")]
    Redb(RedbCache),
    #[cfg(feature = "redis")]
    Redis(RedisCache),
}

impl Cache {
    #[cfg(feature = "pg")]
    pub async fn try_new_pg(pool: sqlx::PgPool) -> Result<Self> {
        Ok(Self::Pg(PgCache::try_new(pool).await?))
    }

    #[cfg(feature = "redb")]
    pub fn try_new_redb(path: impl AsRef<std::path::Path>) -> Result<Self> {
        Ok(Self::Redb(RedbCache::try_new(path)?))
    }

    #[cfg(feature = "redis")]
    pub async fn try_new_redis(pool: Pool<RedisConnectionManager>) -> Result<Self> {
        Ok(Self::Redis(RedisCache::try_new(pool).await?))
    }

    pub async fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let raw = match self {
            #[cfg(feature = "pg")]
            Self::Pg(inner) => inner.get_raw(key).await?,
            #[cfg(feature = "redb")]
            Self::Redb(inner) => inner.get_raw(key).await?,
            #[cfg(feature = "redis")]
            Self::Redis(inner) => inner.get_raw(key).await?,
        };
        Ok(raw.map(|r| serde_json::from_str(&r)).transpose()?)
    }

    pub async fn set_ex<T: Serialize + Send + Sync>(
        &self,
        key: &str,
        value: &T,
        period: Duration,
    ) -> Result<()> {
        let raw = serde_json::to_string(value)?;
        match self {
            #[cfg(feature = "pg")]
            Self::Pg(inner) => inner.set_ex_raw(key, &raw, period).await,
            #[cfg(feature = "redb")]
            Self::Redb(inner) => inner.set_ex_raw(key, &raw, period).await,
            #[cfg(feature = "redis")]
            Self::Redis(inner) => inner.set_ex_raw(key, &raw, period).await,
        }
    }

    pub async fn take<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let raw = match self {
            #[cfg(feature = "pg")]
            Self::Pg(inner) => inner.take_raw(key).await?,
            #[cfg(feature = "redb")]
            Self::Redb(inner) => inner.take_raw(key).await?,
            #[cfg(feature = "redis")]
            Self::Redis(inner) => inner.take_raw(key).await?,
        };
        Ok(raw.map(|r| serde_json::from_str(&r)).transpose()?)
    }

    pub async fn del(&self, key: &str) -> Result<()> {
        match self {
            #[cfg(feature = "pg")]
            Self::Pg(inner) => inner.del_raw(key).await,
            #[cfg(feature = "redb")]
            Self::Redb(inner) => inner.del_raw(key).await,
            #[cfg(feature = "redis")]
            Self::Redis(inner) => inner.del_raw(key).await,
        }
    }

    pub async fn allow_sliding_window(
        &self,
        key: &str,
        window: Duration,
        max_hits: usize,
    ) -> Result<bool> {
        match self {
            #[cfg(feature = "pg")]
            Self::Pg(inner) => inner.allow_sliding_window(key, window, max_hits).await,
            #[cfg(feature = "redb")]
            Self::Redb(inner) => inner.allow_sliding_window(key, window, max_hits).await,
            #[cfg(feature = "redis")]
            Self::Redis(inner) => inner.allow_sliding_window(key, window, max_hits).await,
        }
    }

    pub async fn purge_expired(&self) -> Result<u64> {
        match self {
            #[cfg(feature = "pg")]
            Self::Pg(inner) => inner.purge_expired().await,
            #[cfg(feature = "redb")]
            Self::Redb(inner) => inner.purge_expired().await,
            #[cfg(feature = "redis")]
            Self::Redis(inner) => inner.purge_expired().await,
        }
    }
}

#[cfg(all(test, feature = "redb"))]
mod facade_tests {
    use super::*;
    use tokio::time::sleep;

    fn test_backend() -> Cache {
        let dir = Box::leak(Box::new(tempfile::tempdir().expect("create temp dir")));
        Cache::Redb(RedbCache::try_new(dir.path().join("cache.redb")).expect("open redb cache"))
    }

    #[tokio::test]
    async fn generic_facade_roundtrip() {
        let cache = test_backend();
        let key = "facade";

        assert!(cache.get::<String>(key).await.unwrap().is_none());

        cache
            .set_ex(key, &"hello", Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(
            cache.get::<String>(key).await.unwrap().as_deref(),
            Some("hello")
        );

        assert_eq!(
            cache.take::<String>(key).await.unwrap().as_deref(),
            Some("hello")
        );
        assert!(cache.get::<String>(key).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn delete_expired_via_facade() {
        let cache = test_backend();
        cache.set_ex("x", &1_i64, Duration::ZERO).await.unwrap();
        cache
            .set_ex("y", &2_i64, Duration::from_secs(60))
            .await
            .unwrap();
        sleep(Duration::from_millis(5)).await;

        assert_eq!(cache.purge_expired().await.unwrap(), 1);
        assert!(cache.get::<i64>("x").await.unwrap().is_none());
        assert_eq!(cache.get::<i64>("y").await.unwrap(), Some(2));
    }
}
