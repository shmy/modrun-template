# 仓库深度评估（2026-09-05）

> **历史快照，可能已过时。** 以根目录 [AGENTS.md](../AGENTS.md) 与当前代码为准；勿作为实现依据。修复进度见 [repo-assessment-progress.md](repo-assessment-progress.md)（亦可能滞后）。

> 基于对全部 28 个 crate、约 11.5k 行 Rust 的实际代码阅读（重点：auth 链路、job queue、
> cache/blob 后端、错误与 i18n、迁移与测试），非仅依据文档。

## 一、优点

### 1. 架构守护是"机械"的，不是"口头"的

- `bin/server/arch_test.rs` 直接解析 workspace `Cargo.toml`，强制执行 5 条依赖方向规则
  （contract 互不依赖、contract 不依赖 infra、infra 不依赖 feature runtime、kit 不依赖
  feature/infra、feature runtime 互不依赖）。架构约定不靠 review 记忆，测试拦截。
- `justfile` 的 `who-uses` / `ai-check` 命令说明作者意识到跨域改动的爆炸半径问题。

### 2. 认证链路的安全实现相当扎实

- `features/identity_contract/value_object/hashed_password.rs`：Argon2id + v0x13；
  `Password` 值对象长度校验 + `Debug` 全部 redacted（`Password` / `TokenHelper` / `SecretString`）。
- JWT 校验四层叠加：HS256 签名 + `iss`/`aud` 校验（`crates/jwt_kit/token_helper.rs`，测试覆盖
  跨 issuer 拒绝）+ 缓存中 JTI 比对实现**即时吊销**（`infra/http_auth/middleware/authorize.rs`）
  + refresh rotation 与重放检测（`features/identity/shared/token_ops.rs`：`used_refresh_key`
  命中即吊销全账号会话）。
- refresh token 只存 digest，且有专门测试 `cache_keys_do_not_store_raw_refresh_token`
  扫表验证不泄漏——这种"验证自己没犯错"的测试很少见。
- 限流：Redis 后端用 Lua 脚本原子实现滑动窗口（`crates/cache_kit/redis.rs`），pg/redb
  语义对齐；IP+账号/refresh digest 双维度。
- `TRUSTED_PROXIES` 白名单之外忽略 `X-Forwarded-For`（防伪造限流键）；文件上传 magic
  number 校验（`file_contract::ensure_image_reader`）+ multipart 2MiB 限制 + 全局
  `DefaultBodyLimit`。

### 3. Job queue 是生产级设计

`crates/job_queue_kit/`：

- `FOR UPDATE SKIP LOCKED` 竞争安全领取（`backend.rs`）、指数退避 + 封顶、孤儿扫描
  at-least-once 兜底、DLQ-in-table（`Failed` + `last_error`）、GC 分批删除防锁表/WAL 膨胀、
  pg `LISTEN/NOTIFY` 桥接降低轮询延迟、pg/sqlite 双后端语义一致。
- `lib.rs` 内嵌约 700 行测试覆盖并发、去重、超时、孤儿恢复。

### 4. 测试覆盖的是"真实行为"而非形式

- 并发 refresh 只有一个成功（`tokio::join!` 两个 consume 断言 ok_count==1）、重放吊销、
  乐观锁 409、删除后 token 失效、LIKE 转义（`repository.rs::like_contains_tests`）、
  限流分维度隔离。identity 全部 endpoint 都有 `#[sqlx::test]`。
- E2E 用 Hurl 覆盖登录流、限流（`e2e/identity_rate_limit.hurl` 的 env 注明要清缓存防旧桶误报）。

### 5. 可观测性完整

- OTLP 三信号（log/metric/trace）一次初始化；`trace_id` 注入 ProblemDetails；
  `sqlx::query=off` 防 SQL 参数（含密码哈希）进日志；防 telemetry-induced-telemetry 的
  filter；`TracingGuard` Drop 时 shutdown provider。
- `readyz` 真查 DB，不是假 200；优雅停机链路贯穿 HTTP（`task_with` + graceful）和
  job worker（watch channel）。

### 6. 细节处理成熟

- `bin/server/internal_jobs.rs` 的 staging GC 只删"整个月早于保留期"的目录防误删当月文件。
- `create_privileged_if_absent` 用 `WHERE NOT EXISTS` + 唯一 partial index
  （`accounts_one_privileged`）处理并发 seed。
- l10n 有测试守护"所有 key 在 en-US/zh-CN 都存在"；fluent 插值剥离 bidi 隔离字符。

## 二、问题与可优化点（按严重度排序）

### P0 — 明显缺失

1. **完全没有 CI/CD**：仓库无 `.github/` 目录。`just pre_commit`（fmt/clippy/test/
   arch_test/cargo machete）全靠本地自觉。feature 组合矩阵（cache×3、job×2、blob×3）
   没有机器兜底验证。建议至少：`cargo clippy --workspace --all-targets --all-features`
   + `cargo test --workspace` + feature 矩阵（可用 `cargo hack`）。
2. **没有应用 Dockerfile**：只有 `docker-compose.yaml` 起依赖，无构建产物定义，
   部署故事断在 `cargo run`。
