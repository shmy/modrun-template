use config::load;
use file::attachment_refs::AttachmentRefs;
use modrun::Modrun;
use std::sync::Arc;
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
        // 附件安全网：至少要有一个域贡献 AttachmentRefs；空 Group 在装配期失败，
        // 避免 AttachmentGc 在无 in-use 集合时误删仍被引用的正式对象。
        .require_group::<Arc<dyn AttachmentRefs>>()
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
