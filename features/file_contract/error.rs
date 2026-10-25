#[derive(Debug, thiserror::Error)]
pub enum FileError {
    #[error("file_not_image")]
    NotImage,
    #[error("file_name_missing")]
    FileNameMissing,
}

/// 域错误对应的 l10n key（与 thiserror `#[error]` 消息保持一致）。
pub trait L10nKeyedError {
    fn l10n_key(&self) -> &'static str;
}

impl L10nKeyedError for FileError {
    fn l10n_key(&self) -> &'static str {
        match self {
            Self::NotImage => "file_not_image",
            Self::FileNameMissing => "file_name_missing",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn l10n_key_matches_thiserror_display() {
        let cases = [FileError::NotImage, FileError::FileNameMissing];
        for err in cases {
            assert_eq!(err.l10n_key(), err.to_string());
        }
    }
}
