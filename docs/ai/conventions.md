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
| `features/{name}/` | 端点、repository、域 FTL、OpenAPI DTO | 依赖其他 feature runtime |
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

## 依赖变更

改 `*_contract`、跨域 `pub` API、OpenAPI/DTO 前：`just who-uses <符号>`；≥3 个 crate 先与用户确认。

## 环境变量

可选基础设施后端的环境变量以 **编译 feature 前缀** 区分，避免不同后端配置互相覆盖；完整列表见根目录 `.env.example`。`cache-pg` / `job-queue-pg` 共用 `DATABASE_URL`。

## 测试

* 集成：`#[sqlx::test]` + `migration::run_migrations`（`cargo test -p identity --features blob-fs` 或 `just ai-check identity`）
* E2E：`just e2e`（变量 `e2e/env.example`）
* 架构：`cargo test -p server arch_test`（含 tree golden）；本地也可 `just tree-server`
