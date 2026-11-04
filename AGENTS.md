# AGENTS.md

面向 AI 编码助手的项目规则。人类可参考 [README.md](README.md)（怎么跑起来）。

**本文是规则唯一真相源。** `docs/ai/` 只是可延后阅读的细则展开，不重复本文内容。

## 上下文加载策略（按需渐进式）

**首次对话**：只读顶层结构（`Cargo.toml`、`features/` 目录列表），不深入模块实现。

**任务相关加载**：

* 根据任务涉及的 feature，按需加载 `features/<name>/` 与 `<name>_contract/`
* 动基础设施时再读对应 `infra/` crate
* **禁止预读** `docs/ai/`（除非新建域、新 endpoint、改分层/错误约定）
* **禁止预读** `docs/archive/`（历史评审台账，只记录"当时发现什么、修没修"，不作实现依据）

**搜索优先级**：

1. 小改动：`grep` / `glob` 定位符号或文件
2. 架构理解：读 `docs/ai/conventions.md` 对应小节
3. 避免对简单任务启动 subagent

**依赖变更**：改 `*_contract`、跨域 `pub` API、OpenAPI/DTO 前执行 `just who-uses <符号>`。

**脚手架命令**：`just new-feature <name>` 生成域骨架；`just load-baseline` 跑 login/search 负载锚点。双组合根规划见 `docs/dual-composition-root-plan.md`（未落地）。

**质量门分级（勿在迭代中反复跑全量门）**：

| 时机 | 命令 | 内容 |
|------|------|------|
| 迭代中小步 | `just lint <crate>` | fmt + clippy（秒级） |
| 端点完成 / 提交前 | `just ai-check <crate>` | + arch_test（含 tree golden）+ 定向测试 |
| 提交前最终门 | `just pre_commit` | 全 workspace 全 feature 全 target（慢，只跑一次） |

## 增量编译友好（AI 必读）

* **默认只跑** `just lint <crate>` / `just ai-check <crate>`（justfile 按 crate 自动补 `blob-fs` / `job-queue-pg`）。
* **禁止**在迭代中默认跑 `cargo test --workspace`、`cargo clippy --all-features`、`just pre_commit`（后者仅提交前一次）。
* **契约保持纯**：`*_contract` / `shared_contract` 禁止依赖 `utoipa` / `axum` / `sqlx` / `validify`（`arch_test` 强制）。改一处 `Id`/`PhoneNumber` 值对象不应牵动 OpenAPI/DB 适配层重编。
* **契约之间不要互相依赖**：`*_contract` 只可依赖 `shared_contract`。为「消除重复 trait」而让两个 contract 互相依赖会扩大 `shared_contract` 的 invert 引用面，触发 tree golden 告警——这是该守卫故意拦的。
* **`WebState` 字段按 feature 门控**：不选后端时 `blob` / `jobs` 字段不存在。因此只读 pool 的 crate（`health`、`http_server`、`http_routing`、`http_auth`）**不要**声明 blob / 队列 feature 透传——曾因 `default = ["job-queue-pg"]` 让 `health` 拖进 apalis + opendal（约 790 crate）。
* **窄 feature 验证**：`just check`（dev：`cache-pg,blob-fs,job-queue-pg`）与 `cargo check -p server`（生产 default：`blob-cos`+`otlp`）分开跑。
* **`arch_test` golden**：生产 default 下核对 `shared_contract` 的 invert 引用面与直接依赖快照（`bin/server/testdata/`）。失败 = 契约又依赖框架/DB，或额外 crate 依赖了 `shared_contract`；有意变更跑 `just tree-server-bless` 后审 diff 再提交。

## 项目概览

基于 **modrun**（DI / 生命周期）+ **Axum**（HTTP）。能力与命令见 [README.md](README.md)。

**Blob feature 与编译隔离：**

* `cargo build/run -p server`（生产 default）只开 `blob-cos`，不编 FS/OpenDAL FS。
* identity / file / web_state / scheduler **无** `default`；单独 check 由 `just lint` / `just ai-check` 显式传 feature。
* workspace 依赖写 `default-features = false`，避免被 server 当依赖时带进无关后端。
* `just pre_commit` / `just test`（`--workspace`）会因 Cargo feature unification 同时编进 cos+fs——**质量门故意全开**；job-queue 的 pg/sqlite 在 `clippy_workspace` 里分趟（apalis 双后端 sqlx 宏共用 `DATABASE_URL`），不提交仓库级 `.sqlx`。要量「轻量图」墙钟用 `just check` / `just build`，不要拿 pre_commit 当增量基线。

