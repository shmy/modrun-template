#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )
)]

use chrono::NaiveDate;
use chrono::Utc;
use imageformat::{ImageFormat, detect_image_format};
use rootcause::Result;
use std::io::Read;
use tempoid::TempoId;

pub mod error;

pub use error::{FileError, L10nKeyedError};

macro_rules! dir_with_prefix {
    ($dir:literal) => {
        pub const STAGING_DIR: &str = $dir;
        pub const STORAGE_DIR_PREFIX: &str = concat!($dir, "/");
    };
}

dir_with_prefix!("staging");

#[inline(always)]
fn random_file_name() -> String {
    TempoId::generate().to_string()
}

pub fn strip_staging_prefix(path: String) -> String {
    path.strip_prefix(STORAGE_DIR_PREFIX)
        .map(|s| s.to_string())
        .unwrap_or(path)
}

pub fn staging_dated_file_path(extension: &str) -> String {
    format!(
        "{}{}/{}.{}",
        STORAGE_DIR_PREFIX,
        Utc::now().format("%Y/%m"),
        random_file_name(),
        extension
    )
}

/// 解析 staging 路径中的月份（`staging/YYYY/MM/...`，目录或文件均可）；
/// 非 `staging/YYYY/MM` 结构返回 `None`（如年份目录 `staging/2025/`）。
/// 返回该月 1 日的日期，供保留期比较。
pub fn staging_dir_date(path: &str) -> Option<chrono::NaiveDate> {
    let rest = path
        .trim_end_matches('/')
        .strip_prefix(STORAGE_DIR_PREFIX)?;
    let mut parts = rest.split('/');
    let year: i32 = parts.next()?.trim().parse().ok()?;
    let month: u32 = parts.next()?.trim().parse().ok()?;
    if !(1..=12).contains(&month) {
        return None;
    }
    NaiveDate::from_ymd_opt(year, month, 1)
}

pub fn ensure_image_reader<R: Read>(reader: &mut R) -> Result<()> {
    let Ok(format) = detect_image_format(reader) else {
        return Err(FileError::NotImage.into());
    };
    match format {
        ImageFormat::Jpeg | ImageFormat::JpegXl | ImageFormat::Png | ImageFormat::Webp => Ok(()),
        _ => Err(FileError::NotImage.into()),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn staging_path_has_prefix_and_extension() {
        let path = staging_dated_file_path("webp");
        assert!(path.starts_with("staging/"));
        assert!(path.ends_with(".webp"));
    }

    #[test]
    fn staging_dir_date_parses_month_dir_and_file() {
        assert_eq!(
            staging_dir_date("staging/2025/03").unwrap(),
            NaiveDate::from_ymd_opt(2025, 3, 1).unwrap()
        );
        assert_eq!(
            staging_dir_date("staging/2025/03/x.webp").unwrap(),
            NaiveDate::from_ymd_opt(2025, 3, 1).unwrap()
        );
        assert_eq!(staging_dir_date("staging/2025/"), None);
        assert_eq!(staging_dir_date("staging/2025/13/"), None);
        assert_eq!(staging_dir_date("other/2025/03"), None);
        assert_eq!(staging_dir_date("staging/abc/def"), None);
    }

    #[test]
    fn ensure_image_accepts_png_magic() {
        let png_header = [
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D,
        ];
        let mut reader = Cursor::new(png_header);
        ensure_image_reader(&mut reader).expect("png header");
    }

    #[test]
    fn ensure_image_rejects_plain_text() {
        let mut reader = Cursor::new(b"not an image");
        let err = ensure_image_reader(&mut reader).expect_err("should reject");
        assert!(err.downcast_current_context::<FileError>().is_some());
    }
}
