# 后端按需参考（AI）

根 [AGENTS.md](../../AGENTS.md) 是唯一规则源。**本文件不要每轮先读**——只在新建域、新 endpoint、或动基础设施时打开对应小节。

分层、错误 key、附件约定：见 [conventions.md](conventions.md)。

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

## 关键 crate

| 路径 | 用途 |
|------|------|
| `bin/migrator` | 独立迁移入口（`just sqlx_up`）；部署/CI 先于 server 滚动 |
| `infra/persistence` | PgPool、启动时迁移（与 migrator 共用 `migration::run_migrations`） |
| `infra/cache` | 缓存 + 过期清理 task |
| `infra/http_extractors` | ValidJson/Query/Path、TypedMultipart、`ClientIp` |
| `infra/web_state` | `WebState` + 能力 trait（`blob` / `jobs` 字段按 feature 门控） |
| `infra/web_error` | `WebError`、`JsonResponseType`、`register_domain_mapper`；状态码按命名约定 |
| `infra/http_auth` | Bearer JWT、`AuthedAccount` |
| `infra/http_routing` | `PublicRouter` / `ProtectedRouter` / `ProbeRouter`、限流层（features 依赖此 crate，勿依赖胖 `http_server`） |
| `infra/http_server` | 路由聚合、超时、Scalar、CORS |
| `infra/locale` | Fluent + `register_ftl`；仅内置 `shared.ftl`，域 FTL 由 feature 自注册 |
| `crates/*_kit` | 框架无关工具库（禁 modrun/axum） |
| `features/shared_contract` | 纯值对象（Id/PhoneNumber/游标分页）+ serde |
| `features/file::attachment_refs` | `AttachmentRefs` 贡献点：新附件列在此协议上注册 |

## 域模块

* `pub fn module() -> modrun::Module`：注册 locale / domain mapper，再 `provide_group` 贡献路由与附件引用
* 在 `bin/server/main.rs` 注册 `.module(<name>::module())`（features 在 `http_server` 之前）
* 蓝本：`features/identity/lib.rs`、`features/health/lib.rs`

## 架构守卫

`cargo test -p server arch_test` 校验 Cargo 依赖方向，并核对生产 default 下 `shared_contract` 的 `cargo tree` golden（同 `just tree-server`）。规则与 [conventions.md](conventions.md) 一致。

守卫覆盖：contract 不依赖 runtime/infra/框架/DB/其他 contract（`shared_contract` 除外）；kit 不依赖 feature/infra/modrun/axum；infra 不依赖 feature runtime；feature 之间仅允许 `ALLOWED_FEATURE_RUNTIME_EDGES` 白名单；所有 workspace 成员（含隐式成员）必须继承 workspace lints。
