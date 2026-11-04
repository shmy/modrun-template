//! 后台任务队列：基于 [apalis](https://github.com/apalis-dev/apalis) 1.0 的薄封装。
//! 业务代码只接触 [`Job`] / [`JobBus`] / [`JobRegistry`]，不接触存储后端细节。
//!
//! - **at-least-once**：worker 心跳超时后孤儿任务会被重新投递（超时 =
//!   [`WorkerOptions::orphan_interval`] × missed，默认约 600s）——handler 必须幂等；
//! - **重试**：失败后 `attempts` 自增，未达 `max_attempts`（= `RETRIES + 1`）则按
//!   指数退避（默认 base 1s、×2、cap 60s，可通过 [`WorkerOptions`] 覆盖）写回 `run_at`
//!   并以 `RetryAfterError` 重新入队；`bump_run_at` 失败则 `AbortError`（→`Killed`）
//!   并 quarantine（钳 `max_attempts` + 推远 `run_at`），避免 `handle_result` 回写后
//!   tight loop / 假 Failed 泄漏；
//! - **超时**：单次执行超过 `Job::TIMEOUT` 按一次失败计（`job_timeout`）；
//! - **panic**：handler 内 `catch_unwind`，记一次失败（与超时同路径）；
//! - **终态**：重试耗尽 → `Failed`；不可恢复中止 → `Killed`；均可审计，由 GC 清理。
//!
//! pg 用 `LISTEN/NOTIFY`（`Pubsub`）+ 短间隔轮询兜底延迟任务；sqlite 用轮询
//! （单机部署）。

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )
)]

use std::io::Error;
use std::marker::PhantomData;
use std::result;
use tokio::task::JoinHandle;
pub mod error;

#[cfg(feature = "sqlite")]
pub mod sqlite_helper;

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use apalis::prelude::*;
use futures_util::future::{BoxFuture, FutureExt};
use rootcause::{Result, report};
use serde::{Serialize, de::DeserializeOwned};
use std::panic::AssertUnwindSafe;
use tokio::sync::watch;
use tokio::time::timeout;

#[cfg(feature = "pg")]
use apalis_postgres::{Config as PgConfig, PostgresStorage, Pubsub};
#[cfg(feature = "sqlite")]
use apalis_sqlite::{Config as SqliteConfig, SqliteStorage};
#[cfg(feature = "pg")]
use sqlx::PgPool;
use sqlx::Row as _;
#[cfg(feature = "sqlite")]
use sqlx::SqlitePool;

#[cfg(not(any(feature = "pg", feature = "sqlite")))]
compile_error!("job_queue requires feature \"pg\" or \"sqlite\"");

pub use crate::error::WorkerError;

/// 分批 GC 每批行数（大表下不长时间锁表）。
pub const GC_BATCH: i64 = 1000;

/// 后台任务定义：**payload 即类型本身**（无关联 type 间接层）。
///
/// 入队侧：`bus.enqueue(GenerateReport { ... })` —— 类型擦除发生在 `JobBus` 内部，
/// 业务代码不知道任何队列后端。
///
/// 配置项为编译期常量（可覆盖默认值），消费侧按 `job_type = NAME` 竞争消费：
/// - [`Job::RETRIES`]：失败后允许的追加重试次数（总执行次数上限 = `RETRIES + 1`）；
/// - [`Job::CONCURRENCY`]：该 job_type 的最大并行执行数；
/// - [`Job::TIMEOUT`]：单次执行超时（按一次失败计入重试）。
pub trait Job: Serialize + DeserializeOwned + Send + Sync + Unpin + 'static {
    /// 注册表键 + 队列路由键（`job_type` 列，蛇形命名，如 `generate_report`）。
    const NAME: &'static str;

    /// 失败后允许的追加重试次数（默认 3，总执行 ≤ 4 次）。
    const RETRIES: usize = 3;

    /// 该 job_type 的最大并行执行数（默认 1，串行）。
    const CONCURRENCY: usize = 1;

    /// 单次执行超时（默认 60s）。
    const TIMEOUT: Duration = Duration::from_secs(60);
}

/// 终态（Done / Failed / Killed）行的默认保留期（天）。超过后由 `JobQueueGc` 清理。
pub const DEFAULT_JOB_RETENTION_DAYS: i64 = 30;

/// 队列积压统计（供可观测性采样，见 [`JobBus::backlog`]）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QueueBacklog {
    pub pending: i64,
    pub running: i64,
    pub failed: i64,
}

/// 队列后端：`pg`（生产，apalis schema 在业务库）/ `sqlite`（单机部署，本地文件）。
#[derive(Clone)]
pub enum JobBus {
    #[cfg(feature = "pg")]
    Pg { pool: PgPool },
    #[cfg(feature = "sqlite")]
    Sqlite { pool: SqlitePool },
}

impl JobBus {
    /// pg 后端：跑 apalis-postgres 自带迁移（`apalis` schema / `apalis._sqlx_migrations`）。
    #[cfg(feature = "pg")]
    pub async fn try_new_pg(pool: PgPool) -> Result<Self> {
        PostgresStorage::<()>::setup(&pool)
            .await
            .map_err(|e| report!("apalis postgres setup: {e}"))?;
        Ok(Self::Pg { pool })
    }

    /// sqlite 后端：跑 apalis-sqlite 自带迁移（`Jobs` / `Workers` 表）。
    #[cfg(feature = "sqlite")]
    pub async fn try_new_sqlite(pool: SqlitePool) -> Result<Self> {
        SqliteStorage::<()>::setup(&pool)
            .await
            .map_err(|e| report!("apalis sqlite setup: {e}"))?;
        Ok(Self::Sqlite { pool })
    }

