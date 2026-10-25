//! 后台任务队列：点对点命令式后台任务（含延迟投递），重试退避 / 超时 / 终态由本 crate 负责。
//! 业务代码只接触 [`Job`] / [`JobBus`] / [`JobRegistry`]，不接触队列后端细节。
//!
//! - **at-least-once**：拉取置 `Running` 后若进程崩溃，孤儿恢复（`lock_at` 超龄）会
//!   重新投递——handler 必须幂等；
//! - **重试**：失败后 `attempts` 自增，未达 `max_attempts`（= `RETRIES + 1`）则按
//!   指数退避（默认 base 1s、×2、cap 60s，可通过 [`WorkerOptions`] 覆盖）重新入队；
//! - **超时**：单次执行超过 `Job::TIMEOUT` 按一次失败计（`job_timeout`）；
//! - **终态**：重试耗尽 → `Failed` 留表（`last_error` 记录原因），可查询、可审计，
//!   不做显式 DLQ 搬运。

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )
)]

use opentelemetry::global::meter;
use opentelemetry::metrics::Counter;
use std::sync::LazyLock;
use tokio::task::JoinSet;
use tokio::time::MissedTickBehavior;
use tokio::time::interval;
use tokio::time::timeout;
pub mod error;

mod backend;

#[cfg(feature = "sqlite")]
pub mod sqlite_helper;

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use futures_util::future::BoxFuture;
use rootcause::{Result, report};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use tokio::sync::{Notify, Semaphore, watch};

#[cfg(feature = "pg")]
use sqlx::PgPool;
use sqlx::Row as _;
#[cfg(feature = "sqlite")]
use sqlx::SqlitePool;

use backend::{FetchJob, JobMeta};

#[cfg(not(any(feature = "pg", feature = "sqlite")))]
compile_error!("job_queue requires feature \"pg\" or \"sqlite\"");

pub use crate::error::WorkerError;

/// 后台任务定义：**payload 即类型本身**（无关联 type 间接层）。
///
/// 入队侧：`bus.enqueue(GenerateReport { ... })` —— 类型擦除发生在 `JobBus` 内部
/// （序列化为 `(NAME, JSON)`），业务代码不知道任何队列后端。
///
/// 配置项为编译期常量（可覆盖默认值），消费侧按 `job_type = NAME` 竞争消费：
/// - [`Job::RETRIES`]：失败后允许的追加重试次数（总执行次数上限 = `RETRIES + 1`）；
/// - [`Job::CONCURRENCY`]：该 job_type 的最大并行执行数；
/// - [`Job::TIMEOUT`]：单次执行超时（按一次失败计入重试）。
pub trait Job: Serialize + DeserializeOwned + Send + Sync + 'static {
    /// 注册表键 + 队列路由键（`job_type` 列，蛇形命名，如 `generate_report`）。
    const NAME: &'static str;

    /// 失败后允许的追加重试次数（默认 3，总执行 ≤ 4 次）。
    const RETRIES: usize = 3;

    /// 该 job_type 的最大并行执行数（默认 1，串行）。
    const CONCURRENCY: usize = 1;

    /// 单次执行超时（默认 60s）。
    const TIMEOUT: Duration = Duration::from_secs(60);
}

/// 终态（Done / Failed）行的默认保留期（天）。超过后由 `JobQueueGc` 清理，
/// 防止统计台账无限膨胀（见 [`JobBus::delete_finished_older_than`]）。
pub const DEFAULT_JOB_RETENTION_DAYS: i64 = 30;

/// 队列后端：`pg`（生产，worker_jobs 表在业务库）/ `sqlite`（单机部署，本地文件）。
///
/// `notify` 是进程内唤醒信号（`tokio::sync::Notify`），入队成功后触发：
/// - pg：INSERT 触发器 `pg_notify` → `WorkerManager` 的 `PgListener` 任务桥接为 `notify_waiters`；
/// - sqlite：`enqueue_after` 直接 `notify_waiters`。
///
/// 消费侧轮询保留为兜底（NOTIFY 不持久，listener 未就绪/断线期间靠轮询）。
/// 队列积压统计（供可观测性采样，见 [`JobBus::backlog`]）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QueueBacklog {
    pub pending: i64,
    pub running: i64,
    pub failed: i64,
}

#[derive(Clone)]
pub enum JobBus {
    #[cfg(feature = "pg")]
    Pg { pool: PgPool, notify: Arc<Notify> },
    #[cfg(feature = "sqlite")]
    Sqlite {
        pool: SqlitePool,
        notify: Arc<Notify>,
    },
}

impl JobBus {
    /// pg 后端：跑队列迁移（`migrations-pg/`，版本表 `_job_queue_migrations`）。
    /// 启动时执行（自愈：旧库自动升到最新），v1 保留幂等写法兼容迁移系统引入前的自建表。
    #[cfg(feature = "pg")]
    pub async fn try_new_pg(pool: PgPool) -> Result<Self> {
        // 迁移表名来自 sqlx.toml 的 table-name（编译期嵌入），与 sqlx CLI 保持一致。
        sqlx::migrate!("./migrations-pg").run(&pool).await?;
        Ok(Self::Pg {
            pool,
            notify: Arc::new(Notify::new()),
        })
    }

    /// sqlite 后端：跑队列迁移（`migrations-sqlite/`，版本表 `_job_queue_migrations` 记录在
    /// sqlite 文件内）。仅支持单进程消费。
    #[cfg(feature = "sqlite")]
    pub async fn try_new_sqlite(pool: SqlitePool) -> Result<Self> {
        // 迁移表名来自 sqlx.toml 的 table-name（编译期嵌入；sqlite 文件内同名版本表，与 pg 互不干扰）。
        sqlx::migrate!("./migrations-sqlite").run(&pool).await?;
        Ok(Self::Sqlite {
            pool,
            notify: Arc::new(Notify::new()),
        })
    }

    /// 进程内唤醒信号（消费侧轮询循环监听；pg 由 NOTIFY 桥接触发，sqlite 由入队直发）。
    pub(crate) fn notifier(&self) -> &Arc<Notify> {
        match self {
            #[cfg(feature = "pg")]
            JobBus::Pg { notify, .. } => notify,
            #[cfg(feature = "sqlite")]
            JobBus::Sqlite { notify, .. } => notify,
        }
    }

