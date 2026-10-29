/// 域自注册 file FTL；infra/locale 只保留 shared.ftl。
pub fn register_file_locales() {
    locale::register_ftl("en-US", include_str!("../locales/en-US/file.ftl"));
    locale::register_ftl("zh-CN", include_str!("../locales/zh-CN/file.ftl"));
}

#[cfg(test)]
mod tests {
    use super::register_file_locales;
    use file_contract::error::{FileError, L10nKeyedError};
    use locale::try_translate;
    use std::sync::Once;

    fn seal() {
        static ONCE: Once = Once::new();
        ONCE.call_once(register_file_locales);
    }

    fn assert_key(key: &str) {
        assert!(
            try_translate("en-US", key).is_some(),
            "missing en-US key `{key}` (file.ftl or shared.ftl)"
        );
        assert!(
            try_translate("zh-CN", key).is_some(),
            "missing zh-CN key `{key}` (file.ftl or shared.ftl)"
        );
    }

    #[test]
    fn file_error_and_ftl_keys_exist_in_bundles() {
        seal();
        for err in [FileError::NotImage, FileError::FileNameMissing] {
            assert_key(err.l10n_key());
        }
        // image_kit 错误 key 落在 shared.ftl（kit 层），此处确认仍可解析
        assert_key("image_dimensions_exceeded");
    }
}
