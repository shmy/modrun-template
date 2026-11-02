#[derive(Debug, thiserror::Error)]
pub enum FileError {
    #[error("file_not_image")]
    NotImage,
    #[error("file_name_missing")]
    FileNameMissing,
    /// 魔数通过但解码失败（截断 / 伪造头 / 不支持的子格式）。
    ///
    /// 与 [`Self::NotImage`] 同为 400：这是**客户端输入问题**，不是服务端故障。
    /// 若映射成 500，攻击者用 12 字节伪造头即可污染错误率指标与告警。
    #[error("file_not_image")]
    Undecodable,
}

/// 域错误对应的 l10n key（与 thiserror `#[error]` 消息保持一致）。
pub trait L10nKeyedError {
    fn l10n_key(&self) -> &'static str;
}

impl L10nKeyedError for FileError {
    fn l10n_key(&self) -> &'static str {
        match self {
            Self::NotImage | Self::Undecodable => "file_not_image",
            Self::FileNameMissing => "file_name_missing",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn l10n_key_matches_thiserror_display() {
        let cases = [
            FileError::NotImage,
            FileError::Undecodable,
            FileError::FileNameMissing,
        ];
        for err in cases {
            assert_eq!(err.l10n_key(), err.to_string());
        }
    }
}
