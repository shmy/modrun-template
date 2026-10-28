# 仓库深度评估 · 第二版：实现与契约细节视角（2026-09-05）

> **历史快照，可能已过时。** 以根目录 [AGENTS.md](../AGENTS.md) 与当前代码为准；勿作为实现依据。

> 与 [repo-assessment.md](repo-assessment.md)（架构/安全/测试外围视角）互补。
> 本版换一个角度：**假设明天要在这份模板上开发第二个 feature**，从 API 契约质量、
> 代码级正确性、性能细节、二次开发 DX 出发逐处审读。所有发现均标注代码出处。

## 一、本次新发现的亮点

### 1. TSID 主键选型正确且处理细致
`features/shared_contract/value_object/id.rs`：
- `tsid` 时间排序，天然适配 keyset 游标分页（`ORDER BY id DESC` + `id < cursor`）；
- `Serialize` 一律输出字符串（防 JS `Number` 53 位精度丢失），`Deserialize` 兼容
  string/int 双格式——对前端友好的常见正确做法；
- 外部输入强制正数（`parse_positive_id`），`new_unchecked` 仅限 DB/已解码 JWT 等
  可信来源，逃逸口有明确注释约定。

### 2. refresh 发放失败有补偿事务
`account_refresh_token.rs`：`consume_refresh_token` 成功但 `issue_tokens` 失败时，
调用 `restore_refresh_token` 把已消费的 refresh 回置，用户可重试。补偿逻辑是
"取走 → 发放失败 → 归还"的完整闭环，并记录 error log。这个细节大多数实现会漏。

### 3. worker_jobs 表设计专业
`crates/job_queue_kit/migrations-pg/0001_init.sql`：
- `worker_jobs_fetch_idx (job_type, status, run_at, id)` 精确匹配 SKIP LOCKED 子查询
  的过滤 + 排序；
- `worker_jobs_gc_idx ON (done_at) WHERE status IN ('Done','Failed')` 部分索引
  配合分批 GC；
- `pg_notify` 在 INSERT 触发器内**事务提交后**投递，无丢失窗口（配合轮询兜底）。

### 4. 请求校验错误分类是对称、可测试的完整体系
`infra/web_state/extractor/valid_json.rs` / `valid_query.rs`：
- serde 阶段用 `serde_path_to_error` 保留完整字段路径（`items.0.quantity`），
  validify 阶段用 `errors_to_key_and_field`，两层都产出 `(l10n_key, field)`；
- body 与 query 的错误分类规则对称（`json_body_missing_field` ↔
  `query_missing_field` 等 5 类一一对应）；
- 分类函数本身有独立单元测试（含空路径 Display 归一 `"." → ""` 这类边角）；
- i18n detail 参数化渲染（"缺少必填字段：phone"），错误信息对用户可用。

### 5. 限流在 argon2 之前执行
`account_login.rs`：`enforce_login_rate_limit` 先于 `find_auth_by_phone` +
`password.verify`。密码哈希验证是刻意的 CPU 开销，把它放在限流之后是防
argon2 DoS 的正确顺序。

### 6. 游标分页实现干净
`cursor_paging_query.rs`：`limit` 默认 10、clamp 1..100；`repository.rs::search`
fetch `limit+1` 判 `has_more`，`next_cursor` 取最后一行，无 offset 翻页的深分页问题。

## 二、本次新发现的代码级问题

### 1. OpenAPI 示例泄露默认管理员密码（P0，放大第一版 P1#4）
`features/identity/dto.rs` 的 `LoginRequest` 与 `account_create.rs` 的
`CreateAccountRequest` 均有 `#[schema(example = "admin123!")]`——默认 seed 密码
被写进公开的 `/openapi.json`，README 又写明密码即 `admin123!`。三者叠加后，
任何一个忘改环境变量的部署都等于公开了管理员凭证。示例应换成无关字符串
（如 `"Str0ng!Passw0rd"`），seed 默认值问题见第一版。

### 2. Body 超限被错误地映射为 400 而非 413
`valid_json.rs`：`Bytes::from_request` 失败（含 `DefaultBodyLimit` 触发的
`LengthLimitError`）一律映射为 `400 invalid_request_body`。超过 `BODY_LIMIT_BYTES`
的请求应返回 `413`（配 `request_too_large` l10n key）。目前客户端无法区分
"格式错"和"体积大"，重试语义也不同。

### 3. 登录手机号枚举的时序侧信道
`account_login.rs`：手机号不存在时直接 `AccountInvalidCredentials` 返回，
**跳过了 argon2 verify**（约几十毫秒级差异）。攻击者可通过响应时间差枚举注册
手机号。标准对策：账号不存在时也执行一次 dummy hash verify。与"密码错误不区分
用户不存在"的语义意图相悖——语义做了模糊，时序没做。