**环境变量**：可选后端按编译 feature 前缀区分（`CACHE_REDB_PATH` / `CACHE_REDIS_URL` / `JOB_QUEUE_SQLITE_PATH` / `BLOB_FILE_*` / `BLOB_S3_*`）；`cache-pg` 与 `job-queue-pg` 共用 `DATABASE_URL`。完整列表见 `.env.example`。

## 水平扩展（多副本）

| 组件 | 多副本 | 说明 |
|------|--------|------|
| `cache-pg` / `cache-redis` | 可用 | 会话与吊销依赖**共享** cache；**勿用** `cache-redb`（本机文件独占） |
| `job-queue-pg` | 可用 | `job-queue-sqlite` 仅适合单机 |
| `blob-cos` | 可用 | `blob-fs` 为本机路径，多副本不共享 |
| `MASTER` | 仅一台 `true` | 只有 master 跑 `scheduler.run` 与 admin seed；副本设 `MASTER=false` |
| 限流（governor） | 进程内 | 多副本配额相加，非集群配额；生产建议 Ingress / 边缘限流 |
| **TSID 节点号** | **必须互不相同** | 主键由 tsid 生成；未设 `TSID_NODE_ID` 时每进程随机取 node，多副本可能撞号导致 INSERT 主键冲突。每实例分配唯一值（`0..N-1`） |

容量：实例数 × `DATABASE_MAX_CONNECTIONS` ≤ PostgreSQL `max_connections`（预留管理连接）。生产先 `just sqlx_up`（或 `migrator`）再滚动发布 server。

## 分层约定（必须遵守）

| 目录 | 放什么 | 禁止 |
|------|--------|------|
| `crates/` | 框架无关、业务无关的可复用库（`*_kit`） | modrun、axum、业务实体、应用配置 |
| `infra/` | modrun 接线、HTTP 栈、运行时组装 | 具体业务域逻辑 |
| `features/` | 业务域实现 + `*_contract` 跨域契约 | 可复用通用库（应下沉到 `crates/`） |
| `bin/server` | 组合根：`.supply()` + `.module()` 注册 | 业务逻辑（含 GC 的领域知识） |
| `bin/migrator` | 独立迁移二进制 | HTTP / 业务逻辑 |

**抽库判断：** 无 modrun/axum 依赖、不绑本应用配置/文案 → 可进 `crates/`；modrun `module()` 留在 `infra/` 薄接线层（如 `infra/cache`）。

细则（contract 边界、kit 约束、跨域例外）见 [docs/ai/conventions.md](docs/ai/conventions.md)「分层」。

## modrun 约定

组合根 `bin/server/main.rs`：

1. `supply(Cli)`、`supply(OpenApi)` — 在 `Modrun::run()` 前初始化 tracing
2. 基础设施 — `provide` / `provide_result_async` 注册单例（`PgPool`、`Cache`、`WebState`）
3. HTTP 路由 — `provide_group` 贡献 `PublicRouter` / `ProtectedRouter`
4. 长任务 — `task_with` 托管 HTTP 服务（bind/listen 需在 OnStart 期失败）；`task` 托管后台周期任务；`invoke` 挂启动钩子
5. 定时任务 — cron 仅负责触发入队（`SchedJobs` + `ScheduledJob` 桥接）；实际执行走 job worker（`JobHandlers` + `provide_group`）。内务 GC 声明在 `bin/server/internal_jobs.rs`；仅 `MASTER=true` 时注册 `scheduler.run`。
   **领域知识在各自 feature**：staging 遍历在 `file::staging_gc`；`AttachmentGc` 的「在用 path」由各域 `provide_group(Arc<dyn AttachmentRefs>)` 贡献——组合根不硬编码业务表；`main` 里 `require_group` 拒绝空贡献（防安全网静默失效）。

模块注册顺序（依赖顺序）：`persistence` → `cache` → `blob` → `job_queue` → `web_state` → `server.wiring` → `job_worker` → `scheduler` → features → `http_server`

## HTTP 与鉴权

