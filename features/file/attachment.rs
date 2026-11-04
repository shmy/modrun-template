//! 正式对象引用登记：commit（staging→正式）/ attach / detach / sync。
//! staging 仍由 BlobGc 清理；本模块只跟踪转正后的 path。
//!
//! `commit_staging` 先校验上传者归属、再 INSERT、再 mv：避免「对象已转正但无登记行」
//! 导致双 GC 都扫不到，并防止他人抢 commit 他人的 staging。
//!
//! **新业务表接附件**：按 `docs/ai/conventions.md`「附件引用」清单做
//! （失败补偿、`*_paths_in_use` 并入 AttachmentGc、第二消费者再拆 crate）。
//! 单 path 蓝本：`identity::shared::avatar_ops`。

use std::collections::HashSet;
use std::time::Duration;

use blob_kit::{Blob, BlobError, STAGING_PREFIX, is_safe_object_path};
use cache_kit::Cache;
use file_contract::FileError;
use rootcause::Result;
use shared_contract::Id;
use sqlx::{PgPool, Postgres, Transaction};

/// pending commit（refcount=0 且未 tombstone）超过此时长可被 AttachmentGc 回收。
pub const PENDING_COMMIT_TTL_HOURS: i64 = 24;
/// tombstone 后保留天数，超过后 AttachmentGc 删 blob + 行。
pub const TOMBSTONE_RETENTION_DAYS: i64 = 7;
/// staging 归属 claim TTL（须覆盖「上传→commit」窗口；过期需重新上传）。
pub const STAGING_OWNER_TTL: Duration = Duration::from_secs(PENDING_COMMIT_TTL_HOURS as u64 * 3600);

fn staging_owner_key(staging_path: &str) -> String {
    format!("blob:staging_owner:{staging_path}")
}

/// 上传成功后绑定 staging path → 上传者；commit 时校验并消费。
pub async fn claim_staging(cache: &Cache, staging_path: &str, owner_id: Id) -> Result<()> {
    if !staging_path.starts_with(STAGING_PREFIX) || !is_safe_object_path(staging_path) {
        return Err(FileError::AttachmentInvalid.into());
    }
    cache
        .set_ex(
            &staging_owner_key(staging_path),
            &owner_id,
            STAGING_OWNER_TTL,
        )
        .await?;
    Ok(())
}

/// 校验并消费 staging 归属；非本人或不存在 → 域错误（不泄露他人归属）。
pub async fn consume_staging_claim(cache: &Cache, staging_path: &str, actor_id: Id) -> Result<()> {
    let key = staging_owner_key(staging_path);
    let Some(owner) = cache.take::<Id>(&key).await? else {
        return Err(FileError::AttachmentNotFound.into());
    };
    if owner != actor_id {
        // 归还合法上传者的 claim，避免误伤。
        restore_staging_claim(cache, staging_path, owner).await;
        return Err(FileError::AttachmentInvalid.into());
    }
    Ok(())
}

/// INSERT/mv 失败后归还 claim，允许同一 staging path 重试 commit。
async fn restore_staging_claim(cache: &Cache, staging_path: &str, owner_id: Id) {
    if let Err(err) = claim_staging(cache, staging_path, owner_id).await {
        tracing::error!(
            error = %err,
            staging_path,
            "restore staging claim failed; uploader must re-upload"
        );
    }
}

/// AttachmentGc：候选中去掉仍被业务列引用的 path（安全网）。
pub fn filter_gc_candidates(candidates: Vec<String>, in_use: &HashSet<String>) -> Vec<String> {
    candidates
        .into_iter()
        .filter(|path| !in_use.contains(path))
        .collect()
}

/// staging（或公网 URL）→ 正式 path，并登记 `ref_count = 0`。
///
/// 顺序：消费上传者 claim → INSERT `blob_objects` → `mv`。
/// INSERT/mv 失败：回滚登记行（若有）并**恢复 claim**，便于同 path 重试。
pub async fn commit_staging(
    pool: &PgPool,
    blob: &Blob,
    cache: &Cache,
    actor_id: Id,
    submitted: &str,
) -> Result<String> {
    let (from, path) = resolve_staging_move(blob, submitted)?;
    consume_staging_claim(cache, &from, actor_id).await?;
    if let Err(err) = sqlx::query!(
        r#"
        INSERT INTO blob_objects (path, ref_count)
        VALUES ($1, 0)
        "#,
        path
    )
    .execute(pool)
    .await
    {
        restore_staging_claim(cache, &from, actor_id).await;
        return Err(err.into());
    }
    if let Err(err) = blob.mv(&from, &path).await {
        if let Err(rollback_err) = delete_registration(pool, &path).await {
            tracing::error!(
                error = %rollback_err,
                path,
                "commit_staging: mv failed and registration rollback failed"
            );
        }
        restore_staging_claim(cache, &from, actor_id).await;
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
    attach_exec(pool, path).await
}

/// 事务内 attach（与业务行写入同事务，避免「行有 path、refcount=0」窗口）。
pub async fn attach_tx(tx: &mut Transaction<'_, Postgres>, path: &str) -> Result<()> {
    attach_exec(&mut **tx, path).await
}

async fn attach_exec<'e, E>(executor: E, path: &str) -> Result<()>
where
    E: sqlx::Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        r#"
        UPDATE blob_objects
        SET ref_count = ref_count + 1,
            tombstoned_at = NULL
        WHERE path = $1
        "#,
        path
    )
    .execute(executor)
    .await?;
    if result.rows_affected() == 0 {
        return Err(FileError::AttachmentNotRegistered.into());
    }
    Ok(())
}

