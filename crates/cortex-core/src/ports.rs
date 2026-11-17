//! Effect boundaries. Implementations belong to the daemon/adapters, not Store.
use crate::CoreResult;
use workspace_protocol::core::{CoreCommand, CoreQuery, MutationReceipt};
pub trait EngineDriver: Send + Sync {
    fn reconcile(&self, attempt_id: &str) -> CoreResult<ProcessObservation>;
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessObservation {
    Alive,
    ConfirmedExited,
    Unknown,
}
pub trait CommandExecutor: Send + Sync {
    fn execute(&self, command: AuthorizedCommand) -> CoreResult<CommandObservation>;
}
/// Constructed by runtime policy; deliberately not Deserialize.
pub struct AuthorizedCommand {
    pub argv: Vec<String>,
    pub cwd: std::path::PathBuf,
}
pub struct CommandObservation {
    pub complete: bool,
    pub exit_code: Option<i32>,
    pub receipt_ref: String,
}
/// Machine adapter delegates policy-bearing operations; never inspects raw DB acceptance.
pub trait CoreMachinePort: Send + Sync {
    fn query(&self, query: CoreQuery) -> CoreResult<workspace_protocol::read::ReadReply>;
    fn command(&self, command: CoreCommand) -> CoreResult<MutationReceipt>;
}
