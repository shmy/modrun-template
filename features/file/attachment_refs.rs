//! 附件引用登记 + AttachmentGc 的「在用路径」贡献点。
//!
//! 每个持有附件列的业务域实现一个 [`AttachmentRefs`] 并通过 modrun
//! `provide_group` 贡献；组合根的 `AttachmentGc` 遍历所有贡献者，把结果并成
//! 一份安全网集合。
//!
//! 这样新增「带附件的表」只需在**本域**注册一行，不必回头改 `bin/server`
//! （曾硬编码 `identity::avatar_paths_in_use`，每加一张表都要动组合根）。
//!
//! 组合根用 `require_group::<Arc<dyn AttachmentRefs>>()` 要求至少一名贡献者；
//! 空 `Group` 在装配期失败，避免安全网静默失效后误删仍被引用的对象。

use futures_util::future::BoxFuture;
use rootcause::Result;
use sqlx::PgPool;
use std::collections::HashSet;
use std::sync::Arc;

/// 一个业务域贡献的「仍被引用的正式 path」集合。
///
/// 实现要**只读**，且必须覆盖该域所有持有附件 path 的列。
/// 漏掉一列 = 补偿失败时可能被 GC 删成死链。
pub trait AttachmentRefs: Send + Sync {
    /// 供日志 / 排查使用的来源名（如 `"accounts.avatar"`）。
    fn source(&self) -> &'static str;

    /// 列出当前仍被引用的正式 path。
    fn paths_in_use<'a>(&'a self, pool: &'a PgPool) -> BoxFuture<'a, Result<Vec<String>>>;
}

/// 把多个来源的在用集合合并去重。
pub async fn collect_in_use(
    contributors: &[Arc<dyn AttachmentRefs>],
    pool: &PgPool,
) -> Result<HashSet<String>> {
    let mut in_use = HashSet::new();
    for contributor in contributors {
        let paths = contributor.paths_in_use(pool).await?;
        tracing::debug!(
            source = contributor.source(),
            count = paths.len(),
            "attachment refs collected"
        );
        in_use.extend(paths);
    }
    Ok(in_use)
}
