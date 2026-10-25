//! 组合根托管 Job worker：`JobRegistry` 在此组装，避免 `job_queue_kit` ↔ `web_state` 循环依赖。
use job_queue_kit::{JobBus, JobRegistry, WorkerManager};
use modrun::{Group, Lifecycle, Module, Result, task};
use tokio::sync::watch;
use web_state::WebState;

use job_queue::JobHandlers;

pub fn module() -> Module {
    Module::builder("job_queue.worker").invoke(spawn_worker)
}

fn spawn_worker(
    lc: Lifecycle,
    bus: JobBus,
    state: WebState,
    handlers: Group<JobHandlers<WebState>>,
) -> Result<()> {
    let mut registry = JobRegistry::default();
    for contrib in handlers {
        registry.merge(contrib.registry());
    }

    lc.append(task("job.worker", move |mut stopped| async move {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let mut manager = WorkerManager::new(bus, state);
        manager.register_all(&registry);
        let mut run = Box::pin(manager.run(shutdown_rx));
        tokio::select! {
            result = run.as_mut() => {
                result.map_err(|e| modrun::Error::hook(format!("job worker: {e:#}")))?;
            }
            _ = &mut stopped => {
                let _ = shutdown_tx.send(true);
                run.await
                    .map_err(|e| modrun::Error::hook(format!("job worker: {e:#}")))?;
            }
        }
        Ok(())
    }))?;
    Ok(())
}
