use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApiErrorCode {
    Unauthorized,
    Forbidden,
    BadRequest,
    NotFound,
    ConflictBusy,
    ConflictStale,
    UnsupportedBackend,
    Internal,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiError {
    pub code: ApiErrorCode,
    pub message: String,
}
