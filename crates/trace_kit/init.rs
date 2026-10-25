#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "init-time panics are acceptable; missing OTLP wiring is a deployment error"
)]
#[cfg(feature = "otlp")]
use opentelemetry::{KeyValue, StringValue, Value};
#[cfg(feature = "otlp")]
use opentelemetry_sdk::Resource;
#[cfg(feature = "otlp")]
use opentelemetry_semantic_conventions::{SCHEMA_URL, resource::SERVICE_VERSION};
use rootcause::hooks::Hooks;
use rootcause_tracing::{RootcauseLayer, SpanCollector};
use tracing_subscriber::layer::Identity;
use tracing_subscriber::{
    EnvFilter, layer::Layer, layer::SubscriberExt as _, registry::Registry,
    util::SubscriberInitExt as _,
};

/// 日志过滤（各层按需取用）：默认 info，OTel 桥接链路全程 trace；
/// 无论是否设置 RUST_LOG，都关闭 `sqlx::query` 语句日志，避免 SQL 参数（含密码哈希）进入日志 / OTLP；
/// `opentelemetry/hyper/h2/tonic/reqwest=off` 防 telemetry-induced-telemetry（导出栈自身日志
/// 不再回灌 OTLP）。
fn log_filter(config: &TraceConfig) -> EnvFilter {
    let mut filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(format!(
            "{},otel::tracing=trace,opentelemetry=off,hyper=off,h2=off,tonic=off,reqwest=off",
            config.level
        ))
    });
    if let Ok(directive) = "sqlx::query=off".parse() {
        filter = filter.add_directive(directive);
    }
    filter
}

/// OTLP/gRPC 传输配置：
/// - endpoint：collector base URL（gRPC 不把租户等信息放进 URL 路径）；
/// - metadata：`OTLP_METADATA` JSON 原样转为 gRPC metadata（如 `Authorization`）。
///   不在库内补默认 header。
#[cfg(feature = "otlp")]
fn otlp_grpc_config(
    endpoint: &str,
    metadata: &str,
) -> (
    String,
    opentelemetry_otlp::tonic_types::metadata::MetadataMap,
) {
    use std::str::FromStr as _;
    let map: std::collections::HashMap<String, String> =
        serde_json::from_str(metadata).expect("parse otlp_metadata");
    let mut m = opentelemetry_otlp::tonic_types::metadata::MetadataMap::new();
    for (k, v) in map {
        let key = tonic::metadata::MetadataKey::from_str(&k).expect("metadata key");
        let value = tonic::metadata::MetadataValue::from_str(&v).expect("metadata value");
        m.insert(key, value);
    }
    (endpoint.to_string(), m)
}

#[derive(Debug)]
pub struct TraceConfig<'a> {
    level: &'a str,
    #[cfg(feature = "otlp")]
    otlp_service_name: &'a str,
    #[cfg(feature = "otlp")]
    otlp_endpoint: &'a str,
    #[cfg(feature = "otlp")]
    otlp_metadata: &'a str,
    /// Trace 采样率 `(0, 1]`；仅在启用 OTLP 时生效。`1.0` = AlwaysOn。
    #[cfg(feature = "otlp")]
    otlp_sampler_ratio: f64,
}

impl<'a> TraceConfig<'a> {
    pub fn new(
        level: &'a str,
        #[cfg(feature = "otlp")] otlp_service_name: &'a str,
        #[cfg(feature = "otlp")] otlp_endpoint: &'a str,
        #[cfg(feature = "otlp")] otlp_metadata: &'a str,
        #[cfg(feature = "otlp")] otlp_sampler_ratio: f64,
    ) -> Self {
        Self {
            level,
            #[cfg(feature = "otlp")]
            otlp_service_name,
            #[cfg(feature = "otlp")]
            otlp_endpoint,
            #[cfg(feature = "otlp")]
            otlp_metadata,
            #[cfg(feature = "otlp")]
            otlp_sampler_ratio,
        }
    }
}

pub fn init_tracing(
    config: TraceConfig,
    extra_layers: Vec<Box<dyn Layer<Registry> + Send + Sync>>,
) -> TracingGuard {
    let filter = log_filter(&config);

    // per-layer filtering：各日志/桥接层独立挂 EnvFilter；extra 层放最前。
    let extra_stack: Box<dyn Layer<Registry> + Send + Sync> = extra_layers
        .into_iter()
        .reduce(|acc, layer| Box::new(acc.and_then(layer)))
        .unwrap_or_else(|| Box::new(Identity::new()));

    #[cfg(feature = "otlp")]
    {
        // `OTLP_ENDPOINT` 为空 → 跳过三信号导出，仅 console / rootcause（本地无 collector 不白烧）。
        if !config.otlp_endpoint.trim().is_empty() {
            return init_with_otlp(config, filter, extra_stack);
        }
    }

    let builder = tracing_subscriber::registry().with(extra_stack);
    let builder = builder.with(RootcauseLayer.with_filter(filter.clone()));
    #[cfg(feature = "console")]
    let builder = builder.with(
        tracing_subscriber::fmt::layer()
            .with_target(false)
            .with_filter(filter),
    );
    builder.init();
    install_rootcause_hooks();
    TracingGuard::None
}

