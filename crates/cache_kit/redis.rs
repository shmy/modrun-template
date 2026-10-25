//! Redis 后端（bb8 连接池）。
use std::time::Duration;

use bb8_redis::RedisConnectionManager;
use bb8_redis::bb8::Pool;
use chrono::Utc;
use redis::AsyncCommands;
use redis::Script;
use rootcause::Result;
use uuid::Uuid;

#[derive(Clone)]
pub struct RedisCache {
    pool: Pool<RedisConnectionManager>,
}

impl RedisCache {
    pub(crate) async fn try_new(pool: Pool<RedisConnectionManager>) -> Result<Self> {
        Ok(Self { pool })
    }

    pub async fn get_raw(&self, key: &str) -> Result<Option<String>> {
        let mut conn = self.pool.get().await?;
        Ok(conn.get(key).await?)
    }

    pub async fn set_ex_raw(&self, key: &str, value: &str, period: Duration) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let _: () = conn.set_ex(key, value, period.as_secs()).await?;
        Ok(())
    }

    pub async fn take_raw(&self, key: &str) -> Result<Option<String>> {
        let mut conn = self.pool.get().await?;
        let value: Option<String> = redis::cmd("GETDEL")
            .arg(key)
            .query_async(&mut *conn)
            .await?;
        Ok(value)
    }

    pub async fn del_raw(&self, key: &str) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let _: () = conn.del(key).await?;
        Ok(())
    }

    pub async fn allow_sliding_window(
        &self,
        key: &str,
        window: Duration,
        max_hits: usize,
    ) -> Result<bool> {
        let mut conn = self.pool.get().await?;
        let now_ms = Utc::now().timestamp_millis();
        let window_ms = window.as_millis() as i64;
        let member = format!("{now_ms}:{}", Uuid::new_v4());
        let allowed: i32 = Script::new(
            r"
            local key = KEYS[1]
            local now = tonumber(ARGV[1])
            local window = tonumber(ARGV[2])
            local max_hits = tonumber(ARGV[3])
            local member = ARGV[4]
            redis.call('ZREMRANGEBYSCORE', key, 0, now - window)
            local count = redis.call('ZCARD', key)
            if count >= max_hits then
                return 0
            end
            redis.call('ZADD', key, now, member)
            redis.call('PEXPIRE', key, window)
            return 1
            ",
        )
        .key(key)
        .arg(now_ms)
        .arg(window_ms)
        .arg(max_hits as i64)
        .arg(member)
        .invoke_async(&mut *conn)
        .await?;
        Ok(allowed == 1)
    }

    pub async fn purge_expired(&self) -> Result<u64> {
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use redis::Redis as RedisRedis;
    use testcontainers_modules::{
        redis::{self, REDIS_PORT, Redis},
        testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner as _},
    };

    async fn setup_redis_pool() -> (ContainerAsync<Redis>, Pool<RedisConnectionManager>) {
        let redis_instance = RedisRedis::default()
            .with_tag("7-alpine")
            .start()
            .await
            .unwrap();
        let host_ip = redis_instance.get_host().await.unwrap();
        let host_port = redis_instance.get_host_port_ipv4(REDIS_PORT).await.unwrap();
        let url = format!("redis://{host_ip}:{host_port}");
        let manager = RedisConnectionManager::new(url).unwrap();
        let pool = Pool::builder().max_size(16).build(manager).await.unwrap();
        (redis_instance, pool)
    }

    #[tokio::test]
    async fn set_get_take_del() {
        let (_guard, pool) = setup_redis_pool().await;

        let cache = RedisCache::try_new(pool).await.unwrap();
        let key = "cache_kit_redis_test";

        cache.del_raw(key).await.unwrap();
        assert!(cache.get_raw(key).await.unwrap().is_none());

        cache
            .set_ex_raw(key, "\"v1\"", Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(cache.get_raw(key).await.unwrap().as_deref(), Some("\"v1\""));

        assert_eq!(
            cache.take_raw(key).await.unwrap().as_deref(),
            Some("\"v1\"")
        );
        assert!(cache.take_raw(key).await.unwrap().is_none());

        cache.del_raw(key).await.unwrap();
    }

    #[tokio::test]
    async fn allow_sliding_window_limits_hits() {
        let (_guard, pool) = setup_redis_pool().await;
        let cache = RedisCache::try_new(pool).await.unwrap();
        let key = "rate";
        let window = Duration::from_secs(60);

        assert!(cache.allow_sliding_window(key, window, 2).await.unwrap());
        assert!(cache.allow_sliding_window(key, window, 2).await.unwrap());
        assert!(!cache.allow_sliding_window(key, window, 2).await.unwrap());
    }
}
