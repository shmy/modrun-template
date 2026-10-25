# 双组合根落地计划（P3）

> 独立于日常脚手架打磨。与 [product-monorepo-plan.md](product-monorepo-plan.md) 互补：本文只定 **Rust 组合根怎么拆**，不绑小程序/SPA。

## 目标

把「单 `bin/server` = HTTP + worker + scheduler」演进为可独立扩缩的进程，同时保持 `features/` / `*_contract` 一份业务真相。

## 两条拆法（选一为主）

### A. api / worker（更贴微服务运维）

| Bin | 职责 | 注册模块 |
|-----|------|----------|
| `bin/api-server`（现 `server` 演进） | HTTP、鉴权、OpenAPI | persistence → cache → blob → web_state → features → http_server；**不**挂 job_worker / scheduler.run |
| `bin/worker` | 消费队列 +（可选）MASTER 调度 | persistence → cache → blob → job_queue → web_state → job_worker → scheduler（仅 MASTER）→ wiring/internal_jobs |

要点：

* 共享同一 `DATABASE_URL` / cache / blob 配置面。
* 副本：多 `api-server` + 少 `worker`；scheduler 仍只一台 MASTER worker。
* CI：两个 release artifact；arch_test 可挂在 `api-server` 或抽 `crates/arch_rules`。

### B. consumer / admin（更贴产品受众）

见 [product-monorepo-plan.md](product-monorepo-plan.md)：

* `bin/consumer-server` / `bin/admin-server`
* `features/*/endpoint/{consumer,admin}/`
* 契约导出 `contracts/*.openapi.json`

可与 A **叠加**（每个受众 bin 再拆 worker），但首期不要一次做完。

## 推荐首期路径

1. **先做 A（api/worker）**，验证队列与 HTTP 独立扩缩。
2. 有 C 端/B 端路由面差异时再做 B。
3. **不做**：业务 gRPC、mesh、模板内 outbox——有跨服务事件需求时单独立项。

## 落地步骤（A）

1. 复制 `bin/server` → `bin/worker`；worker `main.rs` 去掉 `http_server` / OpenApi / Scalar。
2. api `main.rs` 去掉 `job_worker` / `scheduler` 的 `scheduler.run`（可保留 job_queue 客户端入队）。
3. workspace members + Dockerfile 多阶段打两个二进制。
4. 文档：AGENTS「水平扩展」补「api 与 worker 角色」；`.env.example` 注明哪类进程读哪些变量。
5. e2e 仍打 api；另加 worker 烟测（入队 → 终态）。

## 明确不做（本阶段）

* 把 `features/` 拆成多仓库
* 为拆而拆的 gRPC 替换 HTTP
* 进程内再引入分布式限流（继续边缘限流）

## 验收

* 仅起 api：登录/CRUD 可用，后台 job 积压但不崩。
* 仅起 worker：消费已有队列；MASTER worker 跑 GC cron。
* `just pre_commit` / CI 双 bin 均可编过。
