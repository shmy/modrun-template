# modrun-template

基于 [modrun](https://docs.rs/modrun) + Axum 的 Rust 服务模板。

## 快速开始

```bash
cp .env.example .env
just db-up                   # 或 docker compose up -d
just sqlx_up                 # 执行数据库迁移（可选；server 启动时也会自动迁移）
cargo run -p server          # 生产默认：blob-cos + otlp
just dev                     # 本地热重载：blob-fs、无 otlp（对象落 BLOB_FILE_ROOT，默认 data/blob）
```

> **`just dev` ≠ `cargo run -p server`**：前者用本地文件系统 blob、关闭 OTLP；要对齐生产 COS / 遥测请用后者（或显式 `--features`）。

可选后端的环境变量按 **编译 feature 前缀** 区分，互不重叠；完整列表见 [`.env.example`](.env.example)：

| Feature | 环境变量 |
|---------|----------|
| `cache-redb` | `CACHE_REDB_PATH` |
| `cache-redis` | `CACHE_REDIS_URL` |
| `cache-pg` | `DATABASE_URL`（共用） |
| `job-queue-sqlite` | `JOB_QUEUE_SQLITE_PATH` |
| `job-queue-pg` | `DATABASE_URL`（共用） |
| `blob-fs` | `BLOB_FILE_ROOT`、`BLOB_FILE_DOMAIN` |
| `blob-cos` | `BLOB_S3_*` |

默认种子管理员（可通过环境变量覆盖）：

* 手机：`13888888888`
* 密码：`admin123!`

## API 文档

服务启动后：

* `GET /openapi.json` — OpenAPI 契约
* `GET /scalar` — Scalar UI（`SCALAR_UI_ENABLED=false` 或 `APP_ENV=production` 时关闭）

## 部署假设

* **TLS / HTTP/2**：由 Ingress / 反向代理终止；本进程只听 HTTP/1.1。
* **限流**：`axum-governor` 为**进程内**令牌桶；多副本配额相加。生产建议在边缘再限一层。
* **可观测**：`OTLP_ENDPOINT` 为空则关闭 OTLP 导出（仅 console）；非空时按 `OTLP_SAMPLER_RATIO`（默认 `0.1`）采样。不提供 Prometheus `/metrics` 拉模型。
* **种子管理员**：`APP_ENV=production` 时禁止使用默认 `SEED_ADMIN_PASSWORD`；`JWT_SECRET` 至少 32 字节。
* **多副本**：见 [AGENTS.md](AGENTS.md)「水平扩展」——共享 PG（及 redis cache），仅一台 `MASTER=true`。

构建：`just build` 用 fat LTO 发布优化；CI 可用 `just build_release_ci`（更快、体积/峰值略逊）。

编译 feature：默认含 `otlp`（OTLP + 业务 metrics）。无 collector 时可 `OTLP_ENDPOINT=` 运行时关闭导出；或  
`cargo build -p server --no-default-features --features cache-pg,blob-cos,job-queue-pg` 去掉 tonic/OTLP 依赖。

## 身份与鉴权

**公开**（无需 Bearer）：

| 方法 | 路径 | 说明 |
|------|------|------|
| `POST` | `/api/v1/identity/login` | 登录，返回 access / refresh token |
| `POST` | `/api/v1/identity/refresh` | 刷新令牌（轮换 refresh） |

**受保护**（需 Bearer；由 `http_server` 在 `ProtectedRouter` 上统一挂鉴权中间件）：

| 方法 | 路径 | 说明 |
|------|------|------|
| `POST` | `/api/v1/identity/logout` | 登出 |
| `GET` | `/api/v1/profile/current` | 当前账号 |
| `PATCH` | `/api/v1/identity/password` | 本人修改密码（成功后吊销该账号令牌） |
| `POST` | `/api/v1/accounts` | 创建账号（需调用方 `privileged`） |
| `GET` | `/api/v1/accounts` | 搜索账号（`cursor` / `limit` / `q` 游标分页；需 `privileged`） |
| `GET` | `/api/v1/accounts/{id}` | 获取账号（含 `version`；需 `privileged`） |
| `PATCH` | `/api/v1/accounts/{id}` | 更新账号（body 带乐观锁 `version`；需 `privileged`） |
| `DELETE` | `/api/v1/accounts/{id}` | 删除账号（幂等；`privileged` 目标不可删；需调用方 `privileged`） |
| `PATCH` | `/api/v1/accounts/{id}/password` | 管理员重置密码（成功后吊销目标令牌） |

`privileged` 种子管理员在 **MASTER=true**（默认）时于启动自动创建（`SEED_ADMIN_PHONE` / `SEED_ADMIN_PASSWORD` 可覆盖）。副本设 `MASTER=false` 跳过 seed。Access token 只接受 `Authorization: Bearer`（scheme 大小写不敏感）。每个账号同时只有一把有效 access / refresh，后登录会踢掉前一端。`JWT_SECRET` 必填（无内置默认）。`APP_ENV=production` 时关闭 Scalar UI。

登录失败为 401（`account_invalid_credentials`）；账号不存在为 404；受保护账号为 403（`account_protected`）；手机号冲突与乐观锁冲突为 409。删除账号会吊销该账号令牌。

全站按客户端 IP 做 `axum-governor` 令牌桶限流（`RATE_LIMIT_GLOBAL_*`）；登录/刷新路由另有更紧配额（`RATE_LIMIT_AUTH_*`）。超限返回 429 `rate_limited`。

**安全取舍**：不做 phone / refresh 维度限流（避免 cache 方案下对受害手机号的定向锁死）；代价是换 IP 后可对同一手机号持续试密码，详见 `AGENTS.md`。

## 文件（file）

**受保护**（需 Bearer）：

| 方法 | 路径 | 说明 |
|------|------|------|
| `POST` | `/api/v1/files/images` | 上传图片（multipart，≤2MiB）。魔数检测 + 解码压缩为 WebP 后写入对象存储，返回 `staging/` 路径与公网 URL |

`staging/` 为暂存区：MASTER 节点每日 04:30 由 `BlobGc` 按**月目录**清理过期对象（默认保留约 30 天；按月粒度，非按天精确，月初文件可能多留近一个月）。COS 可叠加生命周期规则、fs 可叠加外部 cron 作补充。上传失败返回 `file_not_image`（400）。

## 探针

* `GET /livez` — 进程存活（不查依赖）
* `GET /readyz` — 就绪（检查 PostgreSQL）
* `GET /healthz` — 运行信息（uptime、连接池状态）

## 分层约定

* **`crates/`** — 框架无关、业务无关的可复用库（如 `jwt_kit`、`cache_kit`、`authn_kit`、`validify_kit`、`trace_kit`）
* **`infra/`** — modrun 接线、HTTP 栈、运行时组装（如 `cache`、`persistence`、`web_state`）
* **`features/`** — 业务域与跨域契约

## 后台任务与调度

* **Job 队列**（`job_queue_kit` + `bin/server/job_worker.rs`）：点对点后台任务，pg / sqlite 双后端；失败按指数退避重试（上限 `RETRIES`）、超时记一次失败、终态 `Failed` 留表可审计（DLQ-in-table）；at-least-once（孤儿恢复）。业务侧只需实现 `Job` + 注册 `JobHandlers`。
* **定时任务**（`sched_kit` + `infra/scheduler`）：cron **只负责到点入队一个 Job**，实际执行由 job worker 承担；内务 GC（`CacheGc` / `JobQueueGc` / `BlobGc`）见 `bin/server/internal_jobs.rs`。仅 `MASTER=true` 节点注册 `scheduler.run`。
* **对象存储**（`blob_kit`）：COS（默认）/ 本地 fs / 测试内存后端；流式写、批量删、presign（仅 COS）。

## 模块结构

```
bin/server              组合根：supply(Cli, OpenApi) + 注册各模块（wiring / job_worker / internal_jobs）
bin/migrator            独立迁移入口：读取 DATABASE_URL，执行 migration::run_migrations（部署/CI 用）
crates/authn_kit        鉴权协议（key 命名、Verifier trait）
crates/jwt_kit          JWT 签发与校验
crates/cache_kit        带 TTL 的缓存抽象（pg / redb / redis 三后端 + 原子滑动窗口限流）
crates/blob_kit         对象存储门面（COS / fs / 内存，opendal）
crates/job_queue_kit    后台任务队列（pg / sqlite，退避重试 / 超时 / 孤儿恢复）
crates/sched_kit        定时任务调度（tokio-cron-scheduler 封装）
crates/image_kit        图片压缩（JPEG/PNG → WebP）
crates/validify_kit     serde / validify 错误 → l10n key + 字段路径
crates/trace_kit        tracing / OTLP 初始化
infra/config            CLI / .env（按 feature 前缀：CACHE_* / JOB_QUEUE_* / BLOB_*，见 `.env.example`）
infra/persistence       PostgreSQL 连接池 + 启动迁移
infra/cache             注册 cache_kit::Cache（modrun 接线，feature 选后端）
infra/blob              注册 blob_kit::Blob（feature 选 cos / fs）
infra/job_queue         注册 job_queue_kit::JobBus + JobHandlers 登记
infra/scheduler         cron 注册表 + MASTER 门控
infra/http_auth         Bearer 鉴权 + AuthedAccount 提取器
infra/web_state         WebState { pg, cache, blob, jobs, token_helper, ... } + 各提取器
infra/web_error         WebError、JsonResponseType、register_domain_mapper、RFC 9457
infra/locale            Fluent i18n（shared / identity / file 按域分 ftl）+ l10n 中间件
infra/http_server       合并 PublicRouter / ProtectedRouter、OpenAPI、Scalar、axum-governor 全局限流
features/health         健康检查
features/shared_contract    跨域值对象（Id、PhoneNumber）与游标分页
features/identity_contract  identity 域实体与错误（L10nKeyedError）
features/identity         登录 / 账号 CRUD / 改密
features/file_contract    file 域契约（格式检测、staging 路径）
features/file             图片上传端点
```

## modrun 约定

* **组合根** `supply(Cli)`、`supply(OpenApi)`；tracing 在 `Modrun::builder()` **之前**初始化
* **基础设施** 用 `provide` / `provide_result_async` 注册单例
* **HTTP 路由** 用 `provide_group` 贡献 `PublicRouter` / `ProtectedRouter`（`OpenApiRouter` + `utoipa_axum::routes!`）
* **长生命周期任务** 用 `task_with` 托管 HTTP 服务

## 新增 feature 模块

分层、endpoint 命名、`module()` 注册、OpenAPI、迁移与测试约定见 [AGENTS.md](AGENTS.md)。

## 数据库迁移

迁移文件：

* `infra/migration/versions/` — 业务表（accounts 等）
* `crates/cache_kit/migrations/` — 缓存表（`_pg_caches`）

启动时由 `persistence` 模块自动执行（与 `bin/migrator` 共用 `migration::run_migrations`）。**部署/CI 推荐先单独跑迁移**，再滚动发布 server：

```bash
just sqlx_up
# 或
cargo run -p migrator
```

需 `.env` 中 `DATABASE_URL` 指向可写库。仍可用 sqlx CLI 手动迁移：

```bash
sqlx migrate run --source infra/migration/versions --database-url "$DATABASE_URL"
```

**SQL 查询**：优先使用 `sqlx::query!` / `query_as!` / `query_scalar!` 编译期校验；编译时需可访问的 `DATABASE_URL`（指向已跑过迁移的库），不使用 offline 模式。

## 集成测试

identity 模块的 account CRUD 使用 `#[sqlx::test]` 直连 PostgreSQL（`cargo test -p identity`）。复杂逻辑可拆 `execute(pool, ...)` 供 handler 与测试共用。

## E2E（Hurl）

```bash
# 服务已监听 .env 中的 LISTEN_ADDR
just e2e                        # identity 主链路
just e2e_file                   # 图片上传（需 blob-fs 或 COS 配置正确）
```

用例见 `e2e/*.hurl`，变量见 `e2e/env`（可从 `e2e/env.example` 复制）。

## 常用命令

```bash
just dev          # cargo watch：cache-pg + blob-fs + job-queue-pg，无 otlp（非生产 default）
just check        # 与 just dev 相同 feature 集的 cargo check
just pre_commit   # machete + fmt + clippy(--all-features) + arch_test（全后端 unification，非轻量图）
just db-up        # docker compose up -d
just sqlx_up      # 独立执行数据库迁移（cargo run -p migrator）
just test         # cargo test --workspace（多根 unification，可能同时编 cos+fs）
just ai-check <crate>  # 架构/格式/编译/测试快速自检（web_state 自动加 blob-fs）
just test-features     # feature 组合测试（redb / redis / test-utils / sqlite）
just who-uses <symbol> # 查符号引用面
just new-feature <name>  # 生成 feature + contract 骨架
just load-baseline       # login/search 负载锚点（服务需已启动）
cargo test -p identity   # 集成测试（需 DATABASE_URL）
```
