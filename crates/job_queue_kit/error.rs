//! worker 内部错误。
//!
//! 基础设施层内部故障（不面向 HTTP，不进 locale）；`last_error` 列与日志统一使用
//! snake_case 消息键（`job_timeout`），保持与领域错误风格一致。

#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    /// 单次执行超过 `Job::TIMEOUT`（计入一次失败，按 `RETRIES` 语义走退避重试）。
    #[error("job_timeout")]
    Timeout,
}
