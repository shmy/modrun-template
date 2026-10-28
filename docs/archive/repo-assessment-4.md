# 仓库深度评估 · 第四版：modrun 最佳实践与工程化视角（2026-09-05）

> **历史快照，可能已过时。** 以根目录 [AGENTS.md](../AGENTS.md) 与当前代码为准；勿作为实现依据。

> 与前三版互补（[v1](repo-assessment.md) 外围设施 / [v2](repo-assessment-2.md) 实现细节 /
> [v3](repo-assessment-3.md) 企业语义）。本版视角：**对照 modrun 1.0 官方 Concepts
> （五个装配动词、Domain module / Group / Wrapper 三模式、task/hook 生命周期语义、
> supply 组合根哲学）逐条核对模板的用法，并评估组合根的工程质量**。
> modrun 源码依据：`modrun-1.0.0`（crates.io，README Concepts + `src/task.rs` /
> `src/lifecycle.rs`）。

## 一、符合 modrun 最佳实践的方面（核对结果）

| modrun 官方概念 | 模板用法 | 结论 |
|---|---|---|
| supply 放组合根、可换测试替身 | `main.rs`：`supply(cli)` + `supply(ApiDoc::openapi())` 在 `run()` 前 | ✅ 标准用法；`supply` OpenApi 这个"非服务值"也走 DI，思路干净 |
| Domain module 模式（隐藏内部类型，只暴露服务） | `features/identity` 只 `provide_group` 路由 + `invoke` 注册 seed 钩子，repository/dto 全部模块私有 | ✅ 与 `examples/basic.rs` 形态一致 |
| Group 模式（多模块贡献同型元素） | 路由用 `Group<PublicRouter>`/`Group<ProtectedRouter>`；job handler 用 `Group<JobHandlers<C>>`；cron 用 `Group<SchedJobs<C>>` | ✅ 与 `examples/handlers.rs` 一致，且 newtype 包裹（`SchedJobs`/`JobHandlers`）符合"同型两份用 newtype"的约定 |
| 同型唯一、Wrapper 自组 | `WebState` 单例聚合；无重复 provide | ✅ |
| task 双形态：需启动期失败的用 task_with | HTTP 用 `task_with`（bind/listen 在 OnStart 期失败 → 启动失败）；后台循环用 `task` | ✅ 精确遵守了 `AGENTS.md` 与 modrun 的契约 |
| `Stopped` 传给 select / graceful_shutdown | `http_server` 把 `stopped` 直接交给 `with_graceful_shutdown` | ✅ |
| task 失败 → Shutdowner.fail → 全局停机 | job worker / scheduler 的 `run()` 返回 Err 时通过 modrun 传播 | ✅ fail-fast 语义成立（结构化错误停机，业务级错误由 worker 内部消化） |
| hook 只做一次性启停动作 | identity 用 `hook().on_start` 做 seed，幂等且并发安全 | ✅ |
| 组合根是唯一的"知道全局的地方" | `bin/server` 只有装配、arch_test、internal_jobs；无业务逻辑 | ✅ |

**组合根工程质量总评：高。** `main.rs` 12 个 module 的注册顺序与依赖方向完全一致
（作为文档可读性很好——modrun 按构造函数签名解析依赖，顺序不承担正确性职责）；
`wiring.rs` 单独存在的唯一目的是收编 `internal_jobs` 的两个 Group，避免
`job_queue_kit ↔ web_state` 循环依赖——这是对"组合根负责打破循环"的正确运用。

**内务任务设计是本仓库 modrun 用法里最漂亮的一笔**（`bin/server/internal_jobs.rs`）：
GC 不用裸 cron 直接执行，而是 cron 只入队（`ScheduledJob` 桥接）、job worker 执行，
`RETRIES = 0` 保留"失败不重试、下个周期再跑、终态 `Failed` 留痕"的语义——
`worker_jobs` 表顺带成了免费的内务执行台账。cron（触发）与 job（执行）分离的
决策本身也是最值得沉淀为 ADR 的架构选择之一。

