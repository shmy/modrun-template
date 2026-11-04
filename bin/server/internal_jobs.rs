//! 系统内务任务（全部落库为 Job；apalis 表即执行统计台账：pg=`apalis.jobs`，sqlite=`Jobs`）。
//!
//! - **清理（GC）**：`CacheGc` 清缓存过期条目；`JobQueueGc` 清超保留期终态 Job 行；
//!   `BlobGc` 清 `staging/` 暂存对象（实现在 `file::staging_gc`）；
//!   `AttachmentGc` 按 `blob_objects` 回收正式对象（pending commit / tombstone）。
//! - 内务语义通过 `RETRIES = 0` 保留：失败不重试（下个周期再跑），终态 `Failed` 留痕。
//!
//! **本文件只负责「声明 job + 定时触发」**：具体领域知识（staging 目录布局、
//! 哪些业务表持有附件 path）都在各自 feature 里。新增带附件列的表时，
//! 该域实现 `file::attachment_refs::AttachmentRefs` 并在自己的 `module()` 里
//! `provide_group` 即可，**不需要改这里**。

use file::attachment;
use file::attachment_refs::{AttachmentRefs, collect_in_use};
use file::staging_gc;
use job_queue::JobHandlers;
use job_queue_kit::{DEFAULT_JOB_RETENTION_DAYS, Job};
use modrun::Group;
use rootcause::Result;
use scheduler::SchedJobs;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
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

/// 清理超过保留期的 staging 暂存对象。
///
/// 路径规则与保留语义都在 `file::staging_gc`（本域知识不下沉到组合根）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobGc {
    retention_days: u32,
}

impl Job for BlobGc {
    const NAME: &'static str = "blob_gc";
    const RETRIES: usize = 0;
}

async fn handle_blob_gc(job: BlobGc, ctx: &WebState) -> Result<()> {
    let deleted = staging_gc::purge_expired(&ctx.blob, job.retention_days).await?;
    log_gc_result(BlobGc::NAME, deleted);
    Ok(())
}

/// 回收正式对象：pending commit（refcount=0 超时）或 tombstone 过期。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentGc {}

impl Job for AttachmentGc {
    const NAME: &'static str = "attachment_gc";
    const RETRIES: usize = 0;
}

async fn handle_attachment_gc(
    _job: AttachmentGc,
    ctx: &WebState,
    refs: &[Arc<dyn AttachmentRefs>],
) -> Result<()> {
    let candidates = attachment::list_gc_candidates(&ctx.pg).await?;
    // 安全网：落库成功但 attach 失败等不一致时，业务列仍引用则跳过，避免误删成死链。
    // 各域通过 `provide_group` 贡献自己的在用集合（组合根不硬编码任何业务表）。
    let in_use = collect_in_use(refs, &ctx.pg).await?;
    for path in candidates.iter().filter(|p| in_use.contains(*p)) {
        tracing::warn!(
            path,
            "attachment gc: path still referenced by a business column; skip"
        );
    }
    let mut deleted = 0u64;
    for path in attachment::filter_gc_candidates(candidates, &in_use) {
        match attachment::purge_object(&ctx.pg, &ctx.blob, &path).await {
            Ok(()) => deleted += 1,
            Err(err) => {
                tracing::warn!(error = %err, path, "attachment gc purge failed; will retry next run");
            }
        }
    }
    log_gc_result(AttachmentGc::NAME, deleted);
    Ok(())
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
    handlers.register::<CacheGc, _>(|job, ctx| {
        Box::pin(async move { handle_cache_gc(job, &ctx).await })
    });
    handlers.register::<JobQueueGc, _>(|job, ctx| {
        Box::pin(async move { handle_job_queue_gc(job, &ctx).await })
    });
    handlers
        .register::<BlobGc, _>(|job, ctx| Box::pin(async move { handle_blob_gc(job, &ctx).await }));
    handlers
}

/// `AttachmentGc` 需要额外注入各域的引用来源，故单独用一个带 `Group` 的注册函数。
///
/// 空贡献者由组合根 `Modrun::builder().require_group::<Arc<dyn AttachmentRefs>>()`
/// 在装配期拒绝；这里再 assert 一道，防止单测 / 旁路组装绕过 require_group。
///
/// # Panics
/// `refs` 为空时（安全网失效，可能误删仍被引用的正式对象）。
pub fn attachment_job_handlers(refs: Group<Arc<dyn AttachmentRefs>>) -> JobHandlers<WebState> {
    let refs: Vec<Arc<dyn AttachmentRefs>> = refs.into_iter().collect();
    assert!(
        !refs.is_empty(),
        "AttachmentGc requires at least one AttachmentRefs contributor \
         (provide_group in the owning feature's module(); empty group disables the in-use safety net)"
    );
    let mut handlers = JobHandlers::default();
    handlers.register::<AttachmentGc, _>(move |job, ctx| {
        let refs = refs.clone();
        Box::pin(async move { handle_attachment_gc(job, &ctx, &refs).await })
    });
    handlers
}

pub fn sched_jobs() -> SchedJobs<WebState> {
    let mut jobs = SchedJobs::default();
    jobs.scheduled("every 5 minutes", CacheGc {});
    jobs.scheduled("0 0 4 * * *", JobQueueGc {});
    jobs.scheduled(
        "0 30 4 * * *",
        BlobGc {
            retention_days: staging_gc::STAGING_RETENTION_DAYS,
        },
    );
    // 紧随 BlobGc：先清 staging，再清正式对象登记表
    jobs.scheduled("0 40 4 * * *", AttachmentGc {});
    jobs
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use file::attachment_refs::AttachmentRefs;
    use futures_util::future::BoxFuture;
    use rootcause::Result;
    use sqlx::PgPool;

    struct FakeRefs(&'static [&'static str]);

    impl AttachmentRefs for FakeRefs {
        fn source(&self) -> &'static str {
            "fake.table"
        }

        fn paths_in_use<'a>(&'a self, _pool: &'a PgPool) -> BoxFuture<'a, Result<Vec<String>>> {
            let paths = self.0.iter().map(|p| (*p).to_string()).collect();
            Box::pin(async move { Ok(paths) })
        }
    }

    /// 多域贡献的在用集合必须合并去重；组合根不再硬编码任何业务表。
    #[sqlx::test]
    async fn collect_in_use_merges_contributors(pool: PgPool) {
        let refs: Vec<Arc<dyn AttachmentRefs>> = vec![
            Arc::new(FakeRefs(&["a.webp", "shared.webp"])),
            Arc::new(FakeRefs(&["shared.webp", "b.webp"])),
        ];
        let in_use = collect_in_use(&refs, &pool).await.expect("collect");
        assert_eq!(in_use.len(), 3);
        assert!(in_use.contains("a.webp"));
        assert!(in_use.contains("b.webp"));
        assert!(in_use.contains("shared.webp"));
    }

    #[test]
    #[should_panic(expected = "AttachmentGc requires at least one AttachmentRefs")]
    fn attachment_job_handlers_rejects_empty_group() {
        let _ = attachment_job_handlers(Group::default());
    }
}