    /// 队列积压统计（观测采样）。
    pub async fn backlog(&self) -> Result<QueueBacklog> {
        match self {
            #[cfg(feature = "pg")]
            JobBus::Pg { pool } => {
                let row = sqlx::query(
                    "SELECT count(*) FILTER (WHERE status = 'Pending') AS pending,
                            count(*) FILTER (WHERE status = 'Running') AS running,
                            count(*) FILTER (
                                WHERE status = 'Killed'
                                   OR (status = 'Failed' AND attempts >= max_attempts)
                            ) AS failed
                     FROM apalis.jobs",
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
            JobBus::Sqlite { pool } => {
                let row = sqlx::query(
                    "SELECT count(*) FILTER (WHERE status = 'Pending') AS pending,
                            count(*) FILTER (WHERE status = 'Running') AS running,
                            count(*) FILTER (
                                WHERE status = 'Killed'
                                   OR (status = 'Failed' AND attempts >= max_attempts)
                            ) AS failed
                     FROM Jobs",
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

    /// 立即入队。
    pub async fn enqueue<T: Job>(&self, job: T) -> Result<()> {
        self.enqueue_after(job, Duration::ZERO).await
    }

    /// 延迟入队：`delay` 之后才可被消费。
    ///
    /// apalis 的 `run_at` 为 **unix 秒**；亚秒 delay 会向上取整到至少 1 秒，
    /// 避免 `run_after(500ms)` 被截成同一秒而立刻可跑。
    pub async fn enqueue_after<T: Job>(&self, job: T, delay: Duration) -> Result<()> {
        let max_attempts = T::RETRIES + 1;
        let task = TaskBuilder::new(job)
            .run_at_timestamp(schedule_run_at_secs(delay))
            .max_attempts(max_attempts)
            .build();
        match self {
            #[cfg(feature = "pg")]
            // Storage::new 仅包一层 pool+config，热路径可接受；不做 per-queue 缓存。
            JobBus::Pg { pool } => {
                let mut storage = PostgresStorage::new(pool)
                    .with_config(PgConfig::default().queue(T::NAME).persist_results(true));
                storage
                    .push_task(task)
                    .await
                    .map_err(|e| report!("enqueue {}: {e}", T::NAME))?;
            }
            #[cfg(feature = "sqlite")]
            JobBus::Sqlite { pool } => {
                let mut storage = SqliteStorage::new(pool)
                    .with_config(SqliteConfig::default().queue(T::NAME).persist_results(true));
                storage
                    .push_task(task)
                    .await
                    .map_err(|e| report!("enqueue {}: {e}", T::NAME))?;
            }
        }
        Ok(())
    }

    /// 清理超过保留期的终态行（Done / Killed / 耗尽重试的 Failed），返回删除行数。
    ///
    /// 分批循环删除（每批 [`GC_BATCH`] 行），大表下不长时间锁表。
    pub async fn delete_finished_older_than(&self, retention_days: i64) -> Result<u64> {
        let mut total = 0u64;
        loop {
            let deleted = match self {
                #[cfg(feature = "pg")]
                JobBus::Pg { pool } => delete_finished_pg(pool, retention_days).await?,
                #[cfg(feature = "sqlite")]
                JobBus::Sqlite { pool } => delete_finished_sqlite(pool, retention_days).await?,
            };
            total += deleted;
            if deleted < GC_BATCH as u64 {
                break;
            }
        }
        Ok(total)
    }
}

#[cfg(feature = "pg")]
async fn delete_finished_pg(pool: &PgPool, retention_days: i64) -> Result<u64> {
    let result = sqlx::query(
        "WITH doomed AS (
             SELECT id FROM apalis.jobs
              WHERE (
                    status IN ('Done', 'Killed')
                    OR (status = 'Failed' AND attempts >= max_attempts)
                )
                AND done_at IS NOT NULL
                AND done_at < now() - make_interval(days => $1::int)
              ORDER BY done_at
              LIMIT $2
              FOR UPDATE SKIP LOCKED
         )
         DELETE FROM apalis.jobs j
          USING doomed
          WHERE j.id = doomed.id",
    )
    .bind(retention_days as i32)
    .bind(GC_BATCH)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

#[cfg(feature = "sqlite")]
async fn delete_finished_sqlite(pool: &SqlitePool, retention_days: i64) -> Result<u64> {
    // sqlite `Jobs.run_at` / `done_at` 为 unix 秒。
    let cutoff = (chrono::Utc::now() - chrono::Duration::days(retention_days)).timestamp();
    let result = sqlx::query(
        "DELETE FROM Jobs
          WHERE id IN (
              SELECT id FROM Jobs
               WHERE (
                     status IN ('Done', 'Killed')
                     OR (status = 'Failed' AND attempts >= max_attempts)
                 )
                 AND done_at IS NOT NULL
                 AND done_at < ?
               ORDER BY done_at
               LIMIT ?
          )",
    )
    .bind(cutoff)
    .bind(GC_BATCH)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

#[cfg(feature = "pg")]
async fn bump_run_at_pg(pool: &PgPool, task_id: String, delay: Duration) -> Result<()> {
    // 用秒的浮点间隔，保留亚秒退避（`as_secs()` 会把 <1s 截成 0）。
    let secs = delay.as_secs_f64();
    let result = sqlx::query(
        "UPDATE apalis.jobs
            SET run_at = now() + ($1::double precision * interval '1 second'),
                done_at = NULL
          WHERE id = $2",
    )
    .bind(secs)
    .bind(task_id)
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(report!("bump_run_at: no row updated"));
    }
    Ok(())
}

#[cfg(feature = "sqlite")]
async fn bump_run_at_sqlite(pool: &SqlitePool, task_id: String, delay: Duration) -> Result<()> {
    // sqlite `run_at` 为整秒；delay>0 时至少推到下一秒，避免同秒立即重捞。
    let now = chrono::Utc::now();
    let target = now + chrono::Duration::milliseconds(delay.as_millis() as i64);
    let mut run_at = target.timestamp();
    if delay > Duration::ZERO && run_at <= now.timestamp() {
        run_at = now.timestamp() + 1;
    }
    let result = sqlx::query(
        "UPDATE Jobs
            SET run_at = ?,
                done_at = NULL
          WHERE id = ?",
    )
    .bind(run_at)
    .bind(task_id)
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(report!("bump_run_at: no row updated"));
    }
    Ok(())
}

async fn bump_run_at(bus: &JobBus, task_id: String, delay: Duration) -> Result<()> {
    match bus {
        #[cfg(feature = "pg")]
        JobBus::Pg { pool } => bump_run_at_pg(pool, task_id, delay).await,
        #[cfg(feature = "sqlite")]
        JobBus::Sqlite { pool } => bump_run_at_sqlite(pool, task_id, delay).await,
    }
}

/// bump 失败时的安全阀（与返回 [`AbortError`] 双保险）。
///
/// apalis `handle_result` 会把 `attempts` 写回当前 attempt。把 `max_attempts`
/// 钳到该值后，即便最终落成 `Failed` 而非 `Killed`，也满足
/// `attempts >= max_attempts`（可进 GC / backlog.failed，且不被 `get_jobs` 重捞）。
/// 同时推远 `run_at`，防止 quarantine 与 persist 之间的窗口被立刻捞起。
async fn quarantine_unretryable(
    bus: &JobBus,
    task_id: &str,
    current_attempts: usize,
) -> Result<()> {
    let attempts = current_attempts.max(1) as i32;
    match bus {
        #[cfg(feature = "pg")]
        JobBus::Pg { pool } => {
            let result = sqlx::query(
                "UPDATE apalis.jobs
                    SET max_attempts = $1,
                        run_at = now() + interval '100 years',
                        done_at = COALESCE(done_at, now())
                  WHERE id = $2",
            )
            .bind(attempts)
            .bind(task_id)
            .execute(pool)
            .await?;
            if result.rows_affected() == 0 {
                return Err(report!("quarantine_unretryable: no row updated"));
            }
        }
        #[cfg(feature = "sqlite")]
        JobBus::Sqlite { pool } => {
            let now = chrono::Utc::now().timestamp();
            let far = now + 100 * 365 * 24 * 60 * 60;
            let result = sqlx::query(
                "UPDATE Jobs
                    SET max_attempts = ?,
                        run_at = ?,
                        done_at = COALESCE(done_at, ?)
                  WHERE id = ?",
            )
            .bind(attempts)
            .bind(far)
            .bind(now)
            .bind(task_id)
            .execute(pool)
            .await?;
            if result.rows_affected() == 0 {
                return Err(report!("quarantine_unretryable: no row updated"));
            }
        }
    }
    Ok(())
}

/// 将 [`WorkerOptions`] 的孤儿超时映射为 apalis `heartbeat × missed`。
fn orphan_heartbeat_config(options: &WorkerOptions) -> (Duration, usize) {
    let heartbeat = options.orphan_interval.max(Duration::from_millis(100));
    let hb_secs = heartbeat.as_secs_f64().max(0.1);
    let missed = (options.orphan_timeout.as_secs_f64() / hb_secs)
        .ceil()
        .max(1.0) as usize;
    (heartbeat, missed)
}

/// 消费侧注册表（泛型上下文；组合根实例化为 `JobRegistry<WebState>` 等）。
pub struct JobRegistry<C> {
    spawners: Vec<Arc<dyn JobSpawner<C>>>,
}

impl<C> Clone for JobRegistry<C> {
    fn clone(&self) -> Self {
        Self {
            spawners: self.spawners.clone(),
        }
    }
}

impl<C: Send + Sync + 'static> Default for JobRegistry<C> {
    fn default() -> Self {
        Self {
            spawners: Vec::new(),
        }
    }
}

impl<C: Clone + Send + Sync + 'static> JobRegistry<C> {
    /// 注册一个 job 的消费 handler。
    ///
    /// 调用方以闭包形态提供（`C: Clone`，future 须 `'static`，以适配 apalis
    /// `parallelize`）：
    /// `register(|job, ctx| Box::pin(async move { handle(job, &ctx).await }))`。
    ///
    /// **同名冲突是编程错误**，注册时立即 panic（fail fast）。
    pub fn register<T, F>(&mut self, handler: F) -> &mut Self
    where
        T: Job,
        F: Fn(T, C) -> BoxFuture<'static, Result<()>> + Send + Sync + 'static,
    {
        assert!(
            !self.spawners.iter().any(|s| s.name() == T::NAME),
            "job name conflict: {} is already registered",
            T::NAME
        );
        self.spawners.push(Arc::new(TypedSpawner {
            handler: Arc::new(handler),
            _marker: PhantomData::<(T, C)>,
        }));
        self
    }

    pub fn merge(&mut self, other: &JobRegistry<C>) {
        for spawner in &other.spawners {
            let name = spawner.name();
            assert!(
                !self.spawners.iter().any(|s| s.name() == name),
                "job name conflict: {name} is already registered"
            );
            self.spawners.push(spawner.clone());
        }
    }

    fn spawners(&self) -> &[Arc<dyn JobSpawner<C>>] {
        &self.spawners
    }
}

/// 终态失败回调：入参为 `job_type`。
pub type PermanentFailureHook = Arc<dyn Fn(&str) + Send + Sync>;

/// 消费侧运行时参数（测试可覆盖以加速）。
#[derive(Clone)]
pub struct WorkerOptions {
    /// 轮询间隔（默认 200ms；延迟投递的最小精度即此值；兼作 NOTIFY 兜底）。
    pub poll_interval: Duration,
    /// 退避基数（默认 1s；第 n 次失败后延迟 = `base * 2^(n-1)`）。
    pub backoff_base: Duration,
    /// 退避封顶（默认 60s）。
    pub backoff_cap: Duration,
    /// 心跳间隔（写入 apalis `Config::heartbeat_interval`；默认 60s）。
    pub orphan_interval: Duration,
    /// 孤儿超时目标（≈ `orphan_interval × missed_heartbeats`；默认 600s）。
    pub orphan_timeout: Duration,
    /// job 进入不可恢复终态（重试耗尽 `Failed` / bump 失败 `Killed`）时的回调。
    pub on_permanent_failure: Option<PermanentFailureHook>,
}

impl fmt::Debug for WorkerOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkerOptions")
            .field("poll_interval", &self.poll_interval)
            .field("backoff_base", &self.backoff_base)
            .field("backoff_cap", &self.backoff_cap)
            .field("orphan_interval", &self.orphan_interval)
            .field("orphan_timeout", &self.orphan_timeout)
            .field("on_permanent_failure", &self.on_permanent_failure.is_some())
            .finish()
    }
}

impl Default for WorkerOptions {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_millis(200),
            backoff_base: Duration::from_secs(1),
            backoff_cap: Duration::from_secs(60),
            orphan_interval: Duration::from_secs(60),
            orphan_timeout: Duration::from_secs(600),
            on_permanent_failure: None,
        }
    }
}

