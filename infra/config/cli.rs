use crate::readable_duration::ReadableDuration;
use clap::Parser;
pub use secrecy::ExposeSecret;
use secrecy::SecretString;
use std::result::Result;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum AppEnv {
    #[default]
    Development,
    Production,
}

impl AppEnv {
    pub fn is_production(self) -> bool {
        matches!(self, Self::Production)
    }
}

#[derive(Clone, Debug, Parser)]
#[command(version, about, long_about = None)]
pub struct Cli {
    #[command(flatten)]
    pub log: LogCli,

    #[command(flatten)]
    pub otlp: OtlpCli,

    #[command(flatten)]
    pub server: ServerCli,

    #[command(flatten)]
    pub database: DatabaseCli,

    #[command(flatten)]
    pub jwt: JwtCli,

    #[command(flatten)]
    pub seed: SeedCli,

    #[command(flatten)]
    pub rate_limit: RateLimitCli,

    #[command(flatten)]
    pub cache: CacheCli,

    #[command(flatten)]
    pub job_queue: JobQueueCli,

    #[command(flatten)]
    pub blob: BlobCli,
}

#[derive(Clone, Debug, clap::Args)]
pub struct LogCli {
    #[arg(
        id = "log_level",
        long = "log-level",
        env = "RUST_LOG",
        default_value = "info"
    )]
    pub level: String,
}

#[derive(Clone, Debug, clap::Args)]
pub struct OtlpCli {
    /// Collector base URL。空字符串则关闭 OTLP 三信号导出（仅 console）。
    #[arg(
        id = "otlp_endpoint",
        long = "otlp-endpoint",
        env = "OTLP_ENDPOINT",
        default_value = "http://localhost:4317"
    )]
    pub endpoint: String,

    #[arg(
        id = "otlp_service_name",
        long = "otlp-service-name",
        env = "OTLP_SERVICE_NAME",
        default_value = "modrun-template"
    )]
    pub service_name: String,

    #[arg(
        id = "otlp_metadata",
        long = "otlp-metadata",
        env = "OTLP_METADATA",
        default_value = "{}",
        value_parser = parse_secret_string
    )]
    pub metadata: SecretString,

    /// Trace 采样率 `(0, 1]`；`1` = 全量。parent-based TraceIdRatio。
    #[arg(
        id = "otlp_sampler_ratio",
        long = "otlp-sampler-ratio",
        env = "OTLP_SAMPLER_RATIO",
        default_value_t = 0.1
    )]
    pub sampler_ratio: f64,
}

#[derive(Clone, Debug, clap::Args)]
pub struct ServerCli {
    #[arg(
        id = "listen_addr",
        long = "listen-addr",
        default_value = "0.0.0.0:8080",
        env = "LISTEN_ADDR"
    )]
    pub listen_addr: String,

    #[arg(id = "master", long = "master", default_value = "true", env = "MASTER")]
    pub master: bool,

    #[arg(
        id = "app_env",
        long = "app-env",
        env = "APP_ENV",
        default_value = "development",
        value_enum
    )]
    pub app_env: AppEnv,

    #[arg(
        id = "request_timeout",
        long = "request-timeout",
        default_value = "60s",
        env = "REQUEST_TIMEOUT"
    )]
    pub request_timeout: ReadableDuration,

    #[arg(
        id = "body_limit_bytes",
        long = "body-limit-bytes",
        default_value = "2097152",
        env = "BODY_LIMIT_BYTES"
    )]
    pub body_limit_bytes: usize,

    #[arg(
        id = "scalar_ui_enabled",
        long = "scalar-ui-enabled",
        default_value = "true",
        env = "SCALAR_UI_ENABLED"
    )]
    pub scalar_ui_enabled: bool,

    /// 逗号分隔的可信代理 IP/CIDR；仅当直连对端在此列表内时才解析 X-Forwarded-For / Forwarded。
    #[arg(
        id = "trusted_proxies",
        long = "trusted-proxies",
        default_value = "",
        env = "TRUSTED_PROXIES"
    )]
    pub trusted_proxies: String,

    /// 逗号分隔浏览器 Origin 白名单；空则不加 CORS 层。
    #[arg(
        id = "cors_origins",
        long = "cors-origins",
        default_value = "",
        env = "CORS_ORIGINS"
    )]
    pub cors_origins: String,
}

#[derive(Clone, Debug, clap::Args)]
pub struct DatabaseCli {
    #[arg(
        id = "database_url",
        long = "database-url",
        env = "DATABASE_URL",
        value_parser = parse_secret_string
    )]
    pub url: SecretString,

    #[arg(
        id = "database_max_connections",
        long = "database-max-connections",
        default_value = "20",
        env = "DATABASE_MAX_CONNECTIONS"
    )]
    pub max_connections: u32,

    #[arg(
        id = "database_min_connections",
        long = "database-min-connections",
        default_value = "1",
        env = "DATABASE_MIN_CONNECTIONS"
    )]
    pub min_connections: u32,

    #[arg(
        id = "database_max_lifetime",
        long = "database-max-lifetime",
        default_value = "15min",
        env = "DATABASE_MAX_LIFETIME"
    )]
    pub max_lifetime: ReadableDuration,

    #[arg(
        id = "database_idle_timeout",
        long = "database-idle-timeout",
        default_value = "5min",
        env = "DATABASE_IDLE_TIMEOUT"
    )]
    pub idle_timeout: ReadableDuration,

    #[arg(
        id = "database_acquire_timeout",
        long = "database-acquire-timeout",
        default_value = "5s",
        env = "DATABASE_ACQUIRE_TIMEOUT"
    )]
    pub acquire_timeout: ReadableDuration,
}