/// 减少引用；归 0（或本已是 pending 0）时打 tombstone。
pub async fn detach(pool: &PgPool, path: &str) -> Result<()> {
    detach_exec(pool, path).await
}

/// 事务内 detach（删实体时与 DELETE 同行，避免先删行导致 ref_count 永久泄漏）。
pub async fn detach_tx(tx: &mut Transaction<'_, Postgres>, path: &str) -> Result<()> {
    detach_exec(&mut **tx, path).await
}

/// 删实体用：登记缺失只 warn（历史脏数据），其它错误仍失败。
pub async fn detach_tx_allowing_missing(
    tx: &mut Transaction<'_, Postgres>,
    path: &str,
) -> Result<()> {
    match detach_tx(tx, path).await {
        Ok(()) => Ok(()),
        Err(err)
            if err
                .downcast_current_context::<FileError>()
                .is_some_and(|e| matches!(e, FileError::AttachmentNotRegistered)) =>
        {
            tracing::warn!(
                path,
                "detach: blob_objects row missing (dirty avatar path); continue delete"
            );
            Ok(())
        }
        Err(err) => Err(err),
    }
}

async fn detach_exec<'e, E>(executor: E, path: &str) -> Result<()>
where
    E: sqlx::Executor<'e, Database = Postgres>,
{
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
    .execute(executor)
    .await?;
    if result.rows_affected() == 0 {
        return Err(FileError::AttachmentNotRegistered.into());
    }
    Ok(())
}

