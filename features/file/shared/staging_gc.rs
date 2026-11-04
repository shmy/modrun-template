//! `staging/` 过期对象清理（`staging_gc` job 的实现）。
//!
//! 这块逻辑属于 file 域（知道 `staging/YYYY/MM/` 布局与保留语义），
//! 曾直接写在 `bin/server/internal_jobs.rs`，导致组合根依赖 `file_contract`
//! 的路径解析细节。现在由本模块提供，组合根只负责定时触发。

use blob_kit::Blob;
use chrono::{Datelike, Days, NaiveDate, Utc};
use file_contract::STORAGE_DIR_PREFIX;
use rootcause::Result;

/// 名义保留天数；实际按整月目录删除（见 [`collect_expired`]）。
pub const STAGING_RETENTION_DAYS: u32 = 30;

/// 计算 cutoff 日期：`retention_days` 天前。
pub fn cutoff_from_now(retention_days: u32) -> NaiveDate {
    Utc::now().date_naive() - Days::new(retention_days as u64)
}

/// 遍历 `staging/` 并返回可删除的对象路径：
/// - 标准 `staging/YYYY/MM/`：整月早于 `cutoff` 月初则收集其下文件；
/// - **非标准文件**（直接挂在 `staging/` 或 `staging/<year>/` 下）：一律回收，避免永久泄漏；
/// - 无法解析为年月的**目录**仍跳过（避免误删未知树）。
pub async fn collect_expired(blob: &Blob, cutoff: NaiveDate) -> Result<Vec<String>> {
    // 只删除「整个月」都早于保留窗口的目录：防止误删当月内较新的对象。
    let Some(cutoff_month) = cutoff.with_day(1) else {
        return Ok(Vec::new());
    };
    let mut expired = Vec::new();
    let year_dirs = blob.list_children(STORAGE_DIR_PREFIX).await?;
    for year_dir in year_dirs {
        if year_dir.is_file {
            // staging/orphan.webp 等非标准顶层文件
            expired.push(year_dir.path);
            continue;
        }
        let month_dirs = blob.list_children(&year_dir.path).await?;
        for month_dir in month_dirs {
            if month_dir.is_file {
                // staging/2026/orphan.webp
                expired.push(month_dir.path);
                continue;
            }
            let Some(month) = file_contract::staging_dir_date(&month_dir.path) else {
                continue;
            };
            if month < cutoff_month {
                let files = blob.list_children(&month_dir.path).await?;
                for file in files {
                    if file.is_file {
                        expired.push(file.path);
                    }
                }
            }
        }
    }
    Ok(expired)
}

/// 清理 `staging/` 下过期的对象，返回删除数量。
pub async fn purge_expired(blob: &Blob, retention_days: u32) -> Result<u64> {
    let cutoff = cutoff_from_now(retention_days);
    let expired = collect_expired(blob, cutoff).await?;
    let deleted = expired.len() as u64;
    if deleted > 0 {
        blob.delete_many(expired).await?;
    }
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[tokio::test]
    async fn collect_only_expired_months_and_stray_files() {
        let blob = Blob::new_for_test().await.expect("test blob");
        for path in [
            "staging/2026/07/old.webp",
            "staging/2026/08/old.webp",
            "staging/2026/09/keep.webp",
        ] {
            blob.write(path, Cursor::new("x")).await.expect("write");
        }
        blob.write("staging/orphan.webp", Cursor::new("x"))
            .await
            .expect("write orphan");

        let cutoff = NaiveDate::from_ymd_opt(2026, 9, 5).expect("valid date");
        let expired = collect_expired(&blob, cutoff).await.expect("collect");

        assert_eq!(expired.len(), 3);
        assert!(expired.iter().any(|p| p.contains("/07/")));
        assert!(expired.iter().any(|p| p.contains("/08/")));
        assert!(!expired.iter().any(|p| p.contains("/09/")));
        assert!(expired.iter().any(|p| p == "staging/orphan.webp"));
    }

    #[test]
    fn cutoff_is_retention_days_ago() {
        let cutoff = cutoff_from_now(0);
        assert_eq!(cutoff, Utc::now().date_naive());
        assert!(cutoff_from_now(30) < cutoff);
    }
}
