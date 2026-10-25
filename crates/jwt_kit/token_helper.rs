use std::fmt::{Debug, Display};

use jsonwebtoken::{
    Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode, get_current_timestamp,
};
use rootcause::Result;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};

use crate::token_realm::TokenRealm;
use std::fmt;

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub iss: String,
    pub aud: String,
    pub exp: u64,
    pub iat: u64,
    pub jti: String,
}

#[derive(Clone)]
pub struct TokenHelper {
    realm: TokenRealm,
    jwt_secret: SecretString,
    access_ttl_secs: u64,
    refresh_ttl_secs: u64,
    issuer: String,
    audience: String,
}

impl Debug for TokenHelper {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenHelper").finish()
    }
}

impl TokenHelper {
    pub fn new(
        realm: TokenRealm,
        jwt_secret: SecretString,
        access_ttl_secs: u64,
        refresh_ttl_secs: u64,
        issuer: impl Into<String>,
        audience: impl Into<String>,
    ) -> Self {
        Self {
            realm,
            jwt_secret,
            access_ttl_secs,
            refresh_ttl_secs,
            issuer: issuer.into(),
            audience: audience.into(),
        }
    }

    pub fn encode_access_token<T: Display>(&self, user_id: T, jti: &str) -> Result<(String, u64)> {
        let key = EncodingKey::from_secret(self.jwt_secret.expose_secret().as_bytes());
        let now = get_current_timestamp();
        let claims = Claims {
            sub: user_id.to_string(),
            iss: self.issuer.clone(),
            aud: self.audience.clone(),
            exp: now + self.access_ttl_secs,
            iat: now,
            jti: jti.to_string(),
        };
        let token = encode(&Header::default(), &claims, &key)?;
        Ok((token, self.access_ttl_secs))
    }

    pub fn decode_access_token(&self, token: &str) -> Result<Claims> {
        let key = DecodingKey::from_secret(self.jwt_secret.expose_secret().as_bytes());
        let mut validation = Validation::new(Algorithm::HS256);
        validation.set_issuer(&[&self.issuer]);
        validation.set_audience(&[&self.audience]);
        let data = decode::<Claims>(token, &key, &validation)?;
        Ok(data.claims)
    }

    pub fn realm(&self) -> &str {
        &self.realm
    }

    pub fn access_ttl_secs(&self) -> u64 {
        self.access_ttl_secs
    }

    pub fn refresh_ttl_secs(&self) -> u64 {
        self.refresh_ttl_secs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn helper() -> TokenHelper {
        TokenHelper::new(
            TokenRealm::Account,
            SecretString::from("test-jwt-secret-at-least-32-bytes!!"),
            60,
            3600,
            "modrun-template",
            "account",
        )
    }

    #[test]
    fn roundtrip_includes_iss_and_aud() {
        let helper = helper();
        let (token, _) = helper.encode_access_token("1", "jti-1").unwrap();
        let claims = helper.decode_access_token(&token).unwrap();
        assert_eq!(claims.sub, "1");
        assert_eq!(claims.iss, "modrun-template");
        assert_eq!(claims.aud, "account");
        assert_eq!(claims.jti, "jti-1");
    }

    #[test]
    fn rejects_token_from_other_issuer() {
        let helper = helper();
        let other = TokenHelper::new(
            TokenRealm::Account,
            SecretString::from("test-jwt-secret-at-least-32-bytes!!"),
            60,
            3600,
            "other-issuer",
            "account",
        );
        let (token, _) = other.encode_access_token("1", "jti-1").unwrap();
        assert!(helper.decode_access_token(&token).is_err());
    }
}
