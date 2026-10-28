use argon2::password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash};
use argon2::{Argon2, Params};
use rootcause::{Report, Result};
use std::fmt::Debug;
use std::ops::Deref;
use std::sync::LazyLock;

use crate::value_object::password::Password;
use argon2::Algorithm;
use argon2::Version;
use identity_contract::IdentityError;
use std::fmt;
use tokio::task::spawn_blocking;

/// 显式固定 argon2id 成本（m=19MiB, t=2, p=1，OWASP 推荐基线）：
/// 不用 `Params::default()`，避免 argon2 crate 升级时静默改变哈希成本（影响登录延迟与安全基线）
static ARGON2: LazyLock<Argon2> = LazyLock::new(|| {
    let params = Params::new(19 * 1024, 2, 1, None).unwrap_or_else(|_| Params::default());
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
});

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

    /// 时序一致性：执行一次与真实校验等价的 argon2（固定哑密码，**不做密码规则校验**，
    /// 规则变化不影响此路径），供登录「手机号不存在」分支抹平响应时间差，防止枚举有效手机号。
    /// 不产生/存储任何凭证，失败仅记录。
    pub async fn equalize_timing() {
        const DUMMY: &[u8] = b"timing-equalizer-dummy-password";
        let result = spawn_blocking(move || (*ARGON2).hash_password(DUMMY)).await;
        if let Err(err) = result {
            tracing::error!(error = %err, "login timing equalizer hash failed");
        }
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
