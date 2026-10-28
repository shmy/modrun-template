//! 统一对象存储后端：`Blob` 枚举 + 方法门面（模式与 `cache_kit::Cache` 一致）。
//!
//! 编译期按 feature 装配（可并存；`infra/blob`：`cos` 优先，仅 fs 独占时落盘）：
//! - `Cos`：feature `cos`（**默认**），腾讯云 COS / S3 兼容对象存储
//! - `Fs`：feature `fs`，本地文件系统（开发/单实例环境）
//! - `TestBlob`：feature `test-utils`，opendal Memory 内存后端（集成测试）
//!
//! 无 trait / 无 `dyn`：`Blob` 内部 match 派发，方法签名稳定。
//! 后端差异仅在构造（`try_new` 装配 Operator）；方法面（写 / 删 / URL 映射 / presign）三端对齐，
//! 其中 presign 仅 Cos 支持 HTTP 直传语义，Fs / TestBlob 返回 `BlobError::PresignUnsupported`。

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )
)]

use std::fmt;
#[cfg(feature = "cos")]
mod cos;
mod error;
#[cfg(feature = "fs")]
mod fs;
#[cfg(feature = "test-utils")]
mod memory;
mod shared;

pub use shared::ListedEntry;

#[cfg(feature = "test-utils")]
use crate::memory::TestBlob;
use std::{
    borrow::Cow,
    fmt::Debug,
    io::{Read, Seek},
    time::Duration,
};

use futures_util::Stream;
use rootcause::Result;
use tokio::fs::File;
use tokio_util::io::ReaderStream;

#[cfg(feature = "cos")]
pub use cos::{Cos, CosConfig};
#[cfg(feature = "fs")]
pub use fs::{Fs, FsConfig};

#[cfg(not(any(feature = "cos", feature = "fs", feature = "test-utils")))]
compile_error!("blob_kit requires feature \"cos\" or \"fs\"");

/// 对象存储后端句柄：克隆共享、方法即 API。
#[derive(Clone)]
pub enum Blob {
    #[cfg(feature = "cos")]
    Cos(Cos),
    #[cfg(feature = "fs")]
    Fs(Fs),
    #[cfg(feature = "test-utils")]
    Test(TestBlob),
}

impl Debug for Blob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Blob").finish()
    }
}

