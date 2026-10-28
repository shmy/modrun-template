use std::ops::Deref;

use axum::body::Bytes;
use axum::extract::{FromRequest, Request};
use serde::de::DeserializeOwned;
use serde_path_to_error::Error as PathError;
use validify::Validify;
use validify_kit::{SerdeErrorKind, classify_serde_message, errors_to_key_and_field};

use axum::http::header::CONTENT_TYPE;
use serde_json::Deserializer;
use serde_json::error::Category;
use web_error::{InvalidParamsKind, WebError};

/// JSON 请求体：先按 serde 反序列化（经 `serde_path_to_error` 保留字段路径），
/// 再执行 **validify**（`#[modify]` + `#[validate]`）。
///
/// 失败时返回 [`WebError::InvalidParams`]（kind = Body）：`key` 为 l10n key，`field` 为出错字段的
/// 完整路径（如 `items.0.quantity`），由 locale 中间件参数化渲染进 detail。
/// 反序列化走 serde_path_to_error 而非 Axum `Json`，以便把字段路径带出错误上下文。
#[derive(Debug, Clone, Copy, Default)]
pub struct ValidJson<T: Validify>(pub T);

impl<T: Validify> Deref for ValidJson<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T: Validify, S> FromRequest<S> for ValidJson<T>
where
    T: DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = WebError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        // 对齐 Axum `Json` 的 Content-Type 语义：缺失/非 JSON → 400 兜底（历史行为，保持）。
        let content_type = req
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok());
        if !content_type.is_some_and(|ct| ct.starts_with("application/json")) {
            return Err(WebError::InvalidParams {
                kind: InvalidParamsKind::Body,
                key: "invalid_request_body".to_string(),
                field: None,
            });
        }

        let bytes = Bytes::from_request(req, state)
            .await
            .map_err(|_| WebError::InvalidParams {
                kind: InvalidParamsKind::Body,
                key: "invalid_request_body".to_string(),
                field: None,
            })?;

        let mut de = Deserializer::from_slice(&bytes);
        let mut inner = match serde_path_to_error::deserialize::<_, T>(&mut de) {
            Ok(value) => value,
            Err(e) => return Err(serde_error_to_web(&e)),
        };
        if let Err(e) = de.end() {
            tracing::debug!(error = %e, "json request body rejected");
            let (key, field) = l10n_key_and_field("", &e);
            return Err(WebError::InvalidParams {
                kind: InvalidParamsKind::Body,
                key: key.to_string(),
                field,
            });
        }
        inner.validify().map_err(|e| {
            let (key, field) = errors_to_key_and_field(e);
            WebError::InvalidParams {
                kind: InvalidParamsKind::Body,
                key,
                field,
            }
        })?;
        Ok(ValidJson(inner))
    }
}

/// serde 反序列化错误 → l10n key + 展示字段路径。
///
/// - missing / unknown / duplicate field：字段名在 serde_json 错误 Display 的反引号里，
///   `path` 为字段所在容器（serde_path_to_error 语义），拼接成完整路径（`items.0.quantity`）。
/// - invalid type 等：字段路径由 serde_path_to_error 直接提供（`path`）。
/// - 语法 / trailing：无字段，`field` 为 `None`。
fn l10n_key_and_field(path: &str, inner: &serde_json::Error) -> (&'static str, Option<String>) {
    let msg = inner.to_string();
    match inner.classify() {
        Category::Syntax | Category::Eof => {
            // TrailingCharacters 在 classify 里归为 Syntax，需文本细分。
            if msg.contains("trailing characters") {
                ("json_body_trailing", None)
            } else {
                ("json_body_syntax", None)
            }
        }
        Category::Io => ("invalid_request_body", None),
        Category::Data => {
            let (kind, field) = classify_serde_message(path, &msg);
            (serde_body_key(kind), field)
        }
    }
}

fn serde_body_key(kind: SerdeErrorKind) -> &'static str {
    match kind {
        SerdeErrorKind::MissingField => "json_body_missing_field",
        SerdeErrorKind::InvalidType => "json_body_invalid_type",
        SerdeErrorKind::UnknownField => "json_body_unknown_field",
        SerdeErrorKind::DuplicateField => "json_body_duplicate_field",
        SerdeErrorKind::Other => "invalid_request_body",
    }
}

fn serde_error_to_web(e: &PathError<serde_json::Error>) -> WebError {
    let path = e.path().to_string();
    let inner = e.inner();
    tracing::debug!(path = %path, error = %inner, "json request body rejected");
    let (key, field) = l10n_key_and_field(&path, inner);
    WebError::InvalidParams {
        kind: InvalidParamsKind::Body,
        key: key.to_string(),
        field,
    }
}

