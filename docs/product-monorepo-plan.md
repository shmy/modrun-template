# 产品 Monorepo 组织计划

> 基于 `modrun-template` 现有分层（`crates/` / `infra/` / `features/` / `bin/`）扩展的产品化改造计划。
> 尚未实施，供后续评审与落地参考。
>
> 复核说明：本文已按当前代码全面核对——组合根 `bin/server/main.rs` 的完整模块注册顺序、`http_server` 的 `PublicRouter`/`ProtectedRouter` 组机制、`config::Cli` 的环境变量清单、`file` feature 与 `*_contract` 拆分现状、`e2e/` 与 CI 现状。与旧版的主要差异见「与当前代码的核对结论」。

## 目标

在单一仓库内承载四个子项目：

| # | 子项目 | 说明 |
|---|--------|------|
| 1 | 微信小程序 | `apps/mini-program/` |
| 2 | C 端 API | `bin/consumer-server/`（modrun 组合根） |
| 3 | B 端 API | `bin/admin-server/`（由现 `bin/server` 演进） |
| 4 | 后台 SPA | `apps/admin-web/` |

## 核心原则

| 原则 | 原因 |
|------|------|
| **单一 monorepo** | AI 与开发者可一次 `grep` 串起「前端 → handler → repository → 迁移」；契约不易漂移 |
| **两个 Rust 组合根，共享 `features/`** | 业务逻辑只写一份；C/B 端只暴露不同路由面 |
| **契约先行** | `*_contract` + OpenAPI 是前后端共享的「真相源」 |
| **文档按应用边界分层** | 根 `AGENTS.md` 讲全局，各 app 有自己的 `AGENTS.md` |

### 是否更省 Token？

**不会因为「放在一个仓库」就自动省 token**，但通过缩小每次对话需读的范围可间接节省：

* 按受众拆 `endpoint/consumer` / `endpoint/admin` → grep 更窄
* `docs/api-surface.md` 作端点索引 → 少翻文件
* `contracts/*.openapi.json` → 前端任务不必读 Rust
* 共享 `repository.rs` → 避免双仓库对照与重复修改
* 配合 `.cursorignore`、短根 `AGENTS.md`、对话时 `@` 限定目录

多仓库仅在「只改单一前端、且完全无后端联动」时略占优；涉及 API 联动时 monorepo 通常更省往返与返工。

***

## 推荐目录结构

