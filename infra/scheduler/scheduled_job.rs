//! cron 到点触发 → 入队一个 [`Job`]；执行、重试与终态由 job worker 承担。

use job_queue_kit::Job;
use sched_kit::{CronJob, CronJobFuture};

use crate::HasJobBus;

/// cron 表达式 → 周期入队的桥接任务。
#[derive(Clone, Debug)]
pub struct ScheduledJob<T: Job + Clone> {
    expr: &'static str,
    job: T,
}

impl<T: Job + Clone> ScheduledJob<T> {
    /// `expr` 为 tokio-cron-scheduler 表达式（6 字段或英文缩写，如 `"every 5 minutes"`）。
    pub fn new(expr: &'static str, job: T) -> Self {
        Self { expr, job }
    }
}

impl<T: Job + Clone, C: Clone + Send + Sync + 'static + HasJobBus> CronJob<C> for ScheduledJob<T> {
    fn name(&self) -> &'static str {
        T::NAME
    }

    fn expr(&self) -> &'static str {
        self.expr
    }

    fn run(&self, context: C) -> CronJobFuture {
        let job = self.job.clone();
        let jobs = context.job_bus().clone();
        Box::pin(async move {
            jobs.enqueue(job).await?;
            Ok(())
        })
    }
}
