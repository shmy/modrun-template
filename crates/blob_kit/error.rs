/// 对象存储后端错误（内部基础设施：HTTP 默认 500，或由调用方映射为域错误；
/// Display key 须落在 `shared.ftl`，供 locale 完整性扫描）。
#[derive(Debug, thiserror::Error)]
#[allow(
    dead_code,
    reason = "fs / test-utils 后端的 presign 路径使用；cos 默认构建不引用"
)]
pub enum BlobError {
    /// 预签名直传仅 Cos 支持；fs / 内存后端无 HTTP 直传语义。
    #[error("blob_presign_unsupported")]
    PresignUnsupported,
    /// `mv_out_of_staging`：path 不以 `staging/` 开头。
    #[error("blob_not_staging_path")]
    NotStagingPath,
    /// path 含穿越 / 空段 / 绝对路径等不安全形态。
    #[error("blob_unsafe_object_path")]
    UnsafeObjectPath,
    /// `mv` / `mv_out_of_staging`：源对象不存在。
    #[error("blob_object_not_found")]
    ObjectNotFound,
}
