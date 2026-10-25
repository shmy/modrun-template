#[derive(Debug, thiserror::Error)]
pub enum AuthnError {
    #[error("access_token_missing")]
    AccessTokenMissing,
    #[error("access_token_invalid")]
    AccessTokenInvalid,
    #[error("access_token_revoked")]
    AccessTokenRevoked,
    #[error("cache_unavailable")]
    CacheUnavailable,
}
