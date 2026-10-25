# 仓库深度评估 · 第三版：企业级全面视角（2026-09-05）

> **历史快照，可能已过时。** 以根目录 [AGENTS.md](../AGENTS.md) 与当前代码为准；勿作为实现依据。

> 与 [repo-assessment.md](repo-assessment.md)（架构/安全/外围）、
> [repo-assessment-2.md](repo-assessment-2.md)（实现/契约细节）互补。
> 本版视角：**如果这是一家企业产品的底座**——可用性工程、安全合规、供应链、
> 配置与密钥治理、部署拓扑、灾备、工程治理。所有发现基于代码验证，标注出处。

## 一、企业级视角下做得好的

1. **连接池治理完整**（`infra/persistence/lib.rs`）：max/min connections、max_lifetime、
   idle_timeout、acquire_timeout 全部可配置——大多数模板只给一个 URL。
2. **PII 防泄漏日志是双层的**：`disable_statement_logging()`（连接层）+
   `sqlx::query=off`（tracing filter，`crates/trace_kit/init.rs` 注释明确写了"防 SQL
   参数（含密码哈希）进入日志/OTLP"）。
3. **12-factor 配置纪律**：全部配置走 clap + env，secret 类型统一 `SecretString`
   （`config/cli.rs`），`DATABASE_URL` 也是 SecretString——比多数项目把连接串当
   普通字符串打印要谨慎。
4. **多实例部署有意识**：`MASTER` 标志让 seed 与 cron scheduler 只在单实例执行
   （`identity/lib.rs`、`AGENTS.md`），job worker 支持多实例并发消费（SKIP LOCKED）。
5. **数据一致性操作语义谨慎**：DELETE 幂等、并发 seed 安全（partial unique index +
   `WHERE NOT EXISTS`）、乐观锁、授权通过后先吊销再删行——
   都是审计时能通过的操作设计。
6. **优雅停机全链路**：HTTP drain、job worker watch channel、孤儿扫描兜底
   （进程被杀后 Running 任务会被回置）——滚动发布时的数据安全有保障。

## 二、按企业维度的问题清单

### 维度 A：可用性与故障模式

**A1. 缓存故障被伪装成 401（本版最重要发现）**
`infra/http_auth/middleware/authorize.rs:29-32`：JTI 查询的缓存错误
`map_err(|_| AuthnError::AccessTokenInvalid)`——cache 后端（pg/redis/redb）抖动时，
**所有受保护接口返回 401**。后果链：客户端认为 token 失效 → 拿 refresh 重刷 →
login/refresh 也依赖 cache → 大量无效重试撞 429 → 用户被登出。企业语义上，
基础设施故障必须是 5xx（触发客户端退避与告警），401 只能表示"凭证确实无效"。
应区分 `CacheUnavailable` → 503。

**A2. 鉴权对 cache 是硬依赖，无降级策略**
JTI 存 cache 是即时吊销的代价，但没有任何 fail-open/fail-closed 的显式决策文档。
企业至少应明确：cache 不可用时段的策略是什么（拒答 vs 降级为纯 JWT 校验 + 缩短
TTL），并写进 runbook。

**A3. 依赖故障半径没有探针**
`readyz` 只查 PG（`features/health/endpoint/readyz.rs`）。cache 挂了 ready 依旧 true，
K8s 不会摘流量，A1 的问题持续到人工介入。

**A4. 队列健康不可见**
job 积压深度、最老 Pending 行龄、Failed 增速——这些企业排班表里的标准告警项
既无 metrics 也无探针端点。

### 维度 B：安全与合规

**B1. PII 治理缺失**
- `phone` 是 PII，明文存储、明文进日志（登录限流 key `phone:{...}` 进 cache，
  有 TTL 但无保留策略文档）。
