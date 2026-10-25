use migration::run_migrations;
use sqlx::PgPool;
use std::env::var;
use std::error::Error;
use std::process::exit;

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("error: {err}");
        exit(1);
    }
}

async fn run() -> Result<(), Box<dyn Error + Send + Sync>> {
    dotenvy::dotenv_override().ok();
    tracing_subscriber::fmt().init();
    let database_url = var("DATABASE_URL")?;
    let pool = PgPool::connect(&database_url).await?;
    run_migrations(&pool).await?;
    Ok(())
}
