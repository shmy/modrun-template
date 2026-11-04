use crate::entity::Account;
use crate::repository::AccountListItem;
use crate::value_object::{Password, RefreshToken};
use serde::{Deserialize, Serialize};
use shared_contract::{CursorPagingResult, Id, PhoneNumber, PhoneNumberError};
use utoipa::{IntoParams, ToSchema};
use validify::{ValidationError, Validify};

/// validify custom：与 `PhoneNumber::validate_str` 共用规则，错误 key 进 InvalidParams。
pub fn validate_phone_number(phone: &str) -> Result<(), ValidationError> {
    match PhoneNumber::validate_str(phone.trim()) {
        Ok(()) => Ok(()),
        Err(PhoneNumberError::TooShort) => {
            Err(validify::field_err!("too_short", "phone_number_too_short"))
        }
        Err(PhoneNumberError::TooLong) => {
            Err(validify::field_err!("too_long", "phone_number_too_long"))
        }
        Err(PhoneNumberError::InvalidCnMobile) => Err(validify::field_err!(
            "invalid_cn_mobile",
            "phone_number_invalid_cn_mobile"
        )),
    }
}

#[derive(Debug, Deserialize, Validify, IntoParams)]
#[into_params(parameter_in = Path)]
pub struct AccountIdPath {
    #[param(value_type = String)]
    pub id: Id,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AccountPublic {
    #[schema(value_type = String)]
    pub id: Id,
    #[schema(example = "Tom")]
    pub name: String,
    #[schema(value_type = String, example = "13888888888")]
    pub phone: PhoneNumber,
    pub privileged: bool,
    /// 已转正对象路径（非 staging；创建/更新时提交 staging，落库为正式 path）。
    #[schema(example = "2026/01/abc.webp")]
    pub avatar: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AccountDetail {
    #[schema(value_type = String)]
    pub id: Id,
    #[schema(example = "Tom")]
    pub name: String,
    #[schema(value_type = String, example = "13888888888")]
    pub phone: PhoneNumber,
    pub privileged: bool,
    #[schema(example = "2026/01/abc.webp")]
    pub avatar: Option<String>,
    pub version: i64,
}

/// OpenAPI 用分页响应壳（字段与 `CursorPagingResult` 对齐；契约类型本身不依赖 utoipa）。
///
/// handler 必须**真的**返回它，而不是声明它却返回 `CursorPagingResult`：
/// 否则 schema 与实际 body 会悄悄分叉。
#[derive(Debug, Serialize, ToSchema)]
pub struct AccountSearchResponse {
    pub items: Vec<AccountPublic>,
    #[schema(value_type = Option<String>)]
    pub next_cursor: Option<Id>,
}

impl From<CursorPagingResult<AccountPublic>> for AccountSearchResponse {
    fn from(page: CursorPagingResult<AccountPublic>) -> Self {
        Self {
            items: page.items,
            next_cursor: page.next_cursor,
        }
    }
}

impl From<Account> for AccountPublic {
    fn from(account: Account) -> Self {
        Self {
            id: account.id,
            name: account.name,
            phone: account.phone,
            privileged: account.privileged,
            avatar: account.avatar,
        }
    }
}

impl From<AccountListItem> for AccountPublic {
    fn from(item: AccountListItem) -> Self {
        Self {
            id: item.id,
            name: item.name,
            phone: item.phone,
            privileged: item.privileged,
            avatar: item.avatar,
        }
    }
}

impl From<Account> for AccountDetail {
    fn from(account: Account) -> Self {
        Self {
            id: account.id,
            name: account.name,
            phone: account.phone,
            privileged: account.privileged,
            avatar: account.avatar,
            version: account.version,
        }
    }
}

/// validify custom：仅在 `Option::Some` 时调用；trim 后长度须 1..=255。
pub fn validate_avatar(avatar: &str) -> Result<(), ValidationError> {
    let trimmed = avatar.trim();
    if trimmed.is_empty() {
        return Err(validify::field_err!(
            "too_short",
            "account_avatar_too_short"
        ));
    }
    if trimmed.len() > 255 {
        return Err(validify::field_err!("too_long", "account_avatar_too_long"));
    }
    Ok(())
}

#[derive(Debug, Deserialize, Validify, ToSchema)]
pub struct LoginRequest {
    #[modify(trim)]
    #[validate(custom(validate_phone_number))]
    #[schema(value_type = String, example = "13888888888")]
    pub phone: String,
    #[schema(value_type = String, example = "P@ssw0rd1")]
    #[validify]
    pub password: Password,
}

impl LoginRequest {
    pub fn phone(&self) -> PhoneNumber {
        PhoneNumber::new_unchecked(self.phone.clone())
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TokenResponse {
    #[schema(example = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9")]
    pub access_token: String,
    #[schema(example = "550e8400-e29b-41d4-a716-446655440000")]
    pub refresh_token: String,
    #[schema(example = "Bearer")]
    pub token_type: String,
    #[schema(example = 900)]
    pub expires_in: u64,
}

impl TokenResponse {
    pub fn bearer(access_token: String, refresh_token: String, expires_in: u64) -> Self {
        Self {
            access_token,
            refresh_token,
            token_type: "Bearer".to_string(),
            expires_in,
        }
    }
}

#[derive(Debug, Deserialize, Validify, ToSchema)]
pub struct RefreshRequest {
    #[schema(example = "550e8400-e29b-41d4-a716-446655440000")]
    // 缺 `#[validify]` 时嵌套的 `RefreshToken::validate` 不会被调用，
    // 空串与超长 token 会直接落到 401，而不是明确的 400 参数错误。
    #[validify]
    pub refresh_token: RefreshToken,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LogoutResponse {
    #[schema(example = true)]
    pub logged_out: bool,
}