#[derive(Clone, Debug, clap::Args)]
pub struct JwtCli {
    #[arg(
        id = "jwt_secret",
        long = "jwt-secret",
        env = "JWT_SECRET",
        value_parser = parse_jwt_secret
    )]
    pub secret: SecretString,

    #[arg(
        id = "access_token_ttl",
        long = "access-token-ttl",
        default_value = "15min",
        env = "ACCESS_TOKEN_TTL"
    )]
    pub access_ttl: ReadableDuration,

    #[arg(
        id = "refresh_token_ttl",
        long = "refresh-token-ttl",
        default_value = "30d",
        env = "REFRESH_TOKEN_TTL"
    )]
    pub refresh_ttl: ReadableDuration,

    #[arg(
        id = "jwt_issuer",
        long = "jwt-issuer",
        env = "JWT_ISSUER",
        default_value = "modrun-template"
    )]
    pub issuer: String,

    #[arg(
        id = "jwt_audience",
        long = "jwt-audience",
        env = "JWT_AUDIENCE",
        default_value = "account"
    )]
    pub audience: String,
}

#[derive(Clone, Debug, clap::Args)]
pub struct SeedCli {
    #[arg(
        id = "seed_admin_phone",
        long = "seed-admin-phone",
        env = "SEED_ADMIN_PHONE",
        default_value = "13888888888"
    )]
    pub admin_phone: String,

    #[arg(
        id = "seed_admin_password",
        long = "seed-admin-password",
        env = "SEED_ADMIN_PASSWORD",
        default_value = "admin123!",
        value_parser = parse_secret_string
    )]
    pub admin_password: SecretString,
}

/// axum-governor 全局限流（按客户端 IP）与登录/刷新路径更紧配额。
#[derive(Clone, Debug, clap::Args)]
pub struct RateLimitCli {
    #[arg(
        id = "rate_limit_global_per_second",
        long = "rate-limit-global-per-second",
        env = "RATE_LIMIT_GLOBAL_PER_SECOND",
        default_value = "100"
    )]
    pub global_per_second: u32,

    #[arg(
        id = "rate_limit_global_burst",
        long = "rate-limit-global-burst",
        env = "RATE_LIMIT_GLOBAL_BURST",
        default_value = "200"
    )]
    pub global_burst: u32,

    #[arg(
        id = "rate_limit_auth_per_second",
        long = "rate-limit-auth-per-second",
        env = "RATE_LIMIT_AUTH_PER_SECOND",
        default_value = "5"
    )]
    pub auth_per_second: u32,

    #[arg(
        id = "rate_limit_auth_burst",
        long = "rate-limit-auth-burst",
        env = "RATE_LIMIT_AUTH_BURST",
        default_value = "10"
    )]
    pub auth_burst: u32,
}

#[derive(Clone, Debug, clap::Args)]
pub struct CacheCli {
    #[arg(
        id = "cache_redb_path",
        long = "cache-redb-path",
        default_value = "data/cache.redb",
        env = "CACHE_REDB_PATH"
    )]
    pub cache_redb_path: String,

    #[arg(
        id = "cache_redis_url",
        long = "cache-redis-url",
        default_value = "redis://127.0.0.1:6379",
        env = "CACHE_REDIS_URL"
    )]
    pub cache_redis_url: String,
}

#[derive(Clone, Debug, clap::Args)]
pub struct JobQueueCli {
    #[arg(
        id = "job_queue_sqlite_path",
        long = "job-queue-sqlite-path",
        default_value = "worker.db",
        env = "JOB_QUEUE_SQLITE_PATH"
    )]
    pub job_queue_sqlite_path: String,
}

