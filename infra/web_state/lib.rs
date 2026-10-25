#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )
)]

use sqlx::PgPool;

pub mod extractor;

pub use extractor::client_ip::ClientIp;
pub use extractor::valid_json::ValidJson;
pub use extractor::valid_path::ValidPath;
pub use extractor::valid_query::ValidQuery;
pub use extractor::valid_typed_multipart::ValidTypedMultipart;

use blob_kit::Blob;
use cache_kit::Cache;
use config::TrustedProxies;
use config::cli::Cli;
use job_queue_kit::JobBus;
use jwt_kit::{TokenHelper, TokenRealm};
use modrun::Module;

#[derive(Clone)]
pub struct WebState {
    pub pg: PgPool,
    pub cache: Cache,
    pub blob: Blob,
    pub jobs: JobBus,
    pub token_helper: TokenHelper,
    pub trusted_proxies: TrustedProxies,
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
