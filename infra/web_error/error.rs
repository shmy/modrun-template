use std::fmt;

use std::sync::Mutex;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use rootcause::Report;
use std::error::Error;
use std::sync::MutexGuard;

#[derive(Debug, Clone)]
pub enum WebError {
    /// 参数解析失败（请求体 / 查询字符串 / 路径参数）：`key` 为 l10n key
    /// （serde 错误由 extractor 层分类映射，validify 错误由 `errors_to_key` 产出），
    /// `field` 为出错字段（serde_path_to_error / ErrorKind / multipart 结构化错误提供，可 None）。
    InvalidParams {
        kind: InvalidParamsKind,
        key: String,
        field: Option<String>,
    },
    /// 本地化 KEY
    L10n(String),
}

/// 参数错误来源：决定 problem_type / problem_title / error_code 契约。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidParamsKind {
    /// JSON / multipart 请求体。
    Body,
    /// URL 查询字符串。
    Query,
    /// 路径参数。
    Path,
}

impl InvalidParamsKind {
    fn problem_type(self) -> &'static str {
        match self {
            InvalidParamsKind::Body => "urn:modrun:problem:invalid-request-body",
            InvalidParamsKind::Query => "urn:modrun:problem:invalid-query-params",
            InvalidParamsKind::Path => "urn:modrun:problem:invalid-path-params",
        }
    }

    fn problem_title(self) -> &'static str {
        match self {
            InvalidParamsKind::Body => "Invalid request body",
            InvalidParamsKind::Query => "Invalid query parameters",
            InvalidParamsKind::Path => "Invalid path parameters",
        }
    }

    fn error_code(self) -> &'static str {
        match self {
            InvalidParamsKind::Body => "invalid_request_body",
            // error_code 是机器可读契约，不渲染 detail，非 l10n key。
            InvalidParamsKind::Query => "invalid_query_params",
            InvalidParamsKind::Path => "invalid_path_params",
        }
    }
}

impl fmt::Display for WebError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WebError::InvalidParams { key, .. } => write!(f, "{key}"),
            WebError::L10n(s) => write!(f, "{s}"),
        }
    }
}

impl Error for WebError {}

type DomainReportMapper = fn(&Report) -> Option<WebError>;

static DOMAIN_REPORT_MAPPERS: Mutex<Vec<DomainReportMapper>> = Mutex::new(Vec::new());

fn mappers_lock() -> MutexGuard<'static, Vec<DomainReportMapper>> {
    DOMAIN_REPORT_MAPPERS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 注册域错误映射（feature 在 `module()` 时调用）；优先于 `is_l10n_key` 启发式。
pub fn register_domain_mapper(mapper: DomainReportMapper) {
    mappers_lock().push(mapper);
}

fn try_domain_mappers(report: &Report) -> Option<WebError> {
    for mapper in mappers_lock().iter() {
        if let Some(err) = mapper(report) {
            return Some(err);
        }
    }
    None
}

impl WebError {
    pub fn status_code(&self) -> StatusCode {
        match self {
            WebError::InvalidParams { .. } => StatusCode::BAD_REQUEST,
            WebError::L10n(key) => l10n_status_code(key),
        }
    }

    pub fn problem_type(&self) -> &'static str {
        match self {
            WebError::InvalidParams { kind, .. } => kind.problem_type(),
            WebError::L10n(key) => match l10n_status_code(key) {
                StatusCode::UNAUTHORIZED => "urn:modrun:problem:unauthorized",
                StatusCode::FORBIDDEN => "urn:modrun:problem:forbidden",
                StatusCode::NOT_FOUND => "urn:modrun:problem:not-found",
                StatusCode::CONFLICT => "urn:modrun:problem:conflict",
                StatusCode::TOO_MANY_REQUESTS => "urn:modrun:problem:rate-limited",
                StatusCode::SERVICE_UNAVAILABLE => "urn:modrun:problem:service-unavailable",
                StatusCode::REQUEST_TIMEOUT => "urn:modrun:problem:request-timeout",
                StatusCode::INTERNAL_SERVER_ERROR => "urn:modrun:problem:internal-server-error",
                _ => "urn:modrun:problem:domain-error",
            },
        }
    }

    pub fn problem_title(&self) -> &'static str {
        match self {
            WebError::InvalidParams { kind, .. } => kind.problem_title(),
            WebError::L10n(key) => match l10n_status_code(key) {
                StatusCode::UNAUTHORIZED => "Unauthorized",
                StatusCode::FORBIDDEN => "Forbidden",
                StatusCode::NOT_FOUND => "Not found",
                StatusCode::CONFLICT => "Conflict",
                StatusCode::TOO_MANY_REQUESTS => "Too many requests",
                StatusCode::SERVICE_UNAVAILABLE => "Service unavailable",
                StatusCode::REQUEST_TIMEOUT => "Request timeout",
                StatusCode::INTERNAL_SERVER_ERROR => "Internal server error",
                _ => "Domain error",
            },
        }
    }

    pub fn error_code(&self) -> &str {
        match self {
            WebError::InvalidParams { kind, .. } => kind.error_code(),
            WebError::L10n(key) => key.as_str(),
        }
    }
}