* **公开路由** `PublicRouter`：登录、刷新等
* **受保护路由** `ProtectedRouter`：由 `http_server` 统一挂 `account_auth_middleware`
* Handler 签名：`State<AppState>`（`AppState = WebState` 别名）+ `ValidJson` / `ValidQuery` / `ValidPath`，返回 `JsonResponseType<T>`（健康探针除外，返回裸 `Json`）
* **`JsonResponseType<T>`** = `Result<axum::Json<T>, WebError>`，定义于 `web_error`；成功用 `Ok(Json(data))`
* 鉴权提取器：`http_auth::AuthedAccount`（`shared_contract::Id`）；`Authorization` scheme 大小写不敏感
* 提取器落点在 `http_extractors`（`ValidJson`、`ValidQuery`、`ValidPath`、`ValidTypedMultipart`、`ClientIp`）
* 会话模型：每个账号同时只有一把 access JTI 与一把 refresh；再次登录或 refresh 会使旧 access 立即 `access_token_revoked`；**已轮换 refresh 被重放**时吊销该账号全部会话（仍返回 `refresh_token_invalid`，不泄露细节）
* `JWT_SECRET` 必填且 ≥32 字节；`APP_ENV=production` 时关闭 Scalar UI 且拒绝默认 `SEED_ADMIN_PASSWORD`
* `OTLP_ENDPOINT` 为空则关闭 OTLP 导出；`OTLP_SAMPLER_RATIO` 默认 `0.1`（`(0,1]`）。`CORS_ORIGINS` 空则不加 CORS 层
* **客户端 IP**：`http_server` 使用 `into_make_service_with_connect_info`。仅当直连对端落在 `TRUSTED_PROXIES` 内才解析 `X-Forwarded-For` / `X-Real-IP`，否则忽略代理头防伪造。**解析从右往左跳过可信网段**，取第一个非可信地址——反代只在右侧追加而不清洗左侧，取最左值可被客户端随意伪造（曾导致限流被完全绕过）。
* **全局限流**（`axum-governor`）：按客户端 IP 令牌桶，`RATE_LIMIT_GLOBAL_*`；登录/刷新另有更紧的 `RATE_LIMIT_AUTH_*`（由 `identity::public_routes` 经 `provide_group_result` 挂载）。两层均 **fail-closed**。**健康探针走 `ProbeRouter`，不经过全局限流**——否则秒级探测会耗尽令牌桶并触发 liveness 误判重启。
* **限流取舍（安全）**：登录/刷新**仅按客户端 IP**，**不做** phone / refresh digest 维度。原因：phone 维度可被攻击者定向锁死受害账号。代价是换 IP 可对同一手机号持续试密码——靠 `RATE_LIMIT_AUTH_*` 收紧，或后续加递增延迟补回。

**关键：`l10n_middleware` 必须在 app 最外层。** `WebError::into_response()` 只写 extensions，需 locale 中间件转成 `ProblemDetails`；否则鉴权失败可能返回 200 空 body。CORS 必须在 l10n 之外（否则错误响应丢 `access-control-*`）。

### 端点一览

| 路由类型 | 方法 | 路径 | endpoint 文件 |
|----------|------|------|---------------|
| 公开 | `POST` | `/api/v1/identity/login` | `account_login.rs` |
| 公开 | `POST` | `/api/v1/identity/refresh` | `account_refresh_token.rs` |
| 受保护 | `POST` | `/api/v1/identity/logout` | `account_logout.rs` |
| 受保护 | `GET` | `/api/v1/profile/current` | `profile_current.rs` |
| 受保护 | `PATCH` | `/api/v1/identity/password` | `account_update_password.rs` |
| 受保护 | `POST` | `/api/v1/accounts` | `account_create.rs` |
| 受保护 | `GET` | `/api/v1/accounts` | `account_search.rs` |
| 受保护 | `GET/PATCH/DELETE` | `/api/v1/accounts/{id}` | `account_get.rs` / `account_update.rs` / `account_delete.rs` |
| 受保护 | `PATCH` | `/api/v1/accounts/{id}/avatar` | `account_update_avatar.rs` |
| 受保护 | `PATCH` | `/api/v1/accounts/{id}/password` | `account_reset_password.rs` |
| 受保护 | `POST` | `/api/v1/files/images` | `file_upload_image.rs` |
| 公开（仅 `blob-fs`） | `GET` | `/files/{*path}` | `file_serve_static.rs` |

账号管理接口（`/api/v1/accounts*`）要求调用方 `privileged`。更新带乐观锁：GET 返回 `version`，PATCH body 必须带当前 `version`；特权账号自身不可修改/删除/重置密码（`account_protected`，403）。**改密 / 管理员重置 / 删号统一「先落库、后 `revoke_tokens`」**：写库失败时不得把用户踢下线（删号在 `tx.commit` 之后吊销）。**DELETE 幂等**：目标不存在也返回 200 `deleted: true`。`MASTER=true`（默认）时启动 seed 管理员；库层 `accounts_one_privileged` 保证最多一个特权账号。

