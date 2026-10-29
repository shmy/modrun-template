/// 域自注册 identity FTL；infra/locale 只保留 shared.ftl。
pub fn register_identity_locales() {
    locale::register_ftl("en-US", include_str!("../locales/en-US/identity.ftl"));
    locale::register_ftl("zh-CN", include_str!("../locales/zh-CN/identity.ftl"));
}

#[cfg(test)]
mod tests {
    use super::register_identity_locales;
    use identity_contract::{IdentityError, L10nKeyedError};
    use locale::try_translate;
    use std::sync::Once;

    fn seal() {
        static ONCE: Once = Once::new();
        ONCE.call_once(register_identity_locales);
    }

    fn assert_key(key: &str) {
        assert!(
            try_translate("en-US", key).is_some(),
            "missing en-US key `{key}` (identity.ftl or shared.ftl)"
        );
        assert!(
            try_translate("zh-CN", key).is_some(),
            "missing zh-CN key `{key}` (identity.ftl or shared.ftl)"
        );
    }

    #[test]
    fn phone_number_keys_in_identity_ftl() {
        seal();
        assert_eq!(
            locale::translate("zh-CN", "phone_number_too_short"),
            "手机号码过短"
        );
        assert_key("phone_number_too_short");
        assert_key("phone_number_too_long");
        assert_key("phone_number_invalid_cn_mobile");
        // DTO field_err! 与 validate_phone_number 共用上述 key
    }

    #[test]
    fn identity_error_keys_exist_in_bundles() {
        seal();
        let cases = [
            IdentityError::AccountNotFound,
            IdentityError::AccountDuplicated,
            IdentityError::AccountPasswordEncodeFailed,
            IdentityError::AccountPasswordDecodeFailed,
            IdentityError::AccountPasswordIncorrect,
            IdentityError::AccountInvalidCredentials,
            IdentityError::AccountProtected,
            IdentityError::AccountForbidden,
            IdentityError::AccountVersionConflict,
            IdentityError::AccountSearchQTooShort,
            IdentityError::AccountSearchQTooLong,
            IdentityError::RefreshTokenInvalid,
            IdentityError::RefreshTokenTooShort,
            IdentityError::RefreshTokenTooLong,
            IdentityError::CacheUnavailable,
        ];
        for err in cases {
            assert_key(err.l10n_key());
        }
        // validify / DTO 文案（非 IdentityError 变体）
        for key in [
            "account_password_too_short",
            "account_password_too_long",
            "account_name_too_short",
            "account_name_too_long",
        ] {
            assert_key(key);
        }
    }
}