    /// 队列积压统计（观测采样；pg / sqlite 后端同构查询，status 值一致）。
    pub async fn backlog(&self) -> Result<QueueBacklog> {
        match self {
            #[cfg(feature = "pg")]
            JobBus::Pg { pool, .. } => {
                let row = sqlx::query(
                    "SELECT count(*) FILTER (WHERE status = 'Pending') AS pending,
                            count(*) FILTER (WHERE status = 'Running') AS running,
                            count(*) FILTER (WHERE status = 'Failed') AS failed
                     FROM worker_jobs",
                )
                .fetch_one(pool)
                .await?;
                Ok(QueueBacklog {
                    pending: row.try_get("pending")?,
                    running: row.try_get("running")?,
                    failed: row.try_get("failed")?,
                })
            }
            #[cfg(feature = "sqlite")]
            JobBus::Sqlite { pool, .. } => {
                // sqlite ≥ 3.30 支持 FILTER 子句；status 值与 pg 后端一致。
                let row = sqlx::query(
                    "SELECT count(*) FILTER (WHERE status = 'Pending') AS pending,
                            count(*) FILTER (WHERE status = 'Running') AS running,
                            count(*) FILTER (WHERE status = 'Failed') AS failed
                     FROM worker_jobs",
                )
                .fetch_one(pool)
                .await?;
                Ok(QueueBacklog {
                    pending: row.try_get("pending")?,
                    running: row.try_get("running")?,
                    failed: row.try_get("failed")?,
                })
            }
        }
    }

    /// pg 后端连接池（供 LISTEN/NOTIFY 桥接任务使用）。
    #[cfg(feature = "pg")]
    pub(crate) fn pg_pool(&self) -> Option<PgPool> {
        match self {
            JobBus::Pg { pool, .. } => Some(pool.clone()),
            #[cfg(feature = "sqlite")]
            JobBus::Sqlite { .. } => None,
        }
    }

    /// 立即入队。
    pub async fn enqueue<T: Job>(&self, job: T) -> Result<()> {
        self.enqueue_after(job, Duration::ZERO).await
    }

    /// 延迟入队：`delay` 之后才可被消费（`run_at = now + delay`，投递参数不进 payload）。
    /// 入队成功（事务已提交）后触发进程内唤醒信号。
    pub async fn enqueue_after<T: Job>(&self, job: T, delay: Duration) -> Result<()> {
        let max_attempts = T::RETRIES + 1;
        let result = match self {
            #[cfg(feature = "pg")]
            JobBus::Pg { pool, .. } => {
                let payload = serde_json::to_value(&job)?;
                let run_at = Utc::now() + chrono::Duration::milliseconds(delay.as_millis() as i64);
                backend::pg::insert(pool, T::NAME, payload, run_at, max_attempts as i32).await
            }
            #[cfg(feature = "sqlite")]
            JobBus::Sqlite { pool, .. } => {
                let payload = serde_json::to_string(&job)?;
                let run_at = (Utc::now()
                    + chrono::Duration::milliseconds(delay.as_millis() as i64))
                .timestamp_millis();
                backend::sqlite::insert(pool, T::NAME, &payload, run_at, max_attempts as i32).await
            }
        };
        if result.is_ok() {
            self.notifier().notify_waiters();
        }
        result
    }

    /// 清理超过保留期的终态行（Done / Failed），返回删除行数。
    ///
    /// 供统计保留期管理：`worker_jobs` 表同时是执行审计台账（落库即统计），
    /// 终态行按 `DEFAULT_JOB_RETENTION_DAYS` 保留，超期由周期任务 `JobQueueGc` 删除。
    /// 后端实现为分批循环删除（每批 `GC_BATCH` 行），大表下不长时间锁表。
    pub async fn delete_finished_older_than(&self, retention_days: i64) -> Result<u64> {
        match self {
            #[cfg(feature = "pg")]
            JobBus::Pg { pool, .. } => {
                backend::pg::delete_finished_older_than(pool, retention_days).await
            }
            #[cfg(feature = "sqlite")]
            JobBus::Sqlite { pool, .. } => {
                backend::sqlite::delete_finished_older_than(pool, retention_days).await
            }
        }
    }
}

/// 消费侧注册表（泛型上下文；组合根实例化为 `JobRegistry<WebState>` 等）。
pub struct JobRegistry<C> {
    specs: Vec<Arc<dyn JobSpec<C>>>,
}

impl<C> Clone for JobRegistry<C> {
    fn clone(&self) -> Self {
        Self {
            specs: self.specs.clone(),
        }
    }
}

impl<C: Send + Sync + 'static> Default for JobRegistry<C> {
    fn default() -> Self {
        Self { specs: Vec::new() }
    }
}

impl<C: Send + Sync + 'static> JobRegistry<C> {
    /// 注册一个 job 的消费 handler（纯函数：`async fn(T, &C) -> rootcause::Result<()>`）。
    ///
    /// 调用方以闭包形态提供：`register(|job, ctx| Box::pin(handle(job, ctx)))`
    /// （async fn 的 future 借用 `&C`，须装箱为 `BoxFuture` 才能对象安全存储）。
    ///
    /// **同名冲突是编程错误**（同一 `Job::NAME` 注册两个 handler 会导致一个永远不被
    /// 消费），注册时立即 panic（fail fast）。
    pub fn register<T, F>(&mut self, handler: F) -> &mut Self
    where
        T: Job,
        F: for<'a> Fn(T, &'a C) -> BoxFuture<'a, Result<()>> + Send + Sync + 'static,
    {
        assert!(
            !self.specs.iter().any(|s| s.name() == T::NAME),
            "job name conflict: {} is already registered",
            T::NAME
        );
        self.specs.push(Arc::new(TypedJob::<T, C> {
            handler: Arc::new(handler),
        }));
        self
    }

    pub fn merge(&mut self, other: &JobRegistry<C>) {
        for spec in other.specs() {
            let name = spec.name();
            assert!(
                !self.specs.iter().any(|s| s.name() == name),
                "job name conflict: {name} is already registered"
            );
            self.specs.push(spec.clone());
        }
    }

    pub(crate) fn specs(&self) -> &[Arc<dyn JobSpec<C>>] {
        &self.specs
    }
}

