use std::future::Future;
use std::pin::Pin;

use rootcause::Result;

pub type CronJobFuture = Pin<Box<dyn Future<Output = Result<()>> + Send>>;

pub trait CronJob<C>: Send + Sync
where
    C: Clone + Send + Sync + 'static,
{
    fn name(&self) -> &'static str;
    fn expr(&self) -> &'static str;
    fn run(&self, context: C) -> CronJobFuture;
}
