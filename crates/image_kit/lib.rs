//! 图片压缩工具（JPEG/PNG → WebP）。

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )
)]

pub mod compress;
pub mod error;
