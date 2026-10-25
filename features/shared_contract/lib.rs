#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )
)]

pub mod query;
pub mod value_object;

pub use query::{CursorPagingQuery, CursorPagingResult};
pub use value_object::id::Id;
pub use value_object::phone_number::PhoneNumber;