#[cfg(test)]
mod tests {
    use super::l10n_key_and_field;
    use serde::Deserialize;
    use serde::de::DeserializeOwned;
    use serde_json::Deserializer;
    use std::fmt;

    #[derive(Debug, Deserialize)]
    struct Dto {
        #[allow(unused)]
        phone: String,
    }

    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct StrictDto {
        #[allow(unused)]
        phone: String,
    }

    #[derive(Debug, Deserialize)]
    struct Outer {
        #[allow(unused)]
        address: Address,
    }

    #[derive(Debug, Deserialize)]
    struct Address {
        #[allow(unused)]
        city: String,
    }

    fn json_err<T: DeserializeOwned + fmt::Debug>(json: &str) -> serde_json::Error {
        serde_json::from_str::<T>(json).unwrap_err()
    }

    #[test]
    fn missing_field_extracts_field_name() {
        let err = json_err::<Dto>("{}");
        let (key, field) = l10n_key_and_field("", &err);
        assert_eq!(key, "json_body_missing_field");
        assert_eq!(field.as_deref(), Some("phone"));
    }

    #[test]
    fn dot_empty_path_produces_plain_field_name() {
        // serde_path_to_error 对空路径的 Display 是 "."，必须归一为 ""，
        // 否则 field_path 会拼出 "..phone"。
        let err = json_err::<Dto>("{}");
        let (key, field) = l10n_key_and_field(".", &err);
        assert_eq!(key, "json_body_missing_field");
        assert_eq!(field.as_deref(), Some("phone"));
    }

    #[test]
    fn dot_empty_path_yields_no_field_for_invalid_type() {
        let err = json_err::<Dto>("123");
        let (key, field) = l10n_key_and_field(".", &err);
        assert_eq!(key, "json_body_invalid_type");
        assert_eq!(field, None);
    }

    #[test]
    fn missing_field_joins_parent_path() {
        let err = json_err::<Address>("{}");
        let (key, field) = l10n_key_and_field("address", &err);
        assert_eq!(key, "json_body_missing_field");
        assert_eq!(field.as_deref(), Some("address.city"));
    }

    #[test]
    fn invalid_type_uses_path() {
        let err = json_err::<Dto>(r#"{"phone": 123}"#);
        let (key, field) = l10n_key_and_field("phone", &err);
        assert_eq!(key, "json_body_invalid_type");
        assert_eq!(field.as_deref(), Some("phone"));
    }

    #[test]
    fn unknown_field_extracts_field_name() {
        let err = json_err::<StrictDto>(r#"{"phone": "1", "xyz": 1}"#);
        let (key, field) = l10n_key_and_field("", &err);
        assert_eq!(key, "json_body_unknown_field");
        assert_eq!(field.as_deref(), Some("xyz"));
    }

    #[test]
    fn duplicate_field_extracts_field_name() {
        let err = json_err::<Dto>(r#"{"phone": "a", "phone": "b"}"#);
        let (key, field) = l10n_key_and_field("", &err);
        assert_eq!(key, "json_body_duplicate_field");
        assert_eq!(field.as_deref(), Some("phone"));
    }

    #[test]
    fn syntax_error_has_no_field() {
        let err = json_err::<Dto>("{");
        let (key, field) = l10n_key_and_field("", &err);
        assert_eq!(key, "json_body_syntax");
        assert_eq!(field, None);
    }

    #[test]
    fn trailing_characters_has_no_field() {
        let err = json_err::<Dto>(r#"{"phone": "1"} extra"#);
        let (key, field) = l10n_key_and_field("", &err);
        assert_eq!(key, "json_body_trailing");
        assert_eq!(field, None);
    }

    #[test]
    fn serde_path_to_error_provides_parent_path_for_missing_field() {
        let mut de = Deserializer::from_str(r#"{"address": {}}"#);
        let err = serde_path_to_error::deserialize::<_, Outer>(&mut de).unwrap_err();
        assert_eq!(err.path().to_string(), "address");
        let (key, field) = l10n_key_and_field(&err.path().to_string(), err.inner());
        assert_eq!(key, "json_body_missing_field");
        assert_eq!(field.as_deref(), Some("address.city"));
    }

    #[test]
    fn serde_path_to_error_provides_invalid_type_path() {
        let mut de = Deserializer::from_str(r#"{"phone": 123}"#);
        let err = serde_path_to_error::deserialize::<_, Dto>(&mut de).unwrap_err();
        assert_eq!(err.path().to_string(), "phone");
        let (key, field) = l10n_key_and_field(&err.path().to_string(), err.inner());
        assert_eq!(key, "json_body_invalid_type");
        assert_eq!(field.as_deref(), Some("phone"));
    }
}
