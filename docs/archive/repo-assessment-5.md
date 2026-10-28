# 仓库深度评估 · 第五版：业务清晰度 / 组织结构 / AI 开发规范 / Agent 友好性（2026-09-05）

> **历史快照，可能已过时。** 以根目录 [AGENTS.md](../AGENTS.md) 与当前代码为准；勿作为实现依据。

> 与前四版互补（[v1](repo-assessment.md) 外围 / [v2](repo-assessment-2.md) 实现细节 /
> [v3](repo-assessment-3.md) 企业语义 / [v4](repo-assessment-4.md) modrun 装配）。
> 本版视角：**把"下一个读者是 AI Agent"当成一等约束**——业务语义是否自解释、
> 结构是否可预测、规范文档是否可执行、AI 协作链路是否顺畅。

## 一、业务清晰度

### 做得好的

1. **endpoint 文件名即业务用例**。`account_login.rs`、`account_refresh_token.rs`、
   `profile_current.rs`——不读代码就能复述出业务能力清单；`operation_id` 与文件名
   一致（`file_upload_image` = `endpoint/file_upload_image.rs`），从 OpenAPI 文档
   能直接 grep 回源码。
2. **业务词汇表完整且双语落地**。`identity_contract/error.rs` 的错误枚举
   （`AccountDuplicated` / `AccountProtected` / `AccountVersionConflict`）+
   ftl 文案（"Protected account cannot be modified"）——业务规则、错误 key、
   用户文案三层同构。
3. **DDD-lite 值对象让业务约束可执行**：`Password`（8–64）、`PhoneNumber`
   （11 位大陆手机号）、`RefreshToken`、`Id`——约束写在类型上而不是散落在
   handler 里的 if。

### 问题

**1. file 域业务闭环不完整（本版最重要发现）**
上传端点把文件写进 `staging/YYYY/MM/`，返回的 URL 直接指向 staging；
`BlobGc` 会把整月早于保留期的 staging 目录删掉（约 30–60 天）。但全仓库
**没有 promote/confirm 端点**把文件转正到长期存储——`file_contract` 里的
`strip_staging_prefix` 定义了却无人使用（grep 全仓库零调用，疑似为尚未实现的
转正流程预留）。结果是：**客户端拿到的文件 URL 会在 30–60 天后静默 404**。
要么补上转正业务流（上传 → 业务确认 → move 到正式目录），要么把 staging
语义改成"即传即用"并明确文档化保留策略。这是业务清晰度上唯一的逻辑断点。

**2. identity 业务模型偏薄但自洽**。单角色（`privileged` 布尔）、phone-only 身份、
`remark` 预留未用——作为模板够用，但"谁能在什么情况下操作谁"的授权语义
分散在 `require_privileged` / `require_unprotected_target` 两个函数里，
没有一个集中的授权规则描述。第二个域出现时这里会分叉。

**3. 缺"业务能力地图"**。README 的模块结构表是**技术**视图（crate 名），
不是业务视图（能力 → 域 → 端点 → 数据表）。Agent 判断"新需求该落在哪个域"
目前只能靠 AGENTS.md 的命名约定反推。建议补一张 10 行的业务地图：
`账号与登录（identity）→ accounts 表 → /api/v1/identity/*, /accounts*`。

**4. TODO.md 只有技术路线**（event_bus / flow / http_client / migrator），
没有业务路线（下一个域是什么、identity 会怎么演进）。对人类 PM 和 Agent
规划都少了一层依据。

## 二、组织结构

### 做得好的

1. 四层目录（`crates/` / `infra/` / `features/` / `bin/server`）+ `*_contract`
   契约层，且有 `arch_test.rs` 机器守护——结构不是"共识"而是"测试"。
2. feature 内部结构完全一致：`lib.rs`（module + 路由）/ `dto.rs`（读模型）/
   `repository.rs`（SQL）/ `endpoint/{entity}_{action}.rs` / `shared/`（域内横切）。
   identity 12 个 endpoint 文件没有一处例外。Agent 学会一个域 = 学会所有域。
