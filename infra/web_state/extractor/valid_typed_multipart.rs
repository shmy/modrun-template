use std::ops::Deref;

use axum::extract::{FromRequest, Request};
use axum_typed_multipart::{TryFromMultipartWithState, TypedMultipart, TypedMultipartError};

use web_error::{InvalidParamsKind, WebError};

/// `multipart/form-data`：与 [`axum_typed_multipart::TypedMultipart`] 相同解析流程，
/// 失败时按结构化 [`TypedMultipartError`] 映射为 [`WebError::InvalidParams`]（kind = Body）
/// （l10n key + 字段名），走统一 l10n 路径。
#[derive(Debug)]
pub struct ValidTypedMultipart<T>(pub T);

impl<T> Deref for ValidTypedMultipart<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T, S> FromRequest<S> for ValidTypedMultipart<T>
where
    T: TryFromMultipartWithState<S> + Send,
    S: Send + Sync,
{
    type Rejection = WebError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let TypedMultipart(inner) = TypedMultipart::<T>::from_request(req, state)
            .await
            .map_err(multipart_error_to_web)?;
        Ok(Self(inner))
    }
}

/// 结构化 [`TypedMultipartError`] → l10n key + 字段名。
fn multipart_error_to_web(e: TypedMultipartError) -> WebError {
    let (key, field) = match e {
        TypedMultipartError::MissingField { field_name } => {
            ("multipart_missing_field", Some(field_name))
        }
        TypedMultipartError::WrongFieldType { field_name, .. } => {
            ("multipart_wrong_field_type", Some(field_name))
        }
        TypedMultipartError::DuplicateField { field_name } => {
            ("multipart_duplicate_field", Some(field_name))
        }
        TypedMultipartError::UnknownField { field_name } => {
            ("multipart_unknown_field", Some(field_name))
        }
        TypedMultipartError::InvalidEnumValue { field_name, .. } => {
            ("multipart_invalid_enum_value", Some(field_name))
        }
        TypedMultipartError::FieldTooLarge { field_name, .. } => {
            ("multipart_field_too_large", Some(field_name))
        }
        // multipart 语法层错误 / 内部错误：无字段语义，兜底 key。
        TypedMultipartError::InvalidRequest { source } => {
            tracing::debug!(error = %source, "multipart body rejected");
            ("invalid_request_body", None)
        }
        TypedMultipartError::InvalidRequestBody { source } => {
            tracing::debug!(error = %source, "multipart body rejected");
            ("invalid_request_body", None)
        }
        TypedMultipartError::NamelessField | TypedMultipartError::Other { .. } => {
            ("invalid_request_body", None)
        }
        // non_exhaustive 枚举兜底。
        _ => ("invalid_request_body", None),
    };
    WebError::InvalidParams {
        kind: InvalidParamsKind::Body,
        key: key.to_string(),
        field,
    }
}

#[cfg(test)]
mod tests {
    use super::multipart_error_to_web;
    use axum_typed_multipart::TypedMultipartError;
    use web_error::{InvalidParamsKind, WebError};

    fn expect_key_field(web: WebError) -> (String, Option<String>) {
        match web {
            WebError::InvalidParams {
                kind: InvalidParamsKind::Body,
                key,
                field,
            } => (key, field),
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn missing_field_maps_to_key_with_field() {
        let (key, field) =
            expect_key_field(multipart_error_to_web(TypedMultipartError::MissingField {
                field_name: "avatar".to_string(),
            }));
        assert_eq!(key, "multipart_missing_field");
        assert_eq!(field.as_deref(), Some("avatar"));
    }

    #[test]
    fn duplicate_field_maps_to_key_with_field() {
        let (key, field) = expect_key_field(multipart_error_to_web(
            TypedMultipartError::DuplicateField {
                field_name: "tags".to_string(),
            },
        ));
        assert_eq!(key, "multipart_duplicate_field");
        assert_eq!(field.as_deref(), Some("tags"));
    }

    #[test]
    fn unknown_field_maps_to_key_with_field() {
        let (key, field) =
            expect_key_field(multipart_error_to_web(TypedMultipartError::UnknownField {
                field_name: "extra".to_string(),
            }));
        assert_eq!(key, "multipart_unknown_field");
        assert_eq!(field.as_deref(), Some("extra"));
    }

    #[test]
    fn invalid_enum_value_maps_to_key_with_field() {
        let (key, field) = expect_key_field(multipart_error_to_web(
            TypedMultipartError::InvalidEnumValue {
                field_name: "status".to_string(),
                value: "x".to_string(),
            },
        ));
        assert_eq!(key, "multipart_invalid_enum_value");
        assert_eq!(field.as_deref(), Some("status"));
    }

    #[test]
    fn field_too_large_maps_to_key_with_field() {
        let (key, field) =
            expect_key_field(multipart_error_to_web(TypedMultipartError::FieldTooLarge {
                field_name: "image".to_string(),
                limit_bytes: 1024,
            }));
        assert_eq!(key, "multipart_field_too_large");
        assert_eq!(field.as_deref(), Some("image"));
    }

    #[test]
    fn nameless_field_maps_to_fallback() {
        let (key, field) =
            expect_key_field(multipart_error_to_web(TypedMultipartError::NamelessField));
        assert_eq!(key, "invalid_request_body");
        assert_eq!(field, None);
    }
}