/// 消费侧运行时参数（测试可覆盖以加速）。
#[derive(Clone, Debug)]
pub struct WorkerOptions {
    /// 轮询间隔（默认 200ms；延迟投递的最小精度即此值）。
    pub poll_interval: Duration,
    /// 退避基数（默认 1s；第 n 次失败后延迟 = `base * 2^(n-1)`）。
    pub backoff_base: Duration,
    /// 退避封顶（默认 60s）。
    pub backoff_cap: Duration,
    /// 孤儿扫描间隔（默认 60s）。
    pub orphan_interval: Duration,
    /// `Running` 超过该时长视为孤儿（默认 10 分钟）。
    pub orphan_timeout: Duration,
}

impl Default for WorkerOptions {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_millis(200),
            backoff_base: Duration::from_secs(1),
            backoff_cap: Duration::from_secs(60),
            orphan_interval: Duration::from_secs(60),
            orphan_timeout: Duration::from_secs(600),
        }
    }
}

/// 消费侧运行时：持有后端 + 上下文，`register_all` 收编注册表后 `run` 启动
/// 每 job_type 一个轮询循环 + 一个孤儿扫描任务（进程内，随 server 生命周期）。
pub struct WorkerManager<C> {
    bus: JobBus,
    ctx: C,
    specs: Vec<Arc<dyn JobSpec<C>>>,
    options: WorkerOptions,
}

impl<C: Clone + Send + Sync + 'static> WorkerManager<C> {
    pub fn new(bus: JobBus, ctx: C) -> Self {
        Self {
            bus,
            ctx,
            specs: Vec::new(),
            options: WorkerOptions::default(),
        }
    }

    pub fn with_options(mut self, options: WorkerOptions) -> Self {
        self.options = options;
        self
    }

    /// 收编注册表（clone 语义，注册表仍可复用）。
    pub fn register_all(&mut self, registry: &JobRegistry<C>) {
        self.specs.extend(registry.specs().iter().cloned());
    }

    /// 启动消费（每 job_type 一个轮询循环 + 一个孤儿扫描），`shutdown` 为 true 时优雅退出。
    pub async fn run(self, shutdown: watch::Receiver<bool>) -> Result<()> {
        let mut handles = Vec::with_capacity(self.specs.len() + 2);
        // pg 后端：LISTEN/NOTIFY 桥接任务——把 INSERT 触发器的 pg_notify 转成进程内 Notify
        // （sqlite 后端无 listener：Notify 由 enqueue_after 直发）。
        #[cfg(feature = "pg")]
        if let Some(pool) = self.bus.pg_pool() {
            let notify = self.bus.notifier().clone();
            let rx = shutdown.clone();
            handles.push(tokio::spawn(async move {
                run_pg_listener(pool, notify, rx).await;
            }));
        }
        for spec in self.specs {
            let bus = self.bus.clone();
            let ctx = self.ctx.clone();
            let options = self.options.clone();
            let notify = self.bus.notifier().clone();
            let rx = shutdown.clone();
            handles.push(tokio::spawn(async move {
                run_job_loop(spec, bus, ctx, options, notify, rx).await;
            }));
        }
        {
            let bus = self.bus.clone();
            let options = self.options.clone();
            let rx = shutdown.clone();
            handles.push(tokio::spawn(async move {
                run_orphan_sweep(bus, options, rx).await;
            }));
        }
        for handle in handles {
            handle
                .await
                .map_err(|e| report!("worker task join failed: {e}"))?;
        }
        Ok(())
    }
}

/// pg LISTEN/NOTIFY 桥接：监听 `_job_queue_events` 通道，收到通知即唤醒进程内 Notify。
/// 连接失败 / 断线时退出（poll 兜底），不做重连——Notify 语义下丢失一次唤醒无妨。
#[cfg(feature = "pg")]
async fn run_pg_listener(pool: PgPool, notify: Arc<Notify>, mut shutdown: watch::Receiver<bool>) {
    if *shutdown.borrow() {
        return;
    }
    let mut listener = match sqlx::postgres::PgListener::connect_with(&pool).await {
        Ok(listener) => listener,
        Err(e) => {
            tracing::error!(error = %e, "pg listener connect failed, falling back to polling");
            return;
        }
    };
    if let Err(e) = listener.listen("_job_queue_events").await {
        tracing::error!(error = %e, "pg listener listen failed, falling back to polling");
        return;
    }
    tracing::info!("pg listener listening on _job_queue_events");
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            result = listener.recv() => {
                match result {
                    Ok(notification) => {
                        tracing::info!(
                            channel = %notification.channel(),
                            payload = %notification.payload(),
                            "pg notify received"
                        );
                        notify.notify_waiters();
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "pg listener recv failed, falling back to polling");
                        break;
                    }
                }
            }
        }
    }
}

/// 擦除后的 job 规格：注册时类型化、消费时在 trait object 内部还原 `T`。
pub(crate) trait JobSpec<C>: Send + Sync {
    fn name(&self) -> &'static str;
    fn concurrency(&self) -> usize;
    fn timeout(&self) -> Duration;
    fn handle<'a>(&'a self, payload: Value, ctx: &'a C) -> BoxFuture<'a, Result<()>>;
}

/// 擦除后的 job 消费 handler：`for<'a>` 高阶签名——future 借用调用方的 `&'a C`，
/// 因此返回 `BoxFuture<'a>` 而非 `'static`（async fn 的 opaque future 无法装箱为
/// `'static`，见 `JobRegistry::register` 的 HRTB 说明）。
type ErasedHandler<T, C> = dyn for<'a> Fn(T, &'a C) -> BoxFuture<'a, Result<()>> + Send + Sync;

struct TypedJob<T: Job, C> {
    handler: Arc<ErasedHandler<T, C>>,
}

