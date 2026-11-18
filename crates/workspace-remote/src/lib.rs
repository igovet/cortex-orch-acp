//! Embedded remote access between a phone and the Cortex Workspace machine service.
//!
//! The crate has two halves that share one wire protocol over [iroh] QUIC connections:
//!
//! - [`Host`] runs inside the desktop machine service. It owns the machine's iroh
//!   identity, the paired-device registry, pairing and the RPC tunnel. It knows nothing
//!   about the service's Unix socket: every tunnelled stream gets a fresh local
//!   connection from an injected [`LocalConnector`], and every JSON-RPC request is
//!   checked by an injected [`MethodPolicy`].
//! - [`Client`] runs in the phone app. It owns the device identity and the list of
//!   paired machines, pairs through a [pairing link](link), and opens tunnelled
//!   streams ([`RemoteStream`]) that carry the service's ordinary framed JSON-RPC.
//!
//! Neither half uses n0 infrastructure: relays are either disabled or exactly the one
//! relay URL configured for the machine, and no DNS/pkarr address lookup is enabled.
//! Peers are authenticated by their endpoint ids through iroh's TLS handshake,
//! independently of the relay.
//!
//! # Protocols
//!
//! Two ALPNs are used:
//!
//! - [`PAIRING_ALPN`]: the device opens one bidirectional stream and sends one frame
//!   `{"secret":"<base64url, 32 bytes>","name":"<device name>"}`. The host answers with
//!   status frames `{"status":"…"}`: first `pending` (secret accepted, waiting for the
//!   owner) or a terminal `rejected` (wrong, consumed or no secret), `expired` or
//!   `invalid` (malformed request); after `pending`, exactly one of `allowed`,
//!   `denied`, `expired` or `cancelled`. Nothing about the machine besides these
//!   statuses is sent before `allowed`.
//! - [`RPC_ALPN`]: the host first rejects devices that are not in the registry by
//!   closing the connection with [`CLOSE_NOT_PAIRED`]. For a paired device it opens one
//!   unidirectional *hello* stream carrying one frame
//!   `{"name":"…","relay":"<url>"|null,"addrs":["ip:port",…]}` so the phone can refresh
//!   what it stored at pairing. Every bidirectional stream the device opens is then
//!   bridged to one fresh local service connection. Client→service frames are parsed
//!   (bounded by [`workspace_protocol::FRAME_BYTES`]); each request's `method`/`params`
//!   go through the method policy, and a refused request is answered by the host with
//!   `{"jsonrpc":"2.0","id":<same id>,"error":{"code":-32000,"message":"remote_operation_refused"}}`
//!   and not forwarded. A malformed or oversized frame resets the stream and drops the
//!   local connection without forwarding anything of it. Service→client frames are
//!   forwarded unchanged.
//!
//! All frames are the service's own framing: a 4-byte big-endian length followed by
//! that many bytes of JSON ([`workspace_protocol::encode_frame`]).
//!
//! # Limits
//!
//! See [`PAIRING_TTL`], [`PAIRING_ATTEMPTS`], [`MAX_CONNECTED_DEVICES`],
//! [`MAX_CONNECTIONS_PER_DEVICE`], [`MAX_STREAMS_PER_DEVICE`] and
//! [`MAX_PAIRING_CONNECTIONS`].
//!
//! # Built-in default relay
//!
//! The one relay URL both the desktop machine service and the phone app use when the
//! owner has not set an override lives in `crates/workspace-remote/default-relay.txt`
//! and is compiled in ([`default_relay`]). Put exactly one `http(s)://` URL there
//! (surrounding whitespace is ignored); an empty file means no built-in relay, so only
//! direct paths are tried until the owner sets one. A malformed value fails this
//! crate's unit tests.

#![cfg_attr(not(unix), allow(unused))]

#[cfg(not(unix))]
compile_error!("workspace-remote stores private state with Unix permissions and supports Unix targets only (Linux, Android)");

mod client;
mod files;
mod frames;
mod host;
pub mod link;

