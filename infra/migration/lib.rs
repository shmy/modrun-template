use sqlx::PgPool;
use sqlx::migrate::Migrator;

pub async fn run_migrations(pool: &PgPool) -> rootcause::Result<()> {
    static MIGRATOR: Migrator = sqlx::migrate!("./versions");
    MIGRATOR.run(pool).await?;
    Ok(())
}