3. 文档分层策略本身就是结构设计：README（人类快速上手）→ AGENTS.md（Agent
   总路口）→ docs/ai/*（按需切片）→ 代码注释（非显而易见逻辑）。

### 问题

1. **`shared_contract` 的归属是分层标准的例外**。按 AGENTS.md 自己的
   "抽库判断"（无 modrun/axum 依赖、不绑应用配置 → `crates/`），
   `features/shared_contract` 应该更像 `crates/` 的东西。arch_test 给它开了
   白名单例外。要么挪到 `crates/shared_contract`，要么在 conventions.md
   明确"`features/shared_contract` 是跨域值对象的既定位置"——现在是靠
   测试例外硬扛的隐性约定。
2. **规则源多头**：`AGENTS.md`（主体）+ `.cursorrules`（commit message，
   自述"只被 Cursor 读取"）+ `docs/ai/*`。`.cursorrules` 里的 commit 规范
   与 AGENTS.md 的 "Commit message" 节重复维护。建议 `.cursorrules` 只留
   一行指向 AGENTS.md。
3. **docs/ai 薄到接近空壳**：`backend.md` 全文 5 行，`conventions.md` 约半数
   内容与 AGENTS.md 重复（分层表、错误映射、测试约定）。渐进式加载的
   设计意图很好，但"按需切片"目前只是 AGENTS.md 的换排版。要么真正切片
   （把 AGENTS.md 的长节搬进去瘦身主文件），要么合并回一个文件。
4. **无 ADR**（第三版 G1 已提，从组织角度重申）：TSID、cache 三后端、
   cron/job 分离、HS256——决策理由都在代码注释和 AGENTS.md 里，没有
   `docs/adr/` 承接点。

## 三、AI 开发规范

### 做得好的（这一节是全仓库相对开源项目的最大差异化优势）

1. **AGENTS.md 有明确的"不要读"策略**——"禁止预读 docs/ai/（除非新建域…）"、
   "避免对简单任务启动 subagent"。绝大多数仓库的 AI 文档只说"读什么"，
   这里同时约束了**上下文预算**，这是 token 纪律的正确姿势。
2. **给 Agent 造了专属工具**：`just who-uses <symbol>`（改 contract 前查爆炸
   半径）、`just ai-check <crate>`（架构测试 + fmt + check + test 一键验证）。
   规范与验证命令一一对应，Agent 可以"照做即合规"。
3. **常见陷阱章节直接编码历史教训**（8 条，每条都是可触发的失败模式：
   extractor 放错层、重复挂鉴权、l10n 只改一处 locale、忘跑 pre_commit…）。
4. **新增错误 key 的状态码约定显式文档化**（conventions.md："使前缀/后缀
   映射生效，或显式补映射"）——把 v2 提到的"静默落 400"风险给了 Agent
   明确的避坑指引。
5. `just pre_commit` 是 Agent 的最终校验闭环：machete → fmt → clippy
   → arch_test → 全量测试。

### 问题

1. **commit 历史违反自己的规范**。`.cursorrules` 要求"标题说清为什么"，
   但近 20 条 commit 大量出现：
   - `feat: 更新文档和代码，优化环境变量配置，提升可读性和一致性。`
     （文档改动用 feat，且"提升一致性"是万能空话）
   - `feat: 更新 AGENTS.md 文档…`（应为 docs）
   - `feat: 优化身份验证模块，统一 axum 导入方式…`（应为 refactor/style）
   明显是 AI 批量生成的 commit message 且无门禁拦截。规范没有 enforcement
   机制（无 commitlint / CI hook），Agent 看到历史范例就会模仿坏范例。
   建议：加 commitlint 式检查，或至少在 AGENTS.md 贴 2–3 条"好例子 + 坏例子"。
2. **Agent 入口单点**：只有 AGENTS.md（Codex 系标准）。Claude Code 找
   `CLAUDE.md`、Cursor 找 `.cursor/rules`。建议 `CLAUDE.md` 一行
   `@AGENTS.md` 引用（或 symlink），避免第三处规则副本。
3. **l10n 双文件同步靠测试兜底但无前置提示**：Agent 新增 key 时最容易
   只改 en-US。AGENTS.md 有写，但更有效的做法是给 `just` 加
   `new-l10n-key <key> <en> <zh>` 之类的小命令，把双写变成单命令。

## 四、Agent 开发友好

### 做得好的（代码组织对 AI 编辑器极友好）

1. **单 endpoint 单文件、文件 ≤250 行、handler 统一叫 `handler`**——Agent
   编辑时锚点完全可预测，不需要读整个域就能安全修改一个端点；带副作用的
   逻辑强制 `execute()` 分离，让 `#[sqlx::test]` 能直接测业务而非测 handler。
2. **命名处处可 grep**：endpoint 文件名 = operation_id = 路由语义；
   错误 key 全 snake_case 且 ftl 里按域分文件；`Job::NAME` 常量即注册表键。
   从任一入口（OpenAPI / 日志错误码 / 队列表 job_type）都能字符串匹配回源码。
3. **护栏机器可查**：arch_test、locale key 覆盖测试、clippy deny 列表——
   Agent 违规会被"测试红"而不是"review 黄"拦住。
4. 每个非常规决策旁都有中文注释解释动机（跨键无事务的自愈语义、
   GC 按整月删的防误删规则、tsid 裁剪到 i64::MAX…）——Agent 读到时
   不会误判为 bug 而"修复"它。

### 问题

1. **无脚手架**（第二/四版已提，从 Agent 角度重申）：新建 feature 要动 8 处，
   AGENTS.md 用 8 条 checklist 描述。对 Agent 而言，`just new-feature order`
   生成骨架 + 在 AGENTS.md 留"已由脚手架生成"标注，比 8 条文字可靠得多。
2. **sqlx 宏编译期依赖活库**：Agent 在无 Docker 沙箱里 `cargo check` 直接
   失败。团队刻意不用 offline 模式（换取"SQL 永远与库同步"），合理，但
   对 Agent 工作流需要配套：`just db-up`（拉起 docker compose）+
   `just db-migrate`，并写进 AGENTS.md 的第一步。
3. **E2E 需要人工起服务**：`just e2e` 前置条件是服务已运行，没有
   `just e2e-full`（起依赖 + 起服务 + 跑 hurl + 清理）。Agent 无法独立
   验证端到端行为，只能退化为单测。
4. **TODO.md 是给人看的散文**（`[ ]` + 反斜杠转义痕迹）。建议改成
   机器可读格式（Markdown checkbox 规范化或 issues 引用），Agent 才能
   把"接手 TODO"纳入任务规划。

## 五、本版结论与优先级

这一版看到的是与前四版不同的画像：**这个仓库是罕见地把"AI 是主要作者
之一"纳入架构设计的项目**——渐进式上下文加载、可执行规范、机器护栏、
可预测命名四处都成立。它的问题不再是"缺规范"，而是：

1. **业务断点**：file 域 staging 无转正流程，客户端 URL 会被 GC（唯一
   逻辑级缺陷，应优先补齐或明确决策）；
2. **规范无 enforcement**：commit 历史已被 AI 空话污染，证明"只有文档
   没有 gate"的规范会衰减；
3. **规则与文档多头**：AGENTS.md / .cursorrules / docs/ai 三源并存，
   单一来源原则待落地；
4. **Agent 自动化缺口**：脚手架、db-up、e2e-full、l10n 双写——四个
   `just` 命令就能把 Agent 的手工清单变成单命令。

建议顺序：file 转正决策 → commitlint/commit 规范治理 → CLAUDE.md 引用 +
.cursorrules 收敛 → 四个 agent 工作流命令 → 业务能力地图 + TODO 规范化。