use std::net::SocketAddr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use iroh::endpoint::{presets, BindOpts, VarInt};
use iroh::{Endpoint, RelayMode, RelayUrl, SecretKey};

pub use client::{
    Client, ClientConfig, ConnectError, ConnectionState, Machine, PairError, PairingAttempt,
    PathKind, RemoteConnection, RemoteStream,
};
pub use host::{
    BoxFuture, DeviceStatus, Host, HostConfig, HostStatus, LocalConnector, LocalIo, LocalStream,
    MethodPolicy, PairingStatus, PendingRequest, RelaySource, RelayStatus, REFUSED_MESSAGE,
};
pub use link::{fingerprint, LinkError, PairingLink};

/// ALPN of the pairing protocol.
pub const PAIRING_ALPN: &[u8] = b"cortex-workspace/pair/1";
/// ALPN of the RPC tunnel.
pub const RPC_ALPN: &[u8] = b"cortex-workspace/rpc/1";

/// Lifetime of a pairing secret, and the ceiling for the owner's decision on a pending
/// request (five minutes). [`HostConfig::pairing_ttl`] overrides it (tests).
pub const PAIRING_TTL: Duration = Duration::from_secs(300);
/// Wrong secrets tolerated per pairing; the fifth wrong attempt cancels the pairing.
pub const PAIRING_ATTEMPTS: u32 = 5;
/// Distinct paired devices connected on the RPC ALPN at once. A further device is
/// closed with [`CLOSE_BUSY`].
pub const MAX_CONNECTED_DEVICES: usize = 8;
/// Connections one device may hold at once (a network change briefly overlaps the old
/// and the new connection). A further connection replaces the oldest one.
pub const MAX_CONNECTIONS_PER_DEVICE: usize = 2;
/// Tunnelled streams one device may have open at once, across its connections. It is
/// also the QUIC bidirectional stream limit per connection; a stream beyond the
/// device-wide bound is reset with [`STREAM_BUSY`].
pub const MAX_STREAMS_PER_DEVICE: u32 = 16;
/// Pairing connections served at once; more are closed with [`CLOSE_BUSY`].
pub const MAX_PAIRING_CONNECTIONS: usize = 4;
/// Incoming handshakes in progress at once; more are refused before the handshake.
pub const MAX_HANDSHAKES: usize = 32;
/// Bound on a pairing or hello frame.
pub const CONTROL_FRAME_BYTES: usize = 4096;
/// Bound on handshakes and on each pairing/hello exchange step.
pub const IO_TIMEOUT: Duration = Duration::from_secs(10);

/// Connection close code: the device is not (or no longer) paired.
pub const CLOSE_NOT_PAIRED: u32 = 0x10;
/// Connection close code: a connection or device bound was reached.
pub const CLOSE_BUSY: u32 = 0x11;
/// Connection close code: the device was revoked or remote access was turned off.
pub const CLOSE_REVOKED: u32 = 0x12;
/// Connection close code: replaced by a newer connection of the same device.
pub const CLOSE_REPLACED: u32 = 0x13;
/// Connection close code: normal close (pairing finished, client closed).
pub const CLOSE_DONE: u32 = 0;
/// Stream reset code: malformed or oversized frame.
pub const STREAM_INVALID_FRAME: u32 = 0x20;
/// Stream reset code: the local service could not be reached or failed.
pub const STREAM_LOCAL_FAILED: u32 = 0x21;
/// Stream reset code: the per-device stream bound was reached.
pub const STREAM_BUSY: u32 = 0x22;

