//! 队列后端方言实现：pg（生产，worker_jobs 表在业务库）/ sqlite（单机部署，本地文件）。
//!
//! 两个后端语义一致：
//! - 入队：独立连接 INSERT（`run_at` 未来时间 = 延迟投递）；
//! - 拉取：竞争安全（pg 用 `FOR UPDATE SKIP LOCKED`；sqlite 用单连接写事务 + 条件 UPDATE），
//!   只取 `status='Pending' AND run_at <= now()`；
//! - 失败：`attempts` 自增，未达上限则回置 `Pending` 并后移 `run_at`（退避）；
//!   耗尽则终态 `Failed`（`last_error` 留原因）；
//! - 孤儿恢复：`Running` 且 `lock_at` 超龄 → 回置 `Pending`（`orphan_abandoned`），
//!   覆盖"进程崩溃在拉取之后、落终态之前"的窗口（at-least-once 语义）。

use rootcause::Result;
use serde_json::Value;
use std::time::Duration;

/// 终态行清理的每批删除上限：分批循环删除，避免单事务大 DELETE 长时间锁表 / WAL 膨胀
/// （见各后端 `delete_finished_older_than`）。
pub(crate) const GC_BATCH: i64 = 1000;

/// 一次拉取到的待执行任务（与方言无关的归一化视图）。
pub(crate) struct FetchJob {
    pub payload: Value,
    pub meta: JobMeta,
}

/// 任务元数据：状态转移所需的最小字段集（id / 重试计数 / 执行上限）。
#[derive(Clone, Copy, Debug)]
pub(crate) struct JobMeta {
    pub id: i64,
    pub attempts: i32,
    pub max_attempts: i32,
}

#[cfg(feature = "pg")]
pub(crate) mod pg {
    use super::*;
    use chrono::Duration as ChronoDuration;
    use chrono::{DateTime, Utc};
    use sqlx::{PgPool, Row};

    pub(crate) async fn insert(
        pool: &PgPool,
        job_type: &str,
        payload: Value,
        run_at: DateTime<Utc>,
        max_attempts: i32,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO worker_jobs (job_type, payload, run_at, max_attempts)
             VALUES ($1, $2, $3, $4)",
        )
        .bind(job_type)
        .bind(payload)
        .bind(run_at)
        .bind(max_attempts)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// 竞争安全拉取：`FOR UPDATE SKIP LOCKED` 保证同一行只被一个消费者选中。
    pub(crate) async fn fetch_due(
        pool: &PgPool,
        job_type: &str,
        limit: i32,
        lock_by: &str,
    ) -> Result<Vec<FetchJob>> {
        let rows = sqlx::query(
            "UPDATE worker_jobs
                SET status = 'Running', lock_by = $1, lock_at = now()
             WHERE id IN (
                SELECT id FROM worker_jobs
                 WHERE job_type = $2 AND status = 'Pending' AND run_at <= now()
                 ORDER BY run_at, id
                 LIMIT $3
                 FOR UPDATE SKIP LOCKED
             )
             RETURNING id, payload, attempts, max_attempts",
        )
        .bind(lock_by)
        .bind(job_type)
        .bind(limit)
        .fetch_all(pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| FetchJob {
                payload: row.get("payload"),
                meta: JobMeta {
                    id: row.get("id"),
                    attempts: row.get("attempts"),
                    max_attempts: row.get("max_attempts"),
                },
            })
            .collect())
    }

    pub(crate) async fn mark_done(pool: &PgPool, id: i64) -> Result<()> {
        sqlx::query("UPDATE worker_jobs SET status = 'Done', done_at = now() WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await?;
        Ok(())
    }

    /// 失败未达上限：回置 Pending，`run_at` 后移实现退避。
    pub(crate) async fn schedule_retry(
        pool: &PgPool,
        id: i64,
        attempts: i32,
        delay: Duration,
        last_error: &str,
    ) -> Result<()> {
        let run_at = Utc::now() + ChronoDuration::milliseconds(delay.as_millis() as i64);
        sqlx::query(
            "UPDATE worker_jobs
                SET status = 'Pending', attempts = $1, run_at = $2, last_error = $3,
                    lock_by = NULL, lock_at = NULL
              WHERE id = $4",
        )
        .bind(attempts)
        .bind(run_at)
        .bind(last_error)
        .bind(id)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// 失败达上限：终态 Failed，`last_error` 留原因（DLQ-in-table）。
    pub(crate) async fn mark_failed(
        pool: &PgPool,
        id: i64,
        attempts: i32,
        last_error: &str,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE worker_jobs
                SET status = 'Failed', attempts = $1, done_at = now(), last_error = $2,
                    lock_by = NULL, lock_at = NULL
              WHERE id = $3",
        )
        .bind(attempts)
        .bind(last_error)
        .bind(id)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// 孤儿恢复：`Running` 且 `lock_at` 早于阈值 → 回置 Pending。
    pub(crate) async fn reenqueue_orphaned(
        pool: &PgPool,
        older_than: DateTime<Utc>,
    ) -> Result<i64> {
        let result = sqlx::query(
            "UPDATE worker_jobs
                SET status = 'Pending', lock_by = NULL, lock_at = NULL, last_error = 'orphan_abandoned'
              WHERE status = 'Running' AND lock_at < $1",
        )
        .bind(older_than)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() as i64)
    }

    /// 清理超过保留期的终态行（Done / Failed），供统计保留期管理（`JobQueueGc`）。
    ///
    /// 分批循环删除（每批 [`GC_BATCH`] 行）：行数大时避免单事务大 DELETE
    /// 长时间锁表 / WAL 膨胀，也便于 gc_idx 逐批走索引。
    pub(crate) async fn delete_finished_older_than(
        pool: &PgPool,
        retention_days: i64,
    ) -> Result<u64> {
        let mut deleted = 0u64;
        loop {
            let result = sqlx::query(
                "DELETE FROM worker_jobs
                  WHERE id IN (
                      SELECT id FROM worker_jobs
                       WHERE status IN ('Done', 'Failed')
                         AND done_at < now() - make_interval(days => $1::int)
                       LIMIT $2
                  )",
            )
            .bind(retention_days)
            .bind(GC_BATCH)
            .execute(pool)
            .await?;
            let batch = result.rows_affected() as u64;
            deleted += batch;
            if batch < GC_BATCH as u64 {
                break;
            }
        }
        Ok(deleted)
    }
}