- 账号删除是**硬删除**，无软删除/匿名化选项，也无数据导出端点——GDPR 的
  被遗忘权反而是硬删直接满足，但**删除前无审计留痕**（谁在何时删了谁，删完
  就什么都没有了，包括审计记录本身）。
- `accounts.remark` 预留列未使用，但其治理规则（是否允许放 PII）未定义。

**B2. 认证体系企业能力缺口**（与第一版互补，这里从合规角度重述）
- 无 MFA / TOTP；无 SSO（OIDC/SAML）——企业采购 B2B 场景的硬门槛；
- 单账号单会话模型（一把 access + 一把 refresh）：多设备支持缺失，企业用户
  "办公室电脑 + 手机" 即互相踢下线；
- 无密码过期/复用历史策略（等保、SOC2 常见要求）；
- 无账号锁定（只有 IP+手机号维度的限流，无"连续失败 N 次锁 M 分钟"）。

**B3. JWT secret 无强度校验**
`config/cli.rs::JwtCli`：`JWT_SECRET` 只要求非空，8 字符也能启动。HS256 下
secret 强度即安全边界，应在加载期校验最小熵/长度（如 ≥32 字节）并 fail-fast。

### 维度 C：供应链与制品安全

**C1. 无任何依赖审计**
578 个锁定 crate（`Cargo.lock`），但仓库无 `cargo-deny.toml`、无 `cargo audit`
调用（`justfile` 只有 `cargo machete`）。RUSTSEC 漏洞告警、license 合规检查、
未维护 crate 检测全部缺失。这是企业落地前的低成本必补项。

**C2. 无 SBOM / 制品溯源**
无 Dockerfile（第一版已提），因此也无镜像构建、SBOM 生成、镜像签名的承接点。
模板应预留 `cargo auditable` 或 `cyclonedx` 生成位。

### 维度 D：配置与密钥管理

**D1. 密钥仅存进程环境变量**
`SecretString` 防 Debug 泄漏做得好，但 env 变量在 `/proc/<pid>/environ`、
K8s downward API、崩溃转储中可见。企业标准是 file/Vault 注入
（`_FILE` 后缀模式或 secret provider trait）。至少应预留接口。

**D2. 生产配置缺乏 fail-fast 校验集合**
除 D2 的 secret 长度外，`APP_ENV=production` 时没有一组集中校验
（如"production 下禁用默认 seed 密码 / 默认 TRUSTED_PROXIES 为空告警 /
MASTER 语义确认"）。目前散落或有缺失（第一版 P1#4 的 seed 密码问题本质
是这里没有校验关卡）。

### 维度 E：部署拓扑与容量

**E1. 连接池默认值需要容量规划护栏**
`database_max_connections` 默认 200，而 PostgreSQL 默认 `max_connections=100`。
单实例模板自测没问题，两个实例就是 400 个潜在连接 → 生产上线即踩雷。
应降默认值（如 20）并在 `.env.example` 注明实例数 × 池大小 ≤ PG 预算。

**E2. 零停机部署纪律未定义**
迁移绑定在每个实例启动时执行（`persistence/lib.rs::provide_pool`）。sqlx
migrator 有并发保护，但"先扩容新实例跑新迁移 → 旧代码还在跑"的
expand-contract 纪律、以及长锁迁移的规避规则都没有文档。TODO.md 里的
独立 `bin/migrator` 是正解，应提前。

**E3. `REQUEST_TIMEOUT` 默认 60s 偏长**
同步 API 的合理值通常在 10–30s。60s 意味着上游（LB/Ingress）需要更长
超时，慢请求会占住连接与池。默认值应更保守。

### 维度 F：灾备与连续性

**F1. 无备份/恢复叙事**
README/AGENTS 均无 PG 备份（PITR、WAL 归档）、cache 重建流程的说明。
token 会话全在 cache（redb 后端 `Durability::None`、redis 可驱逐），cache
丢失 = 全员重新登录——这本身是可接受的（cache 语义正确），但应作为
**已声明的 RTO/RPO 承诺**写下来，而不是留给运维猜。