```
your-product/                              # 单一 monorepo
│
├── AGENTS.md                              # 全局：分层约定、双服务差异、常用命令（保持简短，链到子文档）
├── README.md
├── .cursorrules
├── .cursorignore                          # 见下文「Token 优化」
├── Cargo.toml                             # workspace：两个 bin + 现有 crates/infra/features
├── Cargo.lock
├── rust-toolchain.toml / rustfmt.toml
├── justfile                               # dev / dev-consumer / e2e-admin / e2e-consumer（现有 dev / e2e / e2e_file 演进）
├── docker-compose.yaml
├── Dockerfile.ci                          # 现为 COPY ./server；双 bin 后需调整
├── .env.example
│
├── docs/
│   ├── ai/                                # backend.md / conventions.md（现有）
│   ├── architecture.md                    # 系统架构、部署拓扑（待写）
│   ├── api-surface.md                     # 端点归属表（consumer / admin / handler 文件，待写）
│   └── product-monorepo-plan.md           # 本文件
│
├── contracts/                             # API 契约（CI 或 just 导出）
│   ├── consumer.openapi.json
│   ├── admin.openapi.json
│   └── generated/                         # openapi-typescript / orval 生成物（应 ignore）
│
├── bin/                                   # Rust 组合根（modrun 入口）
│   ├── migrator/                          # 独立迁移二进制（现有，两服务共用）
│   ├── consumer-server/                   # 面向小程序 API
│   │   ├── Cargo.toml
│   │   ├── main.rs
│   │   ├── api_doc.rs
│   │   ├── wiring.rs                      # 对应现 bin/server/wiring.rs（internal_jobs 组合，按需裁剪）
│   │   └── internal_jobs.rs
│   └── admin-server/                      # 面向后台 API（由现 bin/server 演进）
│       ├── Cargo.toml
│       ├── main.rs
│       ├── api_doc.rs
│       ├── wiring.rs                      # server.wiring：JobHandlers + SchedJobs（CacheGc/JobQueueGc/BlobGc）
│       ├── internal_jobs.rs
│       └── arch_test.rs                   # 架构守护测试随 bin 迁移
│
├── crates/                                # 框架无关可复用库（现有 9 个，不变）
│   ├── authn_kit/  jwt_kit/  cache_kit/   blob_kit/  image_kit/
│   ├── job_queue_kit/  sched_kit/  trace_kit/  validify_kit/
│
├── infra/                                 # modrun 接线 + HTTP 栈（几乎全部共享）
│   ├── config/                            # Cli / 环境变量（双 bin 复用同一 Cli，见「双服务差异」）
│   ├── persistence/  migration/versions/  cache/  blob/
│   ├── job_queue/  scheduler/  web_state/  web_error/
│   ├── locale/locales/{en-US,zh-CN}/      # shared.ftl / identity.ftl / file.ftl
│   ├── http_auth/                         # 鉴权中间件（现状：http_server 统一挂，共享）
│   └── http_server/                       # module() + PublicRouter/ProtectedRouter + 限流（单模块，两 bin 各自运行一份）
│
├── features/                              # 业务域（repository 共享，路由面分离）
│   ├── shared_contract/                   # Id / CursorPaging / PhoneNumber 等
│   ├── health/
│   ├── identity/
│   │   ├── dto.rs
│   │   ├── repository.rs                  # 共享（含 privileged/管理语义 SQL，见待决策项）
│   │   ├── shared/                        # token_ops / domain_error / cache_error（现有）
│   │   ├── endpoint/
│   │   │   ├── consumer/                  # 如 wechat_login.rs
│   │   │   └── admin/                     # 现有 account_*.rs / profile_current.rs 迁入
│   │   ├── lib.rs                         # consumer_module() + admin_module()
│   │   └── tests.rs
│   ├── identity_contract/
│   ├── file/                              # 现有第二业务域（endpoint + file_contract 拆分的既成范例）
│   │   ├── endpoint/  shared/  lib.rs
│   ├── file_contract/
│   └── order/                             # 示例新业务域
│       ├── endpoint/
│       │   ├── consumer/
│       │   ├── admin/
│       │   └── common/                    # pub(crate) execute，两边复用（避免与域级 shared/ 混淆）
│       ├── repository.rs
│       └── lib.rs
│
├── apps/                                  # 非 Rust 前端
│   ├── mini-program/
│   │   ├── AGENTS.md
│   │   ├── project.config.json
│   │   └── src/                           # pages/ components/ services/ utils/
│   └── admin-web/
│       ├── AGENTS.md
│       ├── package.json
│       └── src/                           # api/ views/ router/
│
└── e2e/                                   # 现状为平铺：env(env.example) / identity.hurl / file.hurl / fixtures/
    ├── admin/                             # 现有 identity.hurl、file.hurl 归入此处
    │   ├── env.example
    │   └── *.hurl
    └── consumer/
        ├── env.example
        └── *.hurl
```

### 依赖关系

```
apps/mini-program ──HTTP──▶ bin/consumer-server ──┐
                                                   ├──▶ features/* ──▶ infra/persistence ──▶ PostgreSQL
apps/admin-web ────HTTP──▶ bin/admin-server ──────┘
         │                        │
         └──── contracts/*.openapi.json（类型/客户端生成）
```

### 与当前仓库的对应