#[derive(Clone, Debug, clap::Args)]
pub struct BlobCli {
    #[arg(
        id = "blob_file_root",
        long = "blob-file-root",
        default_value = "data/blob",
        env = "BLOB_FILE_ROOT"
    )]
    pub blob_file_root: String,

    #[arg(
        id = "blob_file_domain",
        long = "blob-file-domain",
        default_value = "http://localhost:8080/files",
        env = "BLOB_FILE_DOMAIN"
    )]
    pub blob_file_domain: String,

    /// COS 公网访问域名（`blob-cos`；`blob-fs` 用 `BLOB_FILE_DOMAIN`）。
    #[arg(
        id = "blob_s3_domain",
        long = "blob-s3-domain",
        default_value = "http://localhost:8080/files",
        env = "BLOB_S3_DOMAIN"
    )]
    pub blob_s3_domain: String,

    #[arg(
        id = "blob_s3_endpoint",
        long = "blob-s3-endpoint",
        env = "BLOB_S3_ENDPOINT",
        default_value = "https://cos.ap-guangzhou.myqcloud.com"
    )]
    pub blob_s3_endpoint: String,

    #[arg(
        id = "blob_s3_bucket",
        long = "blob-s3-bucket",
        env = "BLOB_S3_BUCKET",
        default_value = "modrun-template"
    )]
    pub blob_s3_bucket: String,

    #[arg(
        id = "blob_s3_secret_id",
        long = "blob-s3-secret-id",
        env = "BLOB_S3_SECRET_ID",
        default_value = "",
        value_parser = parse_secret_string
    )]
    pub blob_s3_secret_id: SecretString,

    #[arg(
        id = "blob_s3_secret_key",
        long = "blob-s3-secret-key",
        env = "BLOB_S3_SECRET_KEY",
        default_value = "",
        value_parser = parse_secret_string
    )]
    pub blob_s3_secret_key: SecretString,
}

/// clap 需要 `T: FromStr`，而 `SecretString` 未实现 FromStr；此 parser 仅作类型适配。
fn parse_secret_string(s: &str) -> Result<SecretString, String> {
    Ok(SecretString::from(s))
}

const MIN_JWT_SECRET_LEN: usize = 32;
pub const DEFAULT_SEED_ADMIN_PASSWORD: &str = "admin123!";

fn parse_jwt_secret(s: &str) -> Result<SecretString, String> {
    if s.len() < MIN_JWT_SECRET_LEN {
        return Err(format!(
            "JWT_SECRET must be at least {MIN_JWT_SECRET_LEN} bytes"
        ));
    }
    Ok(SecretString::from(s))
}

impl Cli {
    pub fn validate(&self) -> Result<(), String> {
        if self.server.app_env.is_production()
            && self.seed.admin_password.expose_secret() == DEFAULT_SEED_ADMIN_PASSWORD
        {
            return Err(
                "SEED_ADMIN_PASSWORD must not use the default value when APP_ENV=production"
                    .to_string(),
            );
        }
        if !(self.otlp.sampler_ratio > 0.0 && self.otlp.sampler_ratio <= 1.0) {
            return Err(
                "OTLP_SAMPLER_RATIO must be in (0, 1] (use empty OTLP_ENDPOINT to disable OTLP)"
                    .to_string(),
            );
        }
        for (name, value) in [
            (
                "RATE_LIMIT_GLOBAL_PER_SECOND",
                self.rate_limit.global_per_second,
            ),
            ("RATE_LIMIT_GLOBAL_BURST", self.rate_limit.global_burst),
            (
                "RATE_LIMIT_AUTH_PER_SECOND",
                self.rate_limit.auth_per_second,
            ),
            ("RATE_LIMIT_AUTH_BURST", self.rate_limit.auth_burst),
        ] {
            if value == 0 {
                return Err(format!("{name} must be greater than 0"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cli_with_seed_password(password: &str) -> Cli {
        let mut cli = Cli::parse_from([
            "server",
            "--database-url",
            "postgres://postgres:postgres@localhost/modrun",
            "--jwt-secret",
            "dev-only-jwt-secret-at-least-32-bytes!!",
        ]);
        cli.seed.admin_password = SecretString::from(password);
        cli
    }

    #[test]
    fn jwt_secret_shorter_than_32_bytes_is_rejected() {
        assert!(parse_jwt_secret("short-secret").is_err_and(|err| err.contains("32")));
    }

    #[test]
    fn production_rejects_default_seed_password() {
        let mut cli = cli_with_seed_password(DEFAULT_SEED_ADMIN_PASSWORD);
        cli.server.app_env = AppEnv::Production;
        assert!(
            cli.validate()
                .is_err_and(|err| err.contains("SEED_ADMIN_PASSWORD"))
        );
    }

    #[test]
    fn production_allows_custom_seed_password() {
        let mut cli = cli_with_seed_password("custom-production-password-32chars!");
        cli.server.app_env = AppEnv::Production;
        assert!(cli.validate().is_ok());
    }

    #[test]
    fn zero_rate_limit_values_are_rejected() {
        let mut cli = cli_with_seed_password("custom-production-password-32chars!");
        cli.rate_limit.global_per_second = 0;
        assert!(
            cli.validate()
                .is_err_and(|err| err.contains("RATE_LIMIT_GLOBAL_PER_SECOND"))
        );
    }

    #[test]
    fn otlp_sampler_ratio_must_be_in_unit_interval() {
        let mut cli = cli_with_seed_password("custom-production-password-32chars!");
        cli.otlp.sampler_ratio = 0.0;
        assert!(
            cli.validate()
                .is_err_and(|err| err.contains("OTLP_SAMPLER_RATIO"))
        );
        cli.otlp.sampler_ratio = 1.5;
        assert!(cli.validate().is_err());
        cli.otlp.sampler_ratio = 0.1;
        assert!(cli.validate().is_ok());
    }
}
