# 仓库深度评估 · 终版：补充视角与六版收敛（2026-09-05）

> **历史快照，可能已过时。** 以根目录 [AGENTS.md](../AGENTS.md) 与当前代码为准；勿作为实现依据。

> 前五版已覆盖外围设施（[v1](repo-assessment.md)）、实现细节（[v2](repo-assessment-2.md)）、
> 企业语义（[v3](repo-assessment-3.md)）、modrun 装配（[v4](repo-assessment-4.md)）、
> 业务与 AI 协作（[v5](repo-assessment-5.md)）。
> 本版补齐最后三个未覆盖的视角——**攻击面走查、性能与负载、依赖治理与模板定位**——
> 并把六版结论收敛成一张总表与一份最终行动清单。

## 一、攻击面走查（攻击者视角）

按入口逐个过一遍威胁面，标注前几版是否覆盖：

### 1. 登录 / 刷新（已覆盖大半，新增一条）
- ✅ 时序侧信道枚举手机号（v2 二.3）
- ✅ argon2 CPU DoS——限流在 verify 之前（v2 一.5 的正面发现）
- ✅ JWT none/算法混淆——`Algorithm::HS256` 显式指定 + iss/aud 校验，无此风险
- 🆕 **限流键可被武器化为"定向锁死"**：`rate_limit.rs` 对 phone 维度也做硬 429，
  攻击者只需对任意受害者手机号发 20 次错误密码，即可让该手机号在整个窗口内
  无法登录（IP 维度换 IP 即可绕过自己的限流）。合法用户被攻击者间接 DoS。
  缓解方向：phone 维度改为递增延迟（成功登录即清零）而非硬拒，或对
  "错误密码导致的 phone 计数"与"成功登录的 phone 计数"分桶。
- ✅ refresh 重放 → 全吊销（v1）；补充说明：这也是一种"知道受害者 refresh 的
  攻击者可以强迫其下线"的能力，语义合理（安全优先于可用性），但应写入安全
  文档让运维知道这是预期行为而非 bug。

### 2. 文件上传（新发现一条，较重要）
- ✅ magic number / 2MiB / 类型白名单（v1）
- 🆕 **图片解码炸弹**：`crates/image_kit/compress.rs::compress_to_webp` 对
  `image::load_from_memory` **没有任何尺寸上限**——`CompressOptions` 只有
  `quality`。一个 2MiB 的恶意 PNG 可声明 30000×30000 的尺寸，解码阶段
  直接分配 ~3.6GB RGBA 内存（在 `spawn_blocking` 里，不阻塞 reactor，但足以
  触发 OOM / 挤爆容器内存限制）。修复成本低：解码前读 PNG/JPEG 头取
  dimensions 校验（如 ≤ 8000×8000），或用带预算的解码器。建议在
  `CompressOptions` 增加 `max_dimension` 并默认启用。

### 3. 其余入口
限流缓存键注入（无）、ClientIp 伪造（v1 已确认 TRUSTED_PROXIES 防护到位）、
Scalar UI 暴露（production 已关）、OpenAPI 信息泄露（v2 的密码示例问题）、
SQL 注入（sqlx 全参数化 + LIKE 转义有测试）——均已覆盖，无新增。

## 二、性能与负载视角

1. 🆕 **argon2 verify 阻塞 tokio worker**：`account_login.rs:41` 的
   `row.password.verify(...)` 直接在 async handler 里执行。Argon2id
   `Params::default()`（19MiB / t=2）单次约 20–50ms 纯 CPU——这是同步阻塞
   调用跑在 tokio reactor 线程上。并发登录稍高就会拖慢**所有**请求（不止登录）。
   修复一行：`tokio::task::spawn_blocking(move || row.password.verify(...))`。
   这是六版评审中性价比最高的单点修复之一。
2. 🆕 **OTLP 采样率不可配且 100% 采样**：`trace_kit/init.rs` 硬编码
   `Sampler::AlwaysOn`。生产环境全量 span 导出是显著的成本项（网络/collector
   负载）。应加 `OTLP_SAMPLER_RATIO` 配置，默认 parent-based ratio（如 0.1）。
3. ✅ 无负载基线（v3 F2）；补充建议：至少给 login（argon2 路径）和
   account_search（trigram 路径）各留一个可复现的基准脚本，作为未来回归的锚点。
4. 说明项：axum 只启用 http1、无响应压缩、无 TLS——都由反向代理承担，
   属合理取舍，建议在 README 一句话注明部署假设（反代终止 TLS/HTTP2）。

## 三、依赖治理与平台可移植性

1. ✅ `rust-toolchain.toml` 固定工具链、`Cargo.lock` 提交、resolver 3——
   可复现构建的基础齐了。
2. 🆕 **无 dependabot / renovate 配置**：578 个依赖既没有 CI 审计（v3 C1：
   cargo-deny），也没有自动化升级提案。两者是互补的：deny 管"能不能用"，
   renovate 管"何时更新"。模板加一个 `.github/dependabot.yml`（weekly cargo）
   成本约 10 行。
