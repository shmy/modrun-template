use std::future::Future;

use crate::error::AuthnError;

#[derive(Clone, Debug)]
pub struct VerifiedToken {
    pub subject: String,
    pub jti: String,
}

pub trait Verifier: Send + Sync {
    fn verify<'a>(
        &'a self,
        token: &'a str,
    ) -> impl Future<Output = Result<VerifiedToken, AuthnError>> + Send + 'a;
}
