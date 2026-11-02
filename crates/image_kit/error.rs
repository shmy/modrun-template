#[derive(Debug, thiserror::Error)]
pub enum ImageKitError {
    #[error(transparent)]
    Decode(image::ImageError),
    #[error(transparent)]
    Encode(image::ImageError),
    #[error("image_dimensions_exceeded")]
    DimensionsExceeded {
        width: u32,
        height: u32,
        max_dimension: u32,
    },
    /// 宽 × 高 超过像素预算（防解压炸弹：边长合法但像素总量过大）。
    #[error("image_dimensions_exceeded")]
    PixelsExceeded {
        width: u32,
        height: u32,
        max_pixels: u64,
    },
    /// 请求已被取消（客户端断开 / 请求超时），提前中止以避免继续占用 CPU。
    #[error("request_cancelled")]
    Cancelled,
}

impl ImageKitError {
    /// 是否属于「客户端输入不合法」（应映射 400）而非服务端故障（500）。
    ///
    /// 解码失败（截断 / 伪造魔数 / 不支持的子格式）与尺寸超限都是输入问题；
    /// 只有编码失败才可能是服务端故障。
    pub fn is_client_error(&self) -> bool {
        match self {
            Self::Decode(_) | Self::DimensionsExceeded { .. } | Self::PixelsExceeded { .. } => true,
            Self::Encode(_) | Self::Cancelled => false,
        }
    }
}
