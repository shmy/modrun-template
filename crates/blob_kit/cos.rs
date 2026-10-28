use std::{
    borrow::Cow,
    fmt::Debug,
    io::{Read, Seek},
    time::Duration,
};

use futures_util::Stream;
use opendal::{Operator, layers::LoggingLayer, options::WriteOptions, services};
use rootcause::Result;
use tokio::fs::File;
use tokio_util::io::ReaderStream;

use crate::shared::{BackendCore, ListedEntry};
use std::fmt;

pub struct CosConfig<'a> {
    pub endpoint: &'a str,
    pub domain: &'a str,
    pub bucket: &'a str,
    pub secret_id: &'a str,
    pub secret_key: &'a str,
}

#[derive(Clone)]
pub struct Cos {
    core: BackendCore,
}

impl Debug for Cos {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Cos").finish()
    }
}

impl Cos {
    pub async fn try_new<'a>(config: CosConfig<'a>) -> Result<Self> {
        // workspace 关闭了 opendal `auto-register-services`（无 ctor 自动注册），
        // HTTP 后端需手动注册默认 transport（幂等，可多次调用）。
        opendal::install_default();
        // no-provider transport 下需在首个 TLS 连接前安装默认 CryptoProvider；
        // ring 是全图唯一编译进来的 provider（见 workspace Cargo.toml 注释），重复安装幂等
        let _ = rustls::crypto::ring::default_provider().install_default();
        let bucket = config.bucket;

        let op = {
            let builder = services::Cos::default()
                .endpoint(config.endpoint)
                .bucket(bucket)
                .secret_id(config.secret_id)
                .secret_key(config.secret_key);
            Operator::new(builder)?.layer(LoggingLayer::default())
        };
        let cloned_op = op.clone();
        let bucket = bucket.to_string();
        tokio::spawn(async move {
            let result = cloned_op.check().await;
            if let Err(e) = result {
                tracing::error!("Blob bucket: {} unavailable: {}", bucket, e);
            } else {
                tracing::info!("Blob bucket: {} available", bucket);
            }
        });
        Ok(Self {
            core: BackendCore {
                op,
                domain: config.domain.to_string(),
            },
        })
    }
}

impl Cos {
    #[inline]
    pub fn fill_public_url<'a>(&self, path: &'a str) -> Cow<'a, str> {
        self.core.fill_public_url(path)
    }

    #[inline]
    pub fn fill_public_url_optional(&self, path: Option<String>) -> Option<String> {
        self.core.fill_public_url_optional(path)
    }

    #[inline]
    pub fn strip_public_url_prefix<'a>(&self, path: &'a str) -> Cow<'a, str> {
        self.core.strip_public_url_prefix(path)
    }

    #[inline]
    pub fn strip_public_url_prefix_optional(&self, path: Option<String>) -> Option<String> {
        self.core.strip_public_url_prefix_optional(path)
    }
}

impl Cos {
    #[tracing::instrument(skip(stream))]
    pub async fn write_stream(
        &self,
        path: impl AsRef<str> + Debug,
        stream: impl Stream<Item = Result<ReaderStream<File>>> + Unpin,
    ) -> Result<u64> {
        self.core.write_stream(path, stream).await
    }

    #[tracing::instrument(skip(reader))]
    pub async fn write(
        &self,
        path: impl AsRef<str> + Debug,
        reader: impl Read + Seek,
    ) -> Result<u64> {
        self.core.write(path, reader).await
    }

    #[tracing::instrument]
    pub async fn delete_many(&self, paths: Vec<String>) -> Result<()> {
        self.core.delete_many(paths).await
    }

    #[tracing::instrument]
    pub async fn list_children(&self, prefix: &str) -> Result<Vec<ListedEntry>> {
        self.core.list_children(prefix).await
    }

    #[tracing::instrument]
    pub async fn presign_video_upload_url(
        &self,
        path: impl AsRef<str> + Debug,
        extension: &str,
        expire: Duration,
    ) -> Result<String> {
        let req = self
            .core
            .op
            .presign_write_options(
                path.as_ref(),
                expire,
                WriteOptions {
                    content_type: Some(format!("video/{}", extension)),
                    ..Default::default()
                },
            )
            .await?;
        Ok(req.uri().to_string())
    }