### 4. serde 错误分类依赖 Display 文本匹配
`valid_json.rs::l10n_key_and_field` 用 `msg.contains("trailing characters")`
细分 Syntax/Eof。serde 升级改措辞会静默落回 `json_body_syntax`。有测试兜底，
但升级 serde 时这是必须重看的隐性契约点，值得加注释或在 CI 里显式覆盖。

### 5. 孤儿扫描查询无索引
`backend.rs::reenqueue_orphaned`：`UPDATE ... WHERE status='Running' AND lock_at < $1`
在 `worker_jobs` 上没有对应索引（fetch_idx 以 job_type 为首列，覆盖不到），
每 60s 全表扫一次。job 表 GC 及时的话量小，但模板用户跑大任务量后会踩到。
建议补部分索引 `ON worker_jobs (lock_at) WHERE status = 'Running'`。

### 6. OpenAPI 错误响应文档不一致
identity 全部 endpoint 完整标注了 400/401/403/404/409/429 的 `ProblemDetails`
（质量很高），但 `file_upload_image.rs` 只标注了 200——连实际的 400
（非图片/缺文件名）都没写。第二个 feature 作者会以哪个为准？应统一。

### 7. 其他小项
- `ValidJson` 的 Content-Type 校验用 `starts_with("application/json")`，
  会放过 `application/json-patch+json` 等衍生类型（极小，注释已说明是对齐
  axum 行为的有意取舍）。
- `LoginRequest` 复用 `Password` 值对象：8–64 长度规则同时绑定了"注册"与
  "登录"。将来放宽/收紧密码策略时，老用户可能连登录都过不了校验。登录侧
  应用不做强度校验的轻量类型。
- 游标 `cursor` 是裸 `Id` 无签名——客户端可任意跳页。keyset 语义无注入风险，
  通常可接受，列为已知取舍。

## 三、二次开发 DX（写第二个 feature 的实际体验）

### 1. 新增一个 feature 要动 8 处
`features/<name>/` + `features/<name>_contract/` + workspace `Cargo.toml` +
`bin/server/Cargo.toml` + `bin/server/main.rs`（module 注册）+ 迁移 SQL +
en-US/zh-CN 两个 ftl + （若新 l10n 文件）`locale/lib.rs`。步骤在 AGENTS.md
写得清楚，但纯手工、无脚手架。建议提供 `just new-feature <name>` 生成骨架，
能显著降低模板使用者的第一步摩擦。

### 2. sqlx 宏要求编译期活库
所有 `query!` 宏需要 `DATABASE_URL` 指向**已迁移**的库。新 clone 后必须先
`docker compose up` + 跑迁移才能 `cargo check`（SQL 离线模式被刻意不用，
见 AGENTS.md）。这是"编译期正确性 > 开发便利"的取舍，合理，但应在 README
顶部用醒目步骤说明（目前埋在文档中段），CI 也因此必须带 Postgres 服务。

### 3. 依赖面 578 个 crate，纪律良好但基数不小
workspace 全面 `default-features = false` + 精准 feature 列表，值得肯定；
但 axum/sqlx/otel/tonic/fluent 等加起来编译链仍偏重，增量开发时的首次
`cargo check` 门槛值得用 sccache / 缓存层缓解（再次指向缺 CI）。

### 4. 文档三处同步成本
AGENTS.md / README.md / docs/ai/* 中 identity 端点表、环境变量表各出现一次，
改一个端点要同步多处。AGENTS.md 的表已经是准文档，长期应让 README/ai-docs
引用单一来源。

### 5. TODO.md 自曝的缺口与第一版结论一致
`event_bus`（Pg Outbox / NATS）、`flow`、`http_client`、独立 `bin/migrator`。
其中 **migrator** 值得提前：目前迁移绑定在 server 启动（master seed 流程），
多实例部署下"谁跑迁移"没有显式答案——k8s 里通常需要 initContainer 或 Job
形态的独立迁移器，TODO 里的规划应该升优先级。

## 四、两版结论合并

第一版（架构/安全外围）指出的问题集中在 CI、容器化、metrics、权限/审计；
本版（实现/契约细节）新增的 P0 是 **OpenAPI 示例泄露默认密码** 与
**413 语义错误**，以及三个低成本高收益修复：登录 dummy-hash 防时序枚举、
孤儿扫描部分索引、file endpoint 补错误响应文档。

代码级实现质量整体依然高于预期——TSID/补偿事务/错误分类体系这类细节说明
作者对"正确性来自细节"有自觉。合并后的修复优先级：

1. CI + Dockerfile（第一版）
2. seed 密码三处收敛（默认值防护 + OpenAPI 示例 + README）（两版合并）
3. `ValidJson` 超限 → 413
4. 登录 dummy hash verify
5. 孤儿扫描部分索引 + `lock_by` 注入实例标识
6. `just new-feature` 脚手架 + 独立 migrator
