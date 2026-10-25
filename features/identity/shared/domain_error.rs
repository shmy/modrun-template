use identity_contract::{IdentityError, L10nKeyedError};
use rootcause::Report;
use web_error::{WebError, register_domain_mapper};

pub fn register_identity_domain_mapper() {
    register_domain_mapper(map_identity_error);
}

fn map_identity_error(report: &Report) -> Option<WebError> {
    report
        .downcast_current_context::<IdentityError>()
        .map(|err| WebError::L10n(err.l10n_key().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_error_maps_via_domain_mapper() {
        let report = Report::from(IdentityError::AccountSearchQTooShort);
        let err = map_identity_error(&report).unwrap();
        assert!(matches!(
            err,
            WebError::L10n(key) if key == "account_search_q_too_short"
        ));
    }
}
