//! The phone's [`Connector`]: each bridge connection is one tunnelled stream of a paired
//! machine's [`RemoteConnection`], which reconnects by itself after a network change.
use crate::bridge::{BoxFuture, Connector, Stream};
use workspace_remote::{ConnectError, ConnectionState, RemoteConnection};

/// Reaches the machine service through a paired machine's remote connection.
#[derive(Clone)]
pub struct RemoteConnector {
    connection: RemoteConnection,
}

impl RemoteConnector {
    /// A connector over `connection` (from `workspace_remote::Client::connection`).
    pub fn new(connection: RemoteConnection) -> Self {
        Self { connection }
    }
    /// The underlying connection, for its own state and path.
    pub fn connection(&self) -> &RemoteConnection {
        &self.connection
    }
}

impl Connector for RemoteConnector {
    fn connect(&self) -> BoxFuture<'_, Result<Stream, String>> {
        Box::pin(async move {
            self.connection
                .open()
                .await
                .map(|stream| Box::new(stream) as Stream)
                .map_err(|error| match error {
                    // The bridge's own word for a service it cannot reach right now.
                    ConnectError::Unreachable | ConnectError::Busy => "service_unavailable".into(),
                    other => other.to_string(),
                })
        })
    }
    fn unavailable(&self) -> Option<String> {
        match *self.connection.state().borrow() {
            ConnectionState::Rejected => Some(
                "This phone is no longer paired with this machine. Pair it again on the machine."
                    .into(),
            ),
            _ => None,
        }
    }
}