| 现有 | 演进后 |
|------|--------|
| `bin/server/` | → `bin/admin-server/`（包名 `server` 改名，牵连项见「迁移路径」） |
| `features/identity/endpoint/*.rs`（平铺 11 个 handler） | → `features/identity/endpoint/admin/*.rs` |
| `features/file/` | 路由面同样拆 consumer/admin；`file_contract` 已是契约拆分范例 |
| `e2e/identity.hurl`、`e2e/file.hurl`、`e2e/env` | → `e2e/admin/` |
| （无） | 新增 `bin/consumer-server/`、`apps/*`、`contracts/`、`docs/api-surface.md` |

***

## 与当前代码的核对结论

旧版计划与代码不符、已在本文修正的要点：

1. **`http_server` 不需要拆 `consumer_module()`/`admin_module()`**。现状机制：各 feature 通过 `provide_group(public_routes)` / `provide_group(protected_routes)` 贡献路由组，`infra/http_server::module()` 把所有 `Group<PublicRouter>` / `Group<ProtectedRouter>` 合并后统一挂鉴权中间件、限流、l10n。两个 bin 各自运行一份 `http_server::module()` 即可，模块内代码无需改动。需要拆分的是 **feature 层**的 `module()`。
2. **组合根 sketch 必须完整**。现 `bin/server/main.rs` 注册顺序为：`supply(Cli)` → `supply(ApiDoc)` → `persistence` → `cache` → `blob` → `job_queue` → `web_state` → `server.wiring` → `job_worker` → `scheduler` → `identity` → `file` → `health` → `http_server`。consumer-server 不能只挂 persistence + identity + http_server：`web_state` 依赖 cache/persistence，鉴权与限流依赖 `Cli`，且缓存 GC、Blob GC 等内务任务在 `bin/server/internal_jobs.rs`（`server.wiring` 提供 `JobHandlers` + `SchedJobs`），属 bin 内代码、需随 bin 复制并按需裁剪。
3. **环境变量名**：现 `config::Cli` 只有 `LISTEN_ADDR`（默认 `0.0.0.0:8080`），不存在 `CONSUMER_PORT`/`ADMIN_PORT`。双 bin 复用同一 `Cli`，端口区分靠各自进程的 `LISTEN_ADDR`（如 `:8080` / `:8081`）；`MASTER` 控制是否 seed 管理员与注册 `scheduler.run`。若两服务需要的默认值差异大，再考虑给 `config` 加前缀分组（待决策）。
4. **API 路径前缀**：现状全部端点为 `/api/v1/...`（无 `a/`、`c/` 前缀），AGENTS.md 端点表、`e2e/*.hurl` 均按此编写。引入 `/api/v1/a` 与 `/api/v1/c` 属**破坏性变更**，必须同步 e2e、AGENTS.md、契约与已有前端；见「待决策项」。
5. **鉴权中间件是共享的，不是分离的**。`account_auth_middleware` 由 `http_server` 统一挂在 `ProtectedRouter` 上；C 端若沿用 identity 的 JWT 会话（`token_ops`），中间件、`AuthedAccount`（`shared_contract::Id`）均直接复用。分离的只是**登录方式**（微信 code 换会话 vs 手机号+密码）。
6. **双进程不能共享本机文件类资源**。旧版「共享 docker compose 基础设施」表述不完整：`CACHE_REDB_PATH`（默认 `data/cache.redb`）、`JOB_QUEUE_SQLITE_PATH`（默认 `worker.db`）、`BLOB_FILE_ROOT`（默认 `data/blob`）都是独占文件/目录，两进程必须指向不同路径，或 C 端改用 redis 缓存、不跑 worker。详见「双进程资源隔离」。
7. **`e2e/` 现状为平铺**（`env`、`identity.hurl`、`file.hurl`、`fixtures/`），justfile 现有命令是 `dev` / `e2e` / `e2e_file` / `build*` / `ai-check` / `who-uses` / `pre_commit`，并非旧版假设的 `dev-consumer` 等；落地时是新增而非改名。
8. **OpenAPI 出口已具备**：每个 bin 暴露 `/openapi.json`（`http_server` 挂载），Scalar UI 在 `APP_ENV=production` 自动关闭。缺的只是 `contracts/` 目录与导出流水线。