impl WorkerOptions {
    /// 注入终态失败回调（组合根用它接 metrics / 告警）。
    pub fn with_permanent_failure_hook(
        mut self,
        hook: impl Fn(&str) + Send + Sync + 'static,
    ) -> Self {
        self.on_permanent_failure = Some(Arc::new(hook));
        self
    }
}

/// 消费侧运行时：持有后端 + 上下文，`register_all` 收编注册表后 `run` 启动
/// 每 job_type 一个 apalis worker（进程内，随 server 生命周期）。
pub struct WorkerManager<C> {
    bus: JobBus,
    ctx: C,
    spawners: Vec<Arc<dyn JobSpawner<C>>>,
    options: WorkerOptions,
}

impl<C: Clone + Send + Sync + 'static> WorkerManager<C> {
    pub fn new(bus: JobBus, ctx: C) -> Self {
        Self {
            bus,
            ctx,
            spawners: Vec::new(),
            options: WorkerOptions::default(),
        }
    }

    pub fn with_options(mut self, options: WorkerOptions) -> Self {
        self.options = options;
        self
    }

    /// 收编注册表（clone 语义，注册表仍可复用）。
    pub fn register_all(&mut self, registry: &JobRegistry<C>) {
        self.spawners.extend(registry.spawners().iter().cloned());
    }

    /// 启动消费（每 job_type 一个 apalis worker），`shutdown` 为 true 时优雅退出。
    pub async fn run(self, shutdown: watch::Receiver<bool>) -> Result<()> {
        let mut handles = Vec::with_capacity(self.spawners.len());
        for spawner in self.spawners {
            let bus = self.bus.clone();
            let ctx = self.ctx.clone();
            let options = self.options.clone();
            let rx = shutdown.clone();
            handles.push(spawner.spawn(bus, ctx, options, rx));
        }
        for handle in handles {
            handle
                .await
                .map_err(|e| report!("worker task join failed: {e}"))?
                .map_err(|e| report!("worker stopped with error: {e}"))?;
        }
        Ok(())
    }
}

trait JobSpawner<C>: Send + Sync {
    fn name(&self) -> &'static str;
    fn spawn(
        &self,
        bus: JobBus,
        ctx: C,
        options: WorkerOptions,
        shutdown: watch::Receiver<bool>,
    ) -> JoinHandle<Result<()>>;
}

struct TypedSpawner<T: Job, C, F> {
    handler: Arc<F>,
    _marker: PhantomData<(T, C)>,
}

async fn wait_shutdown(mut shutdown: watch::Receiver<bool>) -> result::Result<(), Error> {
    while !*shutdown.borrow() {
        if shutdown.changed().await.is_err() {
            break;
        }
    }
    Ok(())
}

trait ErasedJobHandler<T, C>: Send + Sync {
    fn call(&self, job: T, ctx: C) -> BoxFuture<'static, Result<()>>;
}

impl<T, C, F> ErasedJobHandler<T, C> for F
where
    F: Fn(T, C) -> BoxFuture<'static, Result<()>> + Send + Sync,
{
    fn call(&self, job: T, ctx: C) -> BoxFuture<'static, Result<()>> {
        (self)(job, ctx)
    }
}

struct JobExec<T, C> {
    handler: Arc<dyn ErasedJobHandler<T, C>>,
    bus: JobBus,
    hook: Option<PermanentFailureHook>,
    job_timeout: Duration,
    max_attempts: usize,
    backoff_base: Duration,
    backoff_cap: Duration,
}

impl<T, C> Clone for JobExec<T, C> {
    fn clone(&self) -> Self {
        Self {
            handler: self.handler.clone(),
            bus: self.bus.clone(),
            hook: self.hook.clone(),
            job_timeout: self.job_timeout,
            max_attempts: self.max_attempts,
            backoff_base: self.backoff_base,
            backoff_cap: self.backoff_cap,
        }
    }
}

