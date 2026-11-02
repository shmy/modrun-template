use crate::error::ImageKitError;
use image::GenericImageView;
use image::ImageError;
use image::ImageReader;
use image::Limits;
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};
use webp::Encoder;

const DEFAULT_MAX_DIMENSION: u32 = 8000;

/// 默认最大总像素数（16,777,216 = 4096×4096）。
///
/// **仅限制边长不够**：8000×8000 合法但会解出 64M 像素，RGBA 中间态约 256MB，
/// 实测单次解码峰值 RSS 达 742MB、debug 构建需 60s+。一个 ~226KB 的压缩 PNG
/// 就能触发，是典型的解压炸弹。按总像素限制可在解码前直接拒绝。
const DEFAULT_MAX_PIXELS: u64 = 4096 * 4096;

/// 每像素按 RGBA 计，再留一倍中间缓冲；作为 `Limits::max_alloc` 硬顶。
/// 即使文件头谎报小尺寸，解码器在分配阶段也会被这笔预算拦住。
const BYTES_PER_PIXEL_BUDGET: u64 = 8;

#[derive(Debug, Clone, Copy)]
pub struct CompressOptions {
    pub quality: u8,
    pub max_dimension: u32,
    /// 最大总像素数（宽 × 高）。
    pub max_pixels: u64,
}

