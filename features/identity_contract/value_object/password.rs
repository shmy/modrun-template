use rootcause::Result;
use serde::Deserialize;
use std::fmt;
use std::result;
use std::{fmt::Debug, ops::Deref};
use utoipa::ToSchema;
use validify::ValidationErrors;
use validify::{Modify, Validate};

const MAX_PASSWORD_LEN: usize = 64;

#[derive(Clone, Deserialize, ToSchema)]
#[serde(transparent)]
pub struct Password(String);

impl Password {
    pub fn try_new(password: &str) -> Result<Self> {
        let mut instance = Self(password.to_owned());
        instance.modify();
        instance.validate()?;
        Ok(instance)
    }

    /// 跳过校验/哈希直接包装（仅测试与已校验来源使用）。
    pub fn new_unchecked(password: String) -> Self {
        Self(password)
    }
}

impl Modify for Password {
    fn modify(&mut self) {
        self.0 = self.0.trim().to_string();
    }
}

impl Validate for Password {
    fn validate(&self) -> result::Result<(), validify::ValidationErrors> {
        let mut errors = ValidationErrors::new();
        if self.0.chars().count() < 8 {
            errors.add(validify::field_err!(
                "too_short",
                "account_password_too_short",
                "password"
            ));
        }
        if self.0.chars().count() > MAX_PASSWORD_LEN {
            errors.add(validify::field_err!(
                "too_long",
                "account_password_too_long",
                "password"
            ));
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

impl Debug for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Password").field(&"<Redacted>").finish()
    }
}

impl Deref for Password {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