**F2. 无故障演练与压测**
测试体系（单元/sqlx::test/Hurl E2E）覆盖功能正确性，但没有负载基线
（QPS/延迟 P99）、没有 cache 故障注入用例。企业上线前这两项是标准动作。

### 维度 G：工程治理

**G1. 无 ADR**：TSID 选型、cache 三后端、HS256、modrun 框架选型——这些
决策的理由散落在代码注释里，没有 `docs/adr/` 承接。人员更替时注释会被
当成"实现细节"而丢失"为什么"。
**G2. 无 CHANGELOG / 版本策略**：API 已有 `/api/v1` 前缀，但 crate 版本、
发布流程、破坏性变更政策未定义。
**G3. License 未声明**：workspace `Cargo.toml` 无 `license` 字段——企业
法务第一问。

## 三、风险矩阵（企业视角 Top 10）

| # | 风险 | 维度 | 等级 | 修复成本 |
|---|---|---|---|---|
| 1 | 缓存故障 → 全站 401 + 登录风暴 | A1 | 高 | 低（错误分类区分） |
| 2 | seed 密码默认值 + OpenAPI 泄露（前两版） | B | 高 | 低 |
| 3 | 无依赖漏洞审计（578 crates） | C1 | 高 | 低 |
| 4 | JWT secret 无强度校验 | B3 | 中高 | 极低 |
| 5 | readyz 不含 cache，故障无探针 | A3 | 中 | 低 |
| 6 | 连接池默认 200 vs PG 100 | E1 | 中 | 极低 |
| 7 | 删除无审计、PII 无保留策略 | B1 | 中 | 中 |
| 8 | 迁移与扩容纪律无文档 | E2 | 中 | 中 |
| 9 | 无备份/RTO 承诺文档 | F1 | 中 | 低（文档） |
| 10 | 单会话模型 / 无 MFA / SSO | B2 | 中（视场景） | 高（产品级） |

## 四、90 天企业化路线建议

**第 1 个月（止血，全部低成本）**
1. A1：`AuthnError` 增加 `CacheUnavailable` → 503，与凭证错误分离；
2. C1：引入 `cargo-deny`（advisories + licenses）进 pre_commit 与未来 CI；
3. B3/D2：启动期集中校验——secret ≥32 字节、production 下拒绝默认 seed 密码；
4. E1：`max_connections` 默认降到 20，`.env.example` 写明容量公式；
5. E3：`REQUEST_TIMEOUT` 默认 60s → 30s。

**第 2 个月（补探针与文档）**
6. A3/A4：readyz 增加 cache 检查；队列积压/失败率探针或 metrics；
7. F1：写 ops runbook（备份/PITR、cache 重建、RTO/RPO 声明、A2 降级策略）；
8. E2：独立 `bin/migrator` + expand-contract 迁移规范文档；
9. B1：删除操作落审计表（actor/target/时间），定义 PII 保留策略。

**第 3 个月（产品级能力，按需）**
10. B2：MFA（TOTP）、多设备会话模型、OIDC SSO 按产品需求排期；
11. G1/G3：补 ADR（把现有代码注释中的决策理由收编）与 license 声明。

## 五、三版合并总结

- **第一版**：外围缺失（CI/容器/metrics/权限/审计）；
- **第二版**：实现细节（OpenAPI 密码泄露、413 语义、时序枚举、索引缺口）；
- **本版**：企业语义（故障被伪装成 401、合规能力缺口、供应链、容量护栏）。

三版共同指向同一个判断：**代码实现质量高，工程基础设施与运营叙事需要补齐**。
企业在意的往往不是"代码写得好不好"，而是"出了事怎么办、谁来负责、如何证明"——
本版列出的问题大多不需要重写代码，属于文档、配置默认值和错误分类层面的低成本修复，
适合在模板被真实采用之前完成。
