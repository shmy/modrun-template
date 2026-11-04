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
    /// 本地化 KEY（已确认属于「可展示给客户端的业务错误」）。
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

/// 域错误 → l10n key 的映射器。返回 `None` 表示「不是我的错误」。
///
/// 为什么是全局注册表而非 DI：`From<Report> for WebError` 是 `?` 的落点，
/// 那里没有 `State` 可用（extractor / 中间件 / 域函数都要能 `?`）。因此注册表必须
/// 进程级可见，只能在「注册时机」上收紧：feature 在 `module()` 里注册，早于首个请求。
type DomainReportMapper = fn(&Report) -> Option<WebError>;

static DOMAIN_REPORT_MAPPERS: Mutex<Vec<DomainReportMapper>> = Mutex::new(Vec::new());

fn mappers_lock() -> MutexGuard<'static, Vec<DomainReportMapper>> {
    DOMAIN_REPORT_MAPPERS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 注册域错误映射（feature 在 `module()` 时调用）。必须在首个请求前完成。
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
                StatusCode::PAYLOAD_TOO_LARGE => "urn:modrun:problem:payload-too-large",
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
                StatusCode::PAYLOAD_TOO_LARGE => "Payload too large",
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
/// - `access_token_*` / `authed_account_not_found` / `*_invalid_credentials` / `*_token_invalid`
///   → 401
/// - `*_forbidden` / `*_protected` → 403
/// - `*_not_found` → 404
/// - `*_duplicated` / `*_version_conflict` → 409
/// - 其余业务 key → 400；基础设施固定 key 见 match 臂
///
/// 注意：不要用全局 `*_incorrect`→401。登录侧信道靠登录路径统一映射
/// `*_invalid_credentials` 收口；已认证场景的 `account_password_incorrect`（改密）
/// 必须保持 400，否则客户端会误判 access token 失效并强制登出。
fn l10n_status_code(key: &str) -> StatusCode {
    match key {
        "request_timeout" => StatusCode::REQUEST_TIMEOUT,
        "internal_server_error" => StatusCode::INTERNAL_SERVER_ERROR,
        "rate_limited" => StatusCode::TOO_MANY_REQUESTS,
        "payload_too_large" => StatusCode::PAYLOAD_TOO_LARGE,
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
        // 先取报告全文再转换：`Report` 不可 Clone，且 5xx 需保留完整上下文链
        // （含源码位置），转成 WebError 后只剩 l10n key。
        let rendered = format!("{report}");
        let err = report_to_web_error(report);
        // 业务 4xx（密码错、token 过期、404 等）属正常拒绝，记 debug 即可，
        // 否则会把错误率指标打满。
        match err.status_code() {
            status if status.is_server_error() => {
                tracing::error!(error = %rendered, "request failed")
            }
            _ => tracing::debug!(error = %rendered, "request rejected"),
        }
        err
    }
}

/// 只有**显式注册**过的域错误才会变成 4xx l10n key；其余一律 500。
///
/// 这里曾用「Display 是否为 snake_case」嗅探，把未注册的小写错误当成业务 key：
/// 任何第三方错误的 Display 恰好形如 `connection_refused` 就会被静默降级成 400，
/// 既掩盖故障又不计入 5xx 指标。现在改为 fail-closed：
/// 想让某个错误对客户端可见，就在 feature 的 `module()` 里注册 mapper。
fn report_to_web_error(report: Report) -> WebError {
    if let Some(err) = try_domain_mappers(&report) {
        return err;
    }
    WebError::L10n("internal_server_error".to_string())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use std::error::Error;
    use std::sync::Once;

    #[derive(Debug)]
    struct FakeDomainError;

    impl fmt::Display for FakeDomainError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "account_not_found")
        }
    }

    impl Error for FakeDomainError {}

    #[derive(Debug)]
    struct FakeInternalError;

    impl fmt::Display for FakeInternalError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            // 刻意长得像 l10n key：旧实现的启发式会把它误判成 400
            write!(f, "connection_refused")
        }
    }

    impl Error for FakeInternalError {}

    fn map_fake_domain(report: &Report) -> Option<WebError> {
        report
            .downcast_current_context::<FakeDomainError>()
            .map(|_| WebError::L10n("account_not_found".to_string()))
    }

    fn register_once() {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| register_domain_mapper(map_fake_domain));
    }

    #[test]
    fn unregistered_lowercase_error_is_500_not_400() {
        // 回归保护：未注册的域错误即便 Display 是 snake_case 也必须是 500，
        // 不能被「像 l10n key」的启发式降级成 400 并掩盖故障。
        let web_err = report_to_web_error(Report::from(FakeInternalError));
        assert!(matches!(web_err, WebError::L10n(ref key) if key == "internal_server_error"));
        assert_eq!(web_err.status_code(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn registered_domain_error_maps_to_its_key() {
        register_once();
        let web_err = report_to_web_error(Report::from(FakeDomainError));
        assert!(matches!(web_err, WebError::L10n(ref key) if key == "account_not_found"));
        assert_eq!(web_err.status_code(), StatusCode::NOT_FOUND);
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
        // 已认证改密场景：旧密码错误是业务 400，不能进 401（否则客户端误登出）。
        assert_eq!(
            WebError::L10n("account_password_incorrect".into()).status_code(),
            StatusCode::BAD_REQUEST
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
