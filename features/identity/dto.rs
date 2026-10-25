use crate::repository::AccountListItem;
use identity_contract::{Account, Password, RefreshToken};
use serde::{Deserialize, Serialize};
use shared_contract::{Id, PhoneNumber};
use utoipa::{IntoParams, ToSchema};
use validify::Validify;

#[derive(Debug, Deserialize, Validify, IntoParams)]
#[into_params(parameter_in = Path)]
pub struct AccountIdPath {
    pub id: Id,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AccountPublic {
    pub id: Id,
    #[schema(example = "Tom")]
    pub name: String,
    #[schema(example = "13888888888")]
    pub phone: PhoneNumber,
    pub privileged: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AccountDetail {
    pub id: Id,
    #[schema(example = "Tom")]
    pub name: String,
    #[schema(example = "13888888888")]
    pub phone: PhoneNumber,
    pub privileged: bool,
    pub version: i64,
}

impl From<Account> for AccountPublic {
    fn from(account: Account) -> Self {
        Self {
            id: account.id,
            name: account.name,
            phone: account.phone,
            privileged: account.privileged,
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
            version: account.version,
        }
    }
}

#[derive(Debug, Deserialize, Validify, ToSchema)]
pub struct LoginRequest {
    #[validify]
    pub phone: PhoneNumber,
    #[schema(example = "P@ssw0rd1")]
    #[validify]
    pub password: Password,
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
    pub refresh_token: RefreshToken,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LogoutResponse {
    #[schema(example = true)]
    pub logged_out: bool,
}
