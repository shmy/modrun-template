use authn_kit::AuthnError;
use axum::http::{HeaderMap, header::AUTHORIZATION};

pub fn access_token_from_headers(headers: &HeaderMap) -> Result<String, AuthnError> {
    let value = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .ok_or(AuthnError::AccessTokenMissing)?;
    let (scheme, token) = value
        .split_once(' ')
        .ok_or(AuthnError::AccessTokenMissing)?;
    if !scheme.eq_ignore_ascii_case("Bearer") {
        return Err(AuthnError::AccessTokenMissing);
    }
    let token = token.trim();
    if token.is_empty() {
        return Err(AuthnError::AccessTokenMissing);
    }
    Ok(token.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(value: &'static str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, HeaderValue::from_static(value));
        headers
    }

    #[test]
    fn accepts_capitalized_bearer() {
        let token = access_token_from_headers(&headers("Bearer abc.def"));
        assert!(matches!(token, Ok(value) if value == "abc.def"));
    }

    #[test]
    fn accepts_lowercase_scheme() {
        let token = access_token_from_headers(&headers("bearer abc.def"));
        assert!(matches!(token, Ok(value) if value == "abc.def"));
    }

    #[test]
    fn rejects_missing_or_empty_token() {
        assert!(access_token_from_headers(&HeaderMap::new()).is_err());
        assert!(access_token_from_headers(&headers("Bearer ")).is_err());
        assert!(access_token_from_headers(&headers("Basic abc")).is_err());
    }
}
