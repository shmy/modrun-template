use authn_kit::{AuthnError, VerifiedToken, Verifier};

use crate::token::access_token_from_headers;
use axum::extract::Request;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use cache_kit::Cache;
use jwt_kit::TokenHelper;
use shared_contract::Id;
use web_error::WebError;
use web_state::{AppState, HasAuthCaps};

use crate::extractor::AuthedAccount;
use axum::extract::State;

struct AuthVerifier<'a> {
    token_helper: &'a TokenHelper,
    cache: &'a Cache,
}

impl Verifier for AuthVerifier<'_> {
    async fn verify<'a>(&'a self, token: &'a str) -> Result<VerifiedToken, AuthnError> {
        let claims = self
            .token_helper
            .decode_access_token(token)
            .map_err(|_| AuthnError::AccessTokenInvalid)?;

        let realm = self.token_helper.realm();
        let stored_jti = match self
            .cache
            .get::<String>(&authn_kit::access_jti_key(realm, &claims.sub))
            .await
        {
            Ok(stored) => stored,
            Err(err) => {
                tracing::error!(error = %err, "cache unavailable during token verification");
                return Err(AuthnError::CacheUnavailable);
            }
        };

        match stored_jti {
            Some(ref jti) if jti == &claims.jti => Ok(VerifiedToken {
                subject: claims.sub,
                jti: claims.jti,
            }),
            _ => Err(AuthnError::AccessTokenRevoked),
        }
    }
}

pub async fn account_auth_middleware(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    match authenticate_account(&state, request, next).await {
        Ok(response) => response,
        Err(err) => err.into_response(),
    }
}

async fn authenticate_account(
    state: &impl HasAuthCaps,
    mut request: Request,
    next: Next,
) -> Result<Response, WebError> {
    let token = access_token_from_headers(request.headers()).map_err(auth_error_to_web_error)?;
    let verifier = AuthVerifier {
        token_helper: state.token_helper(),
        cache: state.cache(),
    };
    let verified = verifier
        .verify(&token)
        .await
        .map_err(auth_error_to_web_error)?;
    let account_id: i64 = verified
        .subject
        .parse()
        .map_err(|_| auth_error_to_web_error(AuthnError::AccessTokenInvalid))?;

    request
        .extensions_mut()
        .insert(AuthedAccount(Id::new_unchecked(account_id)));
    Ok(next.run(request).await)
}

fn auth_error_to_web_error(err: AuthnError) -> WebError {
    WebError::L10n(err.to_string())
}
