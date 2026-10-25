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
}
