# 后端按需参考（AI）

根 [AGENTS.md](../../AGENTS.md) 是路口。**本文件不要每轮先读**——只在新建域、新 endpoint、或动基础设施时打开对应小节。

切片边界、错误 key、分层规则：见 [conventions.md](conventions.md)。

## 增量编译（置顶）

* AI 默认只跑 `just ai-check <crate>` / `just lint <crate>`；**禁止**默认 `cargo test --workspace` / `--all-features`。
* `just check` = 开发 feature 集；`cargo check -p server` = 生产 default。提交前再 `just pre_commit`。
* `shared_contract` / `*_contract` 保持纯（无 utoipa/sqlx/axum/validify）；OpenAPI 适配在使用方 DTO。

## 新增功能

```
├── 已有域加端点 → 对照 identity 蓝本 → `provide_group` 注册路由；State 用 HasPool/HasBlob/…
├── 新建域 → just new-feature <name>
│            → {name}_contract（跨域错误；读模型待跨域消费再下沉）
│            → features/{name}/（端点/repository/lib.rs + locales 自注册）
│            → workspace members + bin/server/Cargo.toml + main.rs `.module()`（http_server 前）
├── 跨域读 → 只 import {other}_contract，禁止 import features/{other}/*
└── 改 DB → infra/migration/versions/ 新 .sql → `just sqlx_up`（或 server 启动时 persistence 自动迁移）
```

写完端点后：`just ai-check {crate}`（迭代中小步用 `just lint {crate}`，秒级）。

## 关键 crate

| 路径 | 用途 |
|------|------|
| `bin/migrator` | 独立迁移入口（`just sqlx_up`）；部署/CI 先于 server 滚动 |
| `infra/persistence` | PgPool、启动时迁移（与 migrator 共用 `migration::run_migrations`） |
| `infra/cache` | 缓存 + 过期清理 task |
| `infra/http_extractors` | ValidJson/Query/Path、TypedMultipart、`ClientIp` |
| `infra/web_state` | `WebState` + `HasPool`/`HasBlob`/… 能力 trait（re-export 提取器以兼容旧路径） |
| `infra/web_error` | `WebError`、`JsonResponseType`、`register_domain_mapper`；状态码按命名约定 |
| `infra/http_auth` | Bearer JWT、`AuthedAccount` |
| `infra/http_routing` | `PublicRouter` / `ProtectedRouter`、限流层（features 依赖此 crate，勿依赖胖 `http_server`） |
| `infra/http_server` | 路由聚合、超时、Scalar、CORS |
| `infra/locale` | Fluent + `register_ftl`；仅内置 `shared.ftl`，域 FTL 由 feature 自注册 |
| `crates/*_kit` | 框架无关工具库（禁 modrun/axum） |
| `features/shared_contract` | 纯值对象（Id/PhoneNumber/游标分页）+ serde |
| `features/identity_contract` | **当前仅错误**；读模型 TODO 下沉 |

动 `pub` API 前：`just who-uses <符号>`。

## 域模块

* `pub fn module() -> modrun::Module`：注册 locale / domain mapper，再用 `provide_group` 贡献路由
* 在 `bin/server/main.rs` 注册 `.module(<name>::module())`（features 在 `http_server` 之前）
* 蓝本：`features/identity/lib.rs`、`features/health/lib.rs`

## 架构守卫

`cargo test -p server arch_test` 校验 Cargo 依赖方向（contract 不依赖 runtime/infra/utoipa/axum/sqlx/validify、kit 不依赖 feature/modrun/axum 等），并核对生产 default 下 `shared_contract` 的 `cargo tree` golden（同 `just tree-server`）。规则与 [conventions.md](conventions.md) 一致。
