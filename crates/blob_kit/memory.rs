//! 测试用内存后端（feature `test-utils`）：opendal `services::Memory`，不落盘、无外部依赖。
//!
//! 仅供集成测试（`appctx::testing::build`）使用，方法面与 Cos / Fs 对齐；
//! presign 无 HTTP 直传语义，返回 `BlobError::PresignUnsupported`（内部 500）。

use std::{
    borrow::Cow,
    fmt::Debug,
    io::{Read, Seek},
    time::Duration,
};

use futures_util::Stream;
use opendal::{Operator, layers::LoggingLayer, services};
use rootcause::Result;
use tokio::fs::File;
use tokio_util::io::ReaderStream;

use crate::shared::{BackendCore, ListedEntry};
use std::fmt;

#[derive(Clone)]
pub struct TestBlob {
    core: BackendCore,
}

impl Debug for TestBlob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TestBlob").finish()
    }
}

impl TestBlob {
    pub async fn new_for_test() -> Result<Self> {
        let op = Operator::new(services::Memory::default())?.layer(LoggingLayer::default());
        op.check().await?;
        Ok(Self {
            core: BackendCore {
                op,
                domain: "http://test.local/files".to_string(),
            },
        })
    }
}

impl TestBlob {
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

impl TestBlob {
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
        _path: impl AsRef<str> + Debug,
        _extension: &str,
        _expire: Duration,
    ) -> Result<String> {
        BackendCore::presign_unsupported()
    }

    #[tracing::instrument]
    pub async fn presign_image_upload_url(
        &self,
        _path: impl AsRef<str> + Debug,
        _extension: &str,
        _expire: Duration,
    ) -> Result<String> {
        BackendCore::presign_unsupported()
    }
}
