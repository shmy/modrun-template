# modrun-template

基于 [modrun](https://docs.rs/modrun) + Axum 的 Rust 服务模板：身份登录、账号 CRUD、JWT 鉴权、OpenAPI、i18n、对象存储、后台任务与 PostgreSQL。

> 规则、分层、错误约定等实现约束见 [AGENTS.md](AGENTS.md)（唯一真相源）。本文只讲**怎么跑起来**与**项目长什么样**。

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

默认种子管理员（`SEED_ADMIN_PHONE` / `SEED_ADMIN_PASSWORD` 可覆盖）：手机 `13888888888`，密码 `admin123!`。`APP_ENV=production` 时禁止使用该默认密码。

## API

* `GET /openapi.json` — OpenAPI 契约（**唯一权威**，本文表格仅为导航）
* `GET /scalar` — Scalar UI（`SCALAR_UI_ENABLED=false` 或 `APP_ENV=production` 时关闭）

| 方法 | 路径 | 说明 |
|------|------|------|
| `POST` | `/api/v1/identity/login` | 公开。登录，返回 access / refresh token |
| `POST` | `/api/v1/identity/refresh` | 公开。刷新令牌（轮换 refresh） |
| `POST` | `/api/v1/identity/logout` | 登出 |
| `GET` | `/api/v1/profile/current` | 当前账号 |
| `PATCH` | `/api/v1/identity/password` | 本人改密（成功后吊销该账号令牌） |
| `POST` | `/api/v1/accounts` | 创建账号（需 `privileged`） |
| `GET` | `/api/v1/accounts` | 搜索（`cursor` / `limit` / `q` 游标分页；需 `privileged`） |
| `GET` `PATCH` `DELETE` | `/api/v1/accounts/{id}` | 读 / 改（乐观锁 `version`）/ 删（幂等）；需 `privileged` |
| `PATCH` | `/api/v1/accounts/{id}/avatar` | 更新头像（乐观锁 `version`） |
| `PATCH` | `/api/v1/accounts/{id}/password` | 管理员重置密码 |
| `POST` | `/api/v1/files/images` | 上传图片（multipart ≤2MiB），压缩为 WebP 写入对象存储 |
| `GET` | `/files/{*path}` | 对象回源（**仅 `blob-fs` 注册**；COS 自带公网读） |

探针：`/livez`（存活，不查依赖）、`/readyz`（就绪，检查 PG + cache）、`/healthz`（uptime、连接池）。三者走 `ProbeRouter`，**不经过全局限流**。

行为细则（鉴权模型、限流取舍、附件生命周期、错误状态码）见 [AGENTS.md](AGENTS.md) 与 [docs/ai/conventions.md](docs/ai/conventions.md)。

## 部署假设

* **TLS / HTTP/2**：由 Ingress / 反向代理终止；本进程只听 HTTP/1.1。
* **限流**：`axum-governor` 为**进程内**令牌桶；多副本配额相加。生产建议在边缘再限一层。
* **可观测**：`OTLP_ENDPOINT` 为空则关闭 OTLP 导出（仅 console）；非空时按 `OTLP_SAMPLER_RATIO`（默认 `0.1`）采样。不提供 Prometheus `/metrics` 拉模型。
* **多副本**：见 [AGENTS.md](AGENTS.md)「水平扩展」——共享 PG（及 redis cache），仅一台 `MASTER=true`，且每个实例必须有唯一 `TSID_NODE_ID`。

构建：`just build` 用 fat LTO 发布优化；CI 可用 `just build_release_ci`（更快、体积/峰值略逊）。去掉 OTLP 依赖：
`cargo build -p server --no-default-features --features cache-pg,blob-cos,job-queue-pg`。

## 模块结构

```
bin/server              组合根：supply(Cli, OpenApi) + 注册各模块（wiring / job_worker）
bin/migrator            独立迁移入口：业务迁移 + apalis schema setup（部署/CI 用）
crates/authn_kit        鉴权协议（cache key 命名、Verifier trait）
crates/jwt_kit          JWT 签发与校验（HS256 自实现，不引入非对称曲线）
crates/cache_kit        带 TTL 的缓存门面（pg / redb / redis + 原子滑动窗口）
crates/blob_kit         对象存储门面（COS / fs / 内存，opendal）
crates/job_queue_kit    后台任务队列（apalis 1.0；pg / sqlite，退避重试 / 超时 / 孤儿恢复）
crates/sched_kit        定时任务调度（tokio-cron-scheduler 封装）
crates/image_kit        图片压缩（JPEG/PNG → WebP，带解码尺寸与像素预算）
crates/validify_kit     serde / validify 错误 → l10n key + 字段路径
crates/trace_kit        tracing / OTLP 初始化
infra/config            CLI / .env（按 feature 前缀，见 `.env.example`）
infra/persistence       PgPool + 启动迁移
infra/cache             modrun 接线：按 feature 装配 Cache
infra/blob              modrun 接线：按 feature 装配 Blob
infra/job_queue         modrun 接线：JobBus + JobHandlers 登记
infra/scheduler         cron 注册表 + MASTER 门控
infra/web_state         WebState + HasPool/HasBlob/… 能力 trait
infra/web_error         WebError、JsonResponseType、域错误 mapper、RFC 9457
infra/http_auth         Bearer 鉴权中间件 + AuthedAccount 提取器
infra/http_extractors   ValidJson / ValidQuery / ValidPath / TypedMultipart / ClientIp
infra/http_routing      PublicRouter / ProtectedRouter / ProbeRouter + 限流层
infra/http_server       路由聚合、超时、Scalar、CORS
infra/locale            Fluent i18n（仅 shared.ftl；域 FTL 由 feature 自注册）
features/shared_contract    纯跨域值对象（Id / PhoneNumber）+ 游标分页
features/identity_contract   identity 域错误
features/identity           登录 / 账号 CRUD / 改密 + locales 自注册
features/file_contract      file 域契约（格式检测、staging 路径）
features/file               图片上传 + 附件引用登记 + staging GC + locales 自注册
features/health             健康探针
```

分层规则、`crates`/`infra`/`features` 各自能放什么 → [AGENTS.md](AGENTS.md)「分层约定」。

## 常用命令

```bash
just dev                 # cargo watch：cache-pg + blob-fs + job-queue-pg，无 otlp
just check               # 与 just dev 相同 feature 集的 cargo check
just lint <crate>        # 快门：fmt + clippy（秒级，迭代中用）
just ai-check <crate>    # 全门：lint + arch_test + 定向测试
just pre_commit          # 提交前最终门（machete + fmt + clippy 全后端 + deny + arch_test）
just test                # cargo test --workspace
just test-features       # feature 组合测试（redb / redis / test-utils / sqlite）
just tree-server         # shared_contract 依赖图快照核对（有意变更用 tree-server-bless）
just who-uses <symbol>   # 查符号引用面
just new-feature <name>  # 生成 feature + contract 骨架
just db-up / just sqlx_up
just e2e / just e2e_file # Hurl 端到端（服务需已启动；变量见 e2e/env）
```
