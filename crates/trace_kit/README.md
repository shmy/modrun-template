# trace_kit

`trace_kit` 提供两类能力：

- `trace_id`：提取并规范化 trace id（优先 `traceparent`，其次 `x-request-id`，最后当前 span fallback）
- `init`：初始化 tracing（可选 `console` / `otlp`）

## Feature 选择

- 仅提取 trace id：
  - `features = ["trace_id"]`
- 控制台日志初始化：
  - `features = ["init", "console"]`
- OTLP 初始化：
  - `features = ["init", "otlp"]`

## 示例

### 1) 在中间件中提取 trace id

```rust
use trace_kit::extract_trace_id;

let trace_id = extract_trace_id(request.headers());
```

### 2) 在服务入口初始化 tracing

```rust
use config::cli::ExposeSecret;
use trace_kit::{TraceConfig, init_tracing};

let _guard = init_tracing(
    TraceConfig::new(
        &cli.log.level,
        &cli.otlp.service_name,
        &cli.otlp.endpoint,
        cli.otlp.metadata.expose_secret(),
    ),
    vec![],
);
```
