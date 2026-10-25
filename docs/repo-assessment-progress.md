# 评审发现进度台账（更新至 2026-09-06 07:35）

> **历史台账，可能已过时。** 以根目录 [AGENTS.md](../AGENTS.md) 与当前代码为准；本文仅记录 2026-09 评审项的修复状态，不作现行架构说明。

> 六份评审（[v1](repo-assessment.md) / [v2](repo-assessment-2.md) / [v3](repo-assessment-3.md) /
> [v4](repo-assessment-4.md) / [v5](repo-assessment-5.md) / [v6](repo-assessment-final.md)）
> 均为 2026-09-05 的快照。本文跟踪各发现的修复状态；状态含义：
> ✅ 已修复（附 commit）/ 🟡 部分完成或已方案变更 / ⬜ 未处理。

## ✅ 已修复（2026-09-06 上午 10 个 commit）

| 发现 | 出处 | 修复 commit |
|---|---|---|
| OpenAPI 示例泄露 `admin123!` | v2 二.1 / v3 | `20a3e73`（示例改 `P@ssw0rd1`） |
| production 拒绝默认 seed 密码 | v3 D2 / v1 P1#4 | `20a3e73`（`Cli::validate` + 测试） |
| JWT secret 无强度校验 | v3 B3 | `20a3e73`（≥32 字节，value parser 拒绝） |
| 连接池默认 200 vs PG 100 | v3 E1 | `20a3e73`（默认 20 + 容量公式注释） |
| 缓存故障伪装成 401 | v3 A1 | `20a3e73`（`cache_unavailable` → 503，双语 ftl，三条路径） |
| argon2 verify 阻塞 tokio worker | v6 二.1 | `20a3e73`（`spawn_blocking`，login/update_password） |
| 图片解码炸弹 | v6 一.2 | `20a3e73`（`into_dimensions()` 预检 ≤8000）+ `3dd1808`（错误改 l10n key 双语） |
| 无独立 migrator | v3 E2 / TODO | `29618c5`（`bin/migrator` + `just sqlx_up`，部署先迁移再滚动） |
| 缺 `just db-up` | v5 四.2 | `29618c5` |
| migrator 未继承 workspace lints | 评审跟踪 | `ef2042f`（`[lints] workspace = true`） |
| migrator 用 `.expect()` | 评审跟踪 | `ef2042f`（`run() -> Result` + `exit(1)`） |
| 新 crate 忘记继承 lints 无守护 | 评审跟踪 | `ef2042f`（arch_test 新增 lint 继承测试） |
| commit 规范文档缺位 | v5 三.1 | `.cursorrules` 重写（未提交）：一 commit 一事、禁用词、正反例 |

## 🟡 部分完成 / 方案变更

| 发现 | 出处 | 现状 |
|---|---|---|
| 无 CI/CD | v1 P0#1 | `deploy.yaml` 构建打通（sccache + migrator + release-ci），但**无 fmt/clippy/test/arch_test/cargo-deny 门禁**；文件名与职责建议拆 ci/deploy |
| 无应用 Dockerfile | v1 P0#2 | `Dockerfile.ci` 仅有 distroless 运行时层，缺 builder 阶段，未接 CI |
| 限流 phone 维度 | v6 一.1 | 方案变更：cache 双维度 → governor 纯 IP；"定向锁死"消除，但**单账号爆破防护随之丢失**，取舍已写入 AGENTS/README，补救（递增延迟 / 补 cache 维度）待做；e2e 限流用例被删 |
| Agent 工作流命令 | v5 四 | `db-up`/`sqlx_up`/`test` ✅；`new-feature`/`e2e-full`/`new-l10n-key` ⬜ |

## ⬜ 未处理（按优先级，同 v6 final 清单）

| 发现 | 出处 |
|---|---|
| CI 门禁三件套（clippy/test/arch_test/deny） | v6 六.第一梯队收尾 |
| Body 超限返回 400 而非 413 | v2 二.2 |
| 登录时序枚举（dummy hash verify） | v2 二.3 |
| file 域 staging 无转正流程，客户端 URL 会被 GC | v5 一.1 |
| 孤儿扫描无索引；`lock_by` 硬编码 | v2 二.5 / v4 三 |
| `AuthnError` 未走 `L10nKeyedError` 正式通道（字符串耦合） | v1 P1#6 / v2 |
| 全局静态 `DOMAIN_REPORT_MAPPERS` 绕过 DI | v4 二.1 |
| kit 关机抽象与 modrun `Stopped` 不对齐（三处样板） | v4 二.3 |
| 优雅停机无总 deadline | v4 二.4 |
| `WebState` 未按 kit 泛型口子窄化 | v4 二.5 |
| OTLP `Sampler::AlwaysOn` 不可配 | v6 二.2 |
| `just new-feature` 脚手架 | v2 三.1 / v5 |
| 业务 metrics / readyz 纳入 cache / 队列探针 | v1 / v3 A3/A4 |
| 审计日志 / PII 保留策略 | v3 B1 |
| ADR / CLAUDE.md 引用 / commit 门禁 / 业务能力地图 | v3 G / v5 |
| MFA / 多设备会话 / OIDC SSO | v3 B2 |

## 一句话现状

第一梯队 6/6 完成；第二梯队完成约 1/3（migrator、2/5 命令、CI 骨架）；
剩余大头是 **CI 门禁**、**file 转正业务流**、**限流爆破防护补回**。
