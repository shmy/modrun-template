# AGENTS.md

面向 AI 编码助手的项目指南。人类读者也可参考 [README.md](README.md)。

## 上下文加载策略（按需渐进式）

**首次对话**：只读顶层结构（`Cargo.toml`、`features/` 目录列表），不深入模块实现。

**任务相关加载**：

* 根据任务涉及的 feature，按需加载 `features/<name>/` 与 `<name>_contract/`
* 动基础设施时再读对应 `infra/` crate
* **禁止预读** `docs/ai/`（除非新建域、新 endpoint、改分层/错误约定）
* **禁止预读** `docs/repo-assessment*.md`（历史评审快照，部分结论已过时；以本文件与当前代码为准）

**搜索优先级**：

1. 小改动：`grep` / `glob` 定位符号或文件
2. 架构理解：读 `docs/ai/conventions.md` 对应小节
3. 避免对简单任务启动 subagent

**依赖变更**：改 `*_contract`、跨域 `pub` API、OpenAPI/DTO 前执行 `just who-uses <符号>`。

**脚手架命令**：`just new-feature <name>` 生成域骨架；`just load-baseline` 跑 login/search 负载锚点。双组合根规划见 `docs/dual-composition-root-plan.md`（未落地）。

## 项目概览

基于 **modrun**（DI / 生命周期）+ **Axum**（HTTP）的 Rust 服务模板，含身份登录、账号 CRUD、JWT 鉴权、OpenAPI、i18n、PostgreSQL。

```bash
cp .env.example .env
docker compose up -d
just sqlx_up                 # 可选；server 启动时 persistence 也会自动迁移
cargo run -p server          # 生产默认 feature（blob-cos + otlp）
just dev                     # 本地轻量：cache-pg + blob-fs + job-queue-pg，无 otlp（≠ 生产 default）
just check                   # 与 just dev 相同 feature 集的 cargo check
just pre_commit              # 提交前检查（含 --all-features）
just e2e                     # Hurl 端到端（服务需已启动）
```

`just dev` 走 `BLOB_FILE_*`（默认 `data/blob`）；需要 COS 时用 `cargo run -p server` 或显式 `blob-cos`。

**Blob feature 与编译隔离：**

* `cargo build/run -p server`（生产 default）只开 `blob-cos`，不编 FS/OpenDAL FS。
* 各 feature/infra 的 `default = ["dev-blob"]` 仅方便单独 `cargo check/test -p <crate>`；workspace 依赖写了 `default-features = false`，避免被 server 当依赖时带进 fs。
* `just pre_commit`（`--all-features`）与 `just test`（`--workspace`）会因 Cargo feature unification 同时编进 cos+fs——**质量门故意全开**；要量「轻量图」墙钟用 `just check` / `just build`，不要拿 pre_commit 当增量编译基线。若用 `cargo run -p server --all-features` 真跑进程，`infra/blob` 会显式选 **cos**（需 `BLOB_S3_*`），不会落盘 fs。
* 单独 `cargo check -p web_state` 需 `--features blob-fs`（或 `blob-cos`）；`just ai-check web_state` 已自动加 `blob-fs`。

**环境变量**：可选后端按编译 feature 前缀区分（`CACHE_REDB_PATH` / `CACHE_REDIS_URL` / `JOB_QUEUE_SQLITE_PATH` / `BLOB_FILE_*` / `BLOB_S3_*`）；`cache-pg` 与 `job-queue-pg` 共用 `DATABASE_URL`。完整列表见 `.env.example`。

## 水平扩展（多副本）

| 组件 | 多副本 | 说明 |
|------|--------|------|
| `cache-pg` / `cache-redis` | 可用 | 会话与吊销依赖**共享** cache；**勿用** `cache-redb`（本机文件独占） |
| `job-queue-pg` | 可用 | `job-queue-sqlite` 仅适合单机 |
| `blob-cos` | 可用 | `blob-fs` 为本机路径，多副本不共享 |
| `MASTER` | 仅一台 `true` | 只有 master 跑 `scheduler.run` 与 admin seed；副本设 `MASTER=false` |
| 限流（governor） | 进程内 | 多副本配额相加，非集群配额；生产建议 Ingress / 边缘限流 |

容量：实例数 × `DATABASE_MAX_CONNECTIONS` ≤ PostgreSQL `max_connections`（预留管理连接）。生产先 `just sqlx_up`（或 `migrator`）再滚动发布 server。

## 分层约定（必须遵守）

