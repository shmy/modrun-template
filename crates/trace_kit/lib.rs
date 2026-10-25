//! `trace_kit` 提供两类能力：
//! - `trace_id`：提取并规范化 trace id（请求头 + 当前 span fallback）
//! - `init`：初始化 tracing（可与 `console` / `otlp` 组合）
//!
//! 推荐按需启用 feature：
//! - 仅提取 trace id：`features = ["trace_id"]`
//! - 初始化日志（控制台）：`features = ["init", "console"]`
//! - 初始化 OTLP：`features = ["init", "otlp"]`

mod trace_id;
pub use trace_id::extract_trace_id;

#[cfg(feature = "init")]
mod init;
#[cfg(feature = "init")]
pub use init::{TraceConfig, TracingGuard, init_tracing};

/// 业务计数器（依赖全局 meter provider；无 OTLP 时为 noop）。
#[cfg(feature = "otlp")]
pub mod metrics;
