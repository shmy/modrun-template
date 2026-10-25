use std::fmt::Display;

use core::fmt;
use sha2::{Digest, Sha256};

pub fn refresh_digest(refresh_token: &str) -> String {
    sha256_hex(refresh_token)
}

pub fn refresh_key(realm: &str, refresh_token: &str) -> String {
    refresh_key_from_digest(realm, &refresh_digest(refresh_token))
}

pub fn refresh_key_from_digest(realm: &str, digest: &str) -> String {
    format!("auth:{realm}:refresh:{digest}")
}

pub fn subject_refresh_key(realm: &str, subject: impl Display) -> String {
    format!("auth:{realm}:subject_refresh:{subject}")
}

pub fn access_jti_key(realm: &str, subject: impl Display) -> String {
    format!("auth:{realm}:access_jti:{subject}")
}

/// 已轮换 refresh 的 digest 标记；用于检测重放并在命中时吊销该账号全部会话。
pub fn used_refresh_key(realm: &str, digest: &str) -> String {
    format!("auth:{realm}:refresh_used:{digest}")
}

fn sha256_hex(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = fmt::Write::write_fmt(&mut hex, format_args!("{byte:02x}"));
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_key_does_not_embed_raw_token() {
        let token = "550e8400-e29b-41d4-a716-446655440000";
        let key = refresh_key("account", token);
        assert!(!key.contains(token));
        assert!(key.starts_with("auth:account:refresh:"));
        assert_eq!(key, refresh_key("account", token));
        assert_ne!(key, refresh_key("account", "other-token"));
        assert_eq!(
            key,
            refresh_key_from_digest("account", &refresh_digest(token))
        );
    }
}
