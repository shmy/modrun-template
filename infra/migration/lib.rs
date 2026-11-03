use sqlx::PgPool;
use sqlx::migrate::Migrator;

pub async fn run_migrations(pool: &PgPool) -> rootcause::Result<()> {
    static MIGRATOR: Migrator = sqlx::migrate!("./versions");
    let _ = sqlx::query("CREATE SCHEMA IF NOT EXISTS public")
        .execute(pool)
        .await?;
    MIGRATOR.run(pool).await?;
    Ok(())
}
