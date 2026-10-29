//! 按能力拆提的 State 访问接口：handlers 只依赖需要的 trait，避免改 blob 牵动无关域的类型图。
//! 组合根仍组装完整 `WebState`；双 bin（api/worker）拆分另议。

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
