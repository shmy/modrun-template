use serde::Deserialize;

use crate::Id;

#[derive(Debug, Default, Deserialize)]
pub struct CursorPagingQuery {
    #[serde(default)]
    pub cursor: Option<Id>,
    #[serde(default)]
    pub limit: Option<i64>,
}

impl CursorPagingQuery {
    pub fn limit(&self) -> u64 {
        self.limit.unwrap_or(10).clamp(1, 100) as u64
    }
}
