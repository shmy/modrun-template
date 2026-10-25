//! redb 嵌入式 KV 后端。
//!
//! 所有写事务使用 `Durability::None`（不 fsync）：缓存是低价值、可重建数据且带 TTL，
//! 崩溃最多丢最近提交，换取明显更高的写吞吐；过期清理由 `purge_expired` 周期执行。
use std::ops::Add;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use redb::{Database, Durability, ReadableDatabase, ReadableTable, TableDefinition};
use rootcause::Result;
use serde::{Deserialize, Serialize};

const TABLE: TableDefinition<&str, &str> = TableDefinition::new("caches");

#[derive(Debug, Serialize, Deserialize)]
struct Entry {
    value: String,
    expires_at: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SlidingWindowHits {
    hits: Vec<i64>,
}

#[derive(Clone)]
pub struct RedbCache {
    db: Arc<Database>,
}

impl RedbCache {
    pub(crate) fn try_new(path: impl AsRef<Path>) -> Result<Self> {
        let db = Arc::new(Database::create(path.as_ref())?);
        let mut write_txn = db.begin_write()?;
        write_txn.set_durability(Durability::None)?;
        write_txn.open_table(TABLE)?;
        write_txn.commit()?;
        Ok(Self { db })
    }

    fn now_ms() -> i64 {
        Utc::now().timestamp_millis()
    }

    pub async fn get_raw(&self, key: &str) -> Result<Option<String>> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(TABLE)?;
        let Some(guard) = table.get(key)? else {
            return Ok(None);
        };
        let entry: Entry = serde_json::from_str(guard.value())?;
        if entry.expires_at < Self::now_ms() {
            return Ok(None);
        }
        Ok(Some(entry.value))
    }

    pub async fn set_ex_raw(&self, key: &str, value: &str, period: Duration) -> Result<()> {
        let expires_at = Utc::now().add(period).timestamp_millis();
        let encoded = serde_json::to_string(&Entry {
            value: value.to_owned(),
            expires_at,
        })?;
        let mut write_txn = self.db.begin_write()?;
        write_txn.set_durability(Durability::None)?;
        {
            let mut table = write_txn.open_table(TABLE)?;
            table.insert(key, encoded.as_str())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub async fn take_raw(&self, key: &str) -> Result<Option<String>> {
        let mut write_txn = self.db.begin_write()?;
        write_txn.set_durability(Durability::None)?;
        let mut taken: Option<String> = None;
        {
            let mut table = write_txn.open_table(TABLE)?;
            if let Some(guard) = table.get(key)? {
                let entry: Entry = serde_json::from_str(guard.value())?;
                if entry.expires_at >= Self::now_ms() {
                    taken = Some(entry.value);
                }
            }
            table.remove(key)?;
        }
        write_txn.commit()?;
        Ok(taken)
    }

    pub async fn del_raw(&self, key: &str) -> Result<()> {
        let mut write_txn = self.db.begin_write()?;
        write_txn.set_durability(Durability::None)?;
        {
            let mut table = write_txn.open_table(TABLE)?;
            table.remove(key)?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub async fn allow_sliding_window(
        &self,
        key: &str,
        window: Duration,
        max_hits: usize,
    ) -> Result<bool> {
        let now = Utc::now().timestamp();
        let window_start = now - window.as_secs() as i64;
        let expires_at = Utc::now().add(window).timestamp_millis();

        let mut write_txn = self.db.begin_write()?;
        write_txn.set_durability(Durability::None)?;
        let allowed = {
            let mut table = write_txn.open_table(TABLE)?;
            let mut bucket = if let Some(guard) = table.get(key)? {
                let entry: Entry = serde_json::from_str(guard.value())?;
                if entry.expires_at >= Self::now_ms() {
                    serde_json::from_str::<SlidingWindowHits>(&entry.value).unwrap_or_default()
                } else {
                    SlidingWindowHits::default()
                }
            } else {
                SlidingWindowHits::default()
            };

            bucket.hits.retain(|at| *at > window_start);
            if bucket.hits.len() >= max_hits {
                false
            } else {
                bucket.hits.push(now);
                let value = serde_json::to_string(&bucket)?;
                let encoded = serde_json::to_string(&Entry { value, expires_at })?;
                table.insert(key, encoded.as_str())?;
                true
            }
        };
        write_txn.commit()?;
        Ok(allowed)
    }

    pub async fn purge_expired(&self) -> Result<u64> {
        let now = Self::now_ms();
        let mut write_txn = self.db.begin_write()?;
        write_txn.set_durability(Durability::None)?;
        let mut expired = Vec::new();
        {
            let table = write_txn.open_table(TABLE)?;
            for item in table.iter()? {
                let (k, v) = item?;
                if let Ok(entry) = serde_json::from_str::<Entry>(v.value())
                    && entry.expires_at < now
                {
                    expired.push(k.value().to_owned());
                }
            }
        }
        let count = expired.len() as u64;
        {
            let mut table = write_txn.open_table(TABLE)?;
            for key in &expired {
                table.remove(key.as_str())?;
            }
        }
        write_txn.commit()?;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::sleep;

    /// 独立临时库：测试进程生命周期内保持目录（drop 过早会删除数据库文件）。
    fn test_cache() -> RedbCache {
        let dir = Box::leak(Box::new(tempfile::tempdir().expect("create temp dir")));
        RedbCache::try_new(dir.path().join("cache.redb")).expect("open redb cache")
    }

    #[tokio::test]
    async fn set_get_take_del() {
        let cache = test_cache();

        assert!(cache.get_raw("k").await.unwrap().is_none());

        cache
            .set_ex_raw("k", "\"v1\"", Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(cache.get_raw("k").await.unwrap().as_deref(), Some("\"v1\""));

        assert_eq!(
            cache.take_raw("k").await.unwrap().as_deref(),
            Some("\"v1\"")
        );
        assert!(cache.take_raw("k").await.unwrap().is_none());
        assert!(cache.get_raw("k").await.unwrap().is_none());

        cache
            .set_ex_raw("k", "\"v2\"", Duration::from_secs(60))
            .await
            .unwrap();
        cache.del_raw("k").await.unwrap();
        assert!(cache.get_raw("k").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn expired_is_invisible_and_cleaned() {
        let cache = test_cache();

        cache
            .set_ex_raw("k1", "\"x\"", Duration::ZERO)
            .await
            .unwrap();
        cache
            .set_ex_raw("k2", "\"y\"", Duration::from_secs(60))
            .await
            .unwrap();
        sleep(Duration::from_millis(5)).await;

        assert!(cache.get_raw("k1").await.unwrap().is_none());
        assert_eq!(cache.get_raw("k2").await.unwrap().as_deref(), Some("\"y\""));

        let n = cache.purge_expired().await.unwrap();
        assert_eq!(n, 1);
        assert_eq!(cache.purge_expired().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn allow_sliding_window_limits_hits() {
        let cache = test_cache();
        let key = "rate";
        let window = Duration::from_secs(60);

        assert!(cache.allow_sliding_window(key, window, 2).await.unwrap());
        assert!(cache.allow_sliding_window(key, window, 2).await.unwrap());
        assert!(!cache.allow_sliding_window(key, window, 2).await.unwrap());
    }
}
