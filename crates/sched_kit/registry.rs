use std::sync::Arc;

use crate::{CronJob, CronScheduler};

/// 定时任务注册表（泛型上下文；组合根实例化为 `CronJobRegistry<WebState>` 等）。
pub struct CronJobRegistry<C>
where
    C: Clone + Send + Sync + 'static,
{
    jobs: Vec<Arc<dyn CronJob<C>>>,
}

impl<C> Clone for CronJobRegistry<C>
where
    C: Clone + Send + Sync + 'static,
{
    fn clone(&self) -> Self {
        Self {
            jobs: self.jobs.clone(),
        }
    }
}

impl<C> Default for CronJobRegistry<C>
where
    C: Clone + Send + Sync + 'static,
{
    fn default() -> Self {
        Self { jobs: Vec::new() }
    }
}

impl<C> CronJobRegistry<C>
where
    C: Clone + Send + Sync + 'static,
{
    pub fn add<J>(&mut self, job: J) -> &mut Self
    where
        J: CronJob<C> + 'static,
    {
        let name = job.name();
        assert!(
            !self.jobs.iter().any(|j| j.name() == name),
            "cron job name conflict: {name} is already registered"
        );
        self.jobs.push(Arc::new(job));
        self
    }

    pub fn merge(&mut self, other: &CronJobRegistry<C>) {
        for job in &other.jobs {
            let name = job.name();
            assert!(
                !self.jobs.iter().any(|j| j.name() == name),
                "cron job name conflict: {name} is already registered"
            );
            self.jobs.push(job.clone());
        }
    }

    pub fn install_all(&self, scheduler: &mut CronScheduler<C>) {
        for job in &self.jobs {
            scheduler.add_arc(job.clone());
        }
    }
}