/// 按命名约定映射状态码；域无需再维护 `account_*` 白名单。
/// 约定（先匹配更具体的后缀/前缀）：
/// - `access_token_*` / `authed_account_not_found` / `*_invalid_credentials` / `*_token_invalid` → 401
/// - `*_forbidden` / `*_protected` → 403
/// - `*_not_found` → 404
/// - `*_duplicated` / `*_version_conflict` → 409
/// - 其余业务 key → 400；基础设施固定 key 见 match 臂
fn l10n_status_code(key: &str) -> StatusCode {
    match key {
        "request_timeout" => StatusCode::REQUEST_TIMEOUT,
        "internal_server_error" => StatusCode::INTERNAL_SERVER_ERROR,
        "rate_limited" => StatusCode::TOO_MANY_REQUESTS,
        "cache_unavailable" => StatusCode::SERVICE_UNAVAILABLE,
        // 鉴权：须在通用 `*_not_found` 之前（authed_account_not_found 语义是未认证而非资源缺失）
        key if key.starts_with("access_token_") || key == "authed_account_not_found" => {
            StatusCode::UNAUTHORIZED
        }
        key if key.ends_with("_invalid_credentials") || key.ends_with("_token_invalid") => {
            StatusCode::UNAUTHORIZED
        }
        key if key.ends_with("_forbidden") || key.ends_with("_protected") => StatusCode::FORBIDDEN,
        key if key.ends_with("_not_found") => StatusCode::NOT_FOUND,
        key if key.ends_with("_duplicated") || key.ends_with("_version_conflict") => {
            StatusCode::CONFLICT
        }
        _ => StatusCode::BAD_REQUEST,
    }
}

impl IntoResponse for WebError {
    fn into_response(self) -> Response {
        let mut response = Response::default();
        response.extensions_mut().insert(self);
        response
    }
}

impl From<Report> for WebError {
    fn from(report: Report) -> Self {
        tracing::error!(error = %report, "request failed");
        report_to_web_error(report)
    }
}

fn report_to_web_error(report: Report) -> WebError {
    if let Some(err) = try_domain_mappers(&report) {
        return err;
    }
    let msg = report.format_current_context().to_string();
    if is_l10n_key(&msg) {
        WebError::L10n(msg)
    } else {
        WebError::L10n("internal_server_error".to_string())
    }
}

/// rootcause `Report` 类型擦除，无法 downcast 到具体域错误；域错误（如
/// `IdentityError`）约定以 snake_case l10n key 作为 Display（locale 测试的
/// 正则扫描强制此格式），据此区分「业务 4xx key」与「内部 5xx」。
/// 约束：非空、长度受限、首字符为小写字母，避免把内部错误误判成 key。
fn is_l10n_key(msg: &str) -> bool {
    !msg.is_empty()
        && msg.len() <= 64
        && msg
            .as_bytes()
            .first()
            .is_some_and(|b| b.is_ascii_lowercase())
        && msg
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    #[derive(Debug)]
    struct FakeL10nError;

    impl fmt::Display for FakeL10nError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "account_not_found")
        }
    }

    impl Error for FakeL10nError {}

    #[derive(Debug)]
    struct FakeInternalError;

    impl fmt::Display for FakeInternalError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "connection refused")
        }
    }

    impl Error for FakeInternalError {}

    #[test]
    fn test_report_with_l10n_key() {
        let report = Report::from(FakeL10nError);
        let web_err = report_to_web_error(report);
        assert!(matches!(web_err, WebError::L10n(key) if key == "account_not_found"));
    }

    #[test]
    fn test_report_without_l10n_prefix() {
        let report = Report::from(FakeInternalError);
        let web_err = report_to_web_error(report);
        assert!(matches!(web_err, WebError::L10n(key) if key == "internal_server_error"));
    }

    #[test]
    fn test_internal_server_error_maps_to_500_problem() {
        let err = WebError::L10n("internal_server_error".to_string());
        assert_eq!(err.status_code(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            err.problem_type(),
            "urn:modrun:problem:internal-server-error"
        );
        assert_eq!(err.problem_title(), "Internal server error");
    }

    #[test]
    fn test_is_l10n_key_rejects_empty_and_non_snake() {
        assert!(!is_l10n_key(""));
        assert!(!is_l10n_key("Internal Server Error"));
        assert!(!is_l10n_key("no-dash"));
        assert!(is_l10n_key("account_not_found"));
    }

    #[test]
    fn test_domain_l10n_error_maps_to_400_problem() {
        let err = WebError::L10n("json_body_missing_field".to_string());
        assert_eq!(err.status_code(), StatusCode::BAD_REQUEST);
        assert_eq!(err.problem_type(), "urn:modrun:problem:domain-error");
        assert_eq!(err.problem_title(), "Domain error");
    }

    #[test]
    fn test_forbidden_l10n_error_maps_to_403_problem() {
        let err = WebError::L10n("account_forbidden".to_string());
        assert_eq!(err.status_code(), StatusCode::FORBIDDEN);
        assert_eq!(err.problem_type(), "urn:modrun:problem:forbidden");
        assert_eq!(err.problem_title(), "Forbidden");
    }

    #[test]
    fn test_not_found_and_conflict_and_credentials_status() {
        assert_eq!(
            WebError::L10n("account_not_found".into()).status_code(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            WebError::L10n("account_protected".into()).status_code(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            WebError::L10n("account_duplicated".into()).status_code(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            WebError::L10n("account_invalid_credentials".into()).status_code(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            WebError::L10n("refresh_token_invalid".into()).status_code(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            WebError::L10n("rate_limited".into()).status_code(),
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            WebError::L10n("cache_unavailable".into()).status_code(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        let err = WebError::L10n(["access", "token", "invalid"].join("_"));
        assert_eq!(err.status_code(), StatusCode::UNAUTHORIZED);
        assert_eq!(err.problem_type(), "urn:modrun:problem:unauthorized");
    }
}
