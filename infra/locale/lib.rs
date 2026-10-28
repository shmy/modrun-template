#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )
)]

use fluent_bundle::FluentArgs;
pub mod middleware;

use std::{collections::HashMap, sync::LazyLock};

use fluent_bundle::{FluentResource, concurrent::FluentBundle as ConcurrentFluentBundle};
use unic_langid::LanguageIdentifier;

pub const DEFAULT_LOCALE: &str = "en-US";

type Bundle = ConcurrentFluentBundle<FluentResource>;

static BUNDLES: LazyLock<HashMap<String, Bundle>> = LazyLock::new(|| {
    let mut map = HashMap::new();
    register(
        &mut map,
        "en-US",
        &[
            include_str!("locales/en-US/shared.ftl"),
            include_str!("locales/en-US/identity.ftl"),
            include_str!("locales/en-US/file.ftl"),
        ],
    );
    register(
        &mut map,
        "zh-CN",
        &[
            include_str!("locales/zh-CN/shared.ftl"),
            include_str!("locales/zh-CN/identity.ftl"),
            include_str!("locales/zh-CN/file.ftl"),
        ],
    );
    map
});

#[allow(
    clippy::expect_used,
    reason = "locale registration is infallible with pre-validated inputs"
)]
fn register(map: &mut HashMap<String, Bundle>, locale: &str, ftls: &[&str]) {
    let langid: LanguageIdentifier = locale.parse().expect("invalid locale identifier");
    let mut bundle = ConcurrentFluentBundle::new_concurrent(vec![langid]);
    for ftl in ftls {
        let resource = FluentResource::try_new(ftl.to_string()).expect("failed to parse FTL");
        bundle
            .add_resource(resource)
            .expect("failed to add FTL resource");
    }
    map.insert(locale.to_string(), bundle);
}

pub fn translate(locale: &str, key: &str) -> String {
    if let Some(text) = try_translate(locale, key) {
        return text;
    }
    if locale != DEFAULT_LOCALE
        && let Some(text) = try_translate(DEFAULT_LOCALE, key)
    {
        return text;
    }
    key.to_string()
}

/// 带参数翻译（Fluent 插值 `{ $name }`）。
/// 参数以 `(&str, String)` 传入，不把 fluent 类型泄漏到公共 API。
/// Fluent 会对变量插值包裹 bidi 隔离字符（FSI/PDI），这里剥离，保证 detail 干净。
pub fn translate_with_args(locale: &str, key: &str, args: &[(&str, String)]) -> String {
    let mut fluent_args = FluentArgs::new();
    for (name, value) in args {
        fluent_args.set(*name, value.clone());
    }
    if let Some(text) = try_translate_with_args(locale, key, &fluent_args) {
        return strip_isolates(text);
    }
    if locale != DEFAULT_LOCALE
        && let Some(text) = try_translate_with_args(DEFAULT_LOCALE, key, &fluent_args)
    {
        return strip_isolates(text);
    }
    key.to_string()
}

fn try_translate(locale: &str, key: &str) -> Option<String> {
    try_translate_with_args(locale, key, &FluentArgs::new())
}

fn try_translate_with_args(
    locale: &str,
    key: &str,
    args: &fluent_bundle::FluentArgs,
) -> Option<String> {
    let bundle = BUNDLES.get(locale)?;
    let msg = bundle.get_message(key)?;
    let pattern = msg.value()?;
    let mut errors = vec![];
    let value = bundle.format_pattern(pattern, Some(args), &mut errors);
    Some(value.into_owned())
}

/// 剥离 Fluent 变量插值引入的 bidi 隔离字符（FSI U+2068 / PDI U+2069）。
fn strip_isolates(s: String) -> String {
    s.replace(['\u{2068}', '\u{2069}'], "")
}

#[cfg(test)]
mod tests {
    use super::*;
    use regex::Regex;
    use std::collections::HashSet;
    use std::fs;
    use std::path::Path;