共享 Path / 读模型放 `features/identity/dto.rs`。**`identity_contract` 口径（当前 B）**：仅域错误（`IdentityError` / `L10nKeyedError`）；公开读模型仍在 runtime DTO，待跨域消费时再下沉。`shared_contract` 仅纯值对象 + serde。

## 错误与 i18n

* Handler 错误统一 `WebError`（`InvalidParams` / `L10n`）；域错误（如 `IdentityError`）实现 `L10nKeyedError` 并在 feature `module()` 注册 `register_domain_mapper`
* **只有注册过的域错误才是 4xx**。`From<Report> for WebError` 无 state 可用，故映射是进程级注册表；未注册的 `Report` 一律 500（**不做**「Display 像 snake_case 就当 l10n key」的启发式——那会把 `connection_refused` 这类内部错误静默降级成 400）
* 通用/鉴权 key 放 `infra/locale/locales/{en-US,zh-CN}/shared.ftl`；**域专属** FTL 放 `features/<name>/locales/{en-US,zh-CN}/`，在 `module()` 里 `locale::register_ftl`（**禁止** `infra/locale` include 域 FTL；**禁止**同一 key 出现在多个 ftl）
* `register_ftl` 在 bundle 密封后调用会 panic（装配顺序错误：此时注册的 key 永不生效）
* 状态码按 **命名约定**（`web_error::l10n_status_code`），无域白名单：`access_token_*` / `authed_account_not_found` / `*_invalid_credentials` / `*_token_invalid` → 401；`*_forbidden` / `*_protected` → 403；`*_not_found` → 404；`*_duplicated` / `*_version_conflict` → 409；`rate_limited` → 429；`cache_unavailable` → 503；`payload_too_large` / `request_timeout` → 413 / 408；其余 L10n 与 `InvalidParams` → 400
* validify 字段错误：`#[validate(..., message = "l10n_key")]` 或 `field_err!` 宏
* serde/validify 错误分类工具在 `validify_kit`，extractor 在 `http_extractors`

## 新增 feature 模块

1. `just new-feature <name>` 生成骨架（含 contract 口径说明、locale 自注册、无 default feature）
2. 在 `features/<name>/` 实现 `module()`，用 `provide_group` 注册路由（蓝本：`features/identity/lib.rs`）
3. **endpoint 文件命名**：`{entity}_{action}.rs`，handler 统一叫 `handler`；`State<AppState>` + 仅调用所需 `Has*`
4. 跨域类型放 `features/<name>_contract/`（纯；OpenAPI 适配在 runtime DTO）
5. 在 workspace `Cargo.toml`、`bin/server/Cargo.toml` 添加成员与依赖
6. 在 `bin/server/main.rs` 加 `.module(<name>::module())`（须在 `http_server` 之前，保证 `register_ftl` 先于首个请求）
7. OpenAPI paths 由各 feature 的 `utoipa_axum::routes!` 自动合并
8. 业务表迁移放 `infra/migration/versions/`
9. 验证：`just lint <name>` → `just ai-check <name>`；提交前 `just pre_commit`

```rust
pub fn module() -> Module {
    shared::l10n::register_my_feature_locales();
    Module::builder("my_feature")
        .provide_group(public_routes)   // 或 protected_routes
}
```

复杂 endpoint 可拆 `pub(crate) async fn execute(...)`，handler 只负责提取器与 `Ok(Json(...))`。**凡含副作用（`revoke_tokens`、发消息等）的逻辑必须放进 `execute`**，handler 不得重复一份，以便集成测试覆盖完整行为。

细则（DTO 写法、`#[schema(value_type)]`、手机号字段约定）见 [docs/ai/conventions.md](docs/ai/conventions.md)「Endpoint」。

## 集成测试

* identity CRUD / 改密 / 重置用 `#[sqlx::test]` + `migration::run_migrations`；辅助见 `features/identity/tests.rs`
* 运行：`just ai-check identity`（或 `cargo test -p identity --features blob-fs`；需 `.env` 的 `DATABASE_URL` 指向可写库）
* E2E：`just e2e` / `just e2e_file`，变量见 `e2e/env`（`e2e/env.example`）
* **注意**：`#[sqlx::test]` 不走 `module()`，因此**域 mapper 未注册**。测试里想让域错误映射成 4xx，要么显式构造 `WebError::L10n`，要么断言原始 `Report`——不要依赖「测试环境恰好和进程启动后一致」

## 代码规范

