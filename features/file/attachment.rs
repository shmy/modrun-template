//! 正式对象引用登记：commit（staging→正式）/ attach / detach / sync。
//! staging 仍由 BlobGc 清理；本模块只跟踪转正后的 path。
//!
//! `commit_staging` 先 INSERT 再 mv：避免「对象已转正但无登记行」导致双 GC 都扫不到。
//!
//! **新业务表接附件**：按 `docs/ai/conventions.md`「附件引用」清单做
//! （失败补偿、`*_paths_in_use` 并入 AttachmentGc、第二消费者再拆 crate）。
//! 单 path 蓝本：`identity::shared::avatar_ops`。

use std::collections::HashSet;

use blob_kit::{Blob, BlobError, STAGING_PREFIX, is_safe_object_path};
use file_contract::FileError;
use rootcause::Result;
use sqlx::PgPool;

/// pending commit（refcount=0 且未 tombstone）超过此时长可被 AttachmentGc 回收。
pub const PENDING_COMMIT_TTL_HOURS: i64 = 24;
/// tombstone 后保留天数，超过后 AttachmentGc 删 blob + 行。
pub const TOMBSTONE_RETENTION_DAYS: i64 = 7;

/// staging（或公网 URL）→ 正式 path，并登记 `ref_count = 0`。
///
/// 顺序：先写 `blob_objects`，再 `mv`。mv 失败则删除登记行，staging 保持不动。
pub async fn commit_staging(pool: &PgPool, blob: &Blob, submitted: &str) -> Result<String> {
    let (from, path) = resolve_staging_move(blob, submitted)?;
    sqlx::query!(
        r#"
        INSERT INTO blob_objects (path, ref_count)
        VALUES ($1, 0)
        "#,
        path
    )
    .execute(pool)
    .await?;
    if let Err(err) = blob.mv(&from, &path).await {
        if let Err(rollback_err) = delete_registration(pool, &path).await {
            tracing::error!(
                error = %rollback_err,
                path,
                "commit_staging: mv failed and registration rollback failed"
            );
        }
        return Err(map_blob_err(err));
    }
    Ok(path)
}

/// 解析 staging → 正式 path；不做 IO。
fn resolve_staging_move(blob: &Blob, submitted: &str) -> Result<(String, String)> {
    let from = blob.strip_public_url_prefix(submitted.trim()).into_owned();
    let Some(to) = from
        .strip_prefix(STAGING_PREFIX)
        .filter(|rest| !rest.is_empty())
        .map(str::to_string)
    else {
        return Err(FileError::AttachmentInvalid.into());
    };
    if !is_safe_object_path(&from) || !is_safe_object_path(&to) {
        return Err(FileError::AttachmentInvalid.into());
    }
    Ok((from, to))
}

async fn delete_registration(pool: &PgPool, path: &str) -> Result<()> {
    sqlx::query!(r#"DELETE FROM blob_objects WHERE path = $1"#, path)
        .execute(pool)
        .await?;
    Ok(())
}

/// 增加引用；清除 tombstone。
pub async fn attach(pool: &PgPool, path: &str) -> Result<()> {
    let result = sqlx::query!(
        r#"
        UPDATE blob_objects
        SET ref_count = ref_count + 1,
            tombstoned_at = NULL
        WHERE path = $1
        "#,
        path
    )
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(FileError::AttachmentNotRegistered.into());
    }
    Ok(())
}

/// 减少引用；归 0（或本已是 pending 0）时打 tombstone。
pub async fn detach(pool: &PgPool, path: &str) -> Result<()> {
    let result = sqlx::query!(
        r#"
        UPDATE blob_objects
        SET
            ref_count = GREATEST(ref_count - 1, 0),
            tombstoned_at = CASE
                WHEN ref_count <= 1 THEN COALESCE(tombstoned_at, CURRENT_TIMESTAMP)
                ELSE NULL
            END
        WHERE path = $1
        "#,
        path
    )
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(FileError::AttachmentNotRegistered.into());
    }
    Ok(())
}

