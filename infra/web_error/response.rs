use axum::{
    Json,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::{Serialize, Serializer};
use utoipa::ToSchema;

use crate::error::WebError;

/// Handler 成功返回 JSON body 时的标准结果类型。
///
/// `axum::Json` 负责序列化与 `Content-Type`；`WebError` 走统一 l10n 错误路径。
pub type JsonResponseType<T> = Result<Json<T>, WebError>;

/// RFC 9457 Problem Details (`application/problem+json`)
#[derive(Debug, Serialize, ToSchema)]
pub struct ProblemDetails {
    #[serde(rename = "type")]
    pub type_url: String,
    pub title: String,
    #[serde(serialize_with = "serialize_status_code")]
    #[schema(value_type = u16, example = 400)]
    pub status: StatusCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
}

fn serialize_status_code<S>(status: &StatusCode, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_u16(status.as_u16())
}

impl ProblemDetails {
    pub fn new(
        type_url: impl Into<String>,
        title: impl Into<String>,
        status: StatusCode,
        detail: Option<String>,
        instance: Option<String>,
        error_code: Option<String>,
        trace_id: Option<String>,
    ) -> Self {
        Self {
            type_url: type_url.into(),
            title: title.into(),
            status,
            detail,
            instance,
            error_code,
            trace_id,
        }
    }
}

impl IntoResponse for ProblemDetails {
    fn into_response(self) -> Response {
        let mut response = (self.status, Json(self)).into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/problem+json"),
        );
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    #[tokio::test]
    async fn test_problem_details_content_type_is_problem_json() {
        let response = ProblemDetails::new(
            "urn:modrun:problem:test",
            "Test",
            StatusCode::BAD_REQUEST,
            Some("detail".to_string()),
            Some("/test".to_string()),
            Some("test_error".to_string()),
            Some("trace-1".to_string()),
        )
        .into_response();

        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert_eq!(content_type, "application/problem+json");
    }

    #[tokio::test]
    async fn test_problem_details_status_matches_http_status() {
        let response = ProblemDetails::new(
            "urn:modrun:problem:test",
            "Test",
            StatusCode::UNPROCESSABLE_ENTITY,
            Some("detail".to_string()),
            Some("/test".to_string()),
            Some("test_error".to_string()),
            Some("trace-2".to_string()),
        )
        .into_response();

        let http_status = response.status().as_u16();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"].as_u64().unwrap() as u16, http_status);
    }

    #[tokio::test]
    async fn test_problem_details_body_contains_instance_path() {
        let response = ProblemDetails::new(
            "urn:modrun:problem:test",
            "Test",
            StatusCode::BAD_REQUEST,
            Some("detail".to_string()),
            Some("/api/v1/identity/refresh".to_string()),
            Some("test_error".to_string()),
            Some("trace-3".to_string()),
        )
        .into_response();

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["instance"], "/api/v1/identity/refresh");
    }
}
