use cache_kit::Cache;
use config::cli::Cli;
use modrun::Module;
use sqlx::PgPool;

pub fn module() -> Module {
    Module::builder("cache").provide_result_async(provide_cache)
}

#[allow(
    unused_variables,
    reason = "modrun provider 按互斥 feature 选用 cli / pool 之一"
)]
async fn provide_cache(cli: Cli, pool: PgPool) -> rootcause::Result<Cache> {
    #[cfg(feature = "redis")]
    {
        use cache_kit::{Pool, RedisConnectionManager};
        let manager = RedisConnectionManager::new(cli.cache.cache_redis_url.clone())?;
        let redis_pool = Pool::builder().max_size(16).build(manager).await?;
        Cache::try_new_redis(redis_pool).await
    }
    #[cfg(all(feature = "redb", not(feature = "redis")))]
    {
        Cache::try_new_redb(&cli.cache.cache_redb_path)
    }
    #[cfg(all(feature = "pg", not(any(feature = "redb", feature = "redis"))))]
    {
        Cache::try_new_pg(pool).await
    }
    #[cfg(not(any(feature = "pg", feature = "redb", feature = "redis")))]
    compile_error!("cache wiring requires a backend feature");
}
