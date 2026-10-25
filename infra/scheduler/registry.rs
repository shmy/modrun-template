use job_queue_kit::Job;
use sched_kit::CronJobRegistry;

use crate::{HasJobBus, ScheduledJob};

/// 各 infra / feature / 组合根通过 `provide_group` 贡献的定时任务集合。
pub struct SchedJobs<C: Clone + Send + Sync + 'static>(CronJobRegistry<C>);

impl<C: Clone + Send + Sync + 'static> Default for SchedJobs<C> {
    fn default() -> Self {
        Self(CronJobRegistry::default())
    }
}

impl<C: Clone + Send + Sync + 'static> Clone for SchedJobs<C> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<C: Clone + Send + Sync + 'static> SchedJobs<C> {
    /// 注册周期任务：cron 到点 → 入队一个 [`Job`]。
    pub fn scheduled<T>(&mut self, expr: &'static str, job: T) -> &mut Self
    where
        T: Job + Clone,
        C: HasJobBus,
    {
        self.0.add(ScheduledJob::new(expr, job));
        self
    }

    pub fn registry(&self) -> &CronJobRegistry<C> {
        &self.0
    }
}
