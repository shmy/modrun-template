use validify::{ValidationError, ValidationErrors};

/// 从 validify 校验错误中提取首个 l10n key 与请求字段路径。
/// 优先取 `message`（若非空），其次退回 `code`。
pub fn errors_to_key_and_field(errors: ValidationErrors) -> (String, Option<String>) {
    for err in errors.errors() {
        if let Some(key) = l10n_key_from_validation_error(err) {
            return (key, field_from_validation_error(err));
        }
    }
    ("invalid_request".to_string(), None)
}

fn l10n_key_from_validation_error(err: &ValidationError) -> Option<String> {
    match err {
        ValidationError::Field { message, code, .. } => {
            if let Some(m) = message
                && !m.is_empty()
            {
                return Some(m.clone());
            }
            Some((*code).to_string())
        }
        ValidationError::Schema { message, code, .. } => {
            if let Some(m) = message
                && !m.is_empty()
            {
                return Some(m.clone());
            }
            Some((*code).to_string())
        }
    }
}

fn field_from_validation_error(err: &ValidationError) -> Option<String> {
    let location = match err {
        ValidationError::Field { location, .. } | ValidationError::Schema { location, .. } => {
            location
        }
    };
    if location.is_empty() || location == "/" {
        return None;
    }
    Some(
        location
            .trim_start_matches('/')
            .replace('/', ".")
            .to_string(),
    )
}
