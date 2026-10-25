use argon2::password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash};
use argon2::{Argon2, Params};
use rootcause::{Report, Result};
use std::fmt::Debug;
use std::ops::Deref;
use std::sync::LazyLock;

use crate::error::IdentityError;
use crate::value_object::password::Password;
use argon2::Algorithm;
use argon2::Version;
use std::fmt;
use tokio::task::spawn_blocking;

static ARGON2: LazyLock<Argon2> =
    LazyLock::new(|| Argon2::new(Algorithm::Argon2id, Version::V0x13, Params::default()));

#[derive(Clone)]
pub struct HashedPassword(String);

impl HashedPassword {
    pub fn new_unchecked(hash: String) -> Self {
        Self(hash)
    }

    /// 同步哈希。仅用于测试或已在 `spawn_blocking` 内的路径；请求处理请用 [`Self::try_new`]。
    pub fn try_new_sync(password: &str) -> Result<Self> {
        let password = Password::try_new(password)?;
        let hash = (*ARGON2)
            .hash_password(password.as_bytes())
            .map_err(|_| IdentityError::AccountPasswordEncodeFailed)?;
        Ok(Self(hash.to_string()))
    }

    /// argon2 哈希，在 blocking 线程池执行，避免阻塞 tokio worker。
    pub async fn try_new(password: &str) -> Result<Self> {
        let password = password.to_string();
        spawn_blocking(move || Self::try_new_sync(&password))
            .await
            .map_err(|err| -> Report {
                tracing::error!(error = %err, "password hash task join failed");
                IdentityError::AccountPasswordEncodeFailed.into()
            })?
    }

    pub fn verify_sync(&self, password: &str) -> Result<()> {
        let parsed_hash =
            PasswordHash::new(&self.0).map_err(|_| IdentityError::AccountPasswordDecodeFailed)?;
        if (*ARGON2)
            .verify_password(password.as_bytes(), &parsed_hash)
            .is_err()
        {
            Err(IdentityError::AccountPasswordIncorrect)?;
        }
        Ok(())
    }

    pub async fn verify(&self, password: &str) -> Result<()> {
        let hashed = self.clone();
        let password = password.to_string();
        spawn_blocking(move || hashed.verify_sync(&password))
            .await
            .map_err(|err| -> Report {
                tracing::error!(error = %err, "password verify task join failed");
                IdentityError::AccountInvalidCredentials.into()
            })?
    }
}

impl Debug for HashedPassword {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("HashedPassword")
            .field(&"<Redacted>")
            .finish()
    }
}

impl Deref for HashedPassword {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
