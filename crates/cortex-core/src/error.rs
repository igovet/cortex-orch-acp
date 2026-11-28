use thiserror::Error;
pub type CoreResult<T> = Result<T, CoreError>;
/// Public codes are value-free. Underlying SQL/IO details stay inside the owner.
#[derive(Debug, Error)]
pub enum CoreError {
    #[error("{0}")]
    Refused(&'static str),
    #[error("external_stop_pending")]
    ExternalStopPending { role: String, reference: Option<String> },
    #[error("owner_file_lease_held")]
    OwnerFileLeaseHeld {
        path: crate::store::owner_file_leases::RelativePath,
    },
    #[error("task_file_lease_held")]
    TaskFileLeaseHeld { task: String },
    #[error("unsupported_storage")]
    UnsupportedStorageFormat10,
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
            Self::ExternalStopPending { .. } => "external_stop_pending",
            Self::OwnerFileLeaseHeld { .. } => "owner_file_lease_held",
            Self::TaskFileLeaseHeld { .. } => "task_file_lease_held",
            Self::UnsupportedStorageFormat10 => "unsupported_storage",
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
