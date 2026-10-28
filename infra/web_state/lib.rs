use sqlx::PgPool;

use blob_kit::Blob;
use cache_kit::Cache;
use config::TrustedProxies;
use config::cli::Cli;
use http_extractors::HasTrustedProxies;
use job_queue_kit::JobBus;
use jwt_kit::{TokenHelper, TokenRealm};
use modrun::Module;

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
