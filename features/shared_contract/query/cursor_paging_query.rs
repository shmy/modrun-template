use serde::Deserialize;
use utoipa::{IntoParams, ToSchema};

use crate::Id;

#[derive(Debug, Default, Deserialize, ToSchema, IntoParams)]
#[into_params(parameter_in = Query)]
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
