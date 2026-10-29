//! 按能力拆提的 State 访问接口：handlers 只依赖需要的 trait，避免改 blob 牵动无关域的类型图。
//! 组合根仍组装完整 `WebState`；双 bin（api/worker）拆分另议。
//!
//! Handler 用 `State<AppState>`（组合根具体类型），业务逻辑取 `&impl HasSession` /
//! `&impl HasAuthCaps` / `&impl HasPool` / `&impl HasBlob` 等窄边界
//! （utoipa `routes!` 不支持泛型 handler）。

use blob_kit::Blob;
use cache_kit::Cache;
use job_queue_kit::JobBus;
use jwt_kit::TokenHelper;
use sqlx::PgPool;

pub trait HasPool {
    fn pool(&self) -> &PgPool;
}

pub trait HasCache {
    fn cache(&self) -> &Cache;
}

pub trait HasBlob {
    fn blob(&self) -> &Blob;
}

pub trait HasJobs {
    fn jobs(&self) -> &JobBus;
}

pub trait HasTokenHelper {
    fn token_helper(&self) -> &TokenHelper;
}

/// 账号会话类端点：pool + cache + token（登录 / 改密 / 删号等）。
pub trait HasSession: HasPool + HasCache + HasTokenHelper {}
impl<T: HasPool + HasCache + HasTokenHelper> HasSession for T {}

/// 鉴权 / 纯令牌端点：cache + token（中间件、refresh、logout）。
pub trait HasAuthCaps: HasCache + HasTokenHelper {}
impl<T: HasCache + HasTokenHelper> HasAuthCaps for T {}
