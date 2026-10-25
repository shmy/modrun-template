#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )
)]

mod error;
mod response;

pub use error::{InvalidParamsKind, WebError, register_domain_mapper};
pub use response::{JsonResponseType, ProblemDetails};
