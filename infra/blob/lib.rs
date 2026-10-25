use blob_kit::Blob;
#[cfg(all(feature = "cos", not(feature = "fs")))]
use blob_kit::CosConfig;
#[cfg(feature = "fs")]
use blob_kit::FsConfig;
use config::cli::Cli;
#[cfg(all(feature = "cos", not(feature = "fs")))]
use config::cli::ExposeSecret;
use modrun::{Module, Result};

pub fn module() -> Module {
    Module::builder("blob").provide_result_async(provide_blob)
}

async fn provide_blob(cli: Cli) -> Result<Blob> {
    #[cfg(all(feature = "cos", not(feature = "fs")))]
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
    #[cfg(feature = "fs")]
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