Workspace clippy（**禁止**）：`unwrap`、`expect`、`panic`、索引切片、`dbg!`。测试模块可用 `#![cfg_attr(test, allow(...))]`。

* 最小改动：不扩 scope、不重构无关代码
* 匹配现有命名与模块风格；注释只解释非显而易见逻辑
* 不主动加测试，除非用户要求或能覆盖真实行为
* Commit message：**简体中文**，Conventional Commits，标题说清「为什么」（见 `.cursorrules`）

## 数据库迁移

| 路径 | 内容 |
|------|------|
| `infra/migration/versions/` | 业务表 |
| `crates/cache_kit/migrations/` | `_pg_caches` 缓存表 |
| apalis（`JobBus::try_new_*`） | 队列表：pg=`apalis.*`；sqlite=`Jobs`/`Workers`（库自带 migrate） |

启动时 `persistence` 自动执行业务迁移；`job_queue` 初始化也会跑 apalis setup（幂等）。**生产/CI 推荐先 `just sqlx_up` 再滚动发布 server**，避免多实例并发迁移，并保证编译期 sqlx 宏能看见 `apalis.jobs`。`accounts` 表另有 `remark` / `created_at` / `updated_at` 预留列，当前 API 未暴露。

**Migration squash 警告**：历史 `0002`/`0003`/`0004` 已并入 `0001_baseline`；其后 baseline 又改过（如 avatar），另有 `0002_blob_objects`。新库按当前 versions 冷启即可。**已按更早 baseline checksum（或旧 versions）跑过的库，不能无脑当兼容升级继续 `sqlx migrate`**。

**SQL 查询**：优先 `sqlx::query!` / `query_as!` / `query_scalar!`；编译时需 `DATABASE_URL` 指向已迁移的库（本仓库不提交 `.sqlx`）。编 `apalis-postgres` 时库内还须已有 `apalis.jobs`：用 `just sqlx_up`（内部 `SQLX_OFFLINE=true` 冷启 migrator）。勿在空库上直接 `cargo clippy --workspace --all-features`；用 `just clippy_workspace` / `just pre_commit`。

## 常见陷阱

1. **把 axum extractors 放进 `crates/`** — 应留在 `http_extractors`
2. **在 `ProtectedRouter` 上重复挂鉴权** — `http_server` 已统一处理
3. **新增 l10n key 只改一处 locale** — `en-US` 与 `zh-CN` 都要补
4. **`crates/` 里引 `features/`** — 依赖方向错误；契约用 `*_contract`
5. **改完不跑 `just pre_commit`** — fmt / clippy / `arch_test` 会拦
6. **在多副本下用本机后端** — `cache-redb` / `job-queue-sqlite` / `blob-fs` 不可跨实例共享
7. **`panic = "abort"`** — `job_queue_kit` 的 `catch_unwind` 会失效，一次 job panic 直接终止进程；改 profile 前先看根 `Cargo.toml` 注释
8. **给只读 crate 加 blob/队列 feature 透传** — 会把 apalis / opendal 拖进 `health` 之类的依赖图

## 参考文件

| 场景 | 参考 |
|------|------|
| 新表接附件（commit/attach/GC） | `docs/ai/conventions.md`「附件引用」；蓝本 `features/identity/shared/avatar_ops.rs` |
| feature 模块结构 | `features/identity/lib.rs` |
| 架构依赖测试 | `bin/server/arch_test.rs` |
| 按需细则 | `docs/ai/conventions.md`、`docs/ai/backend.md` |
| 共享 Path / 读模型 | `features/identity/dto.rs` |
| 测试辅助 | `features/identity/tests.rs` |
| account CRUD | `features/identity/endpoint/account_*.rs` |
| repository / SQL | `features/identity/repository.rs` |
| 游标分页类型 | `features/shared_contract/query/` |
| 登录 / token 流程 | `features/identity/shared/token_ops.rs` |
| staging 清理 | `features/file/shared/staging_gc.rs` |
| 附件引用贡献点 | `features/file/attachment_refs.rs` |
| 鉴权中间件 | `infra/http_auth/middleware/authorize.rs` |
| 路由新类型 / 限流 | `infra/http_routing/` |
| HTTP 服务组装 | `infra/http_server/lib.rs` |
| 请求体验证 | `infra/http_extractors/valid_json.rs` |
| 客户端 IP 提取 | `infra/http_extractors/client_ip.rs` |
| 独立 DB 迁移 | `bin/migrator/main.rs` |
| 历史评审台账 | `docs/archive/repo-assessment.md` |
