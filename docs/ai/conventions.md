# 编码细则（AI 按需）

根 [AGENTS.md](../../AGENTS.md) 是路口。**本文件不要每轮先读**——写错误 key、HTTP 方法、或踩陷阱时打开对应小节。

新建域与 crate 落位：见 [backend.md](backend.md)。

## 分层

| 目录 | 允许 | 禁止 |
|------|------|------|
| `crates/` | 通用 `*_kit` | modrun、axum、业务实体 |
| `infra/` | 运行时接线 | 业务域逻辑 |
| `features/{name}_contract/` | 跨域错误/读模型（纯 serde/thiserror） | utoipa/axum/sqlx/validify、其他 contract（除 `shared_contract`）、infra、runtime |
| `features/shared_contract/` | 跨域值对象 + 游标分页（纯） | 同上 |
| `features/{name}/` | 端点、repository、域 FTL、OpenAPI DTO | 依赖其他 feature runtime（例外：`identity`→`file` 附件登记；第二消费者再拆 `attachment`） |
| `bin/server` | 组合根 | 业务逻辑 |

`arch_test` 在 CI / `just pre_commit` / `just ai-check` 中强制执行上表（含 kit 禁 modrun/axum、contract 禁框架/DB）。另含生产 default 下 `shared_contract` 的 `cargo tree` golden（`just tree-server`）；失败=依赖图回潮，有意改动用 `just tree-server-bless`。

## Endpoint

* 单文件 `features/{domain}/endpoint/{entity}_{action}.rs`：DTO + `#[utoipa::path]` + `handler`（+ `execute` 若有副作用）
* handler 返回 `JsonResponseType<T>`；健康探针返回裸 `Json`
* `State<AppState>`；业务逻辑 `&impl HasSession` / `HasAuthCaps` / `HasPool` / `HasBlob`（`routes!` 不支持泛型 handler）
* 含副作用（`revoke_tokens` 等）必须在 `execute`，handler 不重复逻辑
* 契约类型无 `ToSchema`：字段 `#[schema(value_type = …)]`；手机号请求体用 `String` + `validate_phone_number`，通过后 `PhoneNumber::new_unchecked` / `phone()`

## 错误与 i18n

* 领域错误 `thiserror` + `#[error("snake_case_key")]`
* 通用/鉴权 key → `infra/locale/locales/*/shared.ftl`（`-p locale` 测）；域 key → `features/{domain}/locales/*/` + `register_ftl`（该 feature 测）
* **禁止** `infra/locale` 测试 `include_str!` features/；**禁止**同一 key 出现在多个 ftl
* 域错误实现 `L10nKeyedError` 并在 `module()` 注册 mapper
* 状态码：命名约定（`*_not_found`→404、`*_forbidden`/`*_protected`→403、`*_duplicated`/`*_version_conflict`→409、`*_invalid_credentials`/`*_token_invalid`/`access_token_*`→401）；勿在 `web_error` 加域特例白名单

## 附件引用（blob / staging → 正式 path）

业务表**只存正式 path**（无 `staging/`、无公网 URL）。上传走 `POST /api/v1/files/images`；转正与引用计数只经 `file::attachment`（蓝本：`identity` 头像 / `avatar_ops`）。

### 状态与谁清理

| 状态 | 条件 | 清理方 |
|------|------|--------|
| staging | 仅对象存储，无 `blob_objects` 行；cache 有上传者 claim | `BlobGc`（标准 `staging/YYYY/MM/` 按月；顶层/年目录下非标准文件一并删） |
| pending | 正式 path，`ref_count=0` 且未 tombstone | `AttachmentGc`（默认 24h） |
| attached | `ref_count≥1` | 业务 `detach` |
| tombstone | `ref_count=0` 且已打标 | `AttachmentGc`（默认 7d） |

`commit_staging`：**消费上传者 claim → INSERT `blob_objects` → mv**；INSERT/mv 失败回滚登记行并**恢复 claim**（同 path 可重试）。禁止自写「先 mv 再 INSERT」。上传成功须 `claim_staging`；非上传者 commit → `file_attachment_invalid`（不泄露归属）。

### 公网读（`blob-fs`）