***

## 双 API 服务如何拆分

### 1. feature 层按受众拆 module（路由组按受众命名）

每个 feature 导出两个 `module()`，各自只 `provide_group` 对应路由面；域错误 mapper 注册（现为 `register_identity_domain_mapper`）需在两个 module 中都调用：

```rust
// features/identity/lib.rs
pub fn consumer_module() -> Module {
    shared::domain_error::register_identity_domain_mapper();
    Module::builder("identity")
        .invoke(register)          // seed 逻辑只应留在 admin_module（见下）
        .provide_group(consumer_public_routes)
}

pub fn admin_module() -> Module {
    shared::domain_error::register_identity_domain_mapper();
    Module::builder("identity")
        .invoke(register)          // seed_admin 受 MASTER 开关控制，归 admin 侧
        .provide_group(admin_public_routes)
        .provide_group(admin_protected_routes)   // 现有 protected_routes 整体迁入
}
```

注意：`http_server::auth_layer(&cli)`（登录/刷新的更紧限流层）目前在 `identity::public_routes` 内自行挂载，拆分后 consumer 侧登录路由也要挂同一层。`modrun` 模块名在同一进程内必须唯一，两个 bin 分别注册各自的 module 函数即可，互不冲突。

### 2. 组合根各自完整组装

以现 `bin/server/main.rs` 为蓝本（模块顺序不可随意精简）：

```rust
// bin/consumer-server/main.rs（示例：C 端暂无文件上传）
Modrun::builder()
    .supply(cli)                       // config::load()
    .supply(ApiDoc::openapi())         // 本 bin 自己的 api_doc.rs
    .module(persistence::module())
    .module(cache::module())           // C 端建议 redis 后端，避免 redb 文件独占
    .module(job_queue::module())       // 若 C 端不跑 worker，见「双进程资源隔离」
    .module(web_state::module())
    .module(wiring::module())          // 本 bin 的 internal_jobs（按需裁剪 GC 任务）
    .module(job_worker::module())
    .module(scheduler::module())
    .module(identity::consumer_module())
    .module(order::consumer_module())
    .module(health::module())
    .module(http_server::module())     // 共享模块，每个 bin 各运行一份
    .run()
    .await
```

裁剪规则：C 端不用的 feature（如 `file`）整体不注册，则 `wiring` 里的 `BlobGc` 也要同步移除（其 handler 依赖 `blob` 模块提供的 `Blob`）；反之 `CacheGc`/`JobQueueGc` 只要跑了 cache/job_queue 就应保留。

### 3. endpoint 按受众分子目录，逻辑共享

* `endpoint/consumer/` — 小程序 handler（薄层，调 execute）
* `endpoint/admin/` — 后台 handler
* `endpoint/common/` 或各 handler 内 `pub(crate) async fn execute(...)` — 共享业务逻辑（**不要**命名为 `endpoint/shared/`：feature 级已有 `shared/` 模块，如 `identity/shared/`，双 `shared` 易混淆）
* **不要**复制 `repository.rs` 或 SQL

（现状惯例：含副作用的逻辑必须放进 `execute` 并由 `#[sqlx::test]` 集成测试覆盖，见 `account_delete.rs` / `account_update_password.rs`；拆分后该惯例继续适用。）

### 4. API 路径前缀（便于 grep 与文档）

| 服务 | 建议前缀 | 示例 |
|------|----------|------|
| consumer | `/api/v1/c/...` | `/api/v1/c/orders` |
| admin | `/api/v1/a/...` | `/api/v1/a/accounts` |

**风险提示**：现有端点全部是 `/api/v1/...`。改 admin 前缀需同步：`e2e/admin/*.hurl`、AGENTS.md 端点表、`docs/api-surface.md`、已导出契约与任何已对接客户端。更保守的替代：admin 保持 `/api/v1/...`，仅 consumer 使用 `/api/v1/c/...`（前缀只在新增侧引入，旧路径零改动）。

