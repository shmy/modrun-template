//! HS256（JWS HMAC-SHA256，RFC 7515 / RFC 7519）最小实现，替代 jsonwebtoken：
//! jwt_kit 只签发/校验 HS256，而 jsonwebtoken 任一 backend（`rust_crypto` / `aws_lc_rs`）
//! 都会把整套非对称曲线（rsa、crypto-bigint、p256/p384/ed25519 等）编进二进制。
//!
//! 本模块只做 JWS 层（签验 + 反序列化）；exp/iss/aud 等业务语义在 `token_helper`。

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, KeyInit, Mac};
use rootcause::Result;
use serde::de::DeserializeOwned;
use sha2::Sha256;
use std::time::{SystemTime, UNIX_EPOCH};

type HmacSha256 = Hmac<Sha256>;

/// 编码侧固定 header；解码侧强制 `alg == HS256`（防 alg 混淆/降级攻击）
const HEADER_JSON: &str = "{\"alg\":\"HS256\",\"typ\":\"JWT\"}";

/// jwt_kit 内部错误；调用方（`http_auth` 中间件）把任何失败统一映射为 token 无效，
/// 错误文本不外露，故无需走 l10n。
#[derive(Debug, thiserror::Error)]
pub(crate) enum JwtError {
    #[error("jwt_invalid")]
    Invalid,
    #[error("jwt_expired")]
    Expired,
}

pub(crate) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        // 系统时钟早于 epoch 时取 0：exp 校验只会更严格
        .unwrap_or(0)
}

fn new_mac(secret: &[u8]) -> std::result::Result<HmacSha256, JwtError> {
    // HMAC 接受任意长度密钥，此分支仅为满足 Result API
    HmacSha256::new_from_slice(secret).map_err(|_| JwtError::Invalid)
}

/// 签名：`base64url(header) + "." + base64url(payload) + "." + base64url(HMAC-SHA256)`
pub(crate) fn sign(payload_json: &str, secret: &[u8]) -> Result<String> {
    let signing_input = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(HEADER_JSON),
        URL_SAFE_NO_PAD.encode(payload_json)
    );
    let mut mac = new_mac(secret)?;
    mac.update(signing_input.as_bytes());
    let sig = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    Ok(format!("{signing_input}.{sig}"))
}

/// 验签并反序列化 claims。签名比较为常量时间（`Mac::verify_slice`）。
pub(crate) fn verify<T: DeserializeOwned>(token: &str, secret: &[u8]) -> Result<T> {
    let parts = token.split('.').collect::<Vec<_>>();
    let [h_b64, p_b64, s_b64] = match parts.as_slice() {
        [h, p, s] => [h, p, s],
        _ => return Err(JwtError::Invalid.into()),
    };

    // 先验签再解析内容：任何未通过签名的 token 都不进入反序列化
    let sig = URL_SAFE_NO_PAD
        .decode(s_b64)
        .map_err(|_| JwtError::Invalid)?;
    let mut mac = new_mac(secret)?;
    mac.update(format!("{h_b64}.{p_b64}").as_bytes());
    mac.verify_slice(&sig).map_err(|_| JwtError::Invalid)?;

    // alg 白名单：签名合法但声称非 HS256（如 RS256/HS512）的 token 一律拒绝
    let header = URL_SAFE_NO_PAD
        .decode(h_b64)
        .map_err(|_| JwtError::Invalid)?;
    let header: serde_json::Value =
        serde_json::from_slice(&header).map_err(|_| JwtError::Invalid)?;
    if header.get("alg").and_then(|v| v.as_str()) != Some("HS256") {
        return Err(JwtError::Invalid.into());
    }

    let payload = URL_SAFE_NO_PAD
        .decode(p_b64)
        .map_err(|_| JwtError::Invalid)?;
    let claims: T = serde_json::from_slice(&payload).map_err(|_| JwtError::Invalid)?;
    Ok(claims)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &[u8] = b"test-jwt-secret-at-least-32-bytes!!";

    #[test]
    fn sign_then_verify_roundtrip() {
        let token = sign("{\"sub\":\"1\"}", SECRET).unwrap();
        let claims: serde_json::Value = verify(&token, SECRET).unwrap();
        assert_eq!(claims["sub"], "1");
    }

    #[test]
    fn tampered_payload_rejected() {
        let token = sign("{\"sub\":\"1\"}", SECRET).unwrap();
        let parts: Vec<&str> = token.split('.').collect();
        // 篡改 payload（换成 "{}" 的 b64）后签名不再匹配
        let tampered = format!("{}.e30.{}", parts[0], parts[2]);
        let result: Result<serde_json::Value> = verify(&tampered, SECRET);
        assert!(result.is_err());
    }

    #[test]
    fn wrong_secret_rejected() {
        let token = sign("{\"sub\":\"1\"}", SECRET).unwrap();
        let result: Result<serde_json::Value> = verify(&token, b"other-secret-32-bytes-long-xxxx");
        assert!(result.is_err());
    }

    #[test]
    fn non_hs256_alg_rejected_even_with_valid_hmac_sig() {
        // 防降级：用正确 HMAC-SHA256 签名但 header 声称 HS512，必须拒绝
        let header = URL_SAFE_NO_PAD.encode("{\"alg\":\"HS512\",\"typ\":\"JWT\"}");
        let payload = URL_SAFE_NO_PAD.encode("{\"sub\":\"1\"}");
        let signing_input = format!("{header}.{payload}");
        let mut mac = HmacSha256::new_from_slice(SECRET).unwrap();
        mac.update(signing_input.as_bytes());
        let sig = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
        let token = format!("{signing_input}.{sig}");
        let result: Result<serde_json::Value> = verify(&token, SECRET);
        assert!(result.is_err());
    }

    #[test]
    fn malformed_tokens_rejected() {
        for token in ["", "a.b", "a.b.c.d", "!!!.b64.b64"] {
            let result: Result<serde_json::Value> = verify(token, SECRET);
            assert!(result.is_err(), "should reject: {token}");
        }
    }
}