## 二、与 modrun 哲学的偏差（按重要性）

### 1. 全局静态错误映射绕过了 DI（最大偏差）
`infra/web_error/error.rs` 的 `DOMAIN_REPORT_MAPPERS: Mutex<Vec<DomainReportMapper>>`
是进程级全局注册表；`identity::module()` / `file::module()` 的**函数体第一行**
调用 `register_identity_domain_mapper()` 写入（`features/identity/lib.rs:28`）。

问题：
- `module()` 应是纯 builder，这里产生了隐藏的全局副作用，且发生在 builder 构造期
  而非生命周期期；
- 注册顺序依赖 module 的调用顺序，测试隔离差（两个 feature 的 mapper 在测试进程
  里互相可见）；
- modrun 的核心主张是"没有全局 get<T>()、一切走构造函数注入"——mapper 完全可以
  是 `provide_group` 贡献的 `Group<ErrorMapper>`，由 `web_state` 组装后注入
  `WebError` 的渲染路径。

建议：定义 `ErrorMappers` 聚合类型，features 用 `provide_group` 贡献，
`l10n_middleware` / `WebError::into_response` 从注入的注册表读取。这能删掉
`static Mutex` 与 `mappers_lock()`。

### 2. `module()` 函数不纯（同源问题）
除上面副作用外，`identity::module()` 在 builder 链外调用注册函数是可接受的风格，
但两者混在一起让"读 module() = 读装配图"的承诺失效。最小修复：把 mapper 注册
挪进 `invoke(register)` 的 OnStart，或按建议 1 彻底 DI 化。

### 3. 双层关机抽象并存，样板重复 3 处
modrun 的关机原语是 `Stopped`（oneshot future）；而 `sched_kit`/`job_queue_kit`
的 `start()/run()` 接口设计成收 `watch::Receiver<bool>`。结果是
`http_server/lib.rs`、`job_worker.rs`、`scheduler/lib.rs` 三处出现同一段
`Box::pin + tokio::select! + shutdown_tx.send(true) + run.await` 样板
（每处约 15 行），且 select 两个分支的错误处理细节必须逐处抄对。

建议二选一：
- kit 层改收 `impl Future<Output = ()>`（与 `Stopped` 对齐），样板消失；
- 或在某个 infra crate 提供一个 `task_with_watch(name, |stopped| ...)` 桥接
  helper，三处收敛为三行。

这是 **kit API 形状与宿主框架 API 形状没对齐**的典型摩擦——两个 kit 既然
目标是"可复用"，关机信号类型应该用最朴素的 `Future` 而非 `watch::Receiver`。

### 4. 优雅停机没有总 deadline
modrun 的 task OnStop 会 join 任务句柄；`WorkerManager::run` 会等所有 job loop
退出，而 job loop 会等 in-flight job 完成（这是正确行为）。但整个 drain 无上限：
一个卡死的 job handler（如外部 IO 无超时）会让 SIGTERM 后的停机无限挂起。
建议：组合根给 drain 加整体 deadline（select 上再叠一层
`tokio::time::timeout`），或依赖 modrun 的 stop 超时能力（若有配置面）。

### 5. `WebState` 上帝对象 vs 窄注入
modrun 的哲学是构造函数按需注入（`provide_cache(cli, pool)` 只拿需要的），
但 `WebState` 聚合了 pg/cache/blob/jobs/token_helper 全量，之后：
- axum handler 经 `State<WebState>` 全量可见（axum 约束，可接受）；
- **job handler 的 ctx 也是整个 `WebState`**（`handle_cache_gc(_job, ctx: &WebState)`）——
  内务任务其实只需要 cache/blob/jobs 三样，却拿到了 token_helper 与 DB 池。
 kit 层用了泛型 `C`（`JobRegistry<C>`）本是为窄化 ctx 留的口子，组合根却传了
 全量 WebState。建议：为 job ctx 定义窄接口（如 `GcCtx { cache, jobs, blob }`），
 `WebState` 拆出所需字段实现它。

