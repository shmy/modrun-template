use std::io::{Cursor, Read as _, Seek as _};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::extract::State;
use axum_typed_multipart::{FieldData, TryFromMultipart};
use blob_kit::Blob;
use file_contract::error::FileError;
use file_contract::{ensure_image_reader, staging_dated_file_path};
use http_extractors::ValidTypedMultipart;
use image_kit::compress::{CompressOptions, ImageKit};
use image_kit::error::ImageKitError;
use serde::Serialize;
use tempfile::NamedTempFile;
use tokio::task::spawn_blocking;
use utoipa::ToSchema;
use web_error::JsonResponseType;
use web_state::{AppState, HasBlob};

#[derive(TryFromMultipart, ToSchema)]
pub(crate) struct UploadImageMultipart {
    #[form_data(limit = "2MiB")]
    #[schema(value_type = Vec<u8>, format = Binary)]
    pub image: FieldData<NamedTempFile>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct UploadImageResponse {
    pub path: String,
    pub url: String,
}

#[utoipa::path(
    post,
    path = "/api/v1/files/images",
    operation_id = "file_upload_image",
    tag = "file",
    request_body(content_type = "multipart/form-data", content = UploadImageMultipart),
    responses((status = 200, body = UploadImageResponse)),
    security(("bearerAuth" = []))
)]
#[tracing::instrument(skip(state, multipart))]
pub(crate) async fn handler(
    State(state): State<AppState>,
    ValidTypedMultipart(multipart): ValidTypedMultipart<UploadImageMultipart>,
) -> JsonResponseType<UploadImageResponse> {
    let response = execute(state.blob().clone(), multipart).await?;
    Ok(axum::Json(response))
}

#[tracing::instrument(skip_all)]
pub(crate) async fn execute(
    blob: Blob,
    multipart: UploadImageMultipart,
) -> rootcause::Result<UploadImageResponse> {
    let UploadImageMultipart { image } = multipart;
    let Some(_file_name) = image.metadata.file_name else {
        return Err(FileError::FileNameMissing)?;
    };
    let mut file = image.contents;
    ensure_image_reader(&mut file)?;
    file.rewind()?;
    let mut raw = Vec::new();
    file.read_to_end(&mut raw)?;

    // 取消守卫：请求被超时 / 客户端断开而 drop 时置位，让 blocking 解码在
    // 阶段之间提前退出。`spawn_blocking` 的任务本身不可取消，但可协作退出，
    // 避免 408 已返回后仍长时间烧 CPU（实测原实现可持续 100% CPU 数十秒）。
    let cancelled = Arc::new(AtomicBool::new(false));
    let guard = CancelOnDrop(cancelled.clone());
    let cancelled_for_task = cancelled.clone();

    let compressed = spawn_blocking(move || {
        ImageKit::compress_to_webp_with_cancel(
            &raw,
            CompressOptions::default(),
            &cancelled_for_task,
        )
    })
    .await
    .map_err(|err| {
        tracing::error!(error = %err, "image compress task join failed");
        FileError::Undecodable
    })?
    .map_err(map_image_error)?;
    drop(guard);
    let path = staging_dated_file_path("webp");
    blob.write(&path, Cursor::new(compressed.bytes)).await?;
    let url = blob.fill_public_url(&path).to_string();
    Ok(UploadImageResponse { path, url })
}

/// 图片处理错误 → 域错误：客户端输入问题（解码失败 / 尺寸超限）映射为 400
/// `file_not_image`；取消不记 ERROR（请求多半已由 TimeoutLayer 回 408）；
/// 只有编码失败保留原始错误走 500。
fn map_image_error(err: ImageKitError) -> rootcause::Report {
    if err.is_client_error() {
        tracing::debug!(error = %err, "image rejected");
        FileError::Undecodable.into()
    } else if matches!(err, ImageKitError::Cancelled) {
        // handler 通常已随超时 drop，这条路径少见；避免污染 5xx 错误率。
        tracing::debug!("image compress cancelled after client disconnect or timeout");
        err.into()
    } else {
        tracing::error!(error = %err, "image encode failed");
        err.into()
    }
}