async fn apalis_handle<T, C>(
    job: T,
    exec: Data<JobExec<T, C>>,
    ctx: Data<C>,
    attempt: Attempt,
    task_id: TaskId,
) -> result::Result<(), BoxDynError>
where
    T: Job,
    C: Clone + Send + Sync + 'static,
{
    // 在 handler 内 catch_unwind：等价 catch_panic，且避开 apalis CatchPanicLayer 的 HRTB。
    let outcome = timeout(
        exec.job_timeout,
        AssertUnwindSafe(exec.handler.call(job, (*ctx).clone())).catch_unwind(),
    )
    .await;
    let err: String = match outcome {
        Ok(Ok(Ok(()))) => return Ok(()),
        Ok(Ok(Err(e))) => format!("{e}"),
        Ok(Err(_panic)) => "job_panicked".to_string(),
        Err(_) => WorkerError::Timeout.to_string(),
    };
    let attempts = attempt.current();
    if attempts < exec.max_attempts {
        let delay = backoff_delay(attempts, exec.backoff_base, exec.backoff_cap);
        tracing::warn!(
            job_type = T::NAME,
            task_id = %task_id,
            attempts,
            error = %err,
            delay_ms = delay.as_millis() as u64,
            "job failed, will retry"
        );
        match bump_run_at(&exec.bus, task_id.to_string(), delay).await {
            Ok(()) => return Err(Box::new(RetryAfterError::new(err, delay))),
            Err(bump_err) => {
                tracing::error!(
                    job_type = T::NAME,
                    task_id = %task_id,
                    error = %bump_err,
                    original = %err,
                    "bump run_at for retry failed; quarantining as unretryable"
                );
                if let Err(e) =
                    quarantine_unretryable(&exec.bus, &task_id.to_string(), attempts).await
                {
                    tracing::error!(
                        job_type = T::NAME,
                        task_id = %task_id,
                        error = %e,
                        "quarantine after bump failure also failed; still AbortError"
                    );
                }
                if let Some(hook) = &exec.hook {
                    hook(T::NAME);
                }
                // AbortError → Killed：不被 get_jobs 重捞，可被 GC；quarantine 失败时也不回退到可重试 Failed。
                return Err(Box::new(AbortError::new(format!(
                    "bump_run_at failed ({bump_err}); original: {err}"
                ))));
            }
        }
    }
    tracing::error!(
        job_type = T::NAME,
        task_id = %task_id,
        attempts,
        error = %err,
        "job failed permanently"
    );
    if let Some(hook) = &exec.hook {
        hook(T::NAME);
    }
    Err(Box::new(Error::other(err)))
}

impl<T, C, F> JobSpawner<C> for TypedSpawner<T, C, F>
where
    T: Job,
    C: Clone + Send + Sync + 'static,
    F: Fn(T, C) -> BoxFuture<'static, Result<()>> + Send + Sync + 'static,
{
    fn name(&self) -> &'static str {
        T::NAME
    }

    fn spawn(
        &self,
        bus: JobBus,
        ctx: C,
        options: WorkerOptions,
        shutdown: watch::Receiver<bool>,
    ) -> JoinHandle<Result<()>> {
        let handler = self.handler.clone() as Arc<dyn ErasedJobHandler<T, C>>;
        tokio::spawn(async move {
            let concurrency = T::CONCURRENCY.max(1);
            let poll_interval = options.poll_interval;
            let exec = JobExec {
                handler,
                bus: bus.clone(),
                hook: options.on_permanent_failure.clone(),
                job_timeout: T::TIMEOUT,
                max_attempts: T::RETRIES + 1,
                backoff_base: options.backoff_base,
                backoff_cap: options.backoff_cap,
            };

            let worker_ctx = WorkerContext::new(T::NAME);
            let stop_ctx = worker_ctx.clone();
            let stopper = tokio::spawn(async move {
                let _ = wait_shutdown(shutdown).await;
                let _ = stop_ctx.stop();
            });

            let (heartbeat, missed) = orphan_heartbeat_config(&options);
            let run_result = match &bus {
                #[cfg(feature = "pg")]
                JobBus::Pg { pool } => {
                    let config = PgConfig::default()
                        .queue(T::NAME)
                        .batch_size((concurrency * 2).max(5))
                        .persist_results(true)
                        .heartbeat_interval(heartbeat)
                        .missed_heartbeats(missed);
                    let backend = PostgresStorage::new(pool)
                        .with_config(config)
                        .poll_with_strategy(Strategy::new().interval(poll_interval).apply(
                            StreamStrategy::new(Pubsub::new(pool.clone(), T::NAME.to_string())),
                        ));
                    // concurrency：CallAll 对 async handler 可协作并行。
                    // 不挂 parallelize/CatchPanicLayer：与 typed `BoxDynError` handler 组合会触发 HRTB。
                    let worker = WorkerBuilder::new(worker_ctx.clone())
                        .backend(backend)
                        .data(ctx)
                        .data(exec)
                        .concurrency(concurrency)
                        .build(apalis_handle::<T, C>);
                    worker.run().await
                }
                #[cfg(feature = "sqlite")]
                JobBus::Sqlite { pool } => {
                    let config = SqliteConfig::default()
                        .queue(T::NAME)
                        .batch_size((concurrency * 2).max(5))
                        .persist_results(true)
                        .heartbeat_interval(heartbeat)
                        .missed_heartbeats(missed);
                    let backend = SqliteStorage::new(pool)
                        .with_config(config)
                        .poll_with_strategy(Strategy::new().interval(poll_interval));
                    let worker = WorkerBuilder::new(worker_ctx.clone())
                        .backend(backend)
                        .data(ctx)
                        .data(exec)
                        .concurrency(concurrency)
                        .build(apalis_handle::<T, C>);
                    worker.run().await
                }
            };
            stopper.abort();
            run_result.map_err(|e| report!("apalis worker {}: {e}", T::NAME))?;
            Ok(())
        })
    }
}

fn backoff_delay(attempt: usize, base: Duration, cap: Duration) -> Duration {
    let exp = 1u32 << (attempt.saturating_sub(1) as u32).min(30);
    base.saturating_mul(exp).min(cap)
}

/// apalis `run_at` 只有秒精度：对 `delay` 做整秒向上取整（`1.5s` → `+2`，亚秒 → `+1`）。
fn ceil_delay_secs(delay: Duration) -> u64 {
    if delay.is_zero() {
        return 0;
    }
    (delay.as_secs() + u64::from(delay.subsec_nanos() > 0)).max(1)
}

fn schedule_run_at_secs(delay: Duration) -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    now.saturating_add(ceil_delay_secs(delay))
}