/// 尽力 detach；未登记只 warn（删号 / 回滚场景）。
pub async fn detach_best_effort(pool: &PgPool, path: &str) {
    if let Err(err) = detach(pool, path).await {
        tracing::warn!(error = %err, path, "detach best-effort failed");
    }
}

/// 列表同步：detach(old−new)，对 new 中 staging 先 commit，再 attach(new−old)。
/// 返回与 `new_paths` 等长的正式 path 列表（顺序保持）。
///
/// attach 中途失败时：回滚本轮已 attach 的 path，并对本轮新 commit 且未进入 old
/// 的 path 打 tombstone，避免留下 ref≥1 孤儿或长期 pending。
pub async fn sync_attachments(
    pool: &PgPool,
    blob: &Blob,
    old_paths: &[String],
    new_paths: &[String],
) -> Result<Vec<String>> {
    let old_set: HashSet<&str> = old_paths.iter().map(String::as_str).collect();
    let mut resolved = Vec::with_capacity(new_paths.len());
    let mut committed_new: Vec<String> = Vec::new();
    for raw in new_paths {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            // 已 commit 的尚无业务引用：tombstone，交 AttachmentGc。
            for path in &committed_new {
                detach_best_effort(pool, path).await;
            }
            return Err(FileError::AttachmentInvalid.into());
        }
        let stripped = blob.strip_public_url_prefix(trimmed);
        let permanent = if stripped.starts_with(STAGING_PREFIX) {
            match commit_staging(pool, blob, trimmed).await {
                Ok(path) => {
                    committed_new.push(path.clone());
                    path
                }
                Err(err) => {
                    for path in &committed_new {
                        detach_best_effort(pool, path).await;
                    }
                    return Err(err);
                }
            }
        } else {
            stripped.into_owned()
        };
        resolved.push(permanent);
    }

    let new_set: HashSet<&str> = resolved.iter().map(String::as_str).collect();
    for path in old_set.difference(&new_set) {
        detach_best_effort(pool, path).await;
    }
    let mut attached_new: Vec<String> = Vec::new();
    for path in new_set.difference(&old_set) {
        // 刚 commit 的尚无行外引用；已存在正式 path 须已登记。
        if sqlx::query_scalar!(
            r#"SELECT 1 AS "one!" FROM blob_objects WHERE path = $1"#,
            *path
        )
        .fetch_optional(pool)
        .await?
        .is_none()
        {
            // 调用方传入未登记的正式 path：补登记为 0 再 attach
            sqlx::query!(
                r#"INSERT INTO blob_objects (path, ref_count) VALUES ($1, 0)"#,
                *path
            )
            .execute(pool)
            .await?;
        }
        if let Err(err) = attach(pool, path).await {
            for a in &attached_new {
                detach_best_effort(pool, a).await;
            }
            // 本轮 commit 但尚未成功 attach 的：tombstone，避免 24h pending 窗口。
            for c in &committed_new {
                if !attached_new.iter().any(|a| a == c) && !old_set.contains(c.as_str()) {
                    detach_best_effort(pool, c).await;
                }
            }
            return Err(err);
        }
        attached_new.push((*path).to_string());
    }
    Ok(resolved)
}

