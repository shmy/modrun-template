use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use shared_contract::Id;
use web_error::WebError;

#[derive(Clone, Debug)]
pub struct AuthedAccount(pub Id);

impl<S> FromRequestParts<S> for AuthedAccount
where
    S: Send + Sync + 'static,
{
    type Rejection = WebError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let Some(subject) = parts.extensions.get::<Self>() else {
            return Err(WebError::L10n("authed_account_not_found".to_string()));
        };
        Ok(subject.clone())
    }
}
