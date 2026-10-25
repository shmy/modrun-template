use identity_contract::IdentityError;
use rootcause::Report;

pub(crate) fn map_cache_err(err: Report) -> Report {
    tracing::error!(error = %err, "cache unavailable");
    Report::from(IdentityError::CacheUnavailable)
}
