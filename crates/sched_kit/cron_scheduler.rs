use rootcause::Result;
use std::sync::Arc;
use tokio::sync::watch::Receiver;
use tokio_cron_scheduler::{Job, JobScheduler};

use crate::CronJob;

pub struct CronScheduler<C>
where
    C: Clone + Send + Sync + 'static,
{
    jobs: Vec<Arc<dyn CronJob<C>>>,
    context: C,
}

impl<C> CronScheduler<C>
where
    C: Clone + Send + Sync + 'static,
{
    pub fn new(context: C) -> Self {
        Self {
            jobs: Vec::new(),
            context,
        }
    }

    pub fn add<J>(&mut self, job: J) -> &mut Self
    where
        J: CronJob<C> + 'static,
    {
        self.add_arc(Arc::new(job))
    }

    pub(crate) fn add_arc(&mut self, job: Arc<dyn CronJob<C>>) -> &mut Self {
        let name = job.name();
        assert!(
            !self.jobs.iter().any(|j| j.name() == name),
            "cron job name conflict: {name} is already registered"
        );
        self.jobs.push(job);
        self
    }

    pub async fn start(mut self, mut shutdown: Receiver<bool>) -> Result<()> {
        let mut sched = JobScheduler::new().await?;
        for job in self.jobs.drain(..) {
            let job_for_runner = job.clone();
            let context = self.context.clone();
            let name = job.name();
            let expr = job.expr();
            sched
                .add(Job::new_async(expr, move |_uuid, _l| {
                    let job_for_runner = job_for_runner.clone();
                    let context = context.clone();
                    Box::pin(async move {
                        if let Err(error) = job_for_runner.run(context).await {
                            tracing::warn!(
                                job = job_for_runner.name(),
                                %error,
                                "cron job run failed"
                            );
                        }
                    })
                })?)
                .await?;
            tracing::info!(job = name, expr, "cron job registered");
        }
        sched.start().await?;
        await_shutdown(&mut shutdown).await;
        tracing::info!("Shutting down scheduler...");
        sched.shutdown().await?;
        Ok(())
    }
}

async fn await_shutdown(shutdown: &mut Receiver<bool>) {
    // watch 值变 true 或发送端关闭即退出。
    while !*shutdown.borrow() && shutdown.changed().await.is_ok() {}
}
