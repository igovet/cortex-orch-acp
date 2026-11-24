//! The phone's [`Connector`]: each bridge connection is one tunnelled stream of a paired
//! machine's [`RemoteConnection`], which reconnects by itself after a network change.
use crate::bridge::{BoxFuture, Connector, Stream};
use workspace_remote::{ConnectError, ConnectionState, RemoteConnection};

pub(crate) const UPDATE_NEEDED: &str =
    "The computer and this app are different versions. Update the app or the computer so both match.";

fn unavailable_reason(state: &ConnectionState) -> Option<&'static str> {
    match state {
        ConnectionState::Rejected => {
            Some("This phone is no longer paired with this machine. Pair it again on the machine.")
        }
        ConnectionState::IncompatibleVersion => Some(UPDATE_NEEDED),
        _ => None,
    }
}

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
        unavailable_reason(&self.connection.state().borrow()).map(str::to_owned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incompatible_and_rejected_connections_have_distinct_recovery_guidance() {
        assert_eq!(
            unavailable_reason(&ConnectionState::IncompatibleVersion),
            Some(UPDATE_NEEDED)
        );
        assert!(unavailable_reason(&ConnectionState::Rejected)
            .unwrap()
            .contains("Pair it again"));
        assert_eq!(
            unavailable_reason(&ConnectionState::Offline {
                last_connected_ms: None,
            }),
            None
        );
    }
}
