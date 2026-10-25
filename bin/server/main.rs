use config::load;
use modrun::Modrun;
use trace_kit::{TraceConfig, init_tracing};
use utoipa::OpenApi as _;

use crate::api_doc::ApiDoc;

mod api_doc;
mod internal_jobs;
mod job_worker;
mod wiring;

#[cfg(test)]
#[path = "arch_test.rs"]
mod arch_test;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[tokio::main]
async fn main() -> modrun::Result<()> {
    let cli = load();
    #[cfg(feature = "otlp")]
    let _guard = {
        use config::cli::ExposeSecret;
        init_tracing(
            TraceConfig::new(
                &cli.log.level,
                &cli.otlp.service_name,
                &cli.otlp.endpoint,
                cli.otlp.metadata.expose_secret(),
                cli.otlp.sampler_ratio,
            ),
            vec![],
        )
    };
    #[cfg(not(feature = "otlp"))]
    let _guard = init_tracing(TraceConfig::new(&cli.log.level), vec![]);

    Modrun::builder()
        .supply(cli)
        .supply(ApiDoc::openapi())
        .module(persistence::module())
        .module(cache::module())
        .module(blob::module())
        .module(job_queue::module())
        .module(web_state::module())
        .module(wiring::module())
        .module(job_worker::module())
        .module(scheduler::module())
        .module(identity::module())
        .module(file::module())
        .module(health::module())
        .module(http_server::module())
        .run()
        .await
}
