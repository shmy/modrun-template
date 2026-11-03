//! 头像绑定：经 `file::attachment` 登记正式对象（commit / attach / detach）。

use blob_kit::Blob;
use file::attachment;
use file_contract::FileError;
use identity_contract::IdentityError;
use rootcause::Result;
use sqlx::PgPool;

/// staging / 公网 URL → 正式 path 并登记（ref_count=0）。
pub async fn commit_avatar(pool: &PgPool, blob: &Blob, submitted: &str) -> Result<String> {
    attachment::commit_staging(pool, blob, submitted)
        .await
        .map_err(map_file_err)
}

pub async fn attach_avatar(pool: &PgPool, path: &str) -> Result<()> {
    attachment::attach(pool, path).await.map_err(map_file_err)
}

pub async fn detach_avatar(pool: &PgPool, path: &str) {
    attachment::detach_best_effort(pool, path).await;
}

fn map_file_err(err: rootcause::Report) -> rootcause::Report {
    match err.downcast_current_context::<FileError>() {
        Some(FileError::AttachmentInvalid) => IdentityError::AccountAvatarInvalid.into(),
        Some(FileError::AttachmentNotFound) => IdentityError::AccountAvatarNotFound.into(),
        _ => err,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use migration::run_migrations;
    use std::io::Cursor;

    #[sqlx::test]
    async fn commit_and_attach_roundtrip(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("migrate");
        let blob = Blob::new_for_test().await.expect("blob");
        let staging = "staging/2026/01/avatar.webp";
        blob.write(staging, Cursor::new(b"img"))
            .await
            .expect("write");

        let path = commit_avatar(&pool, &blob, staging).await.expect("commit");
        assert_eq!(path, "2026/01/avatar.webp");
        attach_avatar(&pool, &path).await.expect("attach");
        let rc: i32 = sqlx::query_scalar!(
            r#"SELECT ref_count AS "ref_count!" FROM blob_objects WHERE path = $1"#,
            path
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(rc, 1);
    }

    #[sqlx::test]
    async fn commit_rejects_non_staging(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("migrate");
        let blob = Blob::new_for_test().await.expect("blob");
        let err = commit_avatar(&pool, &blob, "2026/01/already.webp")
            .await
            .expect_err("reject");
        assert!(err.to_string().contains("account_avatar_invalid"));
    }
}