| 目录 | 放什么 | 禁止 |
|------|--------|------|
| `crates/` | 框架无关、业务无关的可复用库（`*_kit`） | modrun、axum、业务实体、应用配置 |
| `infra/` | modrun 接线、HTTP 栈、运行时组装 | 具体业务域逻辑 |
| `features/` | 业务域实现 + `*_contract` 跨域契约 | 可复用通用库（应下沉到 `crates/`） |
| `bin/server` | 组合根：`.supply()` + `.module()` 注册 | 业务逻辑 |
| `bin/migrator` | 独立迁移二进制：读 `DATABASE_URL` 执行 `migration::run_migrations` | HTTP / 业务逻辑 |

**已有 `crates/`：** `authn_kit`、`jwt_kit`、`cache_kit`、`blob_kit`、`job_queue_kit`、`sched_kit`、`image_kit`、`validify_kit`、`trace_kit`

**抽库判断：** 无 modrun/axum 依赖、不绑本应用配置/文案 → 可进 `crates/`；modrun `module()` 留在 `infra/` 薄接线层（如 `infra/cache`）。

## modrun 约定

组合根 `bin/server/main.rs`：

1. `supply(Cli)`、`supply(OpenApi)` — 在 `Modrun::run()` 前初始化 tracing
2. 基础设施 — `provide` / `provide_result_async` 注册单例（`PgPool`、`Cache`、`WebState`）
3. HTTP 路由 — `provide_group` 贡献 `PublicRouter` / `ProtectedRouter`
4. 长任务 — `task_with` 托管 HTTP 服务（bind/listen 需在 OnStart 期失败）；`task` 托管后台周期任务；`invoke` 挂启动钩子
5. 定时任务 — cron 仅负责触发入队（`SchedJobs` + `ScheduledJob` 桥接）；实际执行走 job worker（`JobHandlers` + `provide_group`）。内务 GC（`CacheGc` / `JobQueueGc` / `BlobGc`）见 `bin/server/internal_jobs.rs`；仅 `MASTER=true` 时注册 `scheduler.run`（非 master 不 `lc.append`）

模块注册顺序（依赖顺序）：`persistence` → `cache` → `blob` → `job_queue` → `web_state` → `server.wiring` → `job_worker` → `scheduler` → features → `http_server`

## HTTP 与鉴权

