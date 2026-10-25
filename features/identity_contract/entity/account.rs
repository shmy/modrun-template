use serde::Serialize;
use shared_contract::{Id, PhoneNumber};

use crate::value_object::hashed_password::HashedPassword;

#[derive(Debug, Clone, Serialize)]
pub struct Account {
    pub id: Id,
    pub name: String,
    pub phone: PhoneNumber,
    #[serde(skip)]
    pub password: HashedPassword,
    pub privileged: bool,
    #[serde(skip)]
    pub version: i64,
}