### 5. 各子项目职责

**小程序 (`apps/mini-program/`)**

* 微信登录、订阅消息、支付等微信特有逻辑只放此处
* HTTP 调 consumer-server；类型可从 `contracts/consumer.openapi.json` 生成

**Consumer API (`bin/consumer-server/`)**

* 鉴权：微信 code 换 session / 自建 JWT（复用 `token_ops` 会话签发）
* 只注册 C 端 feature 子集（不含 privileged 账号管理）
* 启动配置示例：`LISTEN_ADDR=0.0.0.0:8080`、`MASTER=false`（跳过 seed 管理员、不注册 `scheduler.run`）

**Admin API (`bin/admin-server/`)**

* 现有 identity privileged CRUD、后台业务
* 鉴权：手机号+密码 / Bearer JWT
* 启动配置示例：`LISTEN_ADDR=0.0.0.0:8081`、`MASTER=true`（种子管理员，唯一特权账号由 `accounts_one_privileged` 部分索引保证）

**后台 SPA (`apps/admin-web/`)**

* 调 admin-server；OpenAPI 生成 API client

***

## 双进程资源隔离（本地与容器部署都必须处理）

两个 server 若同机部署（本地 `cargo run` 或同一 docker-compose），以下配置**必须按进程区分**：

| 资源 | 环境变量 | 默认值 | 冲突 |
|------|----------|--------|------|
| redb 缓存文件 | `CACHE_REDB_PATH` | `data/cache.redb` | 文件锁独占，双进程共用会启动失败或数据损坏 |
| job 队列 SQLite | `JOB_QUEUE_SQLITE_PATH` | `worker.db` | 双进程共库=双 worker 抢占同一队列（job_queue_kit 支持与否待确认）；推荐 C 端不同文件或完全不跑 worker |
| blob 根目录 | `BLOB_FILE_ROOT` | `data/blob` | staging/ 清理（`BlobGc`）会互删对方目录 |
| OSS/COS 前缀 | `BLOB_S3_*` | — | 若共用 bucket，需按服务前缀区分 key |

无状态资源可安全共享：PostgreSQL（同一库、migration 只跑一份）、redis 缓存（建议按 key 前缀分命名空间）、OpenAPI/l10n 等只读资源。

`worker.db` 双写问题落地前必须实测；若 job_queue_kit SQLite 不支持多消费者，C 端需要 job 时应改用 PG 队列或独立文件。

***

## 共享 vs 分离

| 共享 | 分离 |
|------|------|
| `crates/*`、`infra/persistence`、`infra/migration`（迁移只跑一份） | `bin/*` 组合根（含各自 `wiring`/`internal_jobs`/`api_doc`） |
| `features/*/repository.rs`、领域实体、`*_contract` | `endpoint/consumer` vs `endpoint/admin` |
| `infra/http_auth` 鉴权中间件、`http_server` 限流与 l10n 中间件 | 登录方式（微信 code vs 手机号+密码）、OpenAPI 文档、CORS 策略 |
| PostgreSQL（同一库）、redis（按前缀分命名空间） | **本机文件资源**：redb 缓存、worker.db、blob 目录（见上表） |
| l10n ftl（`shared.ftl` + 各域 `identity.ftl`/`file.ftl`，双 locale 同步补） | `apps/*` 前端代码；E2E 用例分 `e2e/consumer` 与 `e2e/admin` |
| docker compose 基础设施（PG/redis） | 服务进程与端口 |

**不建议**为 C/B 端各建一套 `features/order` 副本。

***

## AI 上下文与文档分层

### AGENTS.md 层次