impl<T: Job, C: Send + Sync + 'static> JobSpec<C> for TypedJob<T, C> {
    fn name(&self) -> &'static str {
        T::NAME
    }

    fn concurrency(&self) -> usize {
        T::CONCURRENCY
    }

    fn timeout(&self) -> Duration {
        T::TIMEOUT
    }

    fn handle<'a>(&'a self, payload: Value, ctx: &'a C) -> BoxFuture<'a, Result<()>> {
        match serde_json::from_value::<T>(payload) {
            Ok(job) => (self.handler)(job, ctx),
            Err(e) => Box::pin(async move { Err(report!("{e}")) }),
        }
    }
}

/// 每 job_type 一个轮询循环：并发受 `Semaphore(CONCURRENCY)` 约束，
/// 拉取 → 逐任务 spawn（占位符即并发许可）。唤醒来源：进程内 `Notify`（入队信号，
/// 立即消费）与固定间隔轮询（兜底：NOTIFY 不持久 / listener 未就绪 / 通知丢失）。
/// shutdown 后等待全部 in-flight 完成（handler 受 `TIMEOUT` 约束，等待有界），即优雅退出。
async fn run_job_loop<C: Clone + Send + Sync + 'static>(
    spec: Arc<dyn JobSpec<C>>,
    bus: JobBus,
    ctx: C,
    options: WorkerOptions,
    notify: Arc<Notify>,
    mut shutdown: watch::Receiver<bool>,
) {
    if *shutdown.borrow() {
        return;
    }
    let semaphore = Arc::new(Semaphore::new(spec.concurrency()));
    let mut in_flight = JoinSet::new();
    let mut ticker = interval(options.poll_interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut notified = Box::pin(notify.notified());
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            _ = ticker.tick() => {}
            _ = &mut notified => {
                notified = Box::pin(notify.notified());
            }
        }
        let permits = semaphore.available_permits() as i32;
        if permits <= 0 {
            continue;
        }
        let jobs = match fetch_due(&bus, spec.name(), permits).await {
            Ok(jobs) => jobs,
            Err(e) => {
                tracing::error!(job_type = spec.name(), error = %e, "worker fetch failed");
                continue;
            }
        };
        if jobs.is_empty() {
            continue;
        }
        for job in jobs {
            let Ok(permit) = semaphore.clone().acquire_owned().await else {
                break;
            };
            let spec = spec.clone();
            let bus = bus.clone();
            let ctx = ctx.clone();
            let options = options.clone();
            in_flight.spawn(async move {
                let _permit = permit;
                run_one_job(spec, bus, ctx, job, options).await;
            });
        }
    }
    // 优雅退出：等待全部 in-flight 任务落终态（Done / Failed / 退避重入队）。
    while in_flight.join_next().await.is_some() {}
}

/// 单任务执行（一次）：成功 → Done；失败 → 未达上限退避重入队 / 已达上限 Failed。
/// 重试调度持久化在 DB（`run_at` 后移），进程崩溃不丢退避进度。
async fn run_one_job<C: Send + Sync + 'static>(
    spec: Arc<dyn JobSpec<C>>,
    bus: JobBus,
    ctx: C,
    job: FetchJob,
    options: WorkerOptions,
) {
    let job_type = spec.name();
    let FetchJob { payload, meta } = job;
    let outcome = timeout(spec.timeout(), spec.handle(payload, &ctx)).await;
    match outcome {
        Ok(Ok(())) => {
            if let Err(e) = mark_done(&bus, meta.id).await {
                tracing::error!(job_type, job_id = meta.id, error = %e, "mark job done failed");
            }
        }
        Ok(Err(e)) => finish_failure(&bus, &meta, job_type, &format!("{e}"), &options).await,
        Err(_) => {
            finish_failure(
                &bus,
                &meta,
                job_type,
                &WorkerError::Timeout.to_string(),
                &options,
            )
            .await
        }
    }
}

async fn finish_failure(
    bus: &JobBus,
    meta: &JobMeta,
    job_type: &str,
    last_error: &str,
    options: &WorkerOptions,
) {
    let attempts = meta.attempts + 1;
    if attempts < meta.max_attempts {
        let delay = backoff_delay(attempts, options);
        tracing::warn!(
            job_type,
            job_id = meta.id,
            attempts,
            error = last_error,
            "job failed, will retry"
        );
        if let Err(e) = schedule_retry(bus, meta.id, attempts, delay, last_error).await {
            tracing::error!(job_type, job_id = meta.id, error = %e, "schedule job retry failed");
        }
    } else {
        tracing::error!(
            job_type,
            job_id = meta.id,
            attempts,
            error = last_error,
            "job failed permanently"
        );
        {
            use opentelemetry::KeyValue;
            static JOB_FAILED: LazyLock<Counter<u64>> = LazyLock::new(|| {
                meter("modrun-template")
                    .u64_counter("job.failed")
                    .with_description("Background job terminal failures")
                    .build()
            });
            JOB_FAILED.add(1, &[KeyValue::new("job_type", job_type.to_string())]);
        }
        if let Err(e) = mark_failed(bus, meta.id, attempts, last_error).await {
            tracing::error!(job_type, job_id = meta.id, error = %e, "mark job failed failed");
        }
    }
}

fn backoff_delay(attempt: i32, options: &WorkerOptions) -> Duration {
    let exp = 1u32 << (attempt.saturating_sub(1) as u32).min(30);
    options
        .backoff_base
        .saturating_mul(exp)
        .min(options.backoff_cap)
}

/// 孤儿扫描：`Running` 且 `lock_at` 超龄 → 回置 Pending（at-least-once 兜底）。
async fn run_orphan_sweep(
    bus: JobBus,
    options: WorkerOptions,
    mut shutdown: watch::Receiver<bool>,
) {
    if *shutdown.borrow() {
        return;
    }
    let mut ticker = interval(options.orphan_interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            _ = ticker.tick() => {}
        }
        match reenqueue_orphaned(&bus, options.orphan_timeout).await {
            Ok(count) => {
                if count > 0 {
                    tracing::warn!(count, "re-enqueued orphaned jobs");
                }
            }
            Err(e) => tracing::error!(error = %e, "orphan sweep failed"),
        }
    }
}

// ---- 后端分发（按 JobBus 变体路由到方言实现） ----