/// drop 时置位取消标志（请求被超时 / 客户端断开时触发）。
struct CancelOnDrop(Arc<AtomicBool>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use axum_typed_multipart::FieldMetadata;
    use image::codecs::png::PngEncoder;
    use image::{ImageBuffer, ImageEncoder, Rgba};

    use super::*;
    use image::ExtendedColorType;
    use tempfile::NamedTempFile as TempfileNamedTempFile;

    fn create_png_fixture() -> Vec<u8> {
        let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
            ImageBuffer::from_fn(4, 4, |_, _| Rgba([200, 100, 50, 255]));
        let mut out = Vec::new();
        let encoder = PngEncoder::new(&mut out);
        encoder
            .write_image(img.as_raw(), 4, 4, ExtendedColorType::Rgba8)
            .expect("encode png fixture");
        out
    }

    fn multipart_with_bytes(bytes: Vec<u8>, file_name: Option<String>) -> UploadImageMultipart {
        let mut temp = TempfileNamedTempFile::new().expect("temp file");
        temp.write_all(&bytes).expect("write bytes");
        temp.rewind().expect("rewind");
        UploadImageMultipart {
            image: FieldData {
                contents: temp,
                metadata: FieldMetadata {
                    name: Some("image".to_string()),
                    file_name,
                    content_type: Some("image/png".to_string()),
                    ..Default::default()
                },
            },
        }
    }

    #[tokio::test]
    async fn upload_image_writes_webp_to_blob() {
        let blob = Blob::new_for_test().await.expect("test blob");
        let png = create_png_fixture();
        let multipart = multipart_with_bytes(png, Some("photo.png".to_string()));

        let response = execute(blob.clone(), multipart).await.expect("upload");

        assert!(response.path.starts_with("staging/"));
        assert!(response.path.ends_with(".webp"));
        assert!(response.url.contains(&response.path));
    }

    #[tokio::test]
    async fn missing_file_name_returns_error() {
        let blob = Blob::new_for_test().await.expect("test blob");
        let multipart = multipart_with_bytes(create_png_fixture(), None);
        let err = execute(blob, multipart).await.expect_err("should fail");
        assert!(err.downcast_current_context::<FileError>().is_some());
    }

    #[tokio::test]
    async fn non_image_content_returns_error() {
        let blob = Blob::new_for_test().await.expect("test blob");
        let multipart = multipart_with_bytes(b"plain text".to_vec(), Some("fake.png".to_string()));
        let err = execute(blob, multipart).await.expect_err("should fail");
        assert!(err.downcast_current_context::<FileError>().is_some());
    }

    /// 魔数合法但内容损坏，必须映射为域错误（400），不能变成 500。
    ///
    /// 回归保护：修复前 `ensure_image_reader` 只查魔数，解码失败时抛出的是
    /// `ImageKitError`（非 `FileError`），domain mapper 认不出 → 500。
    /// 攻击者用 12 字节伪造头即可把请求变成 500 并污染错误率指标。
    #[tokio::test]
    async fn undecodable_image_with_valid_magic_is_domain_error() {
        let blob = Blob::new_for_test().await.expect("test blob");
        // JPEG XL 魔数（FF 0A）+ 垃圾数据：`ensure_image_reader` 放行，解码必失败。
        let mut fake_jxl = vec![0xFF, 0x0A];
        fake_jxl.extend_from_slice(&[0u8; 200]);
        let multipart = multipart_with_bytes(fake_jxl, Some("x.jxl".to_string()));

        let err = execute(blob, multipart).await.expect_err("should fail");
        assert!(
            err.downcast_current_context::<FileError>().is_some(),
            "undecodable image must surface as FileError (400), not an internal error (500): {err}"
        );
    }

    /// PNG 魔数 + 截断内容，同样是客户端输入问题。
    #[tokio::test]
    async fn truncated_png_is_domain_error() {
        let blob = Blob::new_for_test().await.expect("test blob");
        let mut truncated = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        truncated.extend_from_slice(&[0u8; 100]);
        let multipart = multipart_with_bytes(truncated, Some("x.png".to_string()));

        let err = execute(blob, multipart).await.expect_err("should fail");
        assert!(
            err.downcast_current_context::<FileError>().is_some(),
            "truncated png must surface as FileError (400): {err}"
        );
    }
}