fn install_rootcause_hooks() {
    Hooks::new()
        .report_creation_hook(SpanCollector::new())
        .install()
        .expect("failed to install rootcause hooks");
}

#[cfg(feature = "otlp")]
fn init_with_otlp(
    config: TraceConfig,
    filter: EnvFilter,
    extra_stack: Box<dyn Layer<Registry> + Send + Sync>,
) -> TracingGuard {
    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
    use opentelemetry_otlp::{
        LogExporter, MetricExporter, WithExportConfig as _, WithTonicConfig as _,
    };
    use opentelemetry_sdk::logs::SdkLoggerProvider;
    use opentelemetry_sdk::metrics::{MeterProviderBuilder, PeriodicReader, Temporality};
    use opentelemetry_sdk::trace::{RandomIdGenerator, Sampler, SdkTracerProvider};
    use tracing_opentelemetry::MetricsLayer;

    let res = resource(config.otlp_service_name);

    let (endpoint, metadata) = otlp_grpc_config(config.otlp_endpoint, config.otlp_metadata);
    let log_exporter = LogExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .with_metadata(metadata)
        .build()
        .expect("build log exporter");
    let log_provider = SdkLoggerProvider::builder()
        .with_batch_exporter(log_exporter)
        .with_resource(res.clone())
        .build();
    let log_layer = OpenTelemetryTracingBridge::new(&log_provider);

    let (endpoint, metadata) = otlp_grpc_config(config.otlp_endpoint, config.otlp_metadata);
    let metric_exporter = MetricExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .with_metadata(metadata)
        .with_temporality(Temporality::Cumulative)
        .build()
        .expect("build metrics exporter");
    let reader = PeriodicReader::builder(metric_exporter)
        .with_interval(std::time::Duration::from_secs(30))
        .build();
    let metrics_provider = MeterProviderBuilder::default()
        .with_reader(reader)
        .with_resource(res.clone())
        .build();
    // 注册全局 meter provider；业务侧用 `opentelemetry::global::meter(service_name)` 取仪表。
    opentelemetry::global::set_meter_provider(metrics_provider.clone());
    let metrics_layer = MetricsLayer::new(metrics_provider.clone());

    let (endpoint, metadata) = otlp_grpc_config(config.otlp_endpoint, config.otlp_metadata);
    let span_exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .with_metadata(metadata)
        .build()
        .expect("build trace exporter");

    let sampler = if config.otlp_sampler_ratio >= 1.0 {
        Sampler::AlwaysOn
    } else {
        Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(
            config.otlp_sampler_ratio,
        )))
    };

    let trace_provider = SdkTracerProvider::builder()
        .with_sampler(sampler)
        .with_resource(res)
        .with_id_generator(RandomIdGenerator::default())
        .with_batch_exporter(span_exporter)
        .build();
    let tracer = trace_provider.tracer(config.otlp_service_name.to_string());
    let trace_layer = tracing_opentelemetry::layer().with_tracer(tracer);

    let builder = tracing_subscriber::registry().with(extra_stack);
    let builder = builder.with(RootcauseLayer.with_filter(filter.clone()));
    #[cfg(feature = "console")]
    let builder = builder.with(
        tracing_subscriber::fmt::layer()
            .with_target(false)
            .with_filter(filter.clone()),
    );
    let builder = builder
        .with(log_layer.with_filter(filter.clone()))
        .with(metrics_layer.with_filter(filter.clone()))
        .with(trace_layer.with_filter(filter));
    builder.init();
    install_rootcause_hooks();

    TracingGuard::Otlp(OtlpGuard {
        log_provider,
        metrics_provider,
        trace_provider,
    })
}

#[cfg(feature = "otlp")]
fn resource(service_name: &str) -> Resource {
    Resource::builder()
        .with_service_name(Value::String(StringValue::from(service_name.to_string())))
        .with_schema_url(
            [KeyValue::new(SERVICE_VERSION, env!("CARGO_PKG_VERSION"))],
            SCHEMA_URL,
        )
        .build()
}

pub enum TracingGuard {
    #[cfg(feature = "otlp")]
    Otlp(OtlpGuard),
    None,
}

#[cfg(feature = "otlp")]
pub struct OtlpGuard {
    log_provider: opentelemetry_sdk::logs::SdkLoggerProvider,
    metrics_provider: opentelemetry_sdk::metrics::SdkMeterProvider,
    trace_provider: opentelemetry_sdk::trace::SdkTracerProvider,
}

#[cfg(feature = "otlp")]
impl Drop for OtlpGuard {
    fn drop(&mut self) {
        eprintln!("shutting down log provider");
        if let Err(err) = self.log_provider.shutdown() {
            eprintln!("{err:?}");
        }
        eprintln!("shutting down metrics provider");
        if let Err(err) = self.metrics_provider.shutdown() {
            eprintln!("{err:?}");
        }
        eprintln!("shutting down tracer provider");
        if let Err(err) = self.trace_provider.shutdown() {
            eprintln!("{err:?}");
        }
    }
}