    #[test]
    fn test_translate_known_key_zh() {
        assert_eq!(
            translate("zh-CN", "internal_server_error"),
            "服务器内部错误"
        );
    }

    #[test]
    fn test_translate_with_args_interpolates_field() {
        let args = [("field", "phone".to_string())];
        assert_eq!(
            translate_with_args("zh-CN", "json_body_missing_field", &args),
            "缺少必填字段：phone"
        );
        assert_eq!(
            translate_with_args("en-US", "json_body_missing_field", &args),
            "Missing required field: phone"
        );
        assert_eq!(
            translate_with_args("ja", "json_body_missing_field", &args),
            "Missing required field: phone"
        );
    }

    #[test]
    fn test_translate_known_key_en() {
        assert_eq!(
            translate("en-US", "internal_server_error"),
            "Internal server error"
        );
    }

    #[test]
    fn test_translate_fallback_to_default() {
        let result = translate("ja", "internal_server_error");
        assert_eq!(result, "Internal server error");
    }

    #[test]
    fn test_translate_unknown_key() {
        let result = translate("en-US", "no_such_key");
        assert_eq!(result, "no_such_key");
    }

    #[test]
    fn test_translate_phone_number_too_short() {
        assert_eq!(translate("zh-CN", "phone_number_too_short"), "手机号码过短");
    }

    #[test]
    fn test_translate_phone_number_invalid_cn_mobile() {
        assert_eq!(
            translate("zh-CN", "phone_number_invalid_cn_mobile"),
            "请输入有效的11位中国大陆手机号码"
        );
        assert_eq!(
            translate("en-US", "phone_number_invalid_cn_mobile"),
            "Must be a valid 11-digit mainland China mobile number"
        );
    }

    #[test]
    fn test_all_l10n_keys_used_in_code_exist_in_bundles() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
        let thiserror_re =
            Regex::new(r#"#\[error\("([^"]*)"\)\]"#).expect("compile thiserror regex");
        let web_error_re = Regex::new(r#"WebError::L10n\("([a-zA-Z0-9_-]+)"\.to_string\(\)\)"#)
            .expect("compile web error regex");
        // 不用 \s：workspace regex 关了 unicode-perl，隔离测 -p locale 时 \s 编不过
        let field_err_re =
            Regex::new(r#"field_err!\([ \t\n\r]*"[^"]*"[ \t\n\r]*,[ \t\n\r]*"([a-zA-Z0-9_-]+)""#)
                .expect("compile field_err regex");

        let mut keys = HashSet::new();
        let mut violations = Vec::new();

        collect_rs_files(&root, &mut |path| {
            let content = fs::read_to_string(path).expect("read source");
            for cap in thiserror_re.captures_iter(&content) {
                let msg = &cap[1];
                if msg
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
                {
                    keys.insert(msg.to_string());
                } else if msg.contains('{') {
                    violations.push(format!(
                        "参数化 #[error] 不允许出现在域代码中：{}: {msg}",
                        path.display()
                    ));
                } else {
                    violations.push(format!(
                        "#[error] 消息必须是 snake_case key：{}: {msg}",
                        path.display()
                    ));
                }
            }
            for cap in web_error_re.captures_iter(&content) {
                keys.insert(cap[1].to_string());
            }
            for cap in field_err_re.captures_iter(&content) {
                keys.insert(cap[1].to_string());
            }
        });

        assert!(
            violations.is_empty(),
            "错误消息格式违规：\n{}",
            violations.join("\n")
        );

        for key in &keys {
            assert!(
                try_translate("en-US", key).is_some(),
                "missing key `{key}` in en-US locales"
            );
            assert!(
                try_translate("zh-CN", key).is_some(),
                "missing key `{key}` in zh-CN locales"
            );
        }
    }

    fn collect_rs_files(root: &Path, on_file: &mut impl FnMut(&Path)) {
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let entries = match fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n == "target" || n == ".git")
                        .unwrap_or(false)
                    {
                        continue;
                    }
                    stack.push(path);
                } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                    on_file(&path);
                }
            }
        }
    }
}