/// 供测试按 id 断言用的表查询入口。
#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    pub(crate) async fn latest_id(bus: &JobBus, job_type: &str) -> Result<String> {
        match bus {
            #[cfg(feature = "pg")]
            JobBus::Pg { pool } => {
                let row = sqlx::query(
                    "SELECT id FROM apalis.jobs WHERE job_type = $1 ORDER BY run_at DESC, id DESC LIMIT 1",
                )
                .bind(job_type)
                .fetch_one(pool)
                .await?;
                Ok(row.get::<String, _>("id"))
            }
            #[cfg(feature = "sqlite")]
            JobBus::Sqlite { pool } => {
                let row = sqlx::query(
                    "SELECT id FROM Jobs WHERE job_type = ? ORDER BY run_at DESC, id DESC LIMIT 1",
                )
                .bind(job_type)
                .fetch_one(pool)
                .await?;
                Ok(row.get::<String, _>("id"))
            }
        }
    }

    pub(crate) struct JobRow {
        pub status: String,
        pub attempts: i32,
        pub max_attempts: i32,
        #[cfg(feature = "pg")]
        pub run_at: chrono::DateTime<chrono::Utc>,
        #[cfg(feature = "sqlite")]
        pub run_at: i64,
    }

    pub(crate) async fn fetch_row(bus: &JobBus, id: &str) -> Result<JobRow> {
        match bus {
            #[cfg(feature = "pg")]
            JobBus::Pg { pool } => {
                let row = sqlx::query(
                    "SELECT status, attempts, max_attempts, run_at FROM apalis.jobs WHERE id = $1",
                )
                .bind(id)
                .fetch_one(pool)
                .await?;
                Ok(JobRow {
                    status: row.get("status"),
                    attempts: row.get("attempts"),
                    max_attempts: row.get("max_attempts"),
                    run_at: row.get("run_at"),
                })
            }
            #[cfg(feature = "sqlite")]
            JobBus::Sqlite { pool } => {
                let row = sqlx::query(
                    "SELECT status, attempts, max_attempts, run_at FROM Jobs WHERE id = ?",
                )
                .bind(id)
                .fetch_one(pool)
                .await?;
                Ok(JobRow {
                    status: row.get("status"),
                    attempts: row.get("attempts"),
                    max_attempts: row.get("max_attempts"),
                    run_at: row.get("run_at"),
                })
            }
        }
    }

    pub(crate) async fn mark_done(bus: &JobBus, id: &str) -> Result<()> {
        match bus {
            #[cfg(feature = "pg")]
            JobBus::Pg { pool } => {
                sqlx::query(
                    "UPDATE apalis.jobs SET status = 'Done', done_at = now(), attempts = GREATEST(attempts, 1) WHERE id = $1",
                )
                .bind(id)
                .execute(pool)
                .await?;
            }
            #[cfg(feature = "sqlite")]
            JobBus::Sqlite { pool } => {
                let now = chrono::Utc::now().timestamp();
                sqlx::query(
                    "UPDATE Jobs SET status = 'Done', done_at = ?, attempts = MAX(attempts, 1) WHERE id = ?",
                )
                .bind(now)
                .bind(id)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{fetch_row, latest_id, mark_done};
    use super::*;
    use serde::{Deserialize, Serialize};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::task::JoinHandle;
    use tokio::time::Instant;
    use tokio::time::sleep;

    #[cfg(feature = "pg")]
    use std::collections::HashSet;
    #[cfg(feature = "pg")]
    use std::sync::Mutex;

    #[derive(Debug, Serialize, Deserialize, Clone)]
    struct EchoJob {
        n: u32,
    }
    impl Job for EchoJob {
        const NAME: &'static str = "test_echo";
        const CONCURRENCY: usize = 4;
    }

    // 以下 job / handler 仅被 `feature = "pg"` 集成测使用；sqlite 分趟编译时不编入，避免 dead_code。
    #[cfg(feature = "pg")]
    #[derive(Debug, Serialize, Deserialize)]
    struct FlakyJob {
        n: u32,
    }
    #[cfg(feature = "pg")]
    impl Job for FlakyJob {
        const NAME: &'static str = "test_flaky";
        const RETRIES: usize = 2;
        const TIMEOUT: Duration = Duration::from_secs(5);
    }

    #[cfg(feature = "pg")]
    #[derive(Debug, Serialize, Deserialize)]
    struct AlwaysFailJob {
        n: u32,
    }
    #[cfg(feature = "pg")]
    impl Job for AlwaysFailJob {
        const NAME: &'static str = "test_always_fail";
        const RETRIES: usize = 1;
        const TIMEOUT: Duration = Duration::from_secs(5);
    }

    #[cfg(feature = "pg")]
    #[derive(Debug, Serialize, Deserialize)]
    struct ParallelJob {
        n: u32,
    }
    #[cfg(feature = "pg")]
    impl Job for ParallelJob {
        const NAME: &'static str = "test_parallel";
        const CONCURRENCY: usize = 4;
        const TIMEOUT: Duration = Duration::from_secs(5);
    }

    #[cfg(feature = "pg")]
    #[derive(Debug, Serialize, Deserialize)]
    struct SlowJob {
        n: u32,
    }
    #[cfg(feature = "pg")]
    impl Job for SlowJob {
        const NAME: &'static str = "test_slow";
        const RETRIES: usize = 0;
        const TIMEOUT: Duration = Duration::from_millis(100);
    }

    #[cfg(feature = "pg")]
    #[derive(Debug, Serialize, Deserialize)]
    struct DedupJob {
        id: u32,
    }
    #[cfg(feature = "pg")]
    impl Job for DedupJob {
        const NAME: &'static str = "test_dedup";
        const CONCURRENCY: usize = 2;
    }

    #[derive(Clone, Default)]
    struct TestCtx {
        calls: Arc<AtomicUsize>,
        #[cfg(feature = "pg")]
        fail_until: Arc<AtomicUsize>,
        #[cfg(feature = "pg")]
        seen: Arc<Mutex<HashSet<u32>>>,
        #[cfg(feature = "pg")]
        effects: Arc<AtomicUsize>,
    }

    async fn handle_echo(_job: EchoJob, ctx: &TestCtx) -> Result<()> {
        ctx.calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    #[cfg(feature = "pg")]
    async fn handle_flaky(_job: FlakyJob, ctx: &TestCtx) -> Result<()> {
        let call = ctx.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if call <= ctx.fail_until.load(Ordering::SeqCst) {
            return Err(report!("flaky_failure"));
        }
        Ok(())
    }

    #[cfg(feature = "pg")]
    async fn handle_always_fail(_job: AlwaysFailJob, ctx: &TestCtx) -> Result<()> {
        ctx.calls.fetch_add(1, Ordering::SeqCst);
        Err(report!("always_fail"))
    }

    #[cfg(feature = "pg")]
    async fn handle_parallel(_job: ParallelJob, ctx: &TestCtx) -> Result<()> {
        ctx.calls.fetch_add(1, Ordering::SeqCst);
        sleep(Duration::from_millis(200)).await;
        ctx.effects.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    #[cfg(feature = "pg")]
    async fn handle_slow(_job: SlowJob, _ctx: &TestCtx) -> Result<()> {
        sleep(Duration::from_secs(30)).await;
        Ok(())
    }

    #[cfg(feature = "pg")]
    async fn handle_dedup(job: DedupJob, ctx: &TestCtx) -> Result<()> {
        ctx.calls.fetch_add(1, Ordering::SeqCst);
        if ctx.seen.lock().unwrap().insert(job.id) {
            ctx.effects.fetch_add(1, Ordering::SeqCst);
        }
        Ok(())
    }

    fn fast_options() -> WorkerOptions {
        WorkerOptions {
            poll_interval: Duration::from_millis(50),
            backoff_base: Duration::from_millis(50),
            backoff_cap: Duration::from_millis(200),
            orphan_interval: Duration::from_secs(3600),
            orphan_timeout: Duration::from_secs(3600),
            ..Default::default()
        }
    }

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

    async fn wait_status(bus: &JobBus, id: &str, expected: &str, timeout: Duration) -> bool {
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

    #[cfg(feature = "pg")]
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

    /// 轮询到 DB 时钟 `now() >= target`（对齐 apalis 用库时钟判 due，避免 host/DB 偏差）。
    #[cfg(feature = "pg")]
    async fn wait_pg_now_at_least(
        pool: &PgPool,
        target: chrono::DateTime<chrono::Utc>,
        timeout: Duration,
    ) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            let now: chrono::DateTime<chrono::Utc> = sqlx::query_scalar("SELECT now()")
                .fetch_one(pool)
                .await
                .expect("SELECT now()");
            if now >= target {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            sleep(Duration::from_millis(20)).await;
        }
    }

    /// sqlite `strftime('%s','now')` 整秒时钟，语义同 [`wait_pg_now_at_least`]。
    #[cfg(feature = "sqlite")]
    async fn wait_sqlite_now_at_least(pool: &sqlx::SqlitePool, target: i64, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            let now: i64 = sqlx::query_scalar("SELECT CAST(strftime('%s','now') AS INTEGER)")
                .fetch_one(pool)
                .await
                .expect("sqlite now");
            if now >= target {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            sleep(Duration::from_millis(20)).await;
        }
    }

    #[cfg(feature = "pg")]
    #[sqlx::test]
    async fn backlog_counts_pending_running_failed(pool: PgPool) {
        let bus = JobBus::try_new_pg(pool).await.unwrap();
        assert_eq!(bus.backlog().await.unwrap(), QueueBacklog::default());

        bus.enqueue(EchoJob { n: 1 }).await.unwrap();
        let backlog = bus.backlog().await.unwrap();
        assert_eq!(backlog.pending, 1);
        assert_eq!(backlog.running, 0);
        assert_eq!(backlog.failed, 0);
    }

    #[cfg(feature = "pg")]
    #[sqlx::test]
    async fn backlog_counts_killed_as_failed(pool: PgPool) {
        let bus = JobBus::try_new_pg(pool.clone()).await.unwrap();
        bus.enqueue(EchoJob { n: 1 }).await.unwrap();
        let id = latest_id(&bus, EchoJob::NAME).await.unwrap();
        sqlx::query(
            "UPDATE apalis.jobs
                SET status = 'Killed', attempts = 1, done_at = now()
              WHERE id = $1",
        )
        .bind(&id)
        .execute(&pool)
        .await
        .unwrap();
        let backlog = bus.backlog().await.unwrap();
        assert_eq!(backlog.pending, 0);
        assert_eq!(backlog.failed, 1, "Killed 应计入 backlog.failed");
    }

    /// bump 因行消失失败 → AbortError → 无行可写 Killed；用「锁行后改坏 id 引用」不可行。
    /// 改为：quarantine 后 worker 路径外直接验证 AbortError 终态语义——
    /// 将行标为 Killed（与 AbortError→handle_result 一致）且不可被 get_jobs 重捞、可 GC。
    #[cfg(feature = "pg")]
    #[sqlx::test]
    async fn killed_terminal_not_refetched_and_gcable(pool: PgPool) {
        let bus = JobBus::try_new_pg(pool.clone()).await.unwrap();
        bus.enqueue(EchoJob { n: 1 }).await.unwrap();
        let id = latest_id(&bus, EchoJob::NAME).await.unwrap();
        sqlx::query(
            "UPDATE apalis.jobs
                SET status = 'Killed',
                    attempts = 1,
                    max_attempts = 1,
                    run_at = now() - interval '1 hour',
                    done_at = now() - interval '1 day'
              WHERE id = $1",
        )
        .bind(&id)
        .execute(&pool)
        .await
        .unwrap();

        let eligible: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM apalis.jobs
              WHERE id = $1
                AND (
                    status = 'Pending'
                    OR (status = 'Failed' AND attempts < max_attempts)
                )
                AND run_at < now()",
        )
        .bind(&id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(eligible, 0, "Killed 不可被 get_jobs 重捞");
        assert_eq!(bus.backlog().await.unwrap().failed, 1);
        assert_eq!(bus.delete_finished_older_than(0).await.unwrap(), 1);
        assert!(fetch_row(&bus, &id).await.is_err());
    }

    #[cfg(feature = "pg")]
    #[sqlx::test]
    async fn immediate_enqueue_consumed_and_done(pool: PgPool) {
        let bus = JobBus::try_new_pg(pool).await.unwrap();
        let ctx = TestCtx::default();
        let mut registry = JobRegistry::default();
        registry.register(|job, ctx| Box::pin(async move { handle_echo(job, &ctx).await }));
        let (_tx, handle) = spawn_worker(bus.clone(), ctx.clone(), &registry);

        bus.enqueue(EchoJob { n: 1 }).await.unwrap();
        let id = latest_id(&bus, EchoJob::NAME).await.unwrap();

        assert!(wait_status(&bus, &id, "Done", Duration::from_secs(5)).await);
        assert_eq!(ctx.calls.load(Ordering::SeqCst), 1);
        handle.abort();
    }

    #[cfg(feature = "pg")]
    #[sqlx::test]
    async fn delayed_enqueue_not_consumed_until_due(pool: PgPool) {
        let bus = JobBus::try_new_pg(pool.clone()).await.unwrap();
        let ctx = TestCtx::default();
        let mut registry = JobRegistry::default();
        registry.register(|job, ctx| Box::pin(async move { handle_echo(job, &ctx).await }));

        // 先入队再启 worker；delay ≥1s（apalis run_at 仅秒精度）。
        bus.enqueue_after(EchoJob { n: 1 }, Duration::from_secs(2))
            .await
            .unwrap();
        let id = latest_id(&bus, EchoJob::NAME).await.unwrap();
        let row = fetch_row(&bus, &id).await.unwrap();
        let due = row.run_at;
        assert_eq!(row.status, "Pending");
        assert_eq!(ctx.calls.load(Ordering::SeqCst), 0);

        let (_tx, handle) = spawn_worker(bus.clone(), ctx.clone(), &registry);

        // 到期前：用 PG now() 采样到 due 前 250ms（已更近则直接采），确认尚未消费。
        let sample_before = due - chrono::Duration::milliseconds(250);
        assert!(
            wait_pg_now_at_least(&pool, sample_before, Duration::from_secs(5)).await,
            "未能在 due 前完成采样"
        );
        assert_eq!(ctx.calls.load(Ordering::SeqCst), 0, "到期前不应被消费");

        // 到期后：等 PG now() 过 due + slack，再等 Done（与 host 时钟脱钩）。
        assert!(
            wait_pg_now_at_least(
                &pool,
                due + chrono::Duration::milliseconds(500),
                Duration::from_secs(10),
            )
            .await,
            "未能等到 PG 侧 due"
        );
        assert!(wait_status(&bus, &id, "Done", Duration::from_secs(10)).await);
        assert_eq!(ctx.calls.load(Ordering::SeqCst), 1);
        handle.abort();
    }

    #[test]
    fn schedule_run_at_ceils_fractional_seconds() {
        assert_eq!(ceil_delay_secs(Duration::ZERO), 0);
        assert_eq!(ceil_delay_secs(Duration::from_millis(1)), 1, "亚秒 → +1s");
        assert_eq!(
            ceil_delay_secs(Duration::from_millis(1500)),
            2,
            "1.5s 应 ceil 为 +2s，而非截成 +1s"
        );
        assert_eq!(ceil_delay_secs(Duration::from_secs(3)), 3, "整秒保持不变");
    }

    #[cfg(feature = "pg")]
    #[sqlx::test]
    async fn flaky_job_retries_then_succeeds(pool: PgPool) {
        let bus = JobBus::try_new_pg(pool).await.unwrap();
        let ctx = TestCtx::default();
        ctx.fail_until.store(2, Ordering::SeqCst);
        let mut registry = JobRegistry::default();
        registry.register(|job, ctx| Box::pin(async move { handle_flaky(job, &ctx).await }));
        let (_tx, handle) = spawn_worker(bus.clone(), ctx.clone(), &registry);

        bus.enqueue(FlakyJob { n: 1 }).await.unwrap();
        let id = latest_id(&bus, FlakyJob::NAME).await.unwrap();

        assert!(wait_status(&bus, &id, "Done", Duration::from_secs(5)).await);
        assert_eq!(ctx.calls.load(Ordering::SeqCst), 3);
        assert_eq!(fetch_row(&bus, &id).await.unwrap().attempts, 3);
        handle.abort();
    }

    #[cfg(feature = "pg")]
    #[sqlx::test]
    async fn exhausted_retries_fail_and_fire_hook(pool: PgPool) {
        let bus = JobBus::try_new_pg(pool).await.unwrap();
        let ctx = TestCtx::default();
        let hook_hits = Arc::new(AtomicUsize::new(0));
        let hook_hits2 = hook_hits.clone();
        let mut registry = JobRegistry::default();
        registry.register(|job, ctx| Box::pin(async move { handle_always_fail(job, &ctx).await }));
        let mut manager =
            WorkerManager::new(bus.clone(), ctx.clone()).with_options(WorkerOptions {
                on_permanent_failure: Some(Arc::new(move |name| {
                    assert_eq!(name, AlwaysFailJob::NAME);
                    hook_hits2.fetch_add(1, Ordering::SeqCst);
                })),
                ..fast_options()
            });
        manager.register_all(&registry);
        let (tx, rx) = watch::channel(false);
        let handle = tokio::spawn(async move {
            manager.run(rx).await.expect("worker run failed");
        });
        let _tx = tx;

        bus.enqueue(AlwaysFailJob { n: 1 }).await.unwrap();
        let id = latest_id(&bus, AlwaysFailJob::NAME).await.unwrap();

        assert!(wait_status(&bus, &id, "Failed", Duration::from_secs(5)).await);
        assert_eq!(
            ctx.calls.load(Ordering::SeqCst),
            2,
            "RETRIES=1 → 共 2 次尝试"
        );
        assert!(
            wait_until(
                || hook_hits.load(Ordering::SeqCst) >= 1,
                Duration::from_secs(2)
            )
            .await,
            "永久失败应触发 hook"
        );
        handle.abort();
    }

    #[cfg(feature = "pg")]
    #[sqlx::test]
    async fn subsecond_backoff_pushes_run_at(pool: PgPool) {
        let bus = JobBus::try_new_pg(pool).await.unwrap();
        // 不启 worker：只验证 bump_run_at 亚秒精度（相对 bump 调用时刻）。
        bus.enqueue(EchoJob { n: 1 }).await.unwrap();
        let id = latest_id(&bus, EchoJob::NAME).await.unwrap();
        let before_bump = chrono::Utc::now();
        bump_run_at(&bus, id.clone(), Duration::from_millis(350))
            .await
            .unwrap();
        let after = fetch_row(&bus, &id).await.unwrap().run_at;
        let delta_ms = (after - before_bump).num_milliseconds();
        assert!(
            (300..=500).contains(&delta_ms),
            "亚秒退避应保留毫秒级精度，实际 delta_ms={delta_ms}"
        );
    }

    #[cfg(feature = "pg")]
    #[sqlx::test]
    async fn quarantine_survives_handle_result_and_is_terminal(pool: PgPool) {
        let bus = JobBus::try_new_pg(pool.clone()).await.unwrap();
        bus.enqueue(EchoJob { n: 1 }).await.unwrap();
        let id = latest_id(&bus, EchoJob::NAME).await.unwrap();

        quarantine_unretryable(&bus, &id, 1).await.unwrap();

        // 模拟 apalis handle_result：把 attempts 写回当前 attempt、status=Failed。
        sqlx::query(
            "UPDATE apalis.jobs
                SET status = 'Failed', attempts = 1, done_at = now() - interval '1 day'
              WHERE id = $1",
        )
        .bind(&id)
        .execute(&pool)
        .await
        .unwrap();

        let row = fetch_row(&bus, &id).await.unwrap();
        assert_eq!(row.status, "Failed");
        assert_eq!(row.attempts, 1);
        assert_eq!(
            row.max_attempts, 1,
            "钳住 max_attempts 后 attempts>=max 为真终态"
        );
        assert!(
            row.run_at > chrono::Utc::now() + chrono::Duration::days(365 * 50),
            "run_at 仍在远未来"
        );

        let eligible: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM apalis.jobs
              WHERE id = $1
                AND (
                    status = 'Pending'
                    OR (status = 'Failed' AND attempts < max_attempts)
                )
                AND run_at < now()",
        )
        .bind(&id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(eligible, 0, "不可被 get_jobs 重捞");

        let backlog = bus.backlog().await.unwrap();
        assert_eq!(backlog.failed, 1, "假 Failed 经钳制后应计入 backlog.failed");

        let deleted = bus.delete_finished_older_than(0).await.unwrap();
        assert_eq!(deleted, 1, "终态行应可被 GC");
        assert!(fetch_row(&bus, &id).await.is_err());
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn sqlite_quarantine_survives_handle_result_and_is_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("worker.db");
        let pool = crate::sqlite_helper::new_sqlite_pool(path.to_str().unwrap())
            .await
            .unwrap();
        let bus = JobBus::try_new_sqlite(pool.clone()).await.unwrap();
        bus.enqueue(EchoJob { n: 1 }).await.unwrap();
        let id = latest_id(&bus, EchoJob::NAME).await.unwrap();

        quarantine_unretryable(&bus, &id, 1).await.unwrap();
        let past = chrono::Utc::now().timestamp() - 86400;
        sqlx::query("UPDATE Jobs SET status = 'Failed', attempts = 1, done_at = ? WHERE id = ?")
            .bind(past)
            .bind(&id)
            .execute(&pool)
            .await
            .unwrap();

        let row = fetch_row(&bus, &id).await.unwrap();
        assert_eq!(row.max_attempts, 1);
        assert!(row.attempts >= row.max_attempts);
        assert!(row.run_at > chrono::Utc::now().timestamp() + 50 * 365 * 24 * 60 * 60);

        let backlog = bus.backlog().await.unwrap();
        assert_eq!(backlog.failed, 1);
        assert_eq!(bus.delete_finished_older_than(0).await.unwrap(), 1);
        assert!(fetch_row(&bus, &id).await.is_err());
    }

    #[cfg(feature = "pg")]
    #[sqlx::test]
    async fn handler_panic_counts_as_failure(pool: PgPool) {
        #[derive(Debug, Serialize, Deserialize)]
        struct PanicJob;
        impl Job for PanicJob {
            const NAME: &'static str = "test_panic";
            const RETRIES: usize = 0;
        }

        let bus = JobBus::try_new_pg(pool).await.unwrap();
        let ctx = TestCtx::default();
        let mut registry = JobRegistry::default();
        registry.register(|_job: PanicJob, _ctx| {
            Box::pin(async move {
                panic!("boom");
            })
        });
        let (_tx, handle) = spawn_worker(bus.clone(), ctx, &registry);

        bus.enqueue(PanicJob).await.unwrap();
        let id = latest_id(&bus, PanicJob::NAME).await.unwrap();
        assert!(wait_status(&bus, &id, "Failed", Duration::from_secs(5)).await);
        handle.abort();
    }

    #[cfg(feature = "pg")]
    #[sqlx::test]
    async fn concurrency_allows_parallel_progress(pool: PgPool) {
        let bus = JobBus::try_new_pg(pool).await.unwrap();
        let ctx = TestCtx::default();
        let mut registry = JobRegistry::default();
        registry.register(|job, ctx| Box::pin(async move { handle_parallel(job, &ctx).await }));
        let (_tx, handle) = spawn_worker(bus.clone(), ctx.clone(), &registry);

        for n in 0..4u32 {
            bus.enqueue(ParallelJob { n }).await.unwrap();
        }

        let started = Instant::now();
        assert!(
            wait_until(
                || ctx.effects.load(Ordering::SeqCst) >= 4,
                Duration::from_secs(5)
            )
            .await,
            "4 个 CONCURRENCY=4 任务应完成"
        );
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_millis(700),
            "并行应明显快于串行 800ms+，实际 {elapsed:?}"
        );
        handle.abort();
    }

    #[test]
    fn orphan_config_maps_timeout_to_missed_heartbeats() {
        let options = WorkerOptions {
            orphan_interval: Duration::from_secs(60),
            orphan_timeout: Duration::from_secs(600),
            ..Default::default()
        };
        let (hb, missed) = orphan_heartbeat_config(&options);
        assert_eq!(hb, Duration::from_secs(60));
        assert_eq!(missed, 10);
        assert_eq!(hb * missed as u32, Duration::from_secs(600));
    }

    #[cfg(feature = "pg")]
    #[sqlx::test]
    async fn slow_job_times_out_and_fails(pool: PgPool) {
        let bus = JobBus::try_new_pg(pool).await.unwrap();
        let ctx = TestCtx::default();
        let mut registry = JobRegistry::default();
        registry.register(|job, ctx| Box::pin(async move { handle_slow(job, &ctx).await }));
        let (_tx, handle) = spawn_worker(bus.clone(), ctx.clone(), &registry);

        bus.enqueue(SlowJob { n: 1 }).await.unwrap();
        let id = latest_id(&bus, SlowJob::NAME).await.unwrap();

        assert!(wait_status(&bus, &id, "Failed", Duration::from_secs(5)).await);
        handle.abort();
    }

    #[cfg(feature = "pg")]
    #[sqlx::test]
    async fn idempotent_handler_dedups_replays(pool: PgPool) {
        let bus = JobBus::try_new_pg(pool).await.unwrap();
        let ctx = TestCtx::default();
        let mut registry = JobRegistry::default();
        registry.register(|job, ctx| Box::pin(async move { handle_dedup(job, &ctx).await }));
        let (_tx, handle) = spawn_worker(bus.clone(), ctx.clone(), &registry);

        bus.enqueue(DedupJob { id: 7 }).await.unwrap();
        bus.enqueue(DedupJob { id: 7 }).await.unwrap();

        assert!(
            wait_until(
                || ctx.calls.load(Ordering::SeqCst) >= 2,
                Duration::from_secs(5)
            )
            .await,
            "两个任务都应被消费"
        );
        assert_eq!(ctx.effects.load(Ordering::SeqCst), 1);
        handle.abort();
    }

    #[cfg(feature = "pg")]
    #[sqlx::test]
    async fn finished_rows_deleted_after_retention(pool: PgPool) {
        let bus = JobBus::try_new_pg(pool).await.unwrap();

        bus.enqueue(EchoJob { n: 1 }).await.unwrap();
        let done_id = latest_id(&bus, EchoJob::NAME).await.unwrap();
        mark_done(&bus, &done_id).await.unwrap();
        sleep(Duration::from_millis(20)).await;

        bus.enqueue(EchoJob { n: 2 }).await.unwrap();
        let pending_id = latest_id(&bus, EchoJob::NAME).await.unwrap();

        let deleted = bus.delete_finished_older_than(0).await.unwrap();
        assert_eq!(deleted, 1, "只删 Done 终态行，Pending 保留");
        assert!(
            fetch_row(&bus, &done_id).await.is_err(),
            "Done 行应已被删除"
        );
        assert_eq!(
            fetch_row(&bus, &pending_id).await.unwrap().status,
            "Pending"
        );
    }

    #[cfg(feature = "pg")]
    #[sqlx::test]
    async fn finished_rows_deleted_in_batches(pool: PgPool) {
        let bus = JobBus::try_new_pg(pool.clone()).await.unwrap();

        sqlx::query(
            "INSERT INTO apalis.jobs (id, job_type, job, status, attempts, max_attempts, run_at, done_at, priority)
             SELECT apalis.generate_ulid(), 'echo_job', '\\x'::bytea, 'Done', 1, 1,
                    now() - interval '1 day', now() - interval '1 day', 0
               FROM generate_series(1, $1)",
        )
        .bind(GC_BATCH + 1)
        .execute(&pool)
        .await
        .unwrap();

        let deleted = bus.delete_finished_older_than(0).await.unwrap();
        assert_eq!(deleted, GC_BATCH as u64 + 1, "分批循环应删净全部终态行");
        let remaining: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM apalis.jobs WHERE status IN ('Done', 'Failed', 'Killed')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(remaining, 0, "终态行应全部删除");
    }

    #[test]
    #[should_panic(expected = "job name conflict")]
    fn duplicate_registration_panics() {
        let mut registry = JobRegistry::<TestCtx>::default();
        registry.register(|job, ctx| Box::pin(async move { handle_echo(job, &ctx).await }));
        registry.register(|job, ctx| Box::pin(async move { handle_echo(job, &ctx).await }));
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn sqlite_smoke_enqueue_consume_and_delay() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("worker.db");
        let pool = crate::sqlite_helper::new_sqlite_pool(path.to_str().unwrap())
            .await
            .unwrap();
        let bus = JobBus::try_new_sqlite(pool.clone()).await.unwrap();
        let ctx = TestCtx::default();
        let mut registry = JobRegistry::default();
        registry.register(|job, ctx| Box::pin(async move { handle_echo(job, &ctx).await }));
        let (_tx, handle) = spawn_worker(bus.clone(), ctx.clone(), &registry);

        bus.enqueue(EchoJob { n: 1 }).await.unwrap();
        let id = latest_id(&bus, EchoJob::NAME).await.unwrap();
        assert!(wait_status(&bus, &id, "Done", Duration::from_secs(5)).await);
        assert_eq!(ctx.calls.load(Ordering::SeqCst), 1);

        bus.enqueue_after(EchoJob { n: 2 }, Duration::from_secs(2))
            .await
            .unwrap();
        let id2 = latest_id(&bus, EchoJob::NAME).await.unwrap();
        let due2 = fetch_row(&bus, &id2).await.unwrap().run_at;
        // 整秒时钟：due 前采样（due-1）；到期后再等 due+1 留一秒 slack。
        assert!(
            wait_sqlite_now_at_least(&pool, due2.saturating_sub(1), Duration::from_secs(5)).await,
            "未能在 due 前完成采样"
        );
        assert_eq!(ctx.calls.load(Ordering::SeqCst), 1, "到期前不应被消费");
        assert!(
            wait_sqlite_now_at_least(&pool, due2 + 1, Duration::from_secs(10)).await,
            "未能等到 sqlite 侧 due"
        );
        assert!(wait_status(&bus, &id2, "Done", Duration::from_secs(10)).await);
        assert_eq!(ctx.calls.load(Ordering::SeqCst), 2);
        handle.abort();
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn sqlite_finished_rows_deleted_after_retention() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("worker.db");
        let pool = crate::sqlite_helper::new_sqlite_pool(path.to_str().unwrap())
            .await
            .unwrap();
        let bus = JobBus::try_new_sqlite(pool).await.unwrap();

        bus.enqueue(EchoJob { n: 1 }).await.unwrap();
        let done_id = latest_id(&bus, EchoJob::NAME).await.unwrap();
        mark_done(&bus, &done_id).await.unwrap();
        sleep(Duration::from_millis(1100)).await;

        bus.enqueue(EchoJob { n: 2 }).await.unwrap();
        let pending_id = latest_id(&bus, EchoJob::NAME).await.unwrap();

        let deleted = bus.delete_finished_older_than(0).await.unwrap();
        assert_eq!(deleted, 1, "只删 Done 终态行，Pending 保留");
        assert!(
            fetch_row(&bus, &done_id).await.is_err(),
            "Done 行应已被删除"
        );
        assert_eq!(
            fetch_row(&bus, &pending_id).await.unwrap().status,
            "Pending"
        );
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn sqlite_finished_rows_deleted_in_batches() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("worker.db");
        let pool = crate::sqlite_helper::new_sqlite_pool(path.to_str().unwrap())
            .await
            .unwrap();
        let bus = JobBus::try_new_sqlite(pool.clone()).await.unwrap();

        let cutoff = (chrono::Utc::now() - chrono::Duration::days(1)).timestamp();
        sqlx::query(
            "WITH RECURSIVE cnt(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM cnt WHERE x < ?)
             INSERT INTO Jobs (id, job_type, job, status, attempts, max_attempts, run_at, done_at, priority)
             SELECT lower(hex(randomblob(16))), 'echo_job', x'7B7D', 'Done', 1, 1, ?, ?, 0 FROM cnt",
        )
        .bind(GC_BATCH + 1)
        .bind(cutoff)
        .bind(cutoff)
        .execute(&pool)
        .await
        .unwrap();

        let deleted = bus.delete_finished_older_than(0).await.unwrap();
        assert_eq!(deleted, GC_BATCH as u64 + 1, "分批循环应删净全部终态行");
        let remaining: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM Jobs WHERE status IN ('Done', 'Failed', 'Killed')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(remaining, 0, "终态行应全部删除");
    }

    #[cfg(feature = "pg")]
    #[sqlx::test]
    async fn pg_notify_wakes_worker_without_polling(pool: PgPool) {
        let bus = JobBus::try_new_pg(pool).await.unwrap();
        let ctx = TestCtx::default();
        let mut registry = JobRegistry::default();
        registry.register(|job, ctx| Box::pin(async move { handle_echo(job, &ctx).await }));
        // 长轮询间隔：NOTIFY 应在 1s 内唤醒；靠轮询要等 10s。
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
        sleep(Duration::from_millis(400)).await;

        bus.enqueue(EchoJob { n: 1 }).await.unwrap();
        let id = latest_id(&bus, EchoJob::NAME).await.unwrap();
        assert!(wait_status(&bus, &id, "Done", Duration::from_secs(2)).await);
        assert_eq!(ctx.calls.load(Ordering::SeqCst), 1);
        handle.abort();
    }
}
