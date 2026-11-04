# 历史评审台账

> **历史记录，不是实现依据。** 本文只回答「当时发现了什么、现在修没修」。
> 现行规则见 [AGENTS.md](../AGENTS.md)，代码现状以仓库为准。

2026-09 由六个视角对仓库做过深度评审（架构/安全、实现与契约、企业语义、modrun 装配、
业务与 AI 协作、攻击面与定位），共七份文档。七份原文已合并为本文：结论保留，过程与
重复论证删除。状态按 **2026-11 的代码**重新核对过，不是照抄当时的判断。

## 一、结论（六版收敛）

**没有一处发现指向「架构推倒重来」**——所有问题都可定位、可低风险修复。
当时的判断是：代码与装配是资深工程师水准（TSID、补偿事务、队列、modrun 用法、
文档纪律），短板在「最后一公里的可交付性」（CI、容器、metrics、门禁）。

六版视角与各自最重要的发现：

| 视角 | 一句话结论 | 当时最重要的发现 |
|---|---|---|
| v1 架构/安全/外围 | 实现超预期，外围缺失 | 无 CI/CD、无 Dockerfile、无业务 metrics |
| v2 实现细节 | 细节质量高，契约有小裂缝 | OpenAPI 示例泄露默认密码；413 语义错误 |
| v3 企业语义 | 代码好，运营叙事缺 | 缓存故障被伪装成 401 |
| v4 modrun 装配 | 示范级使用者 | 全局静态 mapper 绕过 DI |
| v5 业务/AI 协作 | AI 协作是最大差异化优势 | file 域 staging 无转正，URL 会被 GC |
| v6 攻击面/性能/定位 | 无新增重大架构问题 | argon2 阻塞 reactor；图片解码炸弹 |

## 二、已修复

| 发现 | 出处 | 现状 |
|---|---|---|
| OpenAPI 示例泄露 `admin123!` | v2 / v3 | 示例改 `P@ssw0rd1` |
| production 拒绝默认 seed 密码 | v1 / v3 | `Cli::validate` + 测试 |
| `JWT_SECRET` 无强度校验 | v3 | ≥32 字节，value parser 拒绝 |
| 连接池默认 200 vs PG 100 | v3 | 默认 20 + 容量公式（`AGENTS.md`「水平扩展」） |
| 缓存故障伪装成 401 | v3 | `cache_unavailable` → 503，双语 ftl，三条路径 |
| argon2 verify 阻塞 tokio worker | v6 | `spawn_blocking`（login / 改密 / reset） |
| 图片解码炸弹 | v6 | 解码前尺寸预检 + 像素预算（`image_kit::decode_limits`） |
| Body 超限返回 400 而非 413 | v2 | `ValidJson` 识别 `LengthLimitError` → `payload_too_large` |
| 登录手机号枚举时序侧信道 | v2 | `HashedPassword::equalize_timing()` + 登录侧统一 key |
| OTLP 100% 采样不可配 | v6 | `OTLP_SAMPLER_RATIO`，默认 0.1 |
| file 域 staging 无转正流程 | v5 | `commit_staging`（claim → INSERT → mv）+ 引用计数 |
| readyz 不查 cache | v1 / v3 | `Cache::probe()` 纳入 `/readyz` |
| 无业务 metrics | v1 | `trace_kit::metrics`（登录成功/失败、限流命中、job 终态失败） |
| 无独立 migrator | v3 | `bin/migrator` + `just sqlx_up`，部署先迁移再滚动 |
| 缺 `just db-up` / 新域脚手架 | v5 | `db-up` / `new-feature` / `who-uses` |
| 新 crate 忘记继承 lints 无守护 | 评审跟踪 | `arch_test` lint 继承测试（现已覆盖隐式 workspace 成员） |
| commit 规范缺位 | v5 | `.cursorrules`（一 commit 一事、禁用词、正反例） |
| 无 CI 门禁 | v1 / v6 | `ci.yml`：fmt + clippy_workspace + arch_test + deny + release-ci 构建 |

## 三、方案变更后解决

| 发现 | 原问题 | 现状 |
|---|---|---|
| 限流 phone 维度可定向锁死 | cache 双维度滑动窗口可被攻击者用来锁死受害手机号 | 改为 governor 纯 IP 限流；「定向锁死」消除，但单号爆破防护随之丢失——取舍已写入 `AGENTS.md`，补救（递增延迟）待做 |
| `WebState` 未按能力窄化 | 聚合 state 让只读 crate 也被拖进全部后端 | `blob` / `jobs` 改为 feature 门控字段；`health` 依赖图从约 790 crate 降到约 660，不再含 apalis/opendal |
| GC 逻辑写在组合根 | `BlobGc` 的 staging 布局、`AttachmentGc` 的业务表引用枚举都在 `bin/server` | staging 下沉到 `file::staging_gc`；附件引用改为 `AttachmentRefs` 贡献式注册，新表不再改组合根 |
| 错误分类靠 Display 嗅探 | 未注册的域错误若 Display 形如 snake_case 会被静默降级成 400 | 改为 fail-closed：只有注册过 mapper 的错误是 4xx，其余一律 500 |
| 改密/重置吊销顺序 | `revoke_tokens` 在写库之前，写库失败会出现「已被踢下线但密码没改」 | 统一「先落库、后吊销」，与删号一致 |

## 四、未处理

按当前优先级排列。

| 发现 | 出处 | 备注 |
|---|---|---|
| CI 不跑 `just test-features` | v6 | redb / redis / sqlite / blob-fs 组合只在本地验证 |
| CI 不跑 `just e2e` | v5 | Hurl 用例需起服务；blob-fs 静态回源等路径无 CI 覆盖 |
| 单账号爆破防护（phone 维度） | v6 | 现仅 IP 限流；需递增延迟或独立 phone 策略 |
| `AuthnError` 未走 `L10nKeyedError` | v1 / v2 | 现经 `err.to_string()` 直接构造 `WebError::L10n`，值正确但类型耦合仍在 |
| 优雅停机无总 deadline | v4 | 各 task 独立 drain，未设整体上限 |
| 业务 metrics 无队列积压 / 探针 | v1 / v3 | `JobBus::backlog()` 已实现但未接入任何探针 |
| 审计日志 / PII 保留策略 | v3 | 未实现 |
| ADR 目录 / 业务能力地图 | v3 / v5 | 未建立 |
| MFA / 多设备会话 / OIDC SSO | v3 | 按产品需求排期 |
| 模板对外发布品质 | v6 | 依赖前几项补齐 |

## 五、已过时的判断（勿再引用）

合并时清理掉的、当时成立但现在不成立的说法：

* 「无 CI / 无 Dockerfile / 无业务 metrics」——CI 与 `Dockerfile.ci` 已存在，metrics 已接。
* 「全局静态 `DOMAIN_REPORT_MAPPERS` 绕过 DI」——**仍存在**且是有意为之：
  `From<Report> for WebError` 是 `?` 的落点，那里没有 `State`。风险已从「类型不安全」
  收敛为「必须注册，否则 500」（fail-closed）。
* 「`WebState` 未按 kit 泛型口子窄化」——能力 trait 与 feature 门控字段已落地。
* 各版重复的「分层表」「状态码约定」——以 `AGENTS.md` 为唯一真相源，本文不再复述。
