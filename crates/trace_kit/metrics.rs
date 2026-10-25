//! 业务侧 OTel 计数器。无 meter provider（`OTLP_ENDPOINT` 为空）时为 noop。

use opentelemetry::KeyValue;
use opentelemetry::global::meter;
use opentelemetry::metrics::Counter;
use std::sync::LazyLock;

const METER: &str = "modrun-template";

static LOGIN: LazyLock<Counter<u64>> = LazyLock::new(|| {
    meter(METER)
        .u64_counter("auth.login")
        .with_description("Identity login attempts")
        .build()
});

static RATE_LIMITED: LazyLock<Counter<u64>> = LazyLock::new(|| {
    meter(METER)
        .u64_counter("http.rate_limited")
        .with_description("Requests rejected by axum-governor")
        .build()
});

static JOB_FAILED: LazyLock<Counter<u64>> = LazyLock::new(|| {
    meter(METER)
        .u64_counter("job.failed")
        .with_description("Background job terminal failures")
        .build()
});

pub fn record_login(success: bool) {
    LOGIN.add(1, &[KeyValue::new("success", success)]);
}

pub fn record_rate_limited() {
    RATE_LIMITED.add(1, &[]);
}

pub fn record_job_failed(job_type: &str) {
    JOB_FAILED.add(1, &[KeyValue::new("job_type", job_type.to_string())]);
}
