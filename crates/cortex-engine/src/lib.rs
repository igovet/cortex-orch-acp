//! Outgoing ACP and channel-bound MCP adapters. No Python runtime dependency.
//! The service must inject Core policy; this crate never opens a second Store.
pub mod broker;
pub mod engine;
pub mod environment;
pub mod external;
pub mod filesystem_view;
pub mod gateway;
pub mod permission;
pub mod schema;
pub mod wire;

use std::{future::Future, pin::Pin};
pub type FutureResult<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug, Clone, thiserror::Error)]
pub enum Error {
    #[error("transport_closed_outcome_unknown")]
    Closed,
    #[error("deadline_outcome_unknown")]
    Unknown,
    #[error("protocol_error")]
    Protocol,
    #[error("frame_too_large")]
    FrameTooLarge,
    #[error("invalid_arguments")]
    Invalid,
    #[error("invalid_arguments")]
    InvalidField { field: String, reason: &'static str },
    #[error("authority_refused")]
    Forbidden,
    /// An authority refusal that names its stable, secret-free refusal code
    /// (for example `scope_not_granted`). Enforcement is identical to
    /// `Forbidden`; only the reported reason is more precise.
    #[error("authority_refused")]
    Refused(&'static str),
    #[error("operation_busy_or_unreconciled")]
    Busy,
    #[error("io_error")]
    Io,
    #[error("engine_rpc_error:{0}")]
    Rpc(i64),
}
impl From<std::io::Error> for Error {
    fn from(_: std::io::Error) -> Self {
        Self::Io
    }
}
