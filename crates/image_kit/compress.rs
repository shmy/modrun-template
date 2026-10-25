use crate::error::ImageKitError;
use image::GenericImageView;
use image::ImageError;
use image::ImageReader;
use std::io::Cursor;
use webp::Encoder;

const DEFAULT_MAX_DIMENSION: u32 = 8000;

#[derive(Debug, Clone, Copy)]
pub struct CompressOptions {
    pub quality: u8,
    pub max_dimension: u32,
}

impl Default for CompressOptions {
    fn default() -> Self {
        Self {
            quality: 75,
            max_dimension: DEFAULT_MAX_DIMENSION,
        }
    }
}

#[derive(Debug)]
pub struct CompressOutput {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub size: u64,
}

pub struct ImageKit;

impl ImageKit {
    #[tracing::instrument(skip_all)]
    pub fn compress_to_webp(
        input: &[u8],
        options: CompressOptions,
    ) -> Result<CompressOutput, ImageKitError> {
        let (width, height) = ImageReader::new(Cursor::new(input))
            .with_guessed_format()
            .map_err(|err| ImageKitError::Decode(ImageError::IoError(err)))?
            .into_dimensions()
            .map_err(ImageKitError::Decode)?;
        if width > options.max_dimension || height > options.max_dimension {
            return Err(ImageKitError::DimensionsExceeded {
                width,
                height,
                max_dimension: options.max_dimension,
            });
        }

        let image = image::load_from_memory(input).map_err(ImageKitError::Decode)?;
        let (width, height) = image.dimensions();
        let rgba = image.to_rgba8();
        let encoder = Encoder::from_rgba(rgba.as_raw(), width, height);
        let bytes = encoder.encode(f32::from(options.quality)).to_vec();
        let size = bytes.len() as u64;
        Ok(CompressOutput {
            bytes,
            width,
            height,
            size,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ExtendedColorType;
    use image::ImageFormat;
    use image::codecs::jpeg::JpegEncoder;
    use image::{ImageBuffer, ImageEncoder, Rgb};
    use std::io::Cursor;

    fn create_jpeg(width: u32, height: u32) -> Vec<u8> {
        let img: ImageBuffer<Rgb<u8>, Vec<u8>> =
            ImageBuffer::from_fn(width, height, |_, _| Rgb([230, 120, 32]));
        let mut out = Vec::new();
        let mut cursor = Cursor::new(&mut out);
        let encoder = JpegEncoder::new_with_quality(&mut cursor, 85);
        encoder
            .write_image(img.as_raw(), width, height, ExtendedColorType::Rgb8)
            .expect("encode jpeg fixture");
        out
    }

    #[test]
    fn convert_jpeg_to_webp() {
        let source = create_jpeg(120, 80);
        let out = ImageKit::compress_to_webp(&source, CompressOptions::default())
            .expect("convert to webp");
        let format = image::guess_format(&out.bytes).expect("guess format");
        assert_eq!(format, ImageFormat::WebP);
        assert_eq!((out.width, out.height), (120, 80));
    }

    #[test]
    fn keep_image_size_unchanged() {
        let source = create_jpeg(2400, 1200);
        let out = ImageKit::compress_to_webp(
            &source,
            CompressOptions {
                quality: 70,
                ..Default::default()
            },
        )
        .expect("compress");
        assert_eq!((out.width, out.height), (2400, 1200));
    }

    #[test]
    fn invalid_input_returns_decode_error() {
        let err = ImageKit::compress_to_webp(b"not an image", CompressOptions::default())
            .expect_err("should fail");
        assert!(matches!(err, ImageKitError::Decode(_)));
    }

    #[test]
    fn oversized_dimensions_rejected_before_decode() {
        let source = create_jpeg(120, 80);
        let err = ImageKit::compress_to_webp(
            &source,
            CompressOptions {
                quality: 75,
                max_dimension: 100,
            },
        )
        .expect_err("should fail");
        assert!(matches!(
            err,
            ImageKitError::DimensionsExceeded {
                width: 120,
                height: 80,
                max_dimension: 100,
            }
        ));
    }
}
