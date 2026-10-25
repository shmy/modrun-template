use file_contract::{FileError, L10nKeyedError};
use rootcause::Report;
use web_error::{WebError, register_domain_mapper};

pub fn register_file_domain_mapper() {
    register_domain_mapper(map_file_error);
}

fn map_file_error(report: &Report) -> Option<WebError> {
    report
        .downcast_current_context::<FileError>()
        .map(|err| WebError::L10n(err.l10n_key().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_error_maps_via_domain_mapper() {
        let report = Report::from(FileError::NotImage);
        let err = map_file_error(&report).unwrap();
        assert!(matches!(err, WebError::L10n(key) if key == "file_not_image"));
    }
}