async fn fetch_due(bus: &JobBus, job_type: &str, limit: i32) -> Result<Vec<FetchJob>> {
    match bus {
        #[cfg(feature = "pg")]
        JobBus::Pg { pool, .. } => {
            backend::pg::fetch_due(pool, job_type, limit, "local-worker").await
        }
        #[cfg(feature = "sqlite")]
        JobBus::Sqlite { pool, .. } => {
            backend::sqlite::fetch_due(pool, job_type, limit, "local-worker").await
        }
    }
}

async fn mark_done(bus: &JobBus, id: i64) -> Result<()> {
    match bus {
        #[cfg(feature = "pg")]
        JobBus::Pg { pool, .. } => backend::pg::mark_done(pool, id).await,
        #[cfg(feature = "sqlite")]
        JobBus::Sqlite { pool, .. } => backend::sqlite::mark_done(pool, id).await,
    }
}

async fn schedule_retry(
    bus: &JobBus,
    id: i64,
    attempts: i32,
    delay: Duration,
    last_error: &str,
) -> Result<()> {
    match bus {
        #[cfg(feature = "pg")]
        JobBus::Pg { pool, .. } => {
            backend::pg::schedule_retry(pool, id, attempts, delay, last_error).await
        }
        #[cfg(feature = "sqlite")]
        JobBus::Sqlite { pool, .. } => {
            backend::sqlite::schedule_retry(pool, id, attempts, delay, last_error).await
        }
    }
}

async fn mark_failed(bus: &JobBus, id: i64, attempts: i32, last_error: &str) -> Result<()> {
    match bus {
        #[cfg(feature = "pg")]
        JobBus::Pg { pool, .. } => backend::pg::mark_failed(pool, id, attempts, last_error).await,
        #[cfg(feature = "sqlite")]
        JobBus::Sqlite { pool, .. } => {
            backend::sqlite::mark_failed(pool, id, attempts, last_error).await
        }
    }
}

async fn reenqueue_orphaned(bus: &JobBus, orphan_timeout: Duration) -> Result<i64> {
    match bus {
        #[cfg(feature = "pg")]
        JobBus::Pg { pool, .. } => {
            let older_than =
                Utc::now() - chrono::Duration::milliseconds(orphan_timeout.as_millis() as i64);
            backend::pg::reenqueue_orphaned(pool, older_than).await
        }
        #[cfg(feature = "sqlite")]
        JobBus::Sqlite { pool, .. } => {
            let older_than = (Utc::now()
                - chrono::Duration::milliseconds(orphan_timeout.as_millis() as i64))
            .timestamp_millis();
            backend::sqlite::reenqueue_orphaned(pool, older_than).await
        }
    }
}

