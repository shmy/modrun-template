/// 对象存储后端错误（内部基础设施：永远 500，不进 locale，对齐 `libs/image_kit`）。
#[derive(Debug, thiserror::Error)]
#[allow(
    dead_code,
    reason = "fs / test-utils 后端的 presign 路径使用；cos 默认构建不引用"
)]
pub enum BlobError {
    /// 预签名直传仅 Cos 支持；fs / 内存后端无 HTTP 直传语义。
    #[error("blob_presign_unsupported")]
    PresignUnsupported,
}
