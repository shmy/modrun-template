# 后端按需参考（AI）

根 [AGENTS.md](../../AGENTS.md) 是路口。**本文件不要每轮先读**——只在新建域、新 endpoint、或动基础设施时打开对应小节。

切片边界、错误 key、分层规则：见 [conventions.md](conventions.md)。

## 新增功能

```
├── 已有域加端点 → 对照 identity 蓝本 → `provide_group` 注册路由
├── 新建域 → {name}_contract（实体/值对象/错误）→ features/{name}/（端点/repository/lib.rs）
│            → workspace members + bin/server/Cargo.toml + main.rs `.module()`
├── 跨域读 → 只 import {other}_contract，禁止 import features/{other}/*
└── 改 DB → infra/migration/versions/ 新 .sql → `just sqlx_up`（或 server 启动时 persistence 自动迁移）
```

写完端点后：`just ai-check {crate}`。

## 关键 crate

| 路径 | 用途 |
|------|------|
| `bin/migrator` | 独立迁移入口（`just sqlx_up`）；部署/CI 先于 server 滚动 |
| `infra/persistence` | PgPool、启动时迁移（与 migrator 共用 `migration::run_migrations`） |
| `infra/cache` | 缓存 + 过期清理 task |
| `infra/http_extractors` | ValidJson/Query/Path、TypedMultipart、`ClientIp` |
| `infra/web_state` | `WebState` 运行时组装（re-export 提取器以兼容旧路径） |
| `infra/web_error` | `WebError`、`JsonResponseType`、`register_domain_mapper` |
| `infra/http_auth` | Bearer JWT、`AuthedAccount` |
| `infra/http_routing` | `PublicRouter` / `ProtectedRouter`、限流层（features 依赖此 crate，勿依赖胖 `http_server`） |
| `infra/http_server` | 路由聚合、超时、Scalar、CORS |
| `infra/locale` | Fluent；域 key 放 `{locale}/identity.ftl` 等 |
| `crates/*_kit` | 框架无关工具库 |

动 `pub` API 前：`just who-uses <符号>`。

## 域模块

* `pub fn module() -> modrun::Module`，用 `provide_group` 贡献 `PublicRouter` / `ProtectedRouter`
* 在 `bin/server/main.rs` 注册 `.module(<name>::module())`
* 蓝本：`features/identity/lib.rs`、`features/health/lib.rs`

## 架构守卫

`cargo test -p server arch_test` 校验 Cargo 依赖方向（contract 不依赖 runtime/infra、kit 不依赖 feature 等）。规则与 [conventions.md](conventions.md) 一致。