```
AGENTS.md                           # 全局 ≤150 行：分层、modrun、双服务差异、链接
apps/mini-program/AGENTS.md         # 页面结构、登录流、禁止调 admin API
apps/admin-web/AGENTS.md            # 路由、权限、表单与 API 对照
docs/api-surface.md                 # 端点总表（见下表模板）
```

### api-surface.md 模板

| 受众 | 方法 | 路径 | handler 文件 |
|------|------|------|--------------|
| consumer | POST | `/api/v1/c/auth/wechat` | `identity/endpoint/consumer/wechat_login.rs` |
| admin | POST | `/api/v1/identity/login` | `identity/endpoint/admin/account_login.rs` |

新 endpoint 必须更新此表。基础数据可直接取自 AGENTS.md 现有 identity/file 端点表。

### OpenAPI 契约枢纽

1. 两个 server 各暴露 `/openapi.json`（现状已具备，Scalar UI 生产自动关闭）
2. `just export-openapi` 或 CI 写入 `contracts/*.openapi.json`（新增：起服务→拉取→落盘→关服务，或加一个 `--print-openapi` 模式）
3. admin-web / 小程序用 orval、openapi-typescript 等生成客户端

### 对话时缩小范围（Cursor `@`）

| 任务 | 建议 @ |
|------|--------|
| 改小程序订单列表 | `apps/mini-program` + `features/order/endpoint/consumer` |
| 改后台账号搜索 | `apps/admin-web` + `features/identity/endpoint/admin` |
| 改共享领域逻辑 | `features/*/repository.rs` + 相关 `*_contract` |

***

## Token 优化：`.cursorignore` 建议

落地 monorepo 后可在仓库根添加：

```gitignore
# 前端依赖与构建产物
apps/*/node_modules/
apps/*/dist/
apps/*/build/
**/.turbo/

# 契约生成物（保留 *.openapi.json 源文件）
contracts/generated/

# 小程序 vendor / 构建
apps/mini-program/miniprogram_npm/

# Rust 构建（若 Cursor 仍索引 target 可保留此项）
/target/
```

根 `AGENTS.md` 保持索引式：细节链到 `docs/` 与子目录 `AGENTS.md`，避免 rules 每次注入过长正文。

***

## 本地开发与部署

### justfile 现状与演进

现有命令（落地时在此基础上新增，不动语义）：`dev` / `build*`（含 CI profile 与 linux 交叉构建，**硬编码 `-p server`**）/ `sqlx_up` / `db-up` / `e2e` / `e2e_file` / `who-uses` / `ai-check` / `pre_commit` / `test` / `test-features`。

```just
# 建议新增
dev-consumer:
    cargo watch -q -c -x "run -p consumer-server" -w bin/consumer-server -w features -w infra -w crates

dev-admin:   # 现有 dev 改名（-p server → -p admin-server）
    cargo watch -q -c -x "run -p admin-server" -w bin/admin-server -w features -w infra -w crates

e2e-admin:
    hurl --test --variables-file e2e/admin/env e2e/admin/*.hurl

e2e-consumer:
    hurl --test --variables-file e2e/consumer/env e2e/consumer/*.hurl
```

### CI / Docker 联动清单（改名 `bin/server` → `bin/admin-server` 的必改点）

| 文件 | 现状 | 需改为 |
|------|------|--------|
| `.github/workflows/deploy.yaml` | `just build_linux_x86_64_gnu_ci`（经 justfile 构建 `-p server`） | 双 bin 构建 + `cargo run --package migrator` 保持 |
| `Dockerfile.ci` | `COPY ./server server` + `ENTRYPOINT /opt/server` | 按 bin 分别 COPY（或双阶段构建两个镜像） |
| justfile `build*` | `cargo build --package server ...` | `--package admin-server`，并补 consumer-server 对应命令 |
| AGENTS.md / pre_commit / ai-check | `cargo test -p server arch_test` | `-p admin-server arch_test` |
| `e2e/` | 平铺 `identity.hurl`/`file.hurl`/`env` | 迁入 `e2e/admin/` |

