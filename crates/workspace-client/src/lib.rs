//! The machine-service bridge and request allowlist shared by the Cortex Workspace shells.
//!
//! The desktop shell (`cortex-workspace-desktop`) and the phone app speak the same framed
//! JSON-RPC to one machine service. This crate holds what they share and nothing that
//! links the service itself, GTK or WebKit:
//!
//! - [`policy`]: the closed allowlist a shell applies to every request its web interface
//!   sends, with the desktop profile ([`policy::authorize`]) and the stricter phone profile
//!   ([`policy::authorize_phone`]);
//! - [`Bridge`]: one fresh connection per request plus one subscription connection, the
//!   refusal texts, the subscription's event validation ([`incoming`]) and its transport
//!   events (`{"type":"connection","state":"connecting"|"connected"|"disconnected"|"unavailable"}`);
//! - [`Connector`]: how the bridge reaches the service. The desktop supplies its Unix
//!   socket; the phone supplies [`RemoteConnector`] (feature `remote`) over a paired
//!   `workspace_remote::RemoteConnection`;
//! - [`frames`]: the service's 4-byte length-prefixed JSON frames.
pub mod bridge;
pub mod frames;
pub mod policy;
#[cfg(feature = "remote")]
mod remote;

pub use bridge::{
    frame_failure, incoming, BoxFuture, Bridge, Connector, Incoming, Refused, Stream, StreamIo,
};
#[cfg(feature = "remote")]
pub use remote::RemoteConnector;
