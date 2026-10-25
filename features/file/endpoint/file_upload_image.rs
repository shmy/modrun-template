use std::io::{Cursor, Read as _, Seek as _};

use axum::extract::State;
use axum_typed_multipart::{FieldData, TryFromMultipart};
use blob_kit::Blob;
use file_contract::error::FileError;
use file_contract::{ensure_image_reader, staging_dated_file_path};
use image_kit::compress::{CompressOptions, ImageKit};
use serde::Serialize;
use tempfile::NamedTempFile;
use tokio::task::spawn_blocking;
use utoipa::ToSchema;
use web_error::JsonResponseType;
use web_state::{ValidTypedMultipart, WebState};

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
    State(state): State<WebState>,
    ValidTypedMultipart(multipart): ValidTypedMultipart<UploadImageMultipart>,
) -> JsonResponseType<UploadImageResponse> {
    let response = execute(state.blob, multipart).await?;
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
    let compressed =
        spawn_blocking(move || ImageKit::compress_to_webp(&raw, CompressOptions::default()))
            .await??;
    let path = staging_dated_file_path("webp");
    blob.write(&path, Cursor::new(compressed.bytes)).await?;
    let url = blob.fill_public_url(&path).to_string();
    Ok(UploadImageResponse { path, url })
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
}
