use std::time::Duration;

use chrono::{DateTime, Utc};
use rootcause::Result;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};

#[derive(Clone)]
pub struct PgCache {
    pool: PgPool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SlidingWindowHits {
    hits: Vec<i64>,
}

impl PgCache {
    pub(crate) async fn try_new(pool: PgPool) -> Result<Self> {
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(Self { pool })
    }

    pub async fn get_raw(&self, key: &str) -> Result<Option<String>> {
        let mut conn = self.pool.acquire().await?;
        let row = sqlx::query("SELECT value FROM _pg_caches WHERE key = $1 AND expires_at > now()")
            .bind(key)
            .fetch_optional(&mut *conn)
            .await?;
        Ok(row.map(|row| row.get("value")))
    }

    pub async fn set_ex_raw(&self, key: &str, value: &str, period: Duration) -> Result<()> {
        let mut conn = self.pool.acquire().await?;
        let expires: DateTime<Utc> = Utc::now() + period;
        sqlx::query(
            r#"
            INSERT INTO _pg_caches (key, value, expires_at)
            VALUES ($1, $2, $3)
            ON CONFLICT (key) DO UPDATE
            SET value = EXCLUDED.value,
                expires_at = EXCLUDED.expires_at
            "#,
        )
        .bind(key)
        .bind(value)
        .bind(expires)
        .execute(&mut *conn)
        .await?;
        Ok(())
    }

    pub async fn take_raw(&self, key: &str) -> Result<Option<String>> {
        let mut conn = self.pool.acquire().await?;
        let row = sqlx::query(
            r#"
            DELETE FROM _pg_caches
            WHERE key = $1 AND expires_at > now()
            RETURNING value
            "#,
        )
        .bind(key)
        .fetch_optional(&mut *conn)
        .await?;
        Ok(row.map(|row| row.get("value")))
    }

    pub async fn del_raw(&self, key: &str) -> Result<()> {
        let mut conn = self.pool.acquire().await?;
        sqlx::query("DELETE FROM _pg_caches WHERE key = $1")
            .bind(key)
            .execute(&mut *conn)
            .await?;
        Ok(())
    }

    /// 滑动窗口限流：在 PG 事务 + advisory lock 下原子判定并记录一次命中。
    pub async fn allow_sliding_window(
        &self,
        key: &str,
        window: Duration,
        max_hits: usize,
    ) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1)::bigint)")
            .bind(key)
            .execute(&mut *tx)
            .await?;

        let now = Utc::now().timestamp();
        let window_start = now - window.as_secs() as i64;
        let expires_at: DateTime<Utc> = Utc::now() + window;

        let row = sqlx::query("SELECT value FROM _pg_caches WHERE key = $1 AND expires_at > now()")
            .bind(key)
            .fetch_optional(&mut *tx)
            .await?;

        let mut bucket = row
            .map(|row| {
                let value: String = row.get("value");
                serde_json::from_str::<SlidingWindowHits>(&value)
            })
            .transpose()?
            .unwrap_or_default();

        bucket.hits.retain(|at| *at > window_start);
        if bucket.hits.len() >= max_hits {
            tx.commit().await?;
            return Ok(false);
        }

        bucket.hits.push(now);
        let value = serde_json::to_string(&bucket)?;
        sqlx::query(
            r#"
            INSERT INTO _pg_caches (key, value, expires_at)
            VALUES ($1, $2, $3)
            ON CONFLICT (key) DO UPDATE
            SET value = EXCLUDED.value,
                expires_at = EXCLUDED.expires_at
            "#,
        )
        .bind(key)
        .bind(value)
        .bind(expires_at)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    /// 清理全部已过期缓存行，返回删除行数。
    /// 过期行只会在被覆写 / `take` 时顺带删除，必须周期调用防表膨胀。
    pub async fn purge_expired(&self) -> Result<u64> {
        let mut conn = self.pool.acquire().await?;
        let result = sqlx::query("DELETE FROM _pg_caches WHERE expires_at <= now()")
            .execute(&mut *conn)
            .await?;
        Ok(result.rows_affected())
    }
}
