use config::cli::{Cli, ExposeSecret};
use modrun::Error;
use modrun::Module;
use sqlx::{
    ConnectOptions, PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};

pub fn module() -> Module {
    Module::builder("persistence").provide_result_async(provide_pool)
}

async fn provide_pool(cli: Cli) -> rootcause::Result<PgPool> {
    let options: PgConnectOptions = cli
        .database
        .url
        .expose_secret()
        .parse()
        .map_err(|e| Error::hook(format!("database_url parse: {e}")))?;
    let options = options.disable_statement_logging();
    let pool = PgPoolOptions::new()
        .max_connections(cli.database.max_connections)
        .min_connections(cli.database.min_connections)
        .max_lifetime(cli.database.max_lifetime.duration())
        .idle_timeout(cli.database.idle_timeout.duration())
        .acquire_timeout(cli.database.acquire_timeout.duration())
        .connect_with(options)
        .await
        .map_err(|e| Error::hook(format!("postgresql connect: {e:#}")))?;

    migration::run_migrations(&pool)
        .await
        .map_err(|e| Error::hook(format!("database migration: {e:#}")))?;
    tracing::info!("database migrations applied");

    let mut conn = pool.acquire().await?;
    let row = sqlx::query!(
        r#"SELECT version() AS "version!", current_setting('TimeZone') AS "timezone!""#
    )
    .fetch_one(&mut *conn)
    .await?;
    tracing::info!(
        "postgresql: {} connected, timezone: {}",
        row.version,
        row.timezone
    );
    drop(conn);
    Ok(pool)
}