### 6. 后端切换用编译期 feature 而非 supply 运行时替换
`infra/cache`、`infra/blob`、`infra/job_queue` 用 `#[cfg(feature)]` 在 provider
内部选后端，没有后端时 `compile_error!` 兜底——工程上稳健。但 modrun 官方
叙事里"换实现"的标准位是组合根 `supply`（`examples/swap.rs`）：三后端都构建、
按配置注入，测试无需重编译即可换替身。当前取舍换来了 zero-cost 单后端，
是合理决策，但值得在 ADR 里写明"为什么不用 supply 换实现"（预计答案：
binary 体积与依赖树）。

### 7. 小项
- 模块命名风格不一：`cache`/`blob`（单词）vs `server.wiring`/`job_queue.worker`
  （点分层级）。建议统一一种。
- `scheduler::boot` 非 master 时静默返回 `Ok(())`，日志只有一行 info；建议把
  "本实例因 MASTER=false 跳过调度器"作为结构化字段（`master=false`）记录，
  便于多实例排障时确认哪些节点没跑。
- `main.rs` 的 `_guard`（`TracingGuard`）在 `run()` 返回后才 drop，OTLP provider
  shutdown 顺序正确——这里没有问题，点名是防止后人"优化"掉它。

## 三、工程化观察（组合根之外）

1. **kit 的 Job trait 设计好**：`NAME`/`RETRIES`/`TIMEOUT`/`CONCURRENCY` 常量 +
   serde payload，注册表按键路由——与 modrun 的"无注解、显式注册"哲学同构。
2. `SchedJobs::scheduled` 的 `C: HasJobBus` 约束把"cron 入队需要 JobBus"表达成
   类型约束而非运行时检查——泛型 ctx 模式的正确用法（也反衬第 5 条：约束已经
   为窄化做好了准备，只是没用上）。
3. `internal_jobs.rs` 的 BlobGc 注释坦承"名义 30 天、实际最长可接近约 60 天"
   （按整月目录删除的粒度代价）——这种自我暴露式注释是好习惯，但更该把这个
   语义放进常量名或配置说明（`BLOB_STAGING_RETENTION_DAYS` 名义与实际不符）。
4. 组合根测试：`arch_test.rs` 守护装配边界、`internal_jobs` 有 GC 行为测试——
   组合根不是"不可测区"，这个意识到位。

## 四、若把 sched_kit / job_queue_kit 下沉为通用库的修正清单

1. 关机信号改 `impl Future`（见二.3），消除对 `tokio::sync::watch` 的依赖暴露；
2. `lock_by` 由调用方注入（第三版 E 提过：现在是硬编码 `"local-worker"`）；
3. `WorkerOptions` 目前只能整体 override，建议暴露 builder 或按字段配置化到 env；
4. 孤儿扫描补 `(lock_at) WHERE status='Running'` 部分索引（第二版二.5）；
5. ctx 窄化：`C` 约束用 trait（如 `trait JobCtx { fn cache(); fn bus(); }`）而不是
   要求调用方传完整应用状态。

## 五、结论

**这个模板是 modrun 的示范级使用者**：五个动词各归其位、Group 模式用出了
官方示例之外的扩展（job handlers / sched jobs 的 newtype 贡献）、组合根干净
且有架构测试守护、内务任务的"cron 入队 + worker 执行 + RETRIES=0 留痕"设计
值得成为 modrun 生态的推荐范例。

偏差集中在三点，且都可低风险修复：**全局静态 mapper 注册绕过 DI**（最值得修，
直接违背 modrun 核心主张）、**kit 关机接口与 `Stopped` 不对齐导致的三处样板**、
**job ctx 未按 kit 预留的泛型口子窄化**。修完这三点，这个仓库可以作为 modrun
的参考实现对外展示。

四版合并的持续主题：实现与装配质量高，缺的是"把注释里的正确理由升级为
机制或文档"——mapper 注册、关机 deadline、lock_by、保留期语义，都属于
半天内可完成、但需要有人排期的小修。
