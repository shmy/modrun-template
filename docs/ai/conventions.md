# 编码细则（AI 按需）

根 [AGENTS.md](../../AGENTS.md) 是路口。**本文件不要每轮先读**——写错误 key、HTTP 方法、或踩陷阱时打开对应小节。

新建域与 crate 落位：见 [backend.md](backend.md)。

## 分层

| 目录 | 允许 | 禁止 |
|------|------|------|
| `crates/` | 通用 `*_kit` | modrun、axum、业务实体 |
| `infra/` | 运行时接线 | 业务域逻辑 |
| `features/{name}_contract/` | 跨域错误/读模型（勿放 argon2/utoipa/runtime） | 依赖其他 contract（除 `shared_contract`）、infra、runtime |
| `features/{name}/` | 端点、repository | 依赖其他 feature runtime |
| `bin/server` | 组合根 | 业务逻辑 |

`arch_test` 在 CI / `just pre_commit` 中强制执行上表。

## Endpoint

* 单文件 `features/{domain}/endpoint/{entity}_{action}.rs`：DTO + `#[utoipa::path]` + `handler`（+ `execute` 若有副作用）
* handler 返回 `JsonResponseType<T>`；健康探针返回裸 `Json`
* 含副作用（`revoke_tokens` 等）必须在 `execute`，handler 不重复逻辑

## 错误与 i18n

* 领域错误 `thiserror` + `#[error("snake_case_key")]`
* key 须在 `infra/locale/locales/{en-US,zh-CN}/` 有翻译；identity 相关放 `identity.ftl`，HTTP/校验通用放 `shared.ftl`
* **禁止**同一 key 出现在多个 ftl（Fluent bundle 重复会 panic）
* 域错误实现 `L10nKeyedError` 并在 `module()` 注册 mapper
* 状态码映射见 `infra/web_error` 的 `l10n_status_code`：`access_token_*` / `account_invalid_credentials` / `refresh_token_invalid` → 401；`*_forbidden` / `account_protected` → 403；`account_not_found` → 404；`account_duplicated` / `*_version_conflict` → 409；`rate_limited` → 429；其余 400。新增业务错误请遵循命名约定（使前缀/后缀映射生效），或显式补映射

## 依赖变更

改 `*_contract`、跨域 `pub` API、OpenAPI/DTO 前：`just who-uses <符号>`；≥3 个 crate 先与用户确认。

## 环境变量

可选基础设施后端的环境变量以 **编译 feature 前缀** 区分，避免不同后端配置互相覆盖；完整列表见根目录 `.env.example`。`cache-pg` / `job-queue-pg` 共用 `DATABASE_URL`。

## 测试

* 集成：`#[sqlx::test]` + `migration::run_migrations`
* E2E：`just e2e`（变量 `e2e/env.example`）
* 架构：`cargo test -p server arch_test`