/// 供测试 / 调用方按时间点断言用的表查询入口（避免测试直接拼 SQL）。
#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    pub(crate) async fn latest_id(bus: &JobBus, job_type: &str) -> Result<i64> {
        match bus {
            #[cfg(feature = "pg")]
            JobBus::Pg { pool, .. } => {
                let row = sqlx::query(
                    "SELECT id FROM worker_jobs WHERE job_type = $1 ORDER BY id DESC LIMIT 1",
                )
                .bind(job_type)
                .fetch_one(pool)
                .await?;
                Ok(row.get("id"))
            }
            #[cfg(feature = "sqlite")]
            JobBus::Sqlite { pool, .. } => {
                let row = sqlx::query(
                    "SELECT id FROM worker_jobs WHERE job_type = ? ORDER BY id DESC LIMIT 1",
                )
                .bind(job_type)
                .fetch_one(pool)
                .await?;
                Ok(row.get("id"))
            }
        }
    }

    /// 测试断言用：按 id 取一行（status / attempts / last_error），避免测试各自拼 SQL。
    pub(crate) struct JobRow {
        pub status: String,
        pub attempts: i32,
        pub last_error: String,
    }

    pub(crate) async fn fetch_row(bus: &JobBus, id: i64) -> Result<JobRow> {
        match bus {
            #[cfg(feature = "pg")]
            JobBus::Pg { pool, .. } => {
                let row = sqlx::query(
                    "SELECT status, attempts, COALESCE(last_error, '') AS last_error
                       FROM worker_jobs WHERE id = $1",
                )
                .bind(id)
                .fetch_one(pool)
                .await?;
                Ok(JobRow {
                    status: row.get("status"),
                    attempts: row.get("attempts"),
                    last_error: row.get("last_error"),
                })
            }
            #[cfg(feature = "sqlite")]
            JobBus::Sqlite { pool, .. } => {
                let row = sqlx::query(
                    "SELECT status, attempts, COALESCE(last_error, '') AS last_error
                       FROM worker_jobs WHERE id = ?",
                )
                .bind(id)
                .fetch_one(pool)
                .await?;
                Ok(JobRow {
                    status: row.get("status"),
                    attempts: row.get("attempts"),
                    last_error: row.get("last_error"),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{fetch_row, latest_id};
    use super::*;
    use serde::{Deserialize, Serialize};
    use std::collections::HashSet;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::task::JoinHandle;
    use tokio::time::Instant;
    use tokio::time::sleep;

    // ---- 示例 Job（框架验证夹具，非业务域） ----

    #[derive(Debug, Serialize, Deserialize, Clone)]
    struct EchoJob {
        n: u32,
    }
    impl Job for EchoJob {
        const NAME: &'static str = "test_echo";
        const CONCURRENCY: usize = 4;
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct FlakyJob {
        n: u32,
    }
    impl Job for FlakyJob {
        const NAME: &'static str = "test_flaky";
        const RETRIES: usize = 2;
        const TIMEOUT: Duration = Duration::from_secs(5);
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct SlowJob {
        n: u32,
    }
    impl Job for SlowJob {
        const NAME: &'static str = "test_slow";
        const RETRIES: usize = 0;
        const TIMEOUT: Duration = Duration::from_millis(100);
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct DedupJob {
        id: u32,
    }
    impl Job for DedupJob {
        const NAME: &'static str = "test_dedup";
        const CONCURRENCY: usize = 2;
    }

    // ---- 测试上下文与 handler ----

    #[derive(Clone, Default)]
    struct TestCtx {
        calls: Arc<AtomicUsize>,
        fail_until: Arc<AtomicUsize>,
        seen: Arc<Mutex<HashSet<u32>>>,
        effects: Arc<AtomicUsize>,
    }

    async fn handle_echo(_job: EchoJob, ctx: &TestCtx) -> Result<()> {
        ctx.calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn handle_flaky(_job: FlakyJob, ctx: &TestCtx) -> Result<()> {
        let call = ctx.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if call <= ctx.fail_until.load(Ordering::SeqCst) {
            return Err(report!("flaky_failure"));
        }
        Ok(())
    }

    async fn handle_slow(_job: SlowJob, _ctx: &TestCtx) -> Result<()> {
        sleep(Duration::from_secs(30)).await;
        Ok(())
    }

    async fn handle_dedup(job: DedupJob, ctx: &TestCtx) -> Result<()> {
        ctx.calls.fetch_add(1, Ordering::SeqCst);
        if ctx.seen.lock().unwrap().insert(job.id) {
            ctx.effects.fetch_add(1, Ordering::SeqCst);
        }
        Ok(())
    }

    // ---- 工具 ----

    fn fast_options() -> WorkerOptions {
        WorkerOptions {
            poll_interval: Duration::from_millis(20),
            backoff_base: Duration::from_millis(50),
            backoff_cap: Duration::from_millis(200),
            orphan_interval: Duration::from_secs(3600),
            orphan_timeout: Duration::from_secs(3600),
        }
    }

    /// 启动 worker（每 job_type 一个轮询循环 + 孤儿扫描），返回关闭信号。
    fn spawn_worker(
        bus: JobBus,
        ctx: TestCtx,
        registry: &JobRegistry<TestCtx>,
    ) -> (watch::Sender<bool>, JoinHandle<()>) {
        let mut manager = WorkerManager::new(bus, ctx).with_options(fast_options());
        manager.register_all(registry);
        let (tx, rx) = watch::channel(false);
        let handle = tokio::spawn(async move {
            manager.run(rx).await.expect("worker run failed");
        });
        (tx, handle)
    }

    async fn wait_status(bus: &JobBus, id: i64, expected: &str, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if fetch_row(bus, id)
                .await
                .map(|row| row.status == expected)
                .unwrap_or(false)
            {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            sleep(Duration::from_millis(20)).await;
        }
    }

    async fn wait_until<F: FnMut() -> bool>(mut check: F, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if check() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            sleep(Duration::from_millis(20)).await;
        }
    }

    // ---- 场景 0：积压统计（backlog，观测采样 API）----

    #[cfg(feature = "pg")]
    #[sqlx::test]
    async fn backlog_counts_pending_running_failed(pool: PgPool) {
        let bus = JobBus::try_new_pg(pool).await.unwrap();
        // 空队列：全零。
        assert_eq!(bus.backlog().await.unwrap(), QueueBacklog::default());

        // 入队（worker 未启动，行保持 Pending）。
        bus.enqueue(EchoJob { n: 1 }).await.unwrap();
        let backlog = bus.backlog().await.unwrap();
        assert_eq!(backlog.pending, 1);
        assert_eq!(backlog.running, 0);
        assert_eq!(backlog.failed, 0);
    }

    // ---- 场景 1：立即入队 → 消费 → Done ----

    #[cfg(feature = "pg")]
    #[sqlx::test]
    async fn immediate_enqueue_consumed_and_done(pool: PgPool) {
        let bus = JobBus::try_new_pg(pool).await.unwrap();
        let ctx = TestCtx::default();
        let mut registry = JobRegistry::default();
        registry.register(|job, ctx| Box::pin(handle_echo(job, ctx)));
        let (_tx, handle) = spawn_worker(bus.clone(), ctx.clone(), &registry);

        bus.enqueue(EchoJob { n: 1 }).await.unwrap();
        let id = latest_id(&bus, EchoJob::NAME).await.unwrap();

        assert!(wait_status(&bus, id, "Done", Duration::from_secs(5)).await);
        assert_eq!(ctx.calls.load(Ordering::SeqCst), 1);
        handle.abort();
    }

    // ---- 场景 2：延迟入队 → 到期前不消费，到期后消费 ----

    #[cfg(feature = "pg")]
    #[sqlx::test]
    async fn delayed_enqueue_not_consumed_until_due(pool: PgPool) {
        let bus = JobBus::try_new_pg(pool).await.unwrap();
        let ctx = TestCtx::default();
        let mut registry = JobRegistry::default();
        registry.register(|job, ctx| Box::pin(handle_echo(job, ctx)));
        let (_tx, handle) = spawn_worker(bus.clone(), ctx.clone(), &registry);

        bus.enqueue_after(EchoJob { n: 1 }, Duration::from_millis(500))
            .await
            .unwrap();
        let id = latest_id(&bus, EchoJob::NAME).await.unwrap();

        // 入队后立即断言未被消费（确定性：run_at 在 DB 视角仍为未来，轮询不会拉取）
        assert_eq!(fetch_row(&bus, id).await.unwrap().status, "Pending");
        assert_eq!(ctx.calls.load(Ordering::SeqCst), 0);

        // 到期后应被消费（DB 时钟自洽：轮询等待 run_at <= now()）
        assert!(wait_status(&bus, id, "Done", Duration::from_secs(5)).await);
        assert_eq!(ctx.calls.load(Ordering::SeqCst), 1);
        handle.abort();
    }

    // ---- 场景 3：失败 → 退避重试 → 成功 ----

    #[cfg(feature = "pg")]
    #[sqlx::test]
    async fn flaky_job_retries_then_succeeds(pool: PgPool) {
        let bus = JobBus::try_new_pg(pool).await.unwrap();
        let ctx = TestCtx::default();
        ctx.fail_until.store(2, Ordering::SeqCst); // 前两次失败
        let mut registry = JobRegistry::default();
        registry.register(|job, ctx| Box::pin(handle_flaky(job, ctx)));
        let (_tx, handle) = spawn_worker(bus.clone(), ctx.clone(), &registry);

        bus.enqueue(FlakyJob { n: 1 }).await.unwrap();
        let id = latest_id(&bus, FlakyJob::NAME).await.unwrap();

        assert!(wait_status(&bus, id, "Done", Duration::from_secs(5)).await);
        assert_eq!(ctx.calls.load(Ordering::SeqCst), 3);
        // 重试通过 attempts 列落库（2 次失败后成功）
        assert_eq!(fetch_row(&bus, id).await.unwrap().attempts, 2);
        handle.abort();
    }

    // ---- 场景 4：超时 → 计入失败，耗尽后 Failed（DLQ-in-table） ----

    #[cfg(feature = "pg")]
    #[sqlx::test]
    async fn slow_job_times_out_and_fails(pool: PgPool) {
        let bus = JobBus::try_new_pg(pool).await.unwrap();
        let ctx = TestCtx::default();
        let mut registry = JobRegistry::default();
        registry.register(|job, ctx| Box::pin(handle_slow(job, ctx)));
        let (_tx, handle) = spawn_worker(bus.clone(), ctx.clone(), &registry);

        bus.enqueue(SlowJob { n: 1 }).await.unwrap();
        let id = latest_id(&bus, SlowJob::NAME).await.unwrap();

        assert!(wait_status(&bus, id, "Failed", Duration::from_secs(5)).await);
        assert_eq!(
            fetch_row(&bus, id).await.unwrap().last_error,
            WorkerError::Timeout.to_string()
        );
        handle.abort();
    }

    // ---- 场景 5：幂等（同一 payload 两次入队，效果只发生一次） ----

    #[cfg(feature = "pg")]
    #[sqlx::test]
    async fn idempotent_handler_dedups_replays(pool: PgPool) {
        let bus = JobBus::try_new_pg(pool).await.unwrap();
        let ctx = TestCtx::default();
        let mut registry = JobRegistry::default();
        registry.register(|job, ctx| Box::pin(handle_dedup(job, ctx)));
        let (_tx, handle) = spawn_worker(bus.clone(), ctx.clone(), &registry);

        bus.enqueue(DedupJob { id: 7 }).await.unwrap();
        bus.enqueue(DedupJob { id: 7 }).await.unwrap(); // 重放：同一 payload

        assert!(
            wait_until(
                || ctx.calls.load(Ordering::SeqCst) >= 2,
                Duration::from_secs(5)
            )
            .await,
            "两个任务都应被消费"
        );
        // 去重：效果只发生一次
        assert_eq!(ctx.effects.load(Ordering::SeqCst), 1);
        handle.abort();
    }

    // ---- 场景 5b：终态行按保留期清理（统计台账不无限膨胀） ----

    #[cfg(feature = "pg")]
    #[sqlx::test]
    async fn finished_rows_deleted_after_retention(pool: PgPool) {
        let bus = JobBus::try_new_pg(pool).await.unwrap();

        // Done 行：入队 → 标记完成，保留期 0 天即应被删
        bus.enqueue(EchoJob { n: 1 }).await.unwrap();
        let done_id = latest_id(&bus, EchoJob::NAME).await.unwrap();
        mark_done(&bus, done_id).await.unwrap();

        // Pending 行：未执行，不应被清理
        bus.enqueue(EchoJob { n: 2 }).await.unwrap();
        let pending_id = latest_id(&bus, EchoJob::NAME).await.unwrap();

        let deleted = bus.delete_finished_older_than(0).await.unwrap();
        assert_eq!(deleted, 1, "只删 Done 终态行，Pending 保留");
        assert!(fetch_row(&bus, done_id).await.is_err(), "Done 行应已被删除");
        assert_eq!(fetch_row(&bus, pending_id).await.unwrap().status, "Pending");
    }

    // ---- 场景 5d：终态行超过单批上限时分批删净（大 DELETE 不长时间锁表） ----

    #[cfg(feature = "pg")]
    #[sqlx::test]
    async fn finished_rows_deleted_in_batches(pool: PgPool) {
        let bus = JobBus::try_new_pg(pool.clone()).await.unwrap();

        // 直插 GC_BATCH + 1 行 Done（保留期 0 天即应全删，跨两批循环）
        sqlx::query(
            "INSERT INTO worker_jobs (job_type, payload, status, done_at)
             SELECT 'echo_job', '{\"n\": 1}'::jsonb, 'Done', now() - interval '1 day'
               FROM generate_series(1, $1)",
        )
        .bind(crate::backend::GC_BATCH + 1)
        .execute(&pool)
        .await
        .unwrap();

        let deleted = bus.delete_finished_older_than(0).await.unwrap();
        assert_eq!(
            deleted,
            crate::backend::GC_BATCH as u64 + 1,
            "分批循环应删净全部终态行"
        );
        let remaining: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM worker_jobs WHERE status IN ('Done', 'Failed')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(remaining, 0, "终态行应全部删除");
    }

    // ---- 场景 6：同名冲突注册立即 panic（fail fast） ----

    #[test]
    #[should_panic(expected = "job name conflict")]
    fn duplicate_registration_panics() {
        let mut registry = JobRegistry::<TestCtx>::default();
        registry.register(|job, ctx| Box::pin(handle_echo(job, ctx)));
        registry.register(|job, ctx| Box::pin(handle_echo(job, ctx)));
    }

    // ---- 场景 7：sqlite 冒烟（enqueue → 消费 → Done + 延迟），仅 sqlite feature ----

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn sqlite_smoke_enqueue_consume_and_delay() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("worker.db");
        let pool = crate::sqlite_helper::new_sqlite_pool(path.to_str().unwrap())
            .await
            .unwrap();
        let bus = JobBus::try_new_sqlite(pool).await.unwrap();
        let ctx = TestCtx::default();
        let mut registry = JobRegistry::default();
        registry.register(|job, ctx| Box::pin(handle_echo(job, ctx)));
        let (_tx, handle) = spawn_worker(bus.clone(), ctx.clone(), &registry);

        // 立即
        bus.enqueue(EchoJob { n: 1 }).await.unwrap();
        let id = latest_id(&bus, EchoJob::NAME).await.unwrap();
        assert!(wait_status(&bus, id, "Done", Duration::from_secs(5)).await);
        assert_eq!(ctx.calls.load(Ordering::SeqCst), 1);

        // 延迟
        bus.enqueue_after(EchoJob { n: 2 }, Duration::from_millis(300))
            .await
            .unwrap();
        let id2 = latest_id(&bus, EchoJob::NAME).await.unwrap();
        sleep(Duration::from_millis(100)).await;
        assert_eq!(ctx.calls.load(Ordering::SeqCst), 1);
        assert!(wait_status(&bus, id2, "Done", Duration::from_secs(5)).await);
        assert_eq!(ctx.calls.load(Ordering::SeqCst), 2);
        handle.abort();
    }

    // ---- 场景 5c：sqlite 保留期清理（与 pg 同语义） ----

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn sqlite_finished_rows_deleted_after_retention() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("worker.db");
        let pool = crate::sqlite_helper::new_sqlite_pool(path.to_str().unwrap())
            .await
            .unwrap();
        let bus = JobBus::try_new_sqlite(pool).await.unwrap();

        // Done 行：入队 → 标记完成，保留期 0 天即应被删
        bus.enqueue(EchoJob { n: 1 }).await.unwrap();
        let done_id = latest_id(&bus, EchoJob::NAME).await.unwrap();
        mark_done(&bus, done_id).await.unwrap();
        // 推进时钟：sqlite 本地文件操作可能在同毫秒内完成，`done_at < cutoff` 要求时间差 > 0
        sleep(Duration::from_millis(5)).await;

        // Pending 行：未执行，不应被清理
        bus.enqueue(EchoJob { n: 2 }).await.unwrap();
        let pending_id = latest_id(&bus, EchoJob::NAME).await.unwrap();

        let deleted = bus.delete_finished_older_than(0).await.unwrap();
        assert_eq!(deleted, 1, "只删 Done 终态行，Pending 保留");
        assert!(fetch_row(&bus, done_id).await.is_err(), "Done 行应已被删除");
        assert_eq!(fetch_row(&bus, pending_id).await.unwrap().status, "Pending");
    }

    // ---- 场景 5e：sqlite 跨批删净（与 pg 同语义） ----

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn sqlite_finished_rows_deleted_in_batches() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("worker.db");
        let pool = crate::sqlite_helper::new_sqlite_pool(path.to_str().unwrap())
            .await
            .unwrap();
        let bus = JobBus::try_new_sqlite(pool.clone()).await.unwrap();

        // 直插 GC_BATCH + 1 行 Done（done_at 昨天，保留期 0 天即应全删，跨两批循环）
        let cutoff = (Utc::now() - chrono::Duration::days(1)).timestamp_millis();
        sqlx::query(
            "WITH RECURSIVE cnt(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM cnt WHERE x < ?)
             INSERT INTO worker_jobs (job_type, payload, status, done_at)
             SELECT 'echo_job', '{}', 'Done', ? FROM cnt",
        )
        .bind(crate::backend::GC_BATCH + 1)
        .bind(cutoff)
        .execute(&pool)
        .await
        .unwrap();

        let deleted = bus.delete_finished_older_than(0).await.unwrap();
        assert_eq!(
            deleted,
            crate::backend::GC_BATCH as u64 + 1,
            "分批循环应删净全部终态行"
        );
        let remaining: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM worker_jobs WHERE status IN ('Done', 'Failed')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(remaining, 0, "终态行应全部删除");
    }

    // ---- 场景 8：通知驱动——入队即唤醒，不依赖轮询间隔 ----

    #[cfg(feature = "pg")]
    #[sqlx::test]
    async fn pg_notify_wakes_worker_without_polling(pool: PgPool) {
        let bus = JobBus::try_new_pg(pool).await.unwrap();
        let ctx = TestCtx::default();
        let mut registry = JobRegistry::default();
        registry.register(|job, ctx| Box::pin(handle_echo(job, ctx)));
        // poll_interval 10s：只有通知路径（INSERT 触发器 NOTIFY → PgListener → Notify）
        // 能在 1s 内消费；靠轮询要等 10s。
        let manager = WorkerManager::new(bus.clone(), ctx.clone()).with_options(WorkerOptions {
            poll_interval: Duration::from_secs(10),
            ..fast_options()
        });
        let mut manager = manager;
        manager.register_all(&registry);
        let (tx, rx) = watch::channel(false);
        let handle = tokio::spawn(async move {
            manager.run(rx).await.unwrap();
        });
        let _tx = tx;
        // 等待 PgListener 完成 connect + listen（避免通知在 LISTEN 前丢失，poll 兜底仍保证最终消费）
        sleep(Duration::from_millis(300)).await;

        bus.enqueue(EchoJob { n: 1 }).await.unwrap();
        let id = latest_id(&bus, EchoJob::NAME).await.unwrap();
        assert!(wait_status(&bus, id, "Done", Duration::from_secs(1)).await);
        assert_eq!(ctx.calls.load(Ordering::SeqCst), 1);
        handle.abort();
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn sqlite_notify_wakes_worker_without_polling() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("worker.db");
        let pool = crate::sqlite_helper::new_sqlite_pool(path.to_str().unwrap())
            .await
            .unwrap();
        let bus = JobBus::try_new_sqlite(pool).await.unwrap();
        let ctx = TestCtx::default();
        let mut registry = JobRegistry::default();
        registry.register(|job, ctx| Box::pin(handle_echo(job, ctx)));
        // poll_interval 10s：只有 enqueue 直发的 Notify 能在 1s 内唤醒消费
        let manager = WorkerManager::new(bus.clone(), ctx.clone()).with_options(WorkerOptions {
            poll_interval: Duration::from_secs(10),
            ..fast_options()
        });
        let mut manager = manager;
        manager.register_all(&registry);
        let (tx, rx) = watch::channel(false);
        let handle = tokio::spawn(async move {
            manager.run(rx).await.unwrap();
        });
        let _tx = tx;

        bus.enqueue(EchoJob { n: 1 }).await.unwrap();
        let id = latest_id(&bus, EchoJob::NAME).await.unwrap();
        assert!(wait_status(&bus, id, "Done", Duration::from_secs(1)).await);
        assert_eq!(ctx.calls.load(Ordering::SeqCst), 1);
        handle.abort();
    }
}