/// 尽力 detach；未登记只 warn（sync 中途补偿等）。删号 / 业务成功路径须用 [`detach`]。
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
    cache: &Cache,
    actor_id: Id,
    old_paths: &[String],
    new_paths: &[String],
) -> Result<Vec<String>> {
    let old_set: HashSet<&str> = old_paths.iter().map(String::as_str).collect();
    let mut resolved = Vec::with_capacity(new_paths.len());
    let mut committed_new: Vec<String> = Vec::new();
    for raw in new_paths {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            for path in &committed_new {
                detach_best_effort(pool, path).await;
            }
            return Err(FileError::AttachmentInvalid.into());
        }
        let stripped = blob.strip_public_url_prefix(trimmed);
        let permanent = if stripped.starts_with(STAGING_PREFIX) {
            match commit_staging(pool, blob, cache, actor_id, trimmed).await {
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
        if sqlx::query_scalar!(
            r#"SELECT 1 AS "one!" FROM blob_objects WHERE path = $1"#,
            *path
        )
        .fetch_optional(pool)
        .await?
        .is_none()
        {
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

    async fn claim_for(pool: &PgPool, staging: &str, owner: Id) -> Cache {
        let cache = Cache::try_new_pg(pool.clone()).await.expect("cache");
        claim_staging(&cache, staging, owner).await.expect("claim");
        cache
    }

    #[sqlx::test]
    async fn commit_attach_detach_refcount(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("migrate");
        let blob = Blob::new_for_test().await.expect("blob");
        let owner = Id::new_unchecked(1);
        let staging = "staging/2026/01/a.webp";
        blob.write(staging, Cursor::new(b"img"))
            .await
            .expect("write");
        let cache = claim_for(&pool, staging, owner).await;

        let path = commit_staging(&pool, &blob, &cache, owner, staging)
            .await
            .expect("commit");
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
    async fn commit_staging_missing_source_rolls_back_and_restores_claim(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("migrate");
        let blob = Blob::new_for_test().await.expect("blob");
        let owner = Id::new_unchecked(2);
        let staging = "staging/2026/01/missing.webp";
        let cache = claim_for(&pool, staging, owner).await;
        let err = commit_staging(&pool, &blob, &cache, owner, staging)
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
        // claim 已恢复：补写 staging 后可同 path 重试
        blob.write(staging, Cursor::new(b"img"))
            .await
            .expect("write");
        let path = commit_staging(&pool, &blob, &cache, owner, staging)
            .await
            .expect("retry after restore");
        assert_eq!(path, "2026/01/missing.webp");
    }

    #[sqlx::test]
    async fn commit_rejects_other_actors_staging(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("migrate");
        let blob = Blob::new_for_test().await.expect("blob");
        let owner = Id::new_unchecked(10);
        let other = Id::new_unchecked(11);
        let staging = "staging/2026/01/owned.webp";
        blob.write(staging, Cursor::new(b"img"))
            .await
            .expect("write");
        let cache = claim_for(&pool, staging, owner).await;
        let err = commit_staging(&pool, &blob, &cache, other, staging)
            .await
            .expect_err("foreign");
        assert!(
            err.downcast_current_context::<FileError>()
                .is_some_and(|e| matches!(e, FileError::AttachmentInvalid)),
            "wrong owner must look like invalid path: {err}"
        );
        // 合法上传者仍可 commit
        let path = commit_staging(&pool, &blob, &cache, owner, staging)
            .await
            .expect("owner commit");
        assert_eq!(path, "2026/01/owned.webp");
    }

    #[sqlx::test]
    async fn commit_without_claim_is_not_found(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("migrate");
        let blob = Blob::new_for_test().await.expect("blob");
        let cache = Cache::try_new_pg(pool.clone()).await.expect("cache");
        let staging = "staging/2026/01/noclaim.webp";
        blob.write(staging, Cursor::new(b"img"))
            .await
            .expect("write");
        let err = commit_staging(&pool, &blob, &cache, Id::new_unchecked(1), staging)
            .await
            .expect_err("no claim");
        assert!(
            err.downcast_current_context::<FileError>()
                .is_some_and(|e| matches!(e, FileError::AttachmentNotFound))
        );
    }

    #[test]
    fn filter_gc_candidates_skips_in_use() {
        let candidates = vec!["a.webp".into(), "b.webp".into(), "c.webp".into()];
        let in_use = HashSet::from(["b.webp".to_string()]);
        assert_eq!(
            filter_gc_candidates(candidates, &in_use),
            vec!["a.webp".to_string(), "c.webp".to_string()]
        );
    }

    #[sqlx::test]
    async fn list_gc_candidates_pending_and_tombstone(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("migrate");
        // 直接造过期 pending / tombstone 行（不走 commit，避免 blob 依赖）
        sqlx::query!(
            r#"
            INSERT INTO blob_objects (path, ref_count, created_at, tombstoned_at)
            VALUES
              ('pending/old.webp', 0, NOW() - INTERVAL '25 hours', NULL),
              ('tomb/old.webp', 0, NOW() - INTERVAL '10 days', NOW() - INTERVAL '8 days'),
              ('fresh.webp', 0, NOW(), NULL),
              ('inuse.webp', 1, NOW() - INTERVAL '25 hours', NULL)
            "#
        )
        .execute(&pool)
        .await
        .unwrap();
        let candidates = list_gc_candidates(&pool).await.unwrap();
        assert!(candidates.contains(&"pending/old.webp".to_string()));
        assert!(candidates.contains(&"tomb/old.webp".to_string()));
        assert!(!candidates.contains(&"fresh.webp".to_string()));
        assert!(!candidates.contains(&"inuse.webp".to_string()));

        let in_use = HashSet::from(["pending/old.webp".to_string()]);
        let filtered = filter_gc_candidates(candidates, &in_use);
        assert!(!filtered.contains(&"pending/old.webp".to_string()));
        assert!(filtered.contains(&"tomb/old.webp".to_string()));
    }

    #[sqlx::test]
    async fn sync_attachments_diffs_sets(pool: sqlx::PgPool) {
        run_migrations(&pool).await.expect("migrate");
        let blob = Blob::new_for_test().await.expect("blob");
        let owner = Id::new_unchecked(20);
        let cache = Cache::try_new_pg(pool.clone()).await.expect("cache");
        for name in ["a.webp", "b.webp", "c.webp"] {
            let staging = format!("staging/2026/01/{name}");
            blob.write(&staging, Cursor::new(b"img"))
                .await
                .expect("write");
            claim_staging(&cache, &staging, owner).await.expect("claim");
        }
        let initial = sync_attachments(
            &pool,
            &blob,
            &cache,
            owner,
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
        claim_staging(&cache, "staging/2026/01/d.webp", owner)
            .await
            .unwrap();
        let next = sync_attachments(
            &pool,
            &blob,
            &cache,
            owner,
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