impl Blob {
    /// 各后端构造器独立命名：同名 `try_new` 在 feature 并集下会因方法重名冲突（Rust 无重载），
    /// 拆名后 cos 与 fs 可并存（与 kv 的 `try_new_pg/redb/redis` 同构）。
    #[cfg(feature = "cos")]
    pub async fn try_new_cos<'a>(config: CosConfig<'a>) -> Result<Self> {
        Ok(Self::Cos(Cos::try_new(config).await?))
    }

    #[cfg(feature = "fs")]
    pub async fn try_new_fs<'a>(config: FsConfig<'a>) -> Result<Self> {
        Ok(Self::Fs(Fs::try_new(config).await?))
    }

    /// 测试用内存后端（opendal Memory）：不落盘、不依赖外部服务。
    #[cfg(feature = "test-utils")]
    pub async fn new_for_test() -> Result<Self> {
        Ok(Self::Test(TestBlob::new_for_test().await?))
    }

    /// 拼接公网访问 URL：已是 http(s) 绝对地址则原样返回，否则 `{domain}/{path}`。
    #[inline]
    pub fn fill_public_url<'a>(&self, path: &'a str) -> Cow<'a, str> {
        match self {
            #[cfg(feature = "cos")]
            Self::Cos(b) => b.fill_public_url(path),
            #[cfg(feature = "fs")]
            Self::Fs(b) => b.fill_public_url(path),
            #[cfg(feature = "test-utils")]
            Self::Test(b) => b.fill_public_url(path),
        }
    }

    #[inline]
    pub fn fill_public_url_optional(&self, path: Option<String>) -> Option<String> {
        match self {
            #[cfg(feature = "cos")]
            Self::Cos(b) => b.fill_public_url_optional(path),
            #[cfg(feature = "fs")]
            Self::Fs(b) => b.fill_public_url_optional(path),
            #[cfg(feature = "test-utils")]
            Self::Test(b) => b.fill_public_url_optional(path),
        }
    }

    /// 从公网 URL 还原存储路径：剥离 `{domain}/` 前缀；非本站 URL 原样返回。
    #[inline]
    pub fn strip_public_url_prefix<'a>(&self, path: &'a str) -> Cow<'a, str> {
        match self {
            #[cfg(feature = "cos")]
            Self::Cos(b) => b.strip_public_url_prefix(path),
            #[cfg(feature = "fs")]
            Self::Fs(b) => b.strip_public_url_prefix(path),
            #[cfg(feature = "test-utils")]
            Self::Test(b) => b.strip_public_url_prefix(path),
        }
    }

    #[inline]
    pub fn strip_public_url_prefix_optional(&self, path: Option<String>) -> Option<String> {
        match self {
            #[cfg(feature = "cos")]
            Self::Cos(b) => b.strip_public_url_prefix_optional(path),
            #[cfg(feature = "fs")]
            Self::Fs(b) => b.strip_public_url_prefix_optional(path),
            #[cfg(feature = "test-utils")]
            Self::Test(b) => b.strip_public_url_prefix_optional(path),
        }
    }

    /// 流式写入，返回写入字节数。fs / test 后端与 cos 行为一致。
    #[tracing::instrument(skip(stream))]
    pub async fn write_stream(
        &self,
        path: impl AsRef<str> + Debug,
        stream: impl Stream<Item = Result<ReaderStream<File>>> + Unpin,
    ) -> Result<u64> {
        match self {
            #[cfg(feature = "cos")]
            Self::Cos(b) => b.write_stream(path, stream).await,
            #[cfg(feature = "fs")]
            Self::Fs(b) => b.write_stream(path, stream).await,
            #[cfg(feature = "test-utils")]
            Self::Test(b) => b.write_stream(path, stream).await,
        }
    }

    /// 整块写入（`Read + Seek`，如 `Cursor` / `NamedTempFile`），返回写入字节数。
    #[tracing::instrument(skip(reader))]
    pub async fn write(
        &self,
        path: impl AsRef<str> + Debug,
        reader: impl Read + Seek,
    ) -> Result<u64> {
        match self {
            #[cfg(feature = "cos")]
            Self::Cos(b) => b.write(path, reader).await,
            #[cfg(feature = "fs")]
            Self::Fs(b) => b.write(path, reader).await,
            #[cfg(feature = "test-utils")]
            Self::Test(b) => b.write(path, reader).await,
        }
    }

    /// 批量删除（空列表安全）。
    #[tracing::instrument]
    pub async fn delete_many(&self, paths: Vec<String>) -> Result<()> {
        match self {
            #[cfg(feature = "cos")]
            Self::Cos(b) => b.delete_many(paths).await,
            #[cfg(feature = "fs")]
            Self::Fs(b) => b.delete_many(paths).await,
            #[cfg(feature = "test-utils")]
            Self::Test(b) => b.delete_many(paths).await,
        }
    }

    /// 列出一层子条目（目录 + 文件）；路径不存在返回空列表。供目录遍历 / 过期清理。
    #[tracing::instrument]
    pub async fn list_children(&self, prefix: &str) -> Result<Vec<ListedEntry>> {
        match self {
            #[cfg(feature = "cos")]
            Self::Cos(b) => b.list_children(prefix).await,
            #[cfg(feature = "fs")]
            Self::Fs(b) => b.list_children(prefix).await,
            #[cfg(feature = "test-utils")]
            Self::Test(b) => b.list_children(prefix).await,
        }
    }

    /// 预签名视频直传 URL（仅 Cos 支持；fs / test 后端返回 `BlobError::PresignUnsupported`）。
    #[tracing::instrument]
    pub async fn presign_video_upload_url(
        &self,
        path: impl AsRef<str> + Debug,
        extension: &str,
        expire: Duration,
    ) -> Result<String> {
        match self {
            #[cfg(feature = "cos")]
            Self::Cos(b) => b.presign_video_upload_url(path, extension, expire).await,
            #[cfg(feature = "fs")]
            Self::Fs(b) => b.presign_video_upload_url(path, extension, expire).await,
            #[cfg(feature = "test-utils")]
            Self::Test(b) => b.presign_video_upload_url(path, extension, expire).await,
        }
    }

    /// 预签名图片直传 URL（仅 Cos 支持；fs / test 后端返回 `BlobError::PresignUnsupported`）。
    #[tracing::instrument]
    pub async fn presign_image_upload_url(
        &self,
        path: impl AsRef<str> + Debug,
        extension: &str,
        expire: Duration,
    ) -> Result<String> {
        match self {
            #[cfg(feature = "cos")]
            Self::Cos(b) => b.presign_image_upload_url(path, extension, expire).await,
            #[cfg(feature = "fs")]
            Self::Fs(b) => b.presign_image_upload_url(path, extension, expire).await,
            #[cfg(feature = "test-utils")]
            Self::Test(b) => b.presign_image_upload_url(path, extension, expire).await,
        }
    }
}