/// Errors of state, configuration and lifecycle operations on either half.
#[derive(Debug)]
pub enum RemoteError {
    /// An I/O failure reading or writing private state.
    Io(std::io::Error),
    /// A state file or directory failed the private-file rules (symlink, foreign owner,
    /// group/other permission bits, wrong size or content). The text names the path and
    /// the problem; it never contains key material.
    InsecureState(String),
    /// A name or relay URL is out of bounds or malformed.
    InvalidConfig(&'static str),
    /// Remote access is off; there is no endpoint.
    Disabled,
    /// No pending pairing request with that id.
    UnknownRequest,
    /// No paired device with that id.
    UnknownDevice,
    /// No paired machine with that id.
    UnknownMachine,
    /// The iroh endpoint could not be bound.
    Bind(String),
}

impl std::fmt::Display for RemoteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RemoteError::Io(e) => write!(f, "remote_state_io: {e}"),
            RemoteError::InsecureState(why) => write!(f, "remote_state_refused: {why}"),
            RemoteError::InvalidConfig(why) => write!(f, "invalid_remote_config: {why}"),
            RemoteError::Disabled => f.write_str("remote_access_disabled"),
            RemoteError::UnknownRequest => f.write_str("unknown_pairing_request"),
            RemoteError::UnknownDevice => f.write_str("unknown_device"),
            RemoteError::UnknownMachine => f.write_str("unknown_machine"),
            RemoteError::Bind(why) => write!(f, "remote_bind_failed: {why}"),
        }
    }
}

impl std::error::Error for RemoteError {}

impl From<std::io::Error> for RemoteError {
    fn from(e: std::io::Error) -> Self {
        RemoteError::Io(e)
    }
}

/// The built-in default relay text (see the crate docs).
const DEFAULT_RELAY_FILE: &str = include_str!("../default-relay.txt");

/// The built-in default relay URL shared by the desktop and the phone, or `None` when
/// none is built in (`default-relay.txt` is empty).
pub fn default_relay() -> Option<&'static str> {
    let text = DEFAULT_RELAY_FILE.trim();
    (!text.is_empty()).then_some(text)
}

pub(crate) fn random_bytes(out: &mut [u8]) {
    getrandom::fill(out).expect("operating system random source unavailable");
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub(crate) fn code(value: u32) -> VarInt {
    VarInt::from_u32(value)
}

/// Binds an endpoint that never uses n0 relays or n0 address lookup.
///
/// `relay`: `None` disables relays; `Some(url)` uses exactly that relay. `binds`:
/// explicit socket addresses with their "required" flag, or empty for iroh's default
/// (`0.0.0.0:0` required, `[::]:0` optional).
pub(crate) async fn bind_endpoint(
    key: &SecretKey,
    relay: Option<&RelayUrl>,
    binds: &[(SocketAddr, bool)],
    alpns: Vec<Vec<u8>>,
) -> Result<Endpoint, RemoteError> {
    let mode = match relay {
        None => RelayMode::Disabled,
        Some(url) => RelayMode::custom([url.clone()]),
    };
    let mut builder = Endpoint::builder(presets::Minimal)
        .secret_key(key.clone())
        .relay_mode(mode)
        .clear_address_lookup()
        .alpns(alpns);
    if !binds.is_empty() {
        builder = builder.clear_ip_transports();
        for (addr, required) in binds {
            builder = builder
                .bind_addr_with_opts(*addr, BindOpts::default().set_is_required(*required))
                .map_err(|e| RemoteError::Bind(e.to_string()))?;
        }
    }
    builder
        .bind()
        .await
        .map_err(|e| RemoteError::Bind(e.to_string()))
}

/// The endpoint's direct addresses, waiting briefly for the first interface scan.
pub(crate) async fn direct_addrs(endpoint: &Endpoint) -> Vec<SocketAddr> {
    use iroh::Watcher;
    let mut watcher = endpoint.watch_addr();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let addrs: Vec<SocketAddr> = watcher.get().ip_addrs().copied().collect();
        if !addrs.is_empty() {
            return addrs;
        }
        match tokio::time::timeout_at(deadline, watcher.updated()).await {
            Ok(Ok(_)) => continue,
            _ => return addrs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The compiled-in default relay is either absent or a relay URL the host and the
    /// phone both accept; a malformed `default-relay.txt` fails here.
    #[test]
    fn built_in_default_relay_is_absent_or_valid() {
        if let Some(url) = default_relay() {
            assert!(
                link::parse_relay(url).is_some(),
                "default-relay.txt does not hold one http(s) relay URL"
            );
            assert!(!url.contains(char::is_whitespace));
        } else {
            assert!(DEFAULT_RELAY_FILE.trim().is_empty());
        }
    }
}
