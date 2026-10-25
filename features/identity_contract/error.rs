#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error("account_not_found")]
    AccountNotFound,
    #[error("account_duplicated")]
    AccountDuplicated,
    #[error("account_password_encode_failed")]
    AccountPasswordEncodeFailed,
    #[error("account_password_decode_failed")]
    AccountPasswordDecodeFailed,
    #[error("account_password_incorrect")]
    AccountPasswordIncorrect,
    #[error("account_invalid_credentials")]
    AccountInvalidCredentials,
    #[error("account_protected")]
    AccountProtected,
    #[error("account_forbidden")]
    AccountForbidden,
    #[error("account_version_conflict")]
    AccountVersionConflict,
    #[error("account_search_q_too_short")]
    AccountSearchQTooShort,
    #[error("account_search_q_too_long")]
    AccountSearchQTooLong,
    #[error("refresh_token_invalid")]
    RefreshTokenInvalid,
    #[error("refresh_token_too_short")]
    RefreshTokenTooShort,
    #[error("refresh_token_too_long")]
    RefreshTokenTooLong,
    #[error("cache_unavailable")]
    CacheUnavailable,
}

/// 域错误对应的 l10n key（与 thiserror `#[error]` 消息保持一致）。
pub trait L10nKeyedError {
    fn l10n_key(&self) -> &'static str;
}

impl L10nKeyedError for IdentityError {
    fn l10n_key(&self) -> &'static str {
        match self {
            Self::AccountNotFound => "account_not_found",
            Self::AccountDuplicated => "account_duplicated",
            Self::AccountPasswordEncodeFailed => "account_password_encode_failed",
            Self::AccountPasswordDecodeFailed => "account_password_decode_failed",
            Self::AccountPasswordIncorrect => "account_password_incorrect",
            Self::AccountInvalidCredentials => "account_invalid_credentials",
            Self::AccountProtected => "account_protected",
            Self::AccountForbidden => "account_forbidden",
            Self::AccountVersionConflict => "account_version_conflict",
            Self::AccountSearchQTooShort => "account_search_q_too_short",
            Self::AccountSearchQTooLong => "account_search_q_too_long",
            Self::RefreshTokenInvalid => "refresh_token_invalid",
            Self::RefreshTokenTooShort => "refresh_token_too_short",
            Self::RefreshTokenTooLong => "refresh_token_too_long",
            Self::CacheUnavailable => "cache_unavailable",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn l10n_key_matches_thiserror_display() {
        let cases = [
            IdentityError::AccountNotFound,
            IdentityError::AccountSearchQTooShort,
            IdentityError::RefreshTokenInvalid,
        ];
        for err in cases {
            assert_eq!(err.l10n_key(), err.to_string());
        }
    }
}
