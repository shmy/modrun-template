use sqlx::PgPool;

use blob_kit::Blob;
use cache_kit::Cache;
use config::TrustedProxies;
use config::cli::Cli;
use http_extractors::HasTrustedProxies;
use job_queue_kit::JobBus;
use jwt_kit::{TokenHelper, TokenRealm};
use modrun::Module;

mod caps;

pub use caps::{HasAuthCaps, HasBlob, HasCache, HasJobs, HasPool, HasSession, HasTokenHelper};

// 兼容旧 import 路径；新代码优先直接依赖 `http_extractors`。
pub use http_extractors::{ClientIp, ValidJson, ValidPath, ValidQuery, ValidTypedMultipart};

#[derive(Clone)]
pub struct WebState {
    pub pg: PgPool,
    pub cache: Cache,
    pub blob: Blob,
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

impl HasBlob for WebState {
    fn blob(&self) -> &Blob {
        &self.blob
    }
}

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

fn build_state(pg: PgPool, cache: Cache, blob: Blob, jobs: JobBus, cli: Cli) -> WebState {
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
        blob,
        jobs,
        token_helper,
        trusted_proxies: TrustedProxies::parse(&cli.server.trusted_proxies),
    }
}
