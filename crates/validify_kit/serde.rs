//! serde 风格反序列化错误 → l10n key 分类的共享工具（`ValidJson` / `ValidQuery` 共用）。

/// 反序列化错误分类（枚举而非字符串判别器，拼错编译期报错）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SerdeErrorKind {
    MissingField,
    InvalidType,
    UnknownField,
    DuplicateField,
    Other,
}

/// 将 serde 风格错误消息分类为（分类, 展示字段路径）。
/// 调用方把分类映射到各自的 key 前缀（`json_body_*` / `query_*`）。
pub fn classify_serde_message(path: &str, msg: &str) -> (SerdeErrorKind, Option<String>) {
    if msg.contains("missing field") {
        (SerdeErrorKind::MissingField, field_path(path, msg))
    } else if is_invalid_type_msg(msg) {
        (SerdeErrorKind::InvalidType, non_empty(path))
    } else if msg.contains("unknown field") {
        (SerdeErrorKind::UnknownField, field_path(path, msg))
    } else if msg.contains("duplicate field") {
        (SerdeErrorKind::DuplicateField, field_path(path, msg))
    } else {
        (SerdeErrorKind::Other, non_empty(path))
    }
}

/// serde 风格 "invalid type" 消息，含 std 解析错误（`ParseIntError` / `ParseFloatError` /
/// `ParseBoolError` 的 Display，serde_urlencoded 的字符串→数字失败走这些，无 "invalid type" 前缀）。
fn is_invalid_type_msg(msg: &str) -> bool {
    msg.contains("invalid type")
        || msg.contains("invalid digit found in string")
        || msg.contains("invalid float literal")
        || msg.contains("cannot parse integer from empty string")
        || msg.contains("provided string was not `true` or `false`")
        || msg.contains("number too large to fit in target type")
}

/// `missing field \`phone\`` 等：取反引号内的字段名，与父级 `path` 拼接成完整路径。
/// 注意 serde_path_to_error 对空路径的 Display 是 `"."`，需先归一为 `""`。
fn field_path(path: &str, msg: &str) -> Option<String> {
    let path = normalize_path(path);
    let name = msg.split('`').nth(1).map(str::to_string)?;
    Some(match path {
        "" => name,
        p => format!("{p}.{name}"),
    })
}

fn non_empty(path: &str) -> Option<String> {
    let path = normalize_path(path);
    if path.is_empty() {
        None
    } else {
        Some(path.to_string())
    }
}

/// serde_path_to_error 空路径的 Display 为 `"."`，统一为 `""`。
fn normalize_path(path: &str) -> &str {
    if path == "." { "" } else { path }
}

#[cfg(test)]
mod tests {
    use super::{SerdeErrorKind, classify_serde_message};

    #[test]
    fn missing_field_with_dot_path_produces_plain_name() {
        let (kind, field) = classify_serde_message(".", "missing field `phone` at line 3 column 1");
        assert_eq!(kind, SerdeErrorKind::MissingField);
        assert_eq!(field.as_deref(), Some("phone"));
    }

    #[test]
    fn invalid_type_uses_path() {
        let (kind, field) =
            classify_serde_message("page", "invalid type: string \"abc\", expected u32");
        assert_eq!(kind, SerdeErrorKind::InvalidType);
        assert_eq!(field.as_deref(), Some("page"));
    }

    #[test]
    fn nested_missing_joins_path() {
        let (kind, field) = classify_serde_message("items.0", "missing field `quantity`");
        assert_eq!(kind, SerdeErrorKind::MissingField);
        assert_eq!(field.as_deref(), Some("items.0.quantity"));
    }

    #[test]
    fn unknown_field_extracts_name() {
        let (kind, field) = classify_serde_message(".", "unknown field `xyz`, expected one of `a`");
        assert_eq!(kind, SerdeErrorKind::UnknownField);
        assert_eq!(field.as_deref(), Some("xyz"));
    }

    #[test]
    fn std_parse_error_counts_as_invalid_type() {
        let (kind, field) = classify_serde_message("page", "invalid digit found in string");
        assert_eq!(kind, SerdeErrorKind::InvalidType);
        assert_eq!(field.as_deref(), Some("page"));
        let (kind, field) =
            classify_serde_message("rate", "provided string was not `true` or `false`");
        assert_eq!(kind, SerdeErrorKind::InvalidType);
        assert_eq!(field.as_deref(), Some("rate"));
    }

    #[test]
    fn unclassified_keeps_path() {
        let (kind, field) = classify_serde_message("id", "something else entirely");
        assert_eq!(kind, SerdeErrorKind::Other);
        assert_eq!(field.as_deref(), Some("id"));
    }
}
