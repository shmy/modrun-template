use std::fmt;
use std::ops::Deref;

use rootcause::Result;
use serde::{Deserialize, Serialize};
use std::result;

#[derive(Debug, thiserror::Error)]
pub enum PhoneNumberError {
    #[error("phone_number_too_short")]
    TooShort,
    #[error("phone_number_too_long")]
    TooLong,
    #[error("phone_number_invalid_cn_mobile")]
    InvalidCnMobile,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
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
        let trimmed = phone_number.trim();
        Self::validate_str(trimmed)?;
        Ok(Self(trimmed.to_owned()))
    }

    /// 跳过校验直接包装（仅测试与已校验来源使用）。
    pub fn new_unchecked(phone_number: String) -> Self {
        Self(phone_number)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 供 HTTP DTO 的 validify `custom` 校验复用；错误 Display 即 l10n key。
    pub fn validate_str(s: &str) -> result::Result<(), PhoneNumberError> {
        let len = s.len();
        if len < 11 {
            return Err(PhoneNumberError::TooShort);
        }
        if len > 11 {
            return Err(PhoneNumberError::TooLong);
        }
        if !is_mainland_china_mobile(s) {
            return Err(PhoneNumberError::InvalidCnMobile);
        }
        Ok(())
    }
}

impl fmt::Debug for PhoneNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PhoneNumber").field(&"****").finish()
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
