use futures_util::future::BoxFuture;
use job_queue_kit::{Job, JobRegistry};
use rootcause::Result;

/// 各 infra / feature / 组合根通过 `provide_group` 贡献的 Job handler 集合。
pub struct JobHandlers<C: Send + Sync + 'static>(JobRegistry<C>);

impl<C: Send + Sync + 'static> Default for JobHandlers<C> {
    fn default() -> Self {
        Self(JobRegistry::default())
    }
}

impl<C: Send + Sync + 'static> Clone for JobHandlers<C> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<C: Send + Sync + 'static> JobHandlers<C> {
    pub fn register<T, F>(&mut self, handler: F) -> &mut Self
    where
        T: Job,
        F: for<'a> Fn(T, &'a C) -> BoxFuture<'a, Result<()>> + Send + Sync + 'static,
    {
        self.0.register(handler);
        self
    }

    pub fn registry(&self) -> &JobRegistry<C> {
        &self.0
    }
}
