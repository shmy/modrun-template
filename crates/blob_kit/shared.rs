//! 后端共享实现：所有后端仅差在 Operator 构造，方法面完全一致。
//!
//! 每个后端类型（`Cos` / `Fs` / `TestBlob`）持有 `BackendCore`，方法一行转发，
//! 避免 fill_public_url / write / delete_many 等在多个后端文件里逐字重复。

use std::{
    borrow::Cow,
    fmt::Debug,
    io::{Read, Seek},
};

use futures_util::{SinkExt as _, Stream, StreamExt as _};
use opendal::ErrorKind;
use opendal::{EntryMode, Operator};
use rootcause::Result;
use std::fmt;
use tokio::fs::File;
use tokio_util::io::ReaderStream;

use crate::error::BlobError;

/// staging 暂存前缀；`BlobGc` 只清理此前缀下的对象。
/// 须与 `file_contract::STORAGE_DIR_PREFIX`（上传 path 生成）保持同值。
pub const STAGING_PREFIX: &str = "staging/";

/// 去掉 `staging/` 前缀；非 staging 路径原样返回。与 [`BackendCore::fill_public_url`] 同属 path/URL 工具。
#[inline]
pub fn strip_staging_prefix(path: &str) -> &str {
    path.strip_prefix(STAGING_PREFIX).unwrap_or(path)
}

/// 相对对象 path：非空、非绝对、无空段、无 `.` / `..`。
#[inline]
pub fn is_safe_object_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && path
            .split('/')
            .all(|seg| !seg.is_empty() && seg != ".." && seg != ".")
}

/// 公网可读：安全 path，且为 `staging/YYYY/MM/<file>.<ext>` 或已转正 `YYYY/MM/<file>.<ext>`。
pub fn is_public_readable_object_path(path: &str) -> bool {
    if !is_safe_object_path(path) {
        return false;
    }
    let rest = match path.strip_prefix(STAGING_PREFIX) {
        Some(rest) => rest,
        None => path,
    };
    is_dated_object_rest(rest)
}

fn is_dated_object_rest(rest: &str) -> bool {
    let mut parts = rest.split('/');
    let (Some(year), Some(month), Some(name), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    if year.len() != 4 || year.parse::<u16>().is_err() {
        return false;
    }
    let Ok(m) = month.parse::<u32>() else {
        return false;
    };
    if month.len() != 2 || !(1..=12).contains(&m) {
        return false;
    }
    let Some((stem, ext)) = name.rsplit_once('.') else {
        return false;
    };
    !stem.is_empty() && !ext.is_empty() && !stem.contains('.')
}

/// 后端共享内核：`opendal Operator` + 公网 `domain`。克隆共享。
#[derive(Clone)]
pub(crate) struct BackendCore {
    pub(crate) op: Operator,
    pub(crate) domain: String,
}

impl Debug for BackendCore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BackendCore").finish()
    }
}

impl BackendCore {
    /// 拼接公网访问 URL：已是 http(s) 绝对地址则原样返回，否则 `{domain}/{path}`。
    #[inline]
    pub(crate) fn fill_public_url<'a>(&self, path: &'a str) -> Cow<'a, str> {
        if path.starts_with("http://") || path.starts_with("https://") {
            Cow::Borrowed(path)
        } else {
            Cow::Owned(format!("{}/{}", self.domain, path))
        }
    }

    #[inline]
    pub(crate) fn fill_public_url_optional(&self, path: Option<String>) -> Option<String> {
        path.filter(|p| !p.is_empty())
            .map(|p| self.fill_public_url(&p).into_owned())
    }

    /// 从公网 URL 还原存储路径：剥离 `{domain}/` 前缀；非本站 URL 原样返回。
    #[inline]
    pub(crate) fn strip_public_url_prefix<'a>(&self, path: &'a str) -> Cow<'a, str> {
        let prefix = format!("{}/", self.domain);
        if let Some(stripped) = path.strip_prefix(&prefix) {
            Cow::Owned(stripped.to_string())
        } else {
            Cow::Borrowed(path)
        }
    }

    #[inline]
    pub(crate) fn strip_public_url_prefix_optional(&self, path: Option<String>) -> Option<String> {
        path.as_deref()
            .map(|p| self.strip_public_url_prefix(p).into_owned())
    }
}

