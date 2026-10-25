use std::fmt;
use std::ops::Deref;

use rootcause::Result;
use serde::{Deserialize, Serialize};
use std::result;
use utoipa::ToSchema;
use validify::ValidationErrors;
use validify::{Modify, Validate};

#[derive(Clone, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[serde(transparent)]
#[sqlx(transparent)]
pub struct PhoneNumber(String);

fn is_mainland_china_mobile(s: &str) -> bool {
    if s.len() != 11 {
        return false;
    }
    if !s.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    let b = s.as_bytes();
    b.first() == Some(&b'1') && b.get(1).is_some_and(|c| (b'3'..=b'9').contains(c))
}

impl PhoneNumber {
    pub fn try_new(phone_number: &str) -> Result<Self> {
        let mut instance = Self(phone_number.to_owned());
        instance.modify();
        instance.validate()?;
        Ok(instance)
    }

    /// 跳过校验直接包装（仅测试与已校验来源使用）。
    pub fn new_unchecked(phone_number: String) -> Self {
        Self(phone_number)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for PhoneNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PhoneNumber").field(&"****").finish()
    }
}

impl Modify for PhoneNumber {
    fn modify(&mut self) {
        self.0 = self.0.trim().to_string();
    }
}

impl Validate for PhoneNumber {
    fn validate(&self) -> result::Result<(), validify::ValidationErrors> {
        let mut errors = ValidationErrors::new();
        let s = self.0.as_str();
        let len = s.len();
        if len < 11 {
            errors.add(validify::field_err!(
                "too_short",
                "phone_number_too_short",
                "phone_number"
            ));
        }
        if len > 11 {
            errors.add(validify::field_err!(
                "too_long",
                "phone_number_too_long",
                "phone_number"
            ));
        }
        if len == 11 && !is_mainland_china_mobile(s) {
            errors.add(validify::field_err!(
                "invalid_cn_mobile",
                "phone_number_invalid_cn_mobile",
                "phone_number"
            ));
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

impl Deref for PhoneNumber {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_try_new_valid() {
        let phone = PhoneNumber::try_new("13888888888").unwrap();
        assert_eq!(phone.as_str(), "13888888888");
    }

    #[test]
    fn test_try_new_too_short() {
        let err = PhoneNumber::try_new("12").unwrap_err();
        assert!(err.to_string().contains("phone_number_too_short"));
    }

    #[test]
    fn test_try_new_invalid_cn_mobile() {
        let err = PhoneNumber::try_new("12812345678").unwrap_err();
        assert!(err.to_string().contains("phone_number_invalid_cn_mobile"));
    }
}
