use apalis_postgres::PostgresStorage;
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
    // 与 `config::load` 一致：真实环境优先于 `.env`（见该处注释）。
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt().init();
    let database_url = var("DATABASE_URL")?;
    let pool = PgPool::connect(&database_url).await?;
    run_migrations(&pool).await?;
    // apalis-postgres 的 sqlx 宏在线校验需要 `apalis.jobs`；部署/CI 的
    // `cargo run -p migrator` 须在首次编 apalis 之前先建好 schema。
    // 空库冷启动请用 `SQLX_OFFLINE=true`（包内自带 .sqlx），见 just sqlx_up。
    PostgresStorage::<()>::setup(&pool).await?;
    Ok(())
}