#[cfg(feature = "sqlite")]
pub(crate) mod sqlite {
    use super::*;
    use chrono::Utc;
    use rootcause::report;
    use sqlx::{Row, SqliteConnection, SqlitePool};

    fn now_millis() -> i64 {
        Utc::now().timestamp_millis()
    }

    pub(crate) async fn insert(
        pool: &SqlitePool,
        job_type: &str,
        payload: &str,
        run_at: i64,
        max_attempts: i32,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO worker_jobs (job_type, payload, run_at, max_attempts)
             VALUES (?, ?, ?, ?)",
        )
        .bind(job_type)
        .bind(payload)
        .bind(run_at)
        .bind(max_attempts)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// 竞争安全拉取：单连接写事务（`BEGIN IMMEDIATE` 拿写锁，sqlite 无 SKIP LOCKED），
    /// 条件 UPDATE 兜底（第二个消费者即使读到同一行也会影响 0 行）。
    /// 仅支持单进程消费（sqlite 文件锁语义）。
    pub(crate) async fn fetch_due(
        pool: &SqlitePool,
        job_type: &str,
        limit: i32,
        lock_by: &str,
    ) -> Result<Vec<FetchJob>> {
        let mut conn = pool.acquire().await?;
        let now = now_millis();
        // 事务内只做 SELECT + 条件 UPDATE（标记 Running）；成功显式 COMMIT、失败显式
        // ROLLBACK——不依赖连接池对未提交事务的隐式回滚。JSON 解析放到事务外。
        let fetched = match lock_and_mark(&mut conn, job_type, now, limit, lock_by).await {
            Ok(rows) => {
                sqlx::query("COMMIT").execute(&mut *conn).await?;
                rows
            }
            Err(e) => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                return Err(e);
            }
        };
        fetched
            .into_iter()
            .map(|(id, payload, attempts, max_attempts)| {
                Ok(FetchJob {
                    payload: serde_json::from_str(payload.as_str()).map_err(|e| report!("{e}"))?,
                    meta: JobMeta {
                        id,
                        attempts,
                        max_attempts,
                    },
                })
            })
            .collect()
    }

    /// `BEGIN IMMEDIATE` 内的 SELECT + 条件 UPDATE；调用方负责配对 COMMIT / ROLLBACK。
    async fn lock_and_mark(
        conn: &mut SqliteConnection,
        job_type: &str,
        now: i64,
        limit: i32,
        lock_by: &str,
    ) -> Result<Vec<(i64, String, i32, i32)>> {
        sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;
        let rows = sqlx::query(
            "SELECT id, payload, attempts, max_attempts FROM worker_jobs
              WHERE job_type = ? AND status = 'Pending' AND run_at <= ?
              ORDER BY id LIMIT ?",
        )
        .bind(job_type)
        .bind(now)
        .bind(limit)
        .fetch_all(&mut *conn)
        .await?;
        let mut jobs = Vec::with_capacity(rows.len());
        for row in rows {
            let id: i64 = row.get("id");
            sqlx::query(
                "UPDATE worker_jobs SET status = 'Running', lock_by = ?, lock_at = ?
                  WHERE id = ? AND status = 'Pending'",
            )
            .bind(lock_by)
            .bind(now)
            .bind(id)
            .execute(&mut *conn)
            .await?;
            jobs.push((
                id,
                row.get("payload"),
                row.get("attempts"),
                row.get("max_attempts"),
            ));
        }
        Ok(jobs)
    }

    pub(crate) async fn mark_done(pool: &SqlitePool, id: i64) -> Result<()> {
        sqlx::query("UPDATE worker_jobs SET status = 'Done', done_at = ? WHERE id = ?")
            .bind(now_millis())
            .bind(id)
            .execute(pool)
            .await?;
        Ok(())
    }

    pub(crate) async fn schedule_retry(
        pool: &SqlitePool,
        id: i64,
        attempts: i32,
        delay: Duration,
        last_error: &str,
    ) -> Result<()> {
        let run_at = now_millis()
            + i64::try_from(delay.as_millis()).map_err(|_| report!("backoff_overflow"))?;
        sqlx::query(
            "UPDATE worker_jobs
                SET status = 'Pending', attempts = ?, run_at = ?, last_error = ?,
                    lock_by = NULL, lock_at = NULL
              WHERE id = ?",
        )
        .bind(attempts)
        .bind(run_at)
        .bind(last_error)
        .bind(id)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub(crate) async fn mark_failed(
        pool: &SqlitePool,
        id: i64,
        attempts: i32,
        last_error: &str,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE worker_jobs
                SET status = 'Failed', attempts = ?, done_at = ?, last_error = ?,
                    lock_by = NULL, lock_at = NULL
              WHERE id = ?",
        )
        .bind(attempts)
        .bind(now_millis())
        .bind(last_error)
        .bind(id)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub(crate) async fn reenqueue_orphaned(pool: &SqlitePool, older_than: i64) -> Result<i64> {
        let result = sqlx::query(
            "UPDATE worker_jobs
                SET status = 'Pending', lock_by = NULL, lock_at = NULL, last_error = 'orphan_abandoned'
              WHERE status = 'Running' AND lock_at < ?",
        )
        .bind(older_than)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() as i64)
    }

    /// 清理超过保留期的终态行（Done / Failed），供统计保留期管理（`JobQueueGc`）。
    ///
    /// 分批循环删除（每批 [`GC_BATCH`] 行）：与 pg 同语义，避免单事务大 DELETE
    /// 长时间持有写锁（sqlite 单写者模型下阻塞其他写入）。
    pub(crate) async fn delete_finished_older_than(
        pool: &SqlitePool,
        retention_days: i64,
    ) -> Result<u64> {
        let cutoff = now_millis() - retention_days * 86_400_000;
        let mut deleted = 0u64;
        loop {
            let result = sqlx::query(
                "DELETE FROM worker_jobs
                  WHERE id IN (
                      SELECT id FROM worker_jobs
                       WHERE status IN ('Done', 'Failed')
                         AND done_at < ?
                       LIMIT ?
                  )",
            )
            .bind(cutoff)
            .bind(GC_BATCH)
            .execute(pool)
            .await?;
            let batch = result.rows_affected() as u64;
            deleted += batch;
            if batch < GC_BATCH as u64 {
                break;
            }
        }
        Ok(deleted)
    }
}