#[cfg(all(test, feature = "test-utils"))]
mod tests {
    use std::io::Cursor;

    use tokio_util::io::ReaderStream;

    use super::*;
    use futures_util::stream::once;
    use std::io::SeekFrom;
    use std::io::Write;
    use tempfile::NamedTempFile;
    use tokio::fs::File as TokioFile;

    #[tokio::test]
    async fn facade_roundtrip() {
        let blob = Blob::new_for_test().await.expect("create test blob");

        let path = "staging/test.txt";
        assert_eq!(
            blob.write(path, Cursor::new("it's working")).await.unwrap(),
            12
        );
        assert_eq!(
            blob.fill_public_url(path),
            "http://test.local/files/staging/test.txt"
        );
        assert_eq!(
            blob.strip_public_url_prefix("http://test.local/files/staging/test.txt"),
            "staging/test.txt"
        );
        assert_eq!(
            blob.fill_public_url_optional(Some(path.to_string())),
            Some("http://test.local/files/staging/test.txt".to_string())
        );
        assert_eq!(
            blob.strip_public_url_prefix_optional(Some(
                "http://test.local/files/staging/test.txt".to_string()
            )),
            Some(path.to_string())
        );

        blob.delete_many(vec![path.to_string()]).await.unwrap();
    }

    #[tokio::test]
    async fn facade_write_stream_roundtrip() {
        use std::io::Seek as _;

        let blob = Blob::new_for_test().await.expect("create test blob");

        let file = NamedTempFile::new().expect("create temp file");
        Write::write_all(&mut file.as_file(), b"streamed data").expect("write temp");
        // 写入后 cursor 停在末尾，回卷到开头再交给 ReaderStream。
        let mut file = file.into_file();
        file.seek(SeekFrom::Start(0)).expect("rewind");
        let file = TokioFile::from_std(file);
        let stream = Box::pin(once(async { Ok(ReaderStream::new(file)) }));

        let path = "staging/stream.txt";
        assert_eq!(
            blob.write_stream(path, stream).await.expect("write stream"),
            13
        );
        assert_eq!(
            blob.fill_public_url(path),
            "http://test.local/files/staging/stream.txt"
        );

        blob.delete_many(vec![path.to_string()]).await.unwrap();
    }

    #[tokio::test]
    async fn list_children_nested_roundtrip() {
        use std::io::Cursor;

        let blob = Blob::new_for_test().await.expect("create test blob");
        // 空目录安全
        assert!(blob.list_children("staging/").await.unwrap().is_empty());

        blob.write("staging/2025/01/a.txt", Cursor::new("a"))
            .await
            .unwrap();
        blob.write("staging/2025/01/b.txt", Cursor::new("b"))
            .await
            .unwrap();
        blob.write("staging/2025/02/c.txt", Cursor::new("c"))
            .await
            .unwrap();

        let year_dirs = blob.list_children("staging/").await.unwrap();
        assert_eq!(year_dirs.len(), 1, "staging 下只有一个年份目录");
        let year_dir = &year_dirs[0];
        assert!(!year_dir.is_file);

        let month_dirs = blob.list_children(&year_dir.path).await.unwrap();
        assert_eq!(month_dirs.len(), 2, "2025 下有两个月份目录");
        assert!(month_dirs.iter().all(|e| !e.is_file));

        let files = blob.list_children(&month_dirs[0].path).await.unwrap();
        assert!(!files.is_empty());
        assert!(files.iter().all(|e| e.is_file));

        // 清理全部测试对象
        let mut all = vec![year_dir.path.clone()];
        for m in &month_dirs {
            all.push(m.path.clone());
            for f in blob.list_children(&m.path).await.unwrap() {
                all.push(f.path);
            }
        }
        blob.delete_many(all).await.unwrap();
        assert!(blob.list_children("staging/").await.unwrap().is_empty());
    }
}
