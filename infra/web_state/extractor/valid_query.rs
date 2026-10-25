use std::ops::Deref;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use serde::de::DeserializeOwned;
use validify::Validify;
use validify_kit::{SerdeErrorKind, classify_serde_message, errors_to_key_and_field};

use serde_urlencoded::Deserializer;
use serde_urlencoded::de::Error;
use web_error::{InvalidParamsKind, WebError};

/// Query 提取器：反序列化后执行 **validify** 校验。
/// 反序列化失败 → [`WebError::InvalidParams`]（kind = Query，l10n key + 字段路径）；校验失败 → 同上（l10n key）。
/// 反序列化走 serde_urlencoded + serde_path_to_error（与 Axum `Query` 内部同构，但保留字段路径）。
#[derive(Debug, Clone)]
pub struct ValidQuery<T>(pub T);

impl<T> Deref for ValidQuery<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T, S> FromRequestParts<S> for ValidQuery<T>
where
    T: DeserializeOwned + Validify + Send,
    S: Send + Sync,
{
    type Rejection = WebError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let query = parts.uri.query().unwrap_or_default();
        let deserializer = Deserializer::new(form_urlencoded::parse(query.as_bytes()));
        let mut inner = serde_path_to_error::deserialize::<_, T>(deserializer)
            .map_err(|e| query_error_to_web(&e))?;
        inner.validify().map_err(|e| {
            let (key, field) = errors_to_key_and_field(e);
            WebError::InvalidParams {
                kind: InvalidParamsKind::Query,
                key,
                field,
            }
        })?;
        Ok(ValidQuery(inner))
    }
}

fn query_error_to_web(e: &serde_path_to_error::Error<Error>) -> WebError {
    let path = e.path().to_string();
    let msg = e.inner().to_string();
    tracing::debug!(path = %path, error = %msg, "query string rejected");
    let (kind, field) = classify_serde_message(&path, &msg);
    let key = match kind {
        SerdeErrorKind::MissingField => "query_missing_field",
        SerdeErrorKind::InvalidType => "query_invalid_type",
        SerdeErrorKind::UnknownField => "query_unknown_field",
        SerdeErrorKind::DuplicateField => "query_duplicate_field",
        SerdeErrorKind::Other => "query_invalid",
    };
    WebError::InvalidParams {
        kind: InvalidParamsKind::Query,
        key: key.to_string(),
        field,
    }
}

#[cfg(test)]
mod tests {
    use super::query_error_to_web;
    use serde::Deserialize;
    use serde::de::DeserializeOwned;
    use serde_urlencoded::Deserializer;
    use serde_urlencoded::de::Error;
    use std::fmt;
    use web_error::{InvalidParamsKind, WebError};

    #[derive(Debug, Deserialize)]
    struct QueryDto {
        #[allow(unused)]
        phone: String,
        #[allow(unused)]
        page: u32,
    }

    fn parse_query<T: DeserializeOwned + fmt::Debug>(
        qs: &str,
    ) -> serde_path_to_error::Error<Error> {
        let de = Deserializer::new(form_urlencoded::parse(qs.as_bytes()));
        serde_path_to_error::deserialize::<_, T>(de).unwrap_err()
    }

    fn expect_key_field(web: WebError) -> (String, Option<String>) {
        match web {
            WebError::InvalidParams {
                kind: InvalidParamsKind::Query,
                key,
                field,
            } => (key, field),
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn missing_field_maps_to_key_with_field() {
        let e = parse_query::<QueryDto>("page=1");
        let (key, field) = expect_key_field(query_error_to_web(&e));
        assert_eq!(key, "query_missing_field");
        assert_eq!(field.as_deref(), Some("phone"));
    }

    #[test]
    fn invalid_type_maps_to_key_with_field() {
        let e = parse_query::<QueryDto>("phone=1&page=abc");
        let (key, field) = expect_key_field(query_error_to_web(&e));
        assert_eq!(key, "query_invalid_type");
        assert_eq!(field.as_deref(), Some("page"));
    }

    #[test]
    fn empty_query_missing_field() {
        let e = parse_query::<QueryDto>("");
        let (key, field) = expect_key_field(query_error_to_web(&e));
        assert_eq!(key, "query_missing_field");
        assert_eq!(field.as_deref(), Some("phone"));
    }
}
