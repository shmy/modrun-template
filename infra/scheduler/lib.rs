mod has_job_bus;
mod registry;
mod scheduled_job;

use config::cli::Cli;
use modrun::{Group, Lifecycle, Module, task};
use sched_kit::{CronJobRegistry, CronScheduler};
use tokio::sync::watch;
use tracing::info;
use web_state::WebState;

pub use has_job_bus::HasJobBus;
pub use registry::SchedJobs;
pub use scheduled_job::ScheduledJob;

pub fn module() -> Module {
    Module::builder("scheduler").invoke(boot)
}

fn boot(
    lc: Lifecycle,
    cli: Cli,
    state: WebState,
    jobs: Group<SchedJobs<WebState>>,
) -> modrun::Result<()> {
    if !cli.server.master {
        info!("[SCHEDULER] Not running scheduler on non-master node");
        return Ok(());
    }

    let mut registry = CronJobRegistry::default();
    for contrib in jobs {
        registry.merge(contrib.registry());
    }

    let mut scheduler = CronScheduler::new(state.clone());
    registry.install_all(&mut scheduler);

    lc.append(task("scheduler.run", move |mut stopped| async move {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let mut run = Box::pin(scheduler.start(shutdown_rx));
        tokio::select! {
            result = run.as_mut() => {
                result.map_err(|e| modrun::Error::hook(format!("scheduler: {e:#}")))?;
            }
            _ = &mut stopped => {
                let _ = shutdown_tx.send(true);
                run.await
                    .map_err(|e| modrun::Error::hook(format!("scheduler: {e:#}")))?;
            }
        }
        Ok(())
    }))?;
    Ok(())
}
