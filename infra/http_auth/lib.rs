#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )
)]

pub mod extractor;
pub mod middleware;
mod token;

pub use extractor::AuthedAccount;
