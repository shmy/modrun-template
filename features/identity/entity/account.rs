use serde::Serialize;
use shared_contract::{Id, PhoneNumber};

use crate::value_object::HashedPassword;

#[derive(Debug, Clone, Serialize)]
pub struct Account {
    pub id: Id,
    pub name: String,
    pub phone: PhoneNumber,
    #[serde(skip)]
    pub password: HashedPassword,
    pub privileged: bool,
    /// 头像对象路径（blob path，可空）；响应侧可再填公网 URL。
    pub avatar: Option<String>,
    #[serde(skip)]
    pub version: i64,
}
