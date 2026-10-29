use serde::Serialize;

use crate::Id;

#[derive(Clone, Debug, Serialize)]
pub struct CursorPagingResult<T>
where
    T: Serialize,
{
    pub items: Vec<T>,
    pub next_cursor: Option<Id>,
}
