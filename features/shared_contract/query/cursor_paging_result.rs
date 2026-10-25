use serde::Serialize;
use utoipa::ToSchema;

use crate::Id;

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct CursorPagingResult<T>
where
    T: Serialize + ToSchema,
{
    pub items: Vec<T>,
    pub next_cursor: Option<Id>,
}