    #[tracing::instrument]
    pub async fn presign_image_upload_url(
        &self,
        path: impl AsRef<str> + Debug,
        extension: &str,
        expire: Duration,
    ) -> Result<String> {
        let req = self
            .core
            .op
            .presign_write_options(
                path.as_ref(),
                expire,
                WriteOptions {
                    content_type: Some(format!("image/{}", extension)),
                    ..Default::default()
                },
            )
            .await?;
        Ok(req.uri().to_string())
    }
}

#[cfg(test)]
mod tests {
    use std::{env::var, io::Cursor};

    use super::*;

    fn cos_config_from_env() -> Option<CosConfig<'static>> {
        dotenvy::dotenv().ok();
        let endpoint = var("BLOB_S3_ENDPOINT").ok()?;
        let domain = var("BLOB_S3_DOMAIN").ok()?;
        let bucket = var("BLOB_S3_BUCKET").ok()?;
        let secret_id = var("BLOB_S3_SECRET_ID").ok()?;
        let secret_key = var("BLOB_S3_SECRET_KEY").ok()?;
        if secret_id.is_empty() || secret_key.is_empty() {
            return None;
        }
        Some(CosConfig {
            endpoint: Box::leak(endpoint.into_boxed_str()),
            domain: Box::leak(domain.into_boxed_str()),
            bucket: Box::leak(bucket.into_boxed_str()),
            secret_id: Box::leak(secret_id.into_boxed_str()),
            secret_key: Box::leak(secret_key.into_boxed_str()),
        })
    }

    #[tokio::test]
    async fn test_upload() {
        let Some(config) = cos_config_from_env() else {
            return;
        };
        let s3 = Cos::try_new(config).await.unwrap();
        let cu = Cursor::new("it's working");
        let _ = s3.write("test.txt", cu).await.unwrap();
        s3.delete_many(vec!["test.txt".to_string()]).await.unwrap();
    }

    #[tokio::test]
    async fn test_presign_video_upload_url() {
        let Some(config) = cos_config_from_env() else {
            return;
        };
        let s3 = Cos::try_new(config).await.unwrap();
        let url = s3
            .presign_video_upload_url("2024/12/test.mp4", "mp4", Duration::from_secs(600))
            .await
            .unwrap();
        println!("Presigned URL: {}", url);
    }

    #[tokio::test]
    async fn test_fill_public_url() {
        let Some(config) = cos_config_from_env() else {
            return;
        };
        let public_domain = config.domain;
        let s3 = Cos::try_new(config).await.unwrap();
        assert_eq!(
            s3.fill_public_url("https://example.com/test.txt"),
            "https://example.com/test.txt"
        );
        assert_eq!(
            s3.fill_public_url("https://www.baidu.com/img/flexible/logo/pc/peak-result.png"),
            "https://www.baidu.com/img/flexible/logo/pc/peak-result.png"
        );

        let link = s3.fill_public_url("assets/icons/logoT0.png");
        assert_eq!(
            link,
            format!("{}/{}", public_domain, "assets/icons/logoT0.png")
        );
    }

    #[tokio::test]
    async fn test_strip_public_url_prefix() {
        let Some(config) = cos_config_from_env() else {
            return;
        };
        let public_domain = config.domain;
        let s3 = Cos::try_new(config).await.unwrap();
        assert_eq!(
            s3.strip_public_url_prefix(
                "https://www.baidu.com/img/flexible/logo/pc/peak-result.png"
            ),
            "https://www.baidu.com/img/flexible/logo/pc/peak-result.png"
        );
        assert_eq!(
            s3.strip_public_url_prefix(&format!("{}/assets/icons/logoT0.png", public_domain)),
            "assets/icons/logoT0.png"
        );
        assert_eq!(
            s3.strip_public_url_prefix("assets/icons/logoT0.png"),
            "assets/icons/logoT0.png"
        );
    }
}