3. **无任何业务 metrics**：`crates/trace_kit/init.rs` 注册了全局 meter provider，但全仓库
   没有一处业务仪表（HTTP 请求计数/延迟直方图、job 队列深度、限流命中数）。
   `OtelAxumLayer` 只贡献 trace span；也无 Prometheus `/metrics` 端点兜底。

### P1 — 设计风险

4. **`SEED_ADMIN_PASSWORD` 有默认值 `admin123!` 且 `MASTER` 默认 true**：与 `JWT_SECRET`
   必填无默认的谨慎形成反差。生产忘改环境变量就会启动出一个弱密码特权账号。
   建议 `APP_ENV=production` 时对默认 seed 密码直接 panic。
5. **HS256 单 secret、无轮换机制**：`crates/jwt_kit` 只支持对称 HMAC，无 `kid` / 多 key
   验签。secret 轮换 = 全员强制下线。至少应文档化；更好的是预留 RS256/EdDSA + key id。
6. **鉴权错误映射靠字符串巧合**：`authorize.rs` 的 `auth_error_to_web_error` 直接
   `WebError::L10n(err.to_string())`——`AuthnError` 的 Display 文本就是 l10n key。
   variant 改名会静默变成 400 domain error 而非编译错误。建议给 `AuthnError` 实现
   `L10nKeyedError` 走 domain mapper 同一条正式通道。
7. **`issue_tokens` 跨键无事务**：`token_ops.rs` 注释自认"部分写入失败可能短暂并存新旧
   refresh"。pg 后端可用单事务 + advisory lock 做到原子；当前靠"下次 login 自愈"兜底。
8. **readyz 只检查 DB**：不检查 cache（鉴权强依赖！cache 不可用时所有受保护接口
   401/500）、blob、queue。K8s 下会出现 ready 但实际不可服务的窗口。
9. **`job_queue_kit/lib.rs` 1286 行单文件**：实现 + ~700 行测试混在一起，worker loop /
   backoff / notify bridge / orphan sweep 应拆模块。全仓库唯一明显超出合理粒度的文件。

### P2 — 可打磨

10. 每请求鉴权 = 1 次缓存查询 + 每次连接池 acquire；access token 有效期内 JTI 可本地
    LRU 缓存数秒，减少 pg cache 热点（性能优化空间）。
11. `lock_by` 硬编码 `"local-worker"`（`backend.rs`）——多实例排障无法区分 worker，
    应注入 hostname/pod name。
12. 密码策略只有长度 8–64，无复杂度/黑名单校验；登录无验证码/失败锁定（只有限流）。
13. 密码修改不走乐观锁语义——`update_password` 直接 bump version，无 `WHERE version = $x`，
    并发修改会静默覆盖（其他字段更新是 409）。不一致但影响小。
14. `PhoneNumber` 语义绑定中国 11 位手机号（`VARCHAR(11)` + 校验），国际化受限；
    login 仅 phone+password，无 email 渠道。
15. locale 协商只取 primary subtag——`zh-TW` 落到 `zh-CN` 简体、`pt-BR` 落到英文。
    可接受，值得注释说明。
16. l10n 状态码映射是 if/else 字符串匹配（`l10n_status_code`），新 key 漏映射静默落 400。
    locale 存在性有测试，但"状态码映射完整性"没有对应守护测试。
17. 无 CORS 层——模板若面向浏览器前端需自行加 `tower-http` cors；若刻意留白应在
    AGENTS.md 注明。

## 三、缺失点清单

| 缺失 | 影响 |
|---|---|
| CI/CD 工作流 | 质量门禁全靠本地 |
| 应用 Dockerfile / 部署清单 | 无法直接交付 |
| 业务 metrics（HTTP/job/限流仪表） | 只有链路观测，无数字可报警 |
| 权限模型（仅 `privileged` 布尔） | 第二个 feature 就要面对 RBAC 问题 |
| 审计日志（账号 CRUD/改密/删除无留痕） | 合规场景缺基础 |
| CORS / 安全响应头 | 前端接入需补 |
| refresh token 的 DB 持久层 | cache 清空（redb `Durability::None` 崩溃丢失、redis 驱逐）= 全员下线；属刻意设计但未文档化风险 |
| API 幂等键（如上传去重） | 客户端重试语义未定义 |
| `Id` 生成策略文档 | i64 主键 + `Id::default()`，snowflake/时间序语义未说明，跨节点插入无冲突保证文档化 |

## 四、总结与优先级

这是一个**工程质量显著高于平均水平的模板**：架构约束机械化、安全实现有测试自证、
job queue 是生产级、错误/i18n/可观测性体系完整一致。短板几乎全部集中在"仓库外围"
——CI、容器化、metrics、权限/审计这些横切面；代码本身的问题（auth 错误映射的字符串
耦合、token 写入非原子、巨型文件）都属于可定位、可低成本修复的范畴。

建议演进优先级：

1. CI
2. Dockerfile
3. metrics 仪表
4. seed 密码生产防护
5. `AuthnError` 正式接入 `L10nKeyedError`
