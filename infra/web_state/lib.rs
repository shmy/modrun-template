use sqlx::PgPool;

use cache_kit::Cache;
use config::TrustedProxies;
use config::cli::Cli;
use http_extractors::HasTrustedProxies;
use jwt_kit::{TokenHelper, TokenRealm};
use modrun::Module;

#[cfg(any(feature = "blob-fs", feature = "blob-cos"))]
use blob_kit::Blob;
#[cfg(any(feature = "job-queue-pg", feature = "job-queue-sqlite"))]
use job_queue_kit::JobBus;

mod caps;

pub use caps::{HasAuthCaps, HasCache, HasPool, HasSession, HasTokenHelper};

#[cfg(any(feature = "blob-fs", feature = "blob-cos"))]
pub use caps::HasBlob;
#[cfg(any(feature = "job-queue-pg", feature = "job-queue-sqlite"))]
pub use caps::HasJobs;

// 兼容旧 import 路径；新代码优先直接依赖 `http_extractors`。
pub use http_extractors::{ClientIp, ValidJson, ValidPath, ValidQuery, ValidTypedMultipart};

#[derive(Clone)]
pub struct WebState {
    pub pg: PgPool,
    pub cache: Cache,
    /// 仅在选择 blob 后端时存在：不选后端的进程（如纯 worker）结构里没有该字段。
    #[cfg(any(feature = "blob-fs", feature = "blob-cos"))]
    pub blob: Blob,
    /// 仅在选择队列后端时存在。
    #[cfg(any(feature = "job-queue-pg", feature = "job-queue-sqlite"))]
    pub jobs: JobBus,
    pub token_helper: TokenHelper,
    pub trusted_proxies: TrustedProxies,
}

/// 组合根组装的具体 state；`routes!(handler::<AppState>)` 用。
pub type AppState = WebState;

impl HasPool for WebState {
    fn pool(&self) -> &PgPool {
        &self.pg
    }
}

impl HasCache for WebState {
    fn cache(&self) -> &Cache {
        &self.cache
    }
}

#[cfg(any(feature = "blob-fs", feature = "blob-cos"))]
impl HasBlob for WebState {
    fn blob(&self) -> &Blob {
        &self.blob
    }
}

#[cfg(any(feature = "job-queue-pg", feature = "job-queue-sqlite"))]
impl HasJobs for WebState {
    fn jobs(&self) -> &JobBus {
        &self.jobs
    }
}

impl HasTokenHelper for WebState {
    fn token_helper(&self) -> &TokenHelper {
        &self.token_helper
    }
}

impl HasTrustedProxies for WebState {
    fn trusted_proxies(&self) -> &TrustedProxies {
        &self.trusted_proxies
    }
}

pub fn module() -> Module {
    Module::builder("web_state").provide(build_state)
}

#[allow(
    unused_variables,
    reason = "modrun provider 按 feature 门控选用 blob / jobs 参数"
)]
fn build_state(
    pg: PgPool,
    cache: Cache,
    cli: Cli,
    #[cfg(any(feature = "blob-fs", feature = "blob-cos"))] blob: Blob,
    #[cfg(any(feature = "job-queue-pg", feature = "job-queue-sqlite"))] jobs: JobBus,
) -> WebState {
    let token_helper = TokenHelper::new(
        TokenRealm::Account,
        cli.jwt.secret.clone(),
        cli.jwt.access_ttl.duration().as_secs(),
        cli.jwt.refresh_ttl.duration().as_secs(),
        cli.jwt.issuer.clone(),
        cli.jwt.audience.clone(),
    );
    WebState {
        pg,
        cache,
        #[cfg(any(feature = "blob-fs", feature = "blob-cos"))]
        blob,
        #[cfg(any(feature = "job-queue-pg", feature = "job-queue-sqlite"))]
        jobs,
        token_helper,
        trusted_proxies: TrustedProxies::parse(&cli.server.trusted_proxies),
    }
}