3. ✅ 跨平台信号处理（modrun signal feature 覆盖 Windows Ctrl-C 系）、redb/fs
   后端无 Unix 专有调用——核心代码可移植。
4. 开发工具链依赖本地安装（just / hurl / docker / psql 客户端），无
   devcontainer 或 Nix flake 固定开发环境——对人类和 Agent 都是新 clone 的
   第一道摩擦（与 v5 的 `db-up` 建议合并处理）。

## 四、模板定位（对比同类）

与常见 Rust 起手模板对比（realworld-axum-sqlx、loco-rs、shuttle starters）：

| 维度 | 本模板 | 典型同类 |
|---|---|---|
| 装配方式 | modrun 显式依赖图 + 组合根，arch_test 守护 | 手写 main / 宏驱动框架 |
| 分层 | contract/runtime/infra 四层 + 机器守护 | 单 crate 或 MVC 目录 |
| 安全默认 | Argon2id、即时吊销、refresh rotation、限流 | 多为"能跑就行" |
| 错误与 i18n | RFC 9457 + Fluent 双语 | 多为英文硬编码 |
| 可观测性 | OTLP 三信号 + trace_id 贯穿 | 多为 tracing 日志 |
| AI 协作 | 渐进式加载 AGENTS.md + 专属命令 | 基本空白 |
| CI/容器/部署 | ❌ 缺失（v1 核心发现） | 通常有 |
| 前端/示例客户端 | ❌ 无 | 通常有 curl 集或前端 |

结论：**差异化优势在"结构与纪律"，短板在"开箱可交付"**。补上 CI + Dockerfile +
一个最小的 curl 冒烟脚本，就能从"优秀内部模板"升级为"可对外发布的模板"。

## 五、六版收敛总表

| 版本 | 视角 | 一句话结论 | 该版最重要发现 |
|---|---|---|---|
| v1 | 架构/安全/外围 | 实现超预期，外围缺失 | 无 CI/CD、无 Dockerfile、无业务 metrics |
| v2 | 实现细节 | 细节质量高，契约有小裂缝 | OpenAPI 示例泄露默认密码；413 语义错误 |
| v3 | 企业语义 | 代码好，运营叙事缺 | 缓存故障被伪装成 401 |
| v4 | modrun 装配 | 示范级使用者 | 全局静态 mapper 绕过 DI |
| v5 | 业务/AI 协作 | AI 协作是最大差异化优势 | file 域 staging 无转正，URL 会被 GC |
| v6 | 攻击面/性能/定位 | 无新增重大架构问题 | argon2 阻塞 reactor；图片解码炸弹 |

**六版无一处发现指向"架构推倒重来"**——所有问题都可定位、可低风险修复。
这本身就是对架构最好的终审结论。

## 六、最终行动清单（六版合并去重，按性价比排序）

**第一梯队：单点修复，半天内**
1. argon2 verify 包 `spawn_blocking`（v6）
2. `CompressOptions` 增加解码尺寸上限（v6）
3. 缓存错误与凭证错误分离：`CacheUnavailable → 503`（v3 A1）
4. OpenAPI 示例去掉 `admin123!` + production 拒绝默认 seed 密码（v2/v3）
5. `JWT_SECRET` 最小长度校验（v3 B3）
6. `max_connections` 默认 200 → 20 + 容量公式注释（v3 E1）

**第二梯队：小特性，1–2 周**
7. CI（clippy/test/arch_test/cargo-deny）+ Dockerfile + dependabot（v1/v3/v6）
8. 业务 metrics 仪表 + readyz 纳入 cache + 队列积压探针（v1/v3）
9. `AuthnError` 走 `L10nKeyedError` 正式通道；mapper 注册 DI 化（v1/v4）
10. file 转正业务流（promote 端点）或 staging 语义决策（v5）
11. 登录 phone 维度限流改递增延迟（v6）
12. `just new-feature` / `db-up` / `e2e-full` / `new-l10n-key` 四命令（v2/v5）
13. OTLP 采样率可配（v6）

**第三梯队：文档与治理，持续**
14. ADR 目录（收编现有代码注释中的决策）、ops runbook（备份/RTO/降级策略）
15. CLAUDE.md 引用 AGENTS.md、`.cursorrules` 收敛、commit 规范门禁（v5）
16. 业务能力地图、`Id` 生成策略与部署假设文档（v5/v6）
17. MFA / 多设备会话 / OIDC SSO 按产品需求排期（v3 B2）

## 七、终审判词

六个视角走完，这个仓库呈现的是一个少见的组合：**代码与装配是资深工程师的
作品（TSID、补偿事务、SKIP LOCKED 队列、modrun 示范用法、AI 文档纪律），
而仓库外围是"未完成的最后一公里"（CI、容器、metrics、门禁、业务闭环）**。
评估过程中没有发现任何需要返工的设计错误——全部发现都是加法而非减法。

它的独特价值在于：这可能是少数几个**把"AI 作为主要贡献者"考虑进架构的
Rust 模板**——分层可机器验证、命名处处可 grep、规范可执行、上下文加载有预算。
把第一、二梯队的 13 项做完（预计两周内的工作量），它就具备对外发布的品质。