impl Default for CompressOptions {
    fn default() -> Self {
        Self {
            quality: 75,
            max_dimension: DEFAULT_MAX_DIMENSION,
            max_pixels: DEFAULT_MAX_PIXELS,
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

fn decode_limits(options: &CompressOptions) -> Limits {
    // `Limits` 是 `#[non_exhaustive]`，只能从 Default 改字段。
    let mut limits = Limits::default();
    limits.max_image_width = Some(options.max_dimension);
    limits.max_image_height = Some(options.max_dimension);
    limits.max_alloc = Some(options.max_pixels.saturating_mul(BYTES_PER_PIXEL_BUDGET));
    limits
}

pub struct ImageKit;

impl ImageKit {
    #[tracing::instrument(skip_all)]
    pub fn compress_to_webp(
        input: &[u8],
        options: CompressOptions,
    ) -> Result<CompressOutput, ImageKitError> {
        Self::compress_to_webp_with_cancel(input, options, &AtomicBool::new(false))
    }

    /// 同 [`Self::compress_to_webp`]，但可在阶段之间响应取消。
    ///
    /// 解码本身不可中断，因此只在**解码与编码之间**检查取消标志：请求被超时
    /// 中断后，跳过最耗时的 WebP 编码。解码峰值由 [`decode_limits`] 的
    /// `max_alloc` / 边长硬顶约束——不依赖文件头是否诚实。
    #[tracing::instrument(skip_all)]
    pub fn compress_to_webp_with_cancel(
        input: &[u8],
        options: CompressOptions,
        cancelled: &AtomicBool,
    ) -> Result<CompressOutput, ImageKitError> {
        // 快路径：只读文件头。头谎报时仍可能放行，真正硬顶在下方带 Limits 的 decode。
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
        let pixels = u64::from(width) * u64::from(height);
        if pixels > options.max_pixels {
            return Err(ImageKitError::PixelsExceeded {
                width,
                height,
                max_pixels: options.max_pixels,
            });
        }
        if cancelled.load(Ordering::Relaxed) {
            return Err(ImageKitError::Cancelled);
        }

        let mut reader = ImageReader::new(Cursor::new(input))
            .with_guessed_format()
            .map_err(|err| ImageKitError::Decode(ImageError::IoError(err)))?;
        reader.limits(decode_limits(&options));
        let image = reader.decode().map_err(ImageKitError::Decode)?;

        if cancelled.load(Ordering::Relaxed) {
            return Err(ImageKitError::Cancelled);
        }

        // 解码后按真实宽高再校验一次（Limits 已挡住分配，这里保证业务错误码准确）。
        let (actual_width, actual_height) = image.dimensions();
        if actual_width != width || actual_height != height {
            tracing::debug!(
                header_width = width,
                header_height = height,
                actual_width,
                actual_height,
                "image header dimensions disagree with decoded pixels"
            );
        }
        if actual_width > options.max_dimension || actual_height > options.max_dimension {
            return Err(ImageKitError::DimensionsExceeded {
                width: actual_width,
                height: actual_height,
                max_dimension: options.max_dimension,
            });
        }
        let actual_pixels = u64::from(actual_width) * u64::from(actual_height);
        if actual_pixels > options.max_pixels {
            return Err(ImageKitError::PixelsExceeded {
                width: actual_width,
                height: actual_height,
                max_pixels: options.max_pixels,
            });
        }

        let rgba = image.to_rgba8();
        let encoder = Encoder::from_rgba(rgba.as_raw(), actual_width, actual_height);
        let bytes = encoder.encode(f32::from(options.quality)).to_vec();
        let size = bytes.len() as u64;
        Ok(CompressOutput {
            bytes,
            width: actual_width,
            height: actual_height,
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
                ..Default::default()
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

    /// 解压炸弹防线：边长合法（各 4000，均 < max_dimension）但总像素超预算时必须拒绝。
    ///
    /// 回归保护：修复前只校验边长，8000×8000 可通过并解出 64M 像素，
    /// 实测单次解码峰值 RSS 742MB。
    #[test]
    fn oversized_pixels_rejected_before_decode() {
        let source = create_jpeg(4000, 4000);
        let err = ImageKit::compress_to_webp(
            &source,
            CompressOptions {
                // 边长上限放宽，确保是像素预算而非边长命中
                max_dimension: 10_000,
                max_pixels: 1_000_000,
                ..Default::default()
            },
        )
        .expect_err("should reject by pixel budget");
        assert!(matches!(
            err,
            ImageKitError::PixelsExceeded {
                width: 4000,
                height: 4000,
                max_pixels: 1_000_000,
            }
        ));
        assert!(err.is_client_error(), "oversized input is a 400, not a 500");
    }

    /// `Limits::max_alloc` 硬顶：即便跳过（或绕过）文件头预检，解码分配也不能超过预算。
    ///
    /// 用极小的 `max_pixels` 推导出的 `max_alloc`，对一张头尺寸看似「边长合法」的
    /// 图强制走 Limits 拒绝路径（这里直接测 `decode_limits` + `ImageReader::decode`，
    /// 避免被快路径 `into_dimensions` 抢先以 `PixelsExceeded` 返回）。
    #[test]
    fn decoder_limits_reject_before_full_alloc() {
        let source = create_jpeg(800, 800);
        let options = CompressOptions {
            max_dimension: 10_000,
            // 800×800=640_000 > 100_000 → 若走快路径会是 PixelsExceeded；
            // 这里单独验证 Limits 路径。
            max_pixels: 100_000,
            ..Default::default()
        };
        let mut reader = ImageReader::new(Cursor::new(&source))
            .with_guessed_format()
            .expect("guess");
        reader.limits(decode_limits(&options));
        let err = reader
            .decode()
            .expect_err("limits must reject oversized decode");
        assert!(
            matches!(err, ImageError::Limits(_)),
            "expected Limits error, got {err:?}"
        );
    }

    /// 默认预算下 4096×4096 仍可处理（不误伤常见大图）。
    #[test]
    fn default_budget_allows_4096_square() {
        let source = create_jpeg(4096, 4096);
        let out = ImageKit::compress_to_webp(&source, CompressOptions::default())
            .expect("4096x4096 should fit default budget");
        assert_eq!((out.width, out.height), (4096, 4096));
    }

    /// 已取消的请求应在阶段之间提前退出，不进入耗时解码。
    #[test]
    fn cancelled_request_aborts_before_decode() {
        let source = create_jpeg(120, 80);
        let cancelled = AtomicBool::new(true);
        let err =
            ImageKit::compress_to_webp_with_cancel(&source, CompressOptions::default(), &cancelled)
                .expect_err("cancelled request should abort");
        assert!(matches!(err, ImageKitError::Cancelled));
        assert!(
            !err.is_client_error(),
            "cancellation is not a client 4xx; it is dropped before responding"
        );
    }
}
