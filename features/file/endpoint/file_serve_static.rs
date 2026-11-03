use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use blob_kit::{STAGING_PREFIX, is_public_readable_object_path};
use utoipa::ToSchema;
use web_state::{AppState, HasBlob};

#[derive(ToSchema)]
pub struct ImageAssetResponse {
    #[schema(value_type = Vec<u8>, format = Binary)]
    #[allow(dead_code, reason = "仅用于 OpenAPI 响应体形状描述")]
    pub bytes: Vec<u8>,
}

#[utoipa::path(
    get,
    path = "/files/{*path}",
    operation_id = "file_serve_static",
    tag = "file",
    params(("path" = String, Path, description = "staging/... 或已转正 YYYY/MM/...")),
    responses(
        (status = 200, description = "对象内容", body = ImageAssetResponse),
        (status = 404, description = "对象不存在"),
    )
)]
pub async fn handler(State(state): State<AppState>, Path(path): Path<String>) -> Response {
    match read_object(&state, &path).await {
        Ok(Some(bytes)) => {
            let content_type = content_type_for(&path);
            let cache_control = if path.starts_with(STAGING_PREFIX) {
                "public, max-age=3600"
            } else {
                "public, max-age=31536000, immutable"
            };
            (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, content_type),
                    (header::CACHE_CONTROL, cache_control),
                    (
                        header::HeaderName::from_static("x-content-type-options"),
                        "nosniff",
                    ),
                ],
                bytes,
            )
                .into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(err) => {
            tracing::error!(error = %err, "static file read failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn read_object(state: &impl HasBlob, path: &str) -> rootcause::Result<Option<Vec<u8>>> {
    if !is_public_readable_object_path(path) {
        return Ok(None);
    }
    state.blob().read(path).await
}

fn content_type_for(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("webp") => "image/webp",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use blob_kit::Blob;
    use std::io::Cursor;

    struct Ctx(Blob);

    impl HasBlob for Ctx {
        fn blob(&self) -> &Blob {
            &self.0
        }
    }

    async fn ctx() -> Ctx {
        Ctx(Blob::new_for_test().await.expect("test blob"))
    }

    #[tokio::test]
    async fn serves_existing_staging_object() {
        let ctx = ctx().await;
        ctx.0
            .write("staging/2026/01/a.webp", Cursor::new(b"img"))
            .await
            .expect("write");

        let body = read_object(&ctx, "staging/2026/01/a.webp")
            .await
            .expect("read ok");
        assert!(body.is_some(), "existing object must be served");
        assert_eq!(content_type_for("staging/2026/01/a.webp"), "image/webp");
    }

    #[tokio::test]
    async fn serves_promoted_object() {
        let ctx = ctx().await;
        ctx.0
            .write("2026/01/a.webp", Cursor::new(b"img"))
            .await
            .expect("write");
        let body = read_object(&ctx, "2026/01/a.webp").await.expect("read ok");
        assert!(body.is_some(), "promoted object must be served");
    }

    #[tokio::test]
    async fn missing_object_is_none() {
        let ctx = ctx().await;
        let body = read_object(&ctx, "staging/2026/01/none.webp")
            .await
            .expect("read ok");
        assert!(body.is_none());
    }

    /// 非上传/转正形态不得通过公网读接口暴露。
    #[tokio::test]
    async fn non_whitelisted_prefix_is_rejected() {
        let ctx = ctx().await;
        ctx.0
            .write("secrets/a.txt", Cursor::new(b"x"))
            .await
            .expect("write");
        assert!(
            read_object(&ctx, "secrets/a.txt")
                .await
                .expect("read ok")
                .is_none(),
            "non-whitelisted objects must not be publicly readable"
        );
    }

    /// 路径穿越不得放过。
    #[tokio::test]
    async fn path_traversal_is_rejected() {
        let ctx = ctx().await;
        for bad in [
            "staging/../../etc/passwd",
            "staging//etc/passwd",
            "staging/2026/01/../../../x",
            "staging/2026/01",
            "staging/2026/01/",
            "staging/2026/13/a.webp",
            "staging/evil/01/a.webp",
            "../2026/01/a.webp",
            "/etc/passwd",
        ] {
            assert!(
                read_object(&ctx, bad).await.expect("read ok").is_none(),
                "should reject {bad}"
            );
        }
    }

    #[test]
    fn allows_canonical_paths() {
        assert!(is_public_readable_object_path("staging/2026/01/abc.webp"));
        assert!(is_public_readable_object_path("2026/01/abc.webp"));
    }
}
