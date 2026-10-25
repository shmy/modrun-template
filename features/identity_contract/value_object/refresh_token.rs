use std::{fmt::Debug, ops::Deref};

use serde::Deserialize;
use std::fmt;
use std::result;
use utoipa::ToSchema;
use validify::ValidationErrors;
use validify::{Modify, Validate};

/// Refresh token 长度上限：正常 UUID 为 36 字符；限制防超大 token 滥用。
const MAX_REFRESH_TOKEN_LEN: usize = 128;

#[derive(Deserialize, ToSchema)]
#[serde(transparent)]
pub struct RefreshToken(String);

impl RefreshToken {
    pub fn new(token: String) -> Self {
        Self(token)
    }
}

impl Modify for RefreshToken {
    fn modify(&mut self) {
        self.0 = self.0.trim().to_string();
    }
}

impl Validate for RefreshToken {
    fn validate(&self) -> result::Result<(), validify::ValidationErrors> {
        let mut errors = ValidationErrors::new();
        let len = self.0.chars().count();
        if len == 0 {
            errors.add(validify::field_err!(
                "too_short",
                "refresh_token_too_short",
                "refresh_token"
            ));
        }
        if len > MAX_REFRESH_TOKEN_LEN {
            errors.add(validify::field_err!(
                "too_long",
                "refresh_token_too_long",
                "refresh_token"
            ));
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

impl Debug for RefreshToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("RefreshToken").field(&"<Redacted>").finish()
    }
}

impl Deref for RefreshToken {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use validify::{Modify, Validate};

    #[test]
    fn rejects_empty_after_trim() {
        let mut token = RefreshToken::new("   ".to_string());
        token.modify();
        assert!(matches!(
            token.validate(),
            Err(e) if e.to_string().contains("refresh_token_too_short")
        ));
    }

    #[test]
    fn rejects_too_long() {
        let token = RefreshToken::new("a".repeat(129));
        assert!(matches!(
            token.validate(),
            Err(e) if e.to_string().contains("refresh_token_too_long")
        ));
    }

    #[test]
    fn accepts_uuid_length() {
        let token = RefreshToken::new("550e8400-e29b-41d4-a716-446655440000".to_string());
        assert!(token.validate().is_ok());
    }
}