* **公开路由** `PublicRouter`：登录、刷新等
* **受保护路由** `ProtectedRouter`：由 `http_server` 统一挂 `account_auth_middleware`
* Handler 签名：`State<WebState>` + `ValidJson` / `ValidQuery` / `ValidPath`，返回 `JsonResponseType<T>`（健康探针除外，返回裸 `Json`）
* **`JsonResponseType<T>`** = `Result<axum::Json<T>, WebError>`，定义于 `web_error`；成功用 `Ok(Json(data))`
* 鉴权提取器：`http_auth::AuthedAccount`（`shared_contract::Id`）；`Authorization` scheme 大小写不敏感（`Bearer` / `bearer`）
* 提取器：`http_extractors` 提供 `ValidJson`、`ValidQuery`、`ValidPath`、`ValidTypedMultipart`、`ClientIp`（`web_state` 仍 re-export 以兼容旧路径）
* 会话模型：每个账号同时只有一把 access JTI 与一把 refresh；再次登录或 refresh 会使旧 access 立即 `access_token_revoked`；**已轮换 refresh 被重放**时吊销该账号全部会话（仍返回 `refresh_token_invalid`，不泄露细节）
* `JWT_SECRET` 必填且 ≥32 字节；`APP_ENV=production` 时关闭 Scalar UI，且拒绝默认 `SEED_ADMIN_PASSWORD`
* `OTLP_ENDPOINT` 为空则关闭 OTLP 导出；`OTLP_SAMPLER_RATIO` 默认 `0.1`（`(0,1]`）
* `CORS_ORIGINS` 空则不加 CORS 层；非空为逗号分隔 Origin 白名单
* **客户端 IP**：`http_server` 使用 `into_make_service_with_connect_info`；限流等取 `http_extractors::ClientIp`。仅当直连对端落在 `TRUSTED_PROXIES`（逗号分隔 IP/CIDR）内时，才用 [`real`](https://crates.io/crates/real) 解析 `X-Forwarded-For` / `X-Real-IP` 等头，否则忽略代理头防伪造
* **全局限流**（`axum-governor`）：按客户端 IP 令牌桶，`RATE_LIMIT_GLOBAL_*`；登录/刷新公开路由另有更紧的 `RATE_LIMIT_AUTH_*` 层（`infra/http_routing/rate_limit.rs`）
* **限流取舍（安全）**：登录/刷新**仅按客户端 IP** 限流，**不做** phone / refresh digest 维度。原因：原 cache 双维度滑动窗口虽能防单号多 IP 爆破，但 phone 维度可被攻击者定向锁死（v6 评审）；去掉该维度后，攻击者换 IP 可对同一手机号持续试密码——需靠 `RATE_LIMIT_AUTH_*` 收紧或后续加递增延迟 / 独立 phone 策略补回

**关键：`l10n_middleware` 必须在 app 最外层。** `WebError::into_response()` 只写 extensions，需 locale 中间件转成 `ProblemDetails`；否则鉴权失败可能返回 200 空 body。

### identity 端点（参考）

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
| 受保护 | `PATCH` | `/api/v1/accounts/{id}/password` | `account_reset_password.rs` |

### file 端点（参考）

| 路由类型 | 方法 | 路径 | endpoint 文件 |
|----------|------|------|---------------|
| 受保护 | `POST` | `/api/v1/files/images` | `file_upload_image.rs` |

账号管理接口（`/api/v1/accounts*`）要求调用方 `privileged`，否则 `account_forbidden`（403）。更新带乐观锁：GET 返回 `version`，PATCH body 必须带当前 `version`；特权账号自身不可修改/删除/重置密码（`account_protected`，403）。改密、管理员重置与删除账号会先 `revoke_tokens` 再写库（删除在授权通过后、删行前吊销）；**DELETE 幂等**：目标不存在也返回 200 `deleted: true`。搜索使用 `shared_contract::CursorPagingQuery` / `CursorPagingResult` 做 keyset 游标分页，`q` 长度 3–64 字符。`MASTER=true`（默认）时启动 seed 管理员；库层 `accounts_one_privileged` 保证最多一个特权账号。

共享 Path / 读模型放 `features/identity/dto.rs`：`AccountIdPath`、`AccountPublic`（profile / 搜索项）、`AccountDetail`（GET / PATCH 账号）。操作专属 body（创建、更新、改密等）仍放在对应 endpoint 文件。

## 错误与 i18n

* Handler 错误统一 `WebError`（`InvalidParams` / `L10n`）；域错误（如 `IdentityError`）实现 `L10nKeyedError` 并在 feature `module()` 注册 `register_domain_mapper`
* `WebError::L10n(key)` 的 key 须在 `infra/locale/locales/{en-US,zh-CN}/` 有翻译；identity 相关放 `identity.ftl`，HTTP/校验通用放 `shared.ftl`（**禁止**同一 key 重复出现在多个 ftl）
* 状态码（`l10n_status_code`）：`access_token_*` / `account_invalid_credentials` / `refresh_token_invalid` → 401；`*_forbidden` / `account_protected` → 403；`account_not_found` → 404；`account_duplicated` / `*_version_conflict` → 409；`rate_limited` → 429；其余 L10n 与 `InvalidParams` → 400
* validify 字段错误：`#[validate(..., message = "l10n_key")]` 或 `field_err!` 宏
* serde/validify 错误分类工具在 `validify_kit`，extractor 在 `web_state`

## 新增 feature 模块

1. 在 `features/<name>/` 实现 `module()`，用 `provide_group` 注册路由（蓝本：`features/identity/lib.rs`）
2. **endpoint 文件命名**：`{entity}_{action}.rs`，handler 函数统一叫 `handler`
3. 跨域类型放 `features/<name>_contract/`
4. 在 workspace `Cargo.toml`、`bin/server/Cargo.toml` 添加成员与依赖
5. 在 `bin/server/main.rs` 加 `.module(<name>::module())`
6. OpenAPI paths 由各 feature 的 `utoipa_axum::routes!` 自动合并
7. 业务表迁移放 `infra/migration/versions/`
8. 域专属 l10n key 放 `infra/locale/locales/*/identity.ftl`（或新建 `{domain}.ftl` 并注册到 `locale/lib.rs`）

```rust
pub fn module() -> Module {
    Module::builder("my_feature")
        .provide_group(public_routes)   // 或 protected_routes
}
```

路由注册：`routes!(endpoint::foo_create::handler)`。

复杂 endpoint 可拆出 `pub(crate) async fn execute(...)`，handler 只负责提取器与 `Ok(Json(...))`；有 `#[sqlx::test]` 的才拆 execute（见 `features/identity/endpoint/account_create.rs`）。**凡含副作用（`revoke_tokens`、发消息等）的逻辑必须放进 `execute`**，handler 不得重复一份，以便集成测试覆盖完整行为（参考 `account_delete.rs`、`account_update_password.rs`）。

## 集成测试

* identity CRUD / 改密 / 重置使用 `#[sqlx::test]` + `migration::run_migrations`；含 `account_forbidden` / `account_protected` / `account_duplicated` / `account_version_conflict` 分支。辅助见 `features/identity/tests.rs` 的 `insert_test_account` / `insert_privileged_account`
* 运行：`cargo test -p identity`（需 `.env` 中 `DATABASE_URL` 指向可写库）
* E2E（`just e2e` / `e2e/identity.hurl`）覆盖登录流、账号 CRUD、乐观锁 409、删号后令牌失效；变量见 `e2e/env`（`e2e/env.example`）

## 代码规范

Workspace clippy（**禁止**，`just pre_commit` 含 `--all-targets`）：`unwrap`、`expect`、`panic`、索引切片、`dbg!`。测试模块可用 `#![cfg_attr(test, allow(...))]`。

* 最小改动：不扩 scope、不重构无关代码
* 匹配现有命名与模块风格；注释只解释非显而易见逻辑
* 不主动加测试，除非用户要求或能覆盖真实行为；feature 内 `#[sqlx::test]` 集成测试可参考 identity CRUD
* Commit message：**简体中文**，Conventional Commits，标题说清「为什么」（见 `.cursorrules`）

## 数据库迁移

| 路径 | 内容 |
|------|------|
| `infra/migration/versions/` | 业务表 |
| `crates/cache_kit/migrations/` | `_pg_caches` 缓存表 |

启动时 `persistence` 自动执行迁移（与 `bin/migrator` 共用同一 migrator）。**生产/CI 推荐先 `just sqlx_up`（或 `cargo run -p migrator`）再滚动发布 server**，避免多实例并发迁移。`accounts` 表另有 `remark` / `created_at` / `updated_at` 预留列，当前领域与 API 未暴露。

**SQL 查询**：优先 `sqlx::query!` / `query_as!` / `query_scalar!`；编译时需 `DATABASE_URL` 指向已迁移的库（不用 offline）。

## 常见陷阱

1. **把 axum extractors 放进 `crates/`** — 应留在 `web_state`
2. **在 `ProtectedRouter` 上重复挂鉴权** — `http_server` 已统一处理
3. **新增 l10n key 只改一处 locale** — `en-US` 与 `zh-CN` 都要补
4. **`crates/` 里引 `features/`** — 依赖方向错误；契约用 `*_contract`
5. **改完不跑 `just pre_commit`** — fmt / clippy / `arch_test` 会拦
6. **违反分层依赖** — `cargo test -p server arch_test` 守护；细则见 `docs/ai/conventions.md`
7. **多副本误用本机后端** — `cache-redb` / `job-queue-sqlite` / `blob-fs` 不可跨实例共享；见「水平扩展」

## 参考文件

| 场景 | 参考 |
|------|------|
| feature 模块结构 | `features/identity/lib.rs` |
| 架构依赖测试 | `bin/server/arch_test.rs` |
| AI 按需文档 | `docs/ai/backend.md`、`docs/ai/conventions.md` |
| 共享 Path / 读模型 | `features/identity/dto.rs` |
| 测试辅助 | `features/identity/tests.rs` |
| endpoint 命名与 handler | `features/identity/endpoint/account_login.rs` |
| account CRUD | `features/identity/endpoint/account_*.rs` |
| repository / SQL | `features/identity/repository.rs` |
| 游标分页类型 | `features/shared_contract/query/`（`cursor_paging_query.rs` / `cursor_paging_result.rs`） |
| 登录 / token 流程 | `features/identity/shared/token_ops.rs` |
| JsonResponseType | `infra/web_error/response.rs` |
| 鉴权中间件 | `infra/http_auth/middleware/authorize.rs` |
| 路由新类型 / 限流 | `infra/http_routing/` |
| HTTP 服务组装 | `infra/http_server/lib.rs` |
| 请求体验证 | `infra/http_extractors/valid_json.rs` |
| 客户端 IP 提取 | `infra/http_extractors/client_ip.rs` |
| `#[sqlx::test]` 集成测试 | `features/identity/endpoint/account_create.rs` |
| 独立 DB 迁移 | `bin/migrator/main.rs` |
| E2E 用例 | `e2e/identity.hurl` |
