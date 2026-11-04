//! 按能力拆提的 State 访问接口：handlers 只依赖需要的 trait，避免改 blob 牵动无关域的类型图。
//! 组合根仍组装完整 `WebState`；双 bin（api/worker）拆分另议。
//!
//! Handler 用 `State<AppState>`（组合根具体类型），业务逻辑取 `&impl HasSession` /
//! `&impl HasAuthCaps` / `&impl HasPool` / `&impl HasBlob` 等窄边界
//! （utoipa `routes!` 不支持泛型 handler）。
//!
//! **后端相关的能力 trait 整段按 feature 门控**：`HasBlob` / `HasJobs` 的签名依赖
//! `Blob` / `JobBus`，而二者是 `web_state` 的 optional dep，所以 trait 本身也必须
//! `cfg`——否则不选后端的 crate 会因解析不到这些类型而编不过。只读 pool 的
//! `health` / `http_routing` / `http_auth` 不引用这两个 trait，即可完全不碰
//! opendal / apalis。

use cache_kit::Cache;
use jwt_kit::TokenHelper;
use sqlx::PgPool;

#[cfg(any(feature = "blob-fs", feature = "blob-cos"))]
use blob_kit::Blob;
#[cfg(any(feature = "job-queue-pg", feature = "job-queue-sqlite"))]
use job_queue_kit::JobBus;

pub trait HasPool {
    fn pool(&self) -> &PgPool;
}

pub trait HasCache {
    fn cache(&self) -> &Cache;
}

#[cfg(any(feature = "blob-fs", feature = "blob-cos"))]
pub trait HasBlob {
    fn blob(&self) -> &Blob;
}

#[cfg(any(feature = "job-queue-pg", feature = "job-queue-sqlite"))]
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
