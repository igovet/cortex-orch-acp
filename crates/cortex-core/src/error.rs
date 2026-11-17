use thiserror::Error;
pub type CoreResult<T> = Result<T, CoreError>;
/// Public codes are value-free. Underlying SQL/IO details stay inside the owner.
#[derive(Debug, Error)]
pub enum CoreError {
    #[error("{0}")]
    Refused(&'static str),
    #[error("storage_unavailable")]
    Sql(#[from] rusqlite::Error),
    #[error("storage_unavailable")]
    Io(#[from] std::io::Error),
    #[error("invalid_json")]
    Json(#[from] serde_json::Error),
}
impl CoreError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Refused(code) => code,
            Self::Sql(rusqlite::Error::SqliteFailure(e, _))
                if matches!(
                    e.code,
                    rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                ) =>
            {
                "busy"
            }
            Self::Sql(_) | Self::Io(_) => "storage_unavailable",
            Self::Json(_) => "invalid_json",
        }
    }
}
pub(crate) fn refuse<T>(code: &'static str) -> CoreResult<T> {
    Err(CoreError::Refused(code))
}
