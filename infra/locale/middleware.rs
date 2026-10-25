use axum::{
    extract::Request,
    http::{StatusCode, header},
    middleware::Next,
    response::{IntoResponse as _, Response},
};
use axum_extra::extract::CookieJar;
use trace_kit::extract_trace_id;
use web_error::{ProblemDetails, WebError};

use crate::{DEFAULT_LOCALE, translate, translate_with_args};

const LANGUAGE_COOKIE_NAME: &str = "accept_language";

pub async fn l10n_middleware(request: Request, next: Next) -> Response {
    let instance = request.uri().path().to_string();
    let trace_id = extract_trace_id(request.headers());
    let locale = parse_locale(&request);
    let response = next.run(request).await;
    if let Some(err) = response.extensions().get::<WebError>() {
        let info = match err {
            WebError::InvalidParams { key, field, .. } => match field {
                Some(f) => translate_with_args(&locale, key, &[("field", f.clone())]),
                None => translate(&locale, key),
            },
            _ => translate(&locale, &err.to_string()),
        };
        let problem = ProblemDetails::new(
            err.problem_type(),
            err.problem_title(),
            err.status_code(),
            Some(info),
            Some(instance),
            Some(err.error_code().to_string()),
            trace_id,
        );
        return problem.into_response();
    }
    if response.status() == StatusCode::REQUEST_TIMEOUT {
        let err = WebError::L10n("request_timeout".to_string());
        let problem = ProblemDetails::new(
            err.problem_type(),
            err.problem_title(),
            err.status_code(),
            Some(translate(&locale, err.error_code())),
            Some(instance),
            Some(err.error_code().to_string()),
            trace_id,
        );
        return problem.into_response();
    }
    response
}

fn parse_locale(request: &Request) -> String {
    let cookie_lang = CookieJar::from_headers(request.headers())
        .get(LANGUAGE_COOKIE_NAME)
        .map(|cookie| cookie.value().to_owned());

    let header_lang = request
        .headers()
        .get(header::ACCEPT_LANGUAGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| accept_language::parse(s).first().cloned());

    cookie_lang
        .or(header_lang)
        .map(|lang| {
            let primary = lang.as_str().split('-').next().unwrap_or("");
            match primary {
                "zh" => "zh-CN",
                "en" => "en-US",
                _ => DEFAULT_LOCALE,
            }
            .to_string()
        })
        .unwrap_or_else(|| DEFAULT_LOCALE.to_string())
}

#[cfg(test)]
mod tests {
    use axum::Router;
    use axum::body::Body;
    use axum::http::header;
    use axum::http::{Request, StatusCode};
    use axum::middleware;
    use axum::routing::get;
    use tower::ServiceExt;
    use web_error::{InvalidParamsKind, WebError};

    use super::l10n_middleware;
    use axum::body::to_bytes;

    async fn handler() -> Result<&'static str, WebError> {
        Err(WebError::InvalidParams {
            kind: InvalidParamsKind::Body,
            key: "json_body_missing_field".to_string(),
            field: Some("phone".to_string()),
        })
    }

    fn router() -> Router {
        Router::new()
            .route("/", get(handler))
            .layer(middleware::from_fn(l10n_middleware))
    }

    #[tokio::test]
    async fn zh_cn_detail_interpolates_field() {
        let resp = router()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(header::ACCEPT_LANGUAGE, "zh-CN")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["detail"], "缺少必填字段：phone");
        assert_eq!(json["error_code"], "invalid_request_body");
    }

    #[tokio::test]
    async fn en_detail_interpolates_field() {
        let resp = router()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(header::ACCEPT_LANGUAGE, "en")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["detail"], "Missing required field: phone");
    }
}