`GET /files/{*path}` 仅 `blob-fs` 注册（COS 自带公网）。走 `PublicRouter`（`<img>` 无 Authorization）；保密性依赖不可猜测 TempoId 文件名。白名单：`is_public_readable_object_path`（`staging/YYYY/MM/...` 与转正 `YYYY/MM/...`）；拒绝穿越 / 空段 / 其它前缀。

### 新表 / 新字段接入清单

1. **列语义**：`TEXT`（或列表）存正式 path；API 入参可收 staging / 公网 URL，落库前必须 commit（且 commit 者须为上传者）。
2. **单 path（头像式）**：域内薄封装（参考 `avatar_ops`）→ `commit_staging` / `attach` / `detach`；错误映射到本域 `L10nKeyedError`。补偿路径才用 `detach_best_effort`。
3. **多 path**：优先 `sync_attachments(old, new)`（返回等长正式 path 列表再落库）；其中途失败会回滚本轮 attach / 对新 commit 打 tombstone，**仍须**对业务行做补偿（见下）。
4. **推荐顺序与补偿**（blob 与 PG 无分布式事务；**业务行与 refcount 须同 PG 事务**）：
   - 无副作用前置（哈希、鉴权、查重）→ `commit` → **单事务**：写业务行（有并发则带 `version`）+ `attach` + 对旧 path `detach` → commit 事务。
   - `commit` 之后事务失败（含乐观锁 409）：对本轮新 path `detach`（tombstone）；**禁止**「错误响应却已占用唯一键」。
   - 禁止写行成功后另开请求再 attach（会留下「行有 path、refcount=0」窗口，只能靠安全网兜底）。
5. **删实体**：授权通过后，**同事务**内先 `detach`（登记缺失可 warn 后继续，勿卡死删号）再 DELETE；**`revoke_tokens` 放在 tx.commit 之后**（避免事务失败时会话已吊销、账号仍在）。禁止先删行再 best-effort detach。
6. **AttachmentGc 安全网（必做）**：`filter_gc_candidates` + 业务 `*_paths_in_use`；组合根见 `bin/server/internal_jobs.rs`。当前仅 `identity::avatar_paths_in_use`。**新表有附件列时**：提供 `*_paths_in_use` 并合并进 GC。漏加 = 补偿失败时可能被 GC 删 blob 而成死链。
7. **分层**：首个消费者允许 `identity`→`file`；**第二个 feature runtime 依赖 `file` 附件时**，把登记 API 抽到独立 crate（见分层表「第二消费者再拆 `attachment`」），再改 `arch_test` / tree 若需要。
8. **测试**：happy path refcount；换绑/清空 tombstone；删实体 detach；乐观锁冲突后新 commit 已 tombstone 且 version 未 bump；`commit` 源缺失无残留登记行；非上传者 claim；`filter_gc_candidates` + `list_gc_candidates`（见 `file::attachment` 测例）。

### 不要做的事

* 业务表直接存 `staging/...` 或依赖 BlobGc「迟早会清」代替引用计数
* 绕过 `blob_objects` 手写正式对象 path（GC 扫不到 → 存储泄漏）
* 只 `commit` 不 `attach` 却长期依赖 24h pending（应用失败路径应立刻 `detach`）
* 在 `features/file` 里写具体业务表 SQL；引用枚举留在各域 / 组合根 GC 处聚合
* 删号用 `detach_best_effort`；创建失败却留下已占手机号的账号

## 依赖变更

改 `*_contract`、跨域 `pub` API、OpenAPI/DTO 前：`just who-uses <符号>`；≥3 个 crate 先与用户确认。

## 环境变量

可选基础设施后端的环境变量以 **编译 feature 前缀** 区分，避免不同后端配置互相覆盖；完整列表见根目录 `.env.example`。`cache-pg` / `job-queue-pg` 共用 `DATABASE_URL`。

## 测试

* 集成：`#[sqlx::test]` + `migration::run_migrations`（`cargo test -p identity --features blob-fs` 或 `just ai-check identity`）
* E2E：`just e2e`（变量 `e2e/env.example`）
* 架构：`cargo test -p server arch_test`（含 tree golden）；本地也可 `just tree-server`
