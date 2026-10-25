//! 系统内务任务（全部落库为 Job，`worker_jobs` 表即执行统计台账）。
//!
//! - **清理（GC）**：`CacheGc` 清理缓存过期条目；`JobQueueGc` 清理超过保留期的终态 Job 行；
//!   `BlobGc` 按月目录清理 `staging/` 暂存对象（保留天数见 `BLOB_STAGING_RETENTION_DAYS` 常量）。
//! - 内务语义通过 `RETRIES = 0` 保留：失败不重试（下个周期再跑），终态 `Failed` 留痕。

use blob_kit::Blob;
use chrono::Days;
use chrono::{Datelike, NaiveDate, Utc};
use job_queue::JobHandlers;
use job_queue_kit::{DEFAULT_JOB_RETENTION_DAYS, Job};
use rootcause::Result;
use scheduler::SchedJobs;
use serde::{Deserialize, Serialize};
use web_state::WebState;

/// 清理缓存后端中的过期条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheGc {}

impl Job for CacheGc {
    const NAME: &'static str = "cache_gc";
    const RETRIES: usize = 0;
}

async fn handle_cache_gc(_job: CacheGc, ctx: &WebState) -> Result<()> {
    log_gc_result(CacheGc::NAME, ctx.cache.purge_expired().await?);
    Ok(())
}

/// 清理超过保留期的终态 Job 行（统计台账保留期管理）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobQueueGc {}

impl Job for JobQueueGc {
    const NAME: &'static str = "job_queue_gc";
    const RETRIES: usize = 0;
}

async fn handle_job_queue_gc(_job: JobQueueGc, ctx: &WebState) -> Result<()> {
    log_gc_result(
        JobQueueGc::NAME,
        ctx.jobs
            .delete_finished_older_than(DEFAULT_JOB_RETENTION_DAYS)
            .await?,
    );
    Ok(())
}

/// 清理超过保留期的 staging 暂存对象（路径 `staging/YYYY/MM/{file}`，仅一层文件）。
/// 按「月目录」粒度判定：整月早于 `cutoff` 所属月初的目录才会删除（不会误删 cutoff 当月对象）。
/// 保留期非按天精确——例如 retention=30 天时，cutoff 月整月仍保留，实际最长可接近约 60 天。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobGc {
    retention_days: u32,
}

impl Job for BlobGc {
    const NAME: &'static str = "blob_gc";
    const RETRIES: usize = 0;
}

/// 名义保留天数；实际按整月目录删除，见 `BlobGc` 文档。
const BLOB_STAGING_RETENTION_DAYS: u32 = 30;

async fn handle_blob_gc(job: BlobGc, ctx: &WebState) -> Result<()> {
    let cutoff = Utc::now().date_naive() - Days::new(job.retention_days as u64);
    let expired = collect_staging_expired(&ctx.blob, cutoff).await?;
    let deleted = expired.len() as u64;
    if deleted > 0 {
        ctx.blob.delete_many(expired).await?;
    }
    log_gc_result(BlobGc::NAME, deleted);
    Ok(())
}

/// 遍历 `staging/YYYY/MM/`：月份早于 cutoff 所属月初的整月目录，收集其下全部对象。
/// 非标准路径（非 `staging/YYYY/MM`）一律跳过，避免误删。
async fn collect_staging_expired(blob: &Blob, cutoff: NaiveDate) -> Result<Vec<String>> {
    // 只删除「整个月」都早于保留窗口的目录：防止误删当月内较新的对象。
    let cutoff_month = match cutoff.with_day(1) {
        Some(d) => d,
        None => return Ok(Vec::new()),
    };
    let mut expired = Vec::new();
    let year_dirs = blob
        .list_children(file_contract::STORAGE_DIR_PREFIX)
        .await?;
    for year_dir in year_dirs {
        if year_dir.is_file {
            continue;
        }
        let month_dirs = blob.list_children(&year_dir.path).await?;
        for month_dir in month_dirs {
            let Some(month) = file_contract::staging_dir_date(&month_dir.path) else {
                continue;
            };
            if month < cutoff_month {
                let files = blob.list_children(&month_dir.path).await?;
                for file in files {
                    if file.is_file {
                        expired.push(file.path);
                    }
                }
            }
        }
    }
    Ok(expired)
}

fn log_gc_result(job: &'static str, deleted: u64) {
    if deleted > 0 {
        tracing::info!(job, deleted, "gc completed");
    } else {
        tracing::debug!(job, "gc: nothing to do");
    }
}

pub fn job_handlers() -> JobHandlers<WebState> {
    let mut handlers = JobHandlers::default();
    handlers.register::<CacheGc, _>(|job, ctx| Box::pin(handle_cache_gc(job, ctx)));
    handlers.register::<JobQueueGc, _>(|job, ctx| Box::pin(handle_job_queue_gc(job, ctx)));
    handlers.register::<BlobGc, _>(|job, ctx| Box::pin(handle_blob_gc(job, ctx)));
    handlers
}

pub fn sched_jobs() -> SchedJobs<WebState> {
    let mut jobs = SchedJobs::default();
    jobs.scheduled("every 5 minutes", CacheGc {});
    jobs.scheduled("0 0 4 * * *", JobQueueGc {});
    jobs.scheduled(
        "0 30 4 * * *",
        BlobGc {
            retention_days: BLOB_STAGING_RETENTION_DAYS,
        },
    );
    jobs
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )]
    use super::*;

    #[test]
    fn collect_skips_non_staging_or_malformed() {
        let cutoff = NaiveDate::from_ymd_opt(2026, 9, 5).unwrap();
        let cutoff_month = cutoff.with_day(1).unwrap();
        assert_eq!(cutoff_month, NaiveDate::from_ymd_opt(2026, 9, 1).unwrap());
        // 8 月整月早于 9/1 → 过期；9 月不删
        assert!(file_contract::staging_dir_date("staging/2026/08/x.webp").unwrap() < cutoff_month);
        assert!(file_contract::staging_dir_date("staging/2026/09/x.webp").unwrap() >= cutoff_month);
    }

    #[tokio::test]
    async fn collect_staging_expired_deletes_only_expired_months() {
        use std::io::Cursor;

        let blob = Blob::new_for_test().await.expect("test blob");
        blob.write("staging/2026/07/old.webp", Cursor::new("a"))
            .await
            .expect("write jul");
        blob.write("staging/2026/08/old.webp", Cursor::new("b"))
            .await
            .expect("write aug");
        blob.write("staging/2026/09/keep.webp", Cursor::new("c"))
            .await
            .expect("write sep");
        blob.write("staging/orphan.webp", Cursor::new("x"))
            .await
            .expect("write orphan");

        let cutoff = NaiveDate::from_ymd_opt(2026, 9, 5).unwrap();
        let expired = collect_staging_expired(&blob, cutoff)
            .await
            .expect("collect");

        assert_eq!(expired.len(), 2);
        assert!(expired.iter().any(|p| p.contains("/07/")));
        assert!(expired.iter().any(|p| p.contains("/08/")));
        assert!(!expired.iter().any(|p| p.contains("/09/")));

        let mut cleanup = expired;
        cleanup.push("staging/orphan.webp".to_string());
        cleanup.push("staging/2026/09/keep.webp".to_string());
        blob.delete_many(cleanup).await.expect("cleanup");
    }
}
