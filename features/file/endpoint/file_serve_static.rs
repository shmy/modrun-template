use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use file_contract::{STORAGE_DIR_PREFIX, staging_dir_date};
use utoipa::ToSchema;
use web_state::{AppState, HasBlob};

#[derive(ToSchema)]
pub struct ImageAssetResponse {
    #[schema(value_type = Vec<u8>, format = Binary)]
    #[allow(dead_code, reason = "仅用于 OpenAPI 响应体形状描述")]
    pub bytes: Vec<u8>,
}

/// `blob-fs` 的公网 URL 由本进程直接回源（`BLOB_FILE_DOMAIN` 默认
/// `http://localhost:8080/files`）。没有这条路由时，上传接口返回的 `url`
/// 永远是 404——COS 后端由对象存储自行提供，不走这里。
///
/// **安全取舍**：走 `PublicRouter`（`<img>` 不带 Authorization）。保密性依赖
/// 对象名是不可猜测的 TempoId；路径一旦泄露即等同公开。因此：
/// - 只放行 `staging/YYYY/MM/<file>` 形态（与上传写入格式一致）
/// - 拒绝路径穿越 / 空段
/// - staging 会被 GC，不做长期 immutable 缓存，缩短 CDN/浏览器残留窗口
#[utoipa::path(
    get,
    path = "/files/{*path}",
    operation_id = "file_serve_static",
    tag = "file",
    params(("path" = String, Path, description = "staging 下的对象路径，如 staging/2026/01/a.webp")),
    responses(
        (status = 200, description = "对象内容", body = ImageAssetResponse),
        (status = 404, description = "对象不存在"),
    )
)]
pub async fn handler(State(state): State<AppState>, Path(path): Path<String>) -> Response {
    match read_object(&state, &path).await {
        Ok(Some(bytes)) => {
            let content_type = content_type_for(&path);
            (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, content_type),
                    (header::CACHE_CONTROL, "public, max-age=3600"),
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
    if !is_allowed_staging_object_path(path) {
        return Ok(None);
    }
    state.blob().read(path).await
}

/// 仅允许上传接口写出的形态：`staging/YYYY/MM/<filename>.<ext>`。
fn is_allowed_staging_object_path(path: &str) -> bool {
    if !path.starts_with(STORAGE_DIR_PREFIX) {
        return false;
    }
    // 拒绝路径穿越与空段（`staging//x`、`staging/../x`）。
    if path.split('/').any(|seg| seg == ".." || seg.is_empty()) {
        return false;
    }
    // 必须能解析出月份目录，且恰好三段：年 / 月 / 文件名。
    if staging_dir_date(path).is_none() {
        return false;
    }
    let Some(rest) = path.strip_prefix(STORAGE_DIR_PREFIX) else {
        return false;
    };
    let mut parts = rest.split('/');
    let (Some(_year), Some(_month), Some(name), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    // 文件名须带扩展名，且不含隐藏/空名。
    let Some((stem, ext)) = name.rsplit_once('.') else {
        return false;
    };
    !stem.is_empty() && !ext.is_empty() && !stem.contains('.')
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
    async fn missing_object_is_none() {
        let ctx = ctx().await;
        let body = read_object(&ctx, "staging/2026/01/none.webp")
            .await
            .expect("read ok");
        assert!(body.is_none());
    }

    /// 非 `staging/` 前缀不得通过公网读接口暴露。
    #[tokio::test]
    async fn non_staging_prefix_is_rejected() {
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
            "non-staging objects must not be publicly readable"
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
        ] {
            assert!(
                read_object(&ctx, bad).await.expect("read ok").is_none(),
                "should reject {bad}"
            );
        }
    }

    #[test]
    fn allows_canonical_staging_object_path() {
        assert!(is_allowed_staging_object_path("staging/2026/01/abc.webp"));
    }
}
