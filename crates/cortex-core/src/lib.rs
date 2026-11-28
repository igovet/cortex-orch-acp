//! Window-independent Cortex domain foundation. No ACP/Tauri or Python runtime dependency.
//! Blocking Store methods must run on a dedicated executor, never a Tokio reactor.
pub mod access;
pub mod assignment_brief;
pub mod required_reports;
pub mod channels;
pub mod deliverables;
pub mod error;
pub mod events;
pub mod external;
pub mod input;
pub mod io_priority;
pub mod lifecycle;
pub mod observability;
pub mod ownership;
pub mod permission_decisions;
pub mod pipeline;
pub mod ports;
pub mod projections;
pub mod recovery_context;
pub mod routing;
pub mod scheduler;
pub mod store;
pub mod strategy;
#[cfg(feature = "test-observations")]
#[doc(hidden)]
pub mod test_observations;
pub mod workspace;
pub use error::{CoreError, CoreResult};
pub use store::{OpenMode, ProjectKey, Store, StoreChannel};
pub use workspace_protocol::core::{AssignmentState, TaskState};
/// UTC timestamp shared by the trusted native host and transactional domains.
pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, false)
}
pub(crate) fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}
