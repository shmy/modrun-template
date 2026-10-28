use blob_kit::Blob;
#[cfg(feature = "cos")]
use blob_kit::CosConfig;
#[cfg(all(feature = "fs", not(feature = "cos")))]
use blob_kit::FsConfig;
use config::cli::Cli;
#[cfg(feature = "cos")]
use config::cli::ExposeSecret;
use modrun::{Module, Result};

pub fn module() -> Module {
    Module::builder("blob").provide_result_async(provide_blob)
}

#[allow(
    unused_variables,
    reason = "modrun provider 按互斥 feature 选用 cli 字段之一"
)]
async fn provide_blob(cli: Cli) -> Result<Blob> {
    // cos 优先：单开 cos、或 `--all-features` 并存时都走生产默认；仅 fs 独占时落盘。
    #[cfg(feature = "cos")]
    {
        Blob::try_new_cos(CosConfig {
            endpoint: &cli.blob.blob_s3_endpoint,
            domain: &cli.blob.blob_s3_domain,
            bucket: &cli.blob.blob_s3_bucket,
            secret_id: cli.blob.blob_s3_secret_id.expose_secret(),
            secret_key: cli.blob.blob_s3_secret_key.expose_secret(),
        })
        .await
        .map_err(|e| modrun::Error::hook(format!("blob init: {e:#}")))
    }
    #[cfg(all(feature = "fs", not(feature = "cos")))]
    {
        Blob::try_new_fs(FsConfig {
            root: &cli.blob.blob_file_root,
            domain: &cli.blob.blob_file_domain,
        })
        .await
        .map_err(|e| modrun::Error::hook(format!("blob init: {e:#}")))
    }
    #[cfg(not(any(feature = "cos", feature = "fs")))]
    compile_error!("blob wiring requires feature \"cos\" or \"fs\"");
}
