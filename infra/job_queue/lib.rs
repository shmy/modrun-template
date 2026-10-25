mod registry;

use config::cli::Cli;
use job_queue_kit::JobBus;
use modrun::{Module, Result};
use sqlx::PgPool;

pub use registry::JobHandlers;

pub fn module() -> Module {
    Module::builder("job_queue").provide_result_async(provide_bus)
}

#[allow(
    unused_variables,
    reason = "modrun provider 按互斥 feature 选用 cli / pool 之一"
)]
async fn provide_bus(cli: Cli, pool: PgPool) -> Result<JobBus> {
    #[cfg(all(feature = "pg", not(feature = "sqlite")))]
    {
        JobBus::try_new_pg(pool)
            .await
            .map_err(|e| modrun::Error::hook(format!("job queue init: {e:#}")))
    }
    #[cfg(all(feature = "sqlite", not(feature = "pg")))]
    {
        let pool =
            job_queue_kit::sqlite_helper::new_sqlite_pool(&cli.job_queue.job_queue_sqlite_path)
                .await
                .map_err(|e| modrun::Error::hook(format!("job queue sqlite: {e:#}")))?;
        JobBus::try_new_sqlite(pool)
            .await
            .map_err(|e| modrun::Error::hook(format!("job queue init: {e:#}")))
    }
    #[cfg(all(feature = "pg", feature = "sqlite"))]
    {
        JobBus::try_new_pg(pool)
            .await
            .map_err(|e| modrun::Error::hook(format!("job queue init: {e:#}")))
    }
    #[cfg(not(any(feature = "pg", feature = "sqlite")))]
    compile_error!("job_queue wiring requires feature \"pg\" or \"sqlite\"");
}