/// AttachmentGc 候选：pending 超时或 tombstone 过期。
pub async fn list_gc_candidates(pool: &PgPool) -> Result<Vec<String>> {
    let rows = sqlx::query_scalar!(
        r#"
        SELECT path
        FROM blob_objects
        WHERE
            (
                ref_count = 0
                AND tombstoned_at IS NULL
                AND created_at < NOW() - ($1::bigint * INTERVAL '1 hour')
            )
            OR (
                tombstoned_at IS NOT NULL
                AND tombstoned_at < NOW() - ($2::bigint * INTERVAL '1 day')
            )
        "#,
        PENDING_COMMIT_TTL_HOURS,
        TOMBSTONE_RETENTION_DAYS
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// 删除 blob 并移除登记行（Gc 用）。
pub async fn purge_object(pool: &PgPool, blob: &Blob, path: &str) -> Result<()> {
    if let Err(err) = blob.delete_many(vec![path.to_string()]).await {
        tracing::warn!(error = %err, path, "attachment gc: blob delete failed");
        return Err(err);
    }
    delete_registration(pool, path).await?;
    Ok(())
}

fn map_blob_err(err: rootcause::Report) -> rootcause::Report {
    match err.downcast_current_context::<BlobError>() {
        Some(BlobError::NotStagingPath | BlobError::UnsafeObjectPath) => {
            FileError::AttachmentInvalid.into()
        }
        Some(BlobError::ObjectNotFound) => FileError::AttachmentNotFound.into(),
        _ => err,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use migration::run_migrations;
    use std::io::Cursor;

    #[sqlx::test]
    async fn commit_attach_detach_refcount(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("migrate");
        let blob = Blob::new_for_test().await.expect("blob");
        let staging = "staging/2026/01/a.webp";
        blob.write(staging, Cursor::new(b"img"))
            .await
            .expect("write");

        let path = commit_staging(&pool, &blob, staging).await.expect("commit");
        assert_eq!(path, "2026/01/a.webp");
        let rc: i32 = sqlx::query_scalar!(
            r#"SELECT ref_count AS "ref_count!" FROM blob_objects WHERE path = $1"#,
            path
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(rc, 0);

        attach(&pool, &path).await.unwrap();
        let rc: i32 = sqlx::query_scalar!(
            r#"SELECT ref_count AS "ref_count!" FROM blob_objects WHERE path = $1"#,
            path
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(rc, 1);

        detach(&pool, &path).await.unwrap();
        let row = sqlx::query!(
            r#"SELECT ref_count, tombstoned_at FROM blob_objects WHERE path = $1"#,
            path
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.ref_count, 0);
        assert!(row.tombstoned_at.is_some());
    }

    #[sqlx::test]
    async fn commit_staging_missing_source_rolls_back_registration(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("migrate");
        let blob = Blob::new_for_test().await.expect("blob");
        let err = commit_staging(&pool, &blob, "staging/2026/01/missing.webp")
            .await
            .expect_err("missing");
        assert!(err.downcast_current_context::<FileError>().is_some());
        let row = sqlx::query_scalar!(
            r#"SELECT 1 AS "one!" FROM blob_objects WHERE path = $1"#,
            "2026/01/missing.webp"
        )
        .fetch_optional(&pool)
        .await
        .unwrap();
        assert!(
            row.is_none(),
            "registration must be rolled back after mv failure"
        );
    }

    #[sqlx::test]
    async fn sync_attachments_diffs_sets(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("migrate");
        let blob = Blob::new_for_test().await.expect("blob");
        for name in ["a.webp", "b.webp", "c.webp"] {
            let staging = format!("staging/2026/01/{name}");
            blob.write(&staging, Cursor::new(b"img"))
                .await
                .expect("write");
        }
        let initial = sync_attachments(
            &pool,
            &blob,
            &[],
            &[
                "staging/2026/01/a.webp".into(),
                "staging/2026/01/b.webp".into(),
                "staging/2026/01/c.webp".into(),
            ],
        )
        .await
        .unwrap();
        assert_eq!(
            initial,
            vec![
                "2026/01/a.webp".to_string(),
                "2026/01/b.webp".to_string(),
                "2026/01/c.webp".to_string()
            ]
        );

        blob.write("staging/2026/01/d.webp", Cursor::new(b"img"))
            .await
            .unwrap();
        let next = sync_attachments(
            &pool,
            &blob,
            &initial,
            &["2026/01/a.webp".into(), "staging/2026/01/d.webp".into()],
        )
        .await
        .unwrap();
        assert_eq!(
            next,
            vec!["2026/01/a.webp".to_string(), "2026/01/d.webp".to_string()]
        );

        let b = sqlx::query!(
            r#"SELECT ref_count, tombstoned_at FROM blob_objects WHERE path = $1"#,
            "2026/01/b.webp"
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(b.ref_count, 0);
        assert!(b.tombstoned_at.is_some());
    }
}