impl BackendCore {
    /// 流式写入，返回写入字节数。
    /// `concurrent(8)` 仅 Cos multipart 直传生效；Fs / Memory 无分片语义，选项被忽略（无害）。
    #[tracing::instrument(skip(stream))]
    pub(crate) async fn write_stream(
        &self,
        path: impl AsRef<str> + Debug,
        mut stream: impl Stream<Item = Result<ReaderStream<File>>> + Unpin,
    ) -> Result<u64> {
        let writer = self.op.writer_with(path.as_ref()).concurrent(8).await?;
        let mut sink = writer.into_bytes_sink();
        let mut total_size: u64 = 0;

        while let Some(rs) = stream.next().await {
            let mut rs = rs?;
            while let Some(chunk) = rs.next().await {
                let chunk = chunk?;
                total_size += chunk.len() as u64;
                sink.send(chunk).await?;
            }
        }

        sink.close().await?;
        Ok(total_size)
    }

    /// 整块写入（`Read + Seek`，如 `Cursor` / `NamedTempFile`），返回写入字节数。
    #[tracing::instrument(skip(reader))]
    pub(crate) async fn write(
        &self,
        path: impl AsRef<str> + Debug,
        mut reader: impl Read + Seek,
    ) -> Result<u64> {
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf)?;
        let total_size = buf.len() as u64;
        let _ = self.op.write(path.as_ref(), buf).await?;
        Ok(total_size)
    }

    /// 批量删除（空列表安全）。
    #[tracing::instrument]
    pub(crate) async fn delete_many(&self, paths: Vec<String>) -> Result<()> {
        self.op.delete_iter(paths).await?;
        Ok(())
    }

    /// 移动对象：优先 `rename`；不支持时退化为读+写+删（三端行为对齐）。
    #[tracing::instrument]
    pub(crate) async fn mv(&self, from: &str, to: &str) -> Result<()> {
        if from == to {
            return Ok(());
        }
        match self.op.rename(from, to).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == ErrorKind::Unsupported => {
                let Some(out) = self.read(from).await? else {
                    return Err(BlobError::ObjectNotFound.into());
                };
                let _ = self.op.write(to, out.bytes).await?;
                // 源删失败不回滚目标：正式 path 已在，交由调用方 / GC 处理残留 staging。
                if let Err(err) = self.op.delete(from).await {
                    tracing::warn!(
                        error = %err,
                        from,
                        to,
                        "mv: copied ok but delete source failed"
                    );
                }
                Ok(())
            }
            Err(e) if e.kind() == ErrorKind::NotFound => Err(BlobError::ObjectNotFound.into()),
            Err(e) => Err(e.into()),
        }
    }

    /// 将 `staging/...` 挪到去掉前缀后的正式路径，返回目标 path。
    #[tracing::instrument]
    pub(crate) async fn mv_out_of_staging(&self, path: &str) -> Result<String> {
        let from = self.strip_public_url_prefix(path.trim());
        let Some(to) = from
            .strip_prefix(STAGING_PREFIX)
            .filter(|rest| !rest.is_empty())
        else {
            return Err(BlobError::NotStagingPath.into());
        };
        if !is_safe_object_path(from.as_ref()) || !is_safe_object_path(to) {
            return Err(BlobError::UnsafeObjectPath.into());
        }
        let to = to.to_string();
        self.mv(from.as_ref(), &to).await?;
        Ok(to)
    }

    /// 预签名直传仅对象存储（Cos）支持；供 fs / 内存后端返回明确错误（内部 500）。
    #[cfg(any(feature = "fs", feature = "test-utils"))]
    pub(crate) fn presign_unsupported() -> Result<String> {
        use crate::error::BlobError;

        Err(BlobError::PresignUnsupported.into())
    }
}

/// 一次 `list` 拉取的子条目（一层），供目录遍历 / 过期清理使用。
#[derive(Debug, Clone)]
pub struct ListedEntry {
    pub path: String,
    pub is_file: bool,
}

impl BackendCore {
    /// 列出一层子条目（目录 + 文件）。路径不存在按空列表处理（容错）。
    #[tracing::instrument(skip(self))]
    pub(crate) async fn list_children(&self, prefix: &str) -> Result<Vec<ListedEntry>> {
        let entries = match self.op.list(prefix).await {
            Ok(entries) => entries,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        Ok(entries
            .into_iter()
            .map(|entry| ListedEntry {
                path: entry.path().to_string(),
                is_file: entry.metadata().mode() == EntryMode::FILE,
            })
            .collect())
    }

    /// 读取整个对象；不存在返回 `Ok(None)`（供静态文件服务区分 404 与故障）。
    #[tracing::instrument(skip(self))]
    pub(crate) async fn read(&self, path: &str) -> Result<Option<ReadOutput>> {
        match self.op.read(path).await {
            Ok(buffer) => Ok(Some(ReadOutput {
                bytes: buffer.to_vec(),
            })),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

/// 一次整块读取的结果。
pub(crate) struct ReadOutput {
    pub bytes: Vec<u8>,
}

impl Debug for ReadOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReadOutput")
            .field("bytes", &self.bytes.len())
            .finish()
    }
}