### docker-compose 概念

```yaml
services:
  postgres: ...
  redis: ...            # C 端缓存后端建议
  consumer-api:
    # target consumer-server
    environment:
      LISTEN_ADDR: 0.0.0.0:8080
      MASTER: "false"
      CACHE_REDIS_URL: redis://redis:6379
      JOB_QUEUE_SQLITE_PATH: /data/consumer-worker.db   # 与 admin 区分
    ports: ["8080:8080"]
  admin-api:
    # target admin-server
    environment:
      LISTEN_ADDR: 0.0.0.0:8081
      MASTER: "true"
      JOB_QUEUE_SQLITE_PATH: /data/admin-worker.db
    ports: ["8081:8081"]
```

***

## 渐进迁移路径

1. **本阶段**：仅文档（本文件），现有 `bin/server` 不动
2. `bin/server` → `bin/admin-server` 改名，同步 justfile `build*`、`Dockerfile.ci`、`deploy.yaml`、AGENTS.md 命令、`arch_test` 引用（`e2e` 迁入 `e2e/admin/`）——此步纯机械改名，改完跑 `just pre_commit` + `just e2e-admin`
3. feature 路由面拆分：`identity/endpoint/*.rs` → `endpoint/admin/`，`lib.rs` 拆出 `consumer_module()`/`admin_module()`（先都注册进 admin-server，行为不变）
4. 新建 `bin/consumer-server`：完整模块组装 + `health` + `identity::consumer_module()`（微信登录 stub）
5. 加 `apps/admin-web`，用 OpenAPI 生成 client；导出 `contracts/admin.openapi.json`
6. 加 `apps/mini-program`，对接 consumer API；导出 `contracts/consumer.openapi.json`
7. 补 `docs/api-surface.md`、各 app 的 `AGENTS.md`、`.cursorignore`
8. 实测 `worker.db` 双进程行为，确定 C 端 job 方案

***

## 待决策项（评审时填写）

* \[ \] API 路径前缀：admin 改 `/api/v1/a`（破坏性）vs admin 保持 `/api/v1`、仅 consumer 用 `/api/v1/c`（推荐，零迁移成本）
* \[ \] 小程序用户与 `accounts` 表：同一表扩展 vs 独立 `wechat_users`。注意共享 `repository.rs` 里有大量 privileged/管理语义 SQL（`accounts_one_privileged`、乐观锁、`account_protected` 等），C 端复用会把管理面 SQL 暴露进 consumer 进程；倾向独立 `wechat_users` + 单独 repository
* \[ \] C 端身份类型：现 `AuthedAccount` 产出 `shared_contract::Id`（accounts.id）；若 C 端独立用户表，需要新的鉴权 claim/提取器或在 `accounts` 上加用户类型列
* \[ \] `worker.db` 双进程：实测 SQLite 多消费者行为；C 端 job 需求若为空可裁掉 `job_worker`+`scheduler`
* \[ \] config 是否按 bin 前缀分组（`CONSUMER_LISTEN_ADDR` vs 共用 `LISTEN_ADDR`）
* \[ \] admin-web 技术栈：Vue / React / 其他
* \[ \] 生产部署：两服务同域不同 path vs 不同子域（影响 TRUSTED_PROXIES 与限流按 IP 的正确性）
* \[ \] OpenAPI 生成工具：orval / openapi-typescript / 其他；契约导出是 just 命令还是 CI 步骤

***

## 参考

* 当前分层约定：[AGENTS.md](../AGENTS.md)
* 现有组合根：[bin/server/main.rs](../bin/server/main.rs)（迁移蓝本，注意完整模块顺序）
* 内务任务与 wiring：[bin/server/internal_jobs.rs](../bin/server/internal_jobs.rs)
* 现有 feature 路由注册：[features/identity/lib.rs](../features/identity/lib.rs)
* 环境变量全表：[infra/config/cli.rs](../infra/config/cli.rs) 与 `.env.example`
