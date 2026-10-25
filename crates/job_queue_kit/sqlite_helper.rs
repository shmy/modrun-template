use std::str::FromStr;

use rootcause::Result;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqliteSynchronous};

/// 打开（不存在则创建）本地 sqlite 队列文件，WAL 模式。
///
/// 仅支持单进程消费（sqlite 文件锁语义）；供单机部署 / 测试使用。
/// `synchronous=Normal`：WAL 模式下提交不 fsync（checkpoint 时才落盘），
/// 入队/fetch/落终态每事务省一次磁盘同步——队列是低价值、可重建数据，
/// 崩溃最多丢最近提交（WAL 自恢复），换取显著的写吞吐提升。
pub async fn new_sqlite_pool(path: impl AsRef<str>) -> Result<SqlitePool> {
    let options = SqliteConnectOptions::from_str(path.as_ref())?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal);
    let pool = SqlitePool::connect_with(options).await?;
    Ok(pool)
}
