//! identity 跨域契约。
//!
//! **当前口径（B）**：本 crate **仅**承载域错误（`IdentityError` / `L10nKeyedError`）。
//! 公开读模型（`AccountPublic` 等）与 `Id` 别名仍在 `features/identity` / `shared_contract`。
//!
//! **TODO（推荐路径 A）**：把跨域需要的公开读模型 / Id 别名下沉到本 crate（纯 serde，无
//! utoipa/sqlx/argon2）；runtime 保留哈希密码等私有细节。待出现第二个消费域时再做。

mod error;

pub use error::{IdentityError, L10nKeyedError};
