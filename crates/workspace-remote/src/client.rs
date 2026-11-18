//! The client half: runs in the phone app.
//!
//! # State directory
//!
//! [`ClientConfig::state_dir`] holds two private (0600) files:
//!
//! - `remote-device.key`: the 32-byte iroh secret key (the device's endpoint id),
//! - `remote-machines.json`: the paired machines.

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll};
use std::time::Duration;

use iroh::endpoint::{Connection, ConnectionError, RecvStream, SendStream};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayUrl, SecretKey, TransportAddr};
use n0_future::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::watch;

use crate::link::{self, LinkError, PairingLink};
use crate::{
    code, files, frames, now_ms, RemoteError, CLOSE_DONE, CLOSE_NOT_PAIRED, CLOSE_REVOKED,
    CONTROL_FRAME_BYTES, PAIRING_ALPN, RPC_ALPN,
};

const KEY_FILE: &str = "remote-device.key";
const MACHINES_FILE: &str = "remote-machines.json";
/// Upper bound the client waits for the owner's decision; the host's own ceiling
/// ([`crate::PAIRING_TTL`]) normally ends the wait first.
const DECISION_WAIT: Duration = Duration::from_secs(15 * 60);

/// Configuration of a [`Client`].
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Private directory for the device key and machine list (created 0700 if absent).
    pub state_dir: PathBuf,
    /// Explicit bind addresses; empty (the default) binds `0.0.0.0:0` and `[::]:0`.
    pub bind_addrs: Vec<SocketAddr>,
    /// Bound on establishing a connection and receiving the host's first answer.
    pub connect_timeout: Duration,
}

impl ClientConfig {
    /// Default bind addresses and a 15-second connect timeout.
    pub fn new(state_dir: impl Into<PathBuf>) -> Self {
        ClientConfig {
            state_dir: state_dir.into(),
            bind_addrs: Vec::new(),
            connect_timeout: Duration::from_secs(15),
        }
    }
}

/// A paired machine as stored on the device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Machine {
    /// The host's endpoint id (lowercase hex).
    pub machine_id: String,
    /// Display name (from the link; editable with [`Client::rename`]).
    pub name: String,
    /// The host's relay URL, from the link or the latest hello.
    pub relay_url: Option<String>,
    /// The host's last known direct addresses.
    pub addrs: Vec<SocketAddr>,
    /// Unix milliseconds.
    pub paired_at_ms: u64,
    /// Unix milliseconds of the last successful connection.
    pub last_connected_ms: Option<u64>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct MachineList {
    machines: Vec<Machine>,
}

/// Why pairing failed.
#[derive(Debug)]
pub enum PairError {
    /// The link is malformed or of an unknown version.
    InvalidLink(LinkError),
    /// The device name is empty, longer than 64 bytes or contains control characters.
    InvalidDeviceName,
    /// The host could not be reached, or the connection was lost before an answer.
    Unreachable,
    /// The host refused the secret (wrong, already used, or no pairing offer active).
    SecretRejected,
    /// The secret or the wait for the owner's decision expired.
    Expired,
    /// The owner denied the request.
    Denied,
    /// The host withdrew the request (remote access turned off or relay changed).
    Cancelled,
    /// The host answered something this build does not understand.
    Protocol,
    /// Local state could not be read or written.
    State(RemoteError),
}

impl std::fmt::Display for PairError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PairError::InvalidLink(e) => e.fmt(f),
            PairError::InvalidDeviceName => f.write_str("invalid_device_name"),
            PairError::Unreachable => f.write_str("machine_unreachable"),
            PairError::SecretRejected => f.write_str("pairing_secret_rejected"),
            PairError::Expired => f.write_str("pairing_expired"),
            PairError::Denied => f.write_str("pairing_denied"),
            PairError::Cancelled => f.write_str("pairing_cancelled"),
            PairError::Protocol => f.write_str("pairing_protocol_error"),
            PairError::State(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for PairError {}

/// Why a connection or stream could not be established.
#[derive(Debug)]
pub enum ConnectError {
    /// No paired machine with that id (or it was forgotten).
    UnknownMachine,
    /// The machine could not be reached.
    Unreachable,
    /// The machine refused this device: it is not paired there (revoked), or the
    /// connection was revoked.
    Rejected,
    /// The machine is busy (device or connection bound reached).
    Busy,
    /// The connection object was closed.
    Closed,
    /// The machine answered something this build does not understand.
    Protocol,
    /// Local state could not be read or written.
    State(RemoteError),
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectError::UnknownMachine => f.write_str("unknown_machine"),
            ConnectError::Unreachable => f.write_str("machine_unreachable"),
            ConnectError::Rejected => f.write_str("device_rejected"),
            ConnectError::Busy => f.write_str("machine_busy"),
            ConnectError::Closed => f.write_str("connection_closed"),
            ConnectError::Protocol => f.write_str("remote_protocol_error"),
            ConnectError::State(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for ConnectError {}

/// The transmission path of a live connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PathKind {
    /// A direct UDP path is selected.
    Direct,
    /// The relay path is selected.
    Relay,
    /// No path is currently selected (or it is neither).
    Unknown,
}

/// The watchable state of a [`RemoteConnection`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "state")]
pub enum ConnectionState {
    /// A connection attempt is in progress.
    Connecting,
    /// Connected and authorized (the hello was received).
    Connected {
        /// The currently selected path.
        path: PathKind,
    },
    /// Not connected; the next [`RemoteConnection::open`] reconnects.
    Offline {
        /// Unix milliseconds of the last successful connection, if any.
        last_connected_ms: Option<u64>,
    },
    /// The machine refused this device (not paired or revoked).
    Rejected,
}

struct ClientInner {
    config: ClientConfig,
    key: SecretKey,
    id: EndpointId,
    machines_path: PathBuf,
    machines: Mutex<Vec<Machine>>,
    writes: Mutex<()>,
    connections: Mutex<HashMap<EndpointId, Weak<Shared>>>,
}

/// The client half of remote access.
#[derive(Clone)]
pub struct Client {
    inner: Arc<ClientInner>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl ClientInner {
    fn persist(&self) -> Result<(), RemoteError> {
        let _order = lock(&self.writes);
        let list = MachineList {
            machines: lock(&self.machines).clone(),
        };
        files::write_json(&self.machines_path, &list)
    }

    fn machine(&self, id: &EndpointId) -> Option<Machine> {
        let id = id.to_string();
        lock(&self.machines)
            .iter()
            .find(|m| m.machine_id == id)
            .cloned()
    }

    async fn endpoint(&self, relay: Option<&RelayUrl>) -> Result<Endpoint, RemoteError> {
        let binds: Vec<_> = self.config.bind_addrs.iter().map(|a| (*a, true)).collect();
        crate::bind_endpoint(&self.key, relay, &binds, Vec::new()).await
    }
}

fn host_addr(id: EndpointId, relay: Option<&RelayUrl>, addrs: &[SocketAddr]) -> EndpointAddr {
    EndpointAddr::from_parts(
        id,
        relay
            .cloned()
            .map(TransportAddr::Relay)
            .into_iter()
            .chain(addrs.iter().copied().map(TransportAddr::Ip)),
    )
}

fn status_of(value: &Value) -> Option<&str> {
    value
        .as_object()
        .filter(|o| o.len() == 1)?
        .get("status")?
        .as_str()
}

impl Client {
    /// Opens the state directory, creating the device key on first use.
    pub fn new(config: ClientConfig) -> Result<Client, RemoteError> {
        files::ensure_private_dir(&config.state_dir)?;
        let key = files::load_or_create_key(&config.state_dir.join(KEY_FILE))?;
        let machines_path = config.state_dir.join(MACHINES_FILE);
        let list: MachineList = files::read_json(&machines_path)?.unwrap_or_default();
        Ok(Client {
            inner: Arc::new(ClientInner {
                config,
                id: key.public(),
                key,
                machines_path,
                machines: Mutex::new(list.machines),
                writes: Mutex::new(()),
                connections: Mutex::new(HashMap::new()),
            }),
        })
    }

    /// The device's endpoint id (lowercase hex).
    pub fn device_id(&self) -> String {
        self.inner.id.to_string()
    }

    /// The paired machines.
    pub fn machines(&self) -> Vec<Machine> {
        lock(&self.inner.machines).clone()
    }

    /// Renames a paired machine locally.
    pub fn rename(&self, machine_id: &str, name: &str) -> Result<(), RemoteError> {
        let name = link::validate_name(name).ok_or(RemoteError::InvalidConfig("machine name"))?;
        {
            let mut machines = lock(&self.inner.machines);
            let machine = machines
                .iter_mut()
                .find(|m| m.machine_id == machine_id)
                .ok_or(RemoteError::UnknownMachine)?;
            machine.name = name;
        }
        self.inner.persist()
    }

    /// Closes any connection to the machine and deletes its entry.
    pub async fn forget(&self, machine_id: &str) -> Result<(), RemoteError> {
        {
            let mut machines = lock(&self.inner.machines);
            let before = machines.len();
            machines.retain(|m| m.machine_id != machine_id);
            if machines.len() == before {
                return Err(RemoteError::UnknownMachine);
            }
        }
        self.inner.persist()?;
        let shared = machine_id
            .parse::<EndpointId>()
            .ok()
            .and_then(|id| lock(&self.inner.connections).remove(&id))
            .and_then(|w| w.upgrade());
        if let Some(shared) = shared {
            shared.close().await;
        }
        Ok(())
    }

    /// Validates `link`, connects to the host on the pairing ALPN and presents the
    /// secret and `device_name`. On success the host waits for its owner; compare
    /// [`PairingAttempt::fingerprint`] with the one the desktop shows, then await
    /// [`PairingAttempt::finish`]. Dropping the attempt closes the connection.
    pub async fn pair_begin(
        &self,
        link_text: &str,
        device_name: &str,
    ) -> Result<PairingAttempt, PairError> {
        let link = PairingLink::decode(link_text).map_err(PairError::InvalidLink)?;
        let raw = device_name;
        let device_name = link::validate_name(raw).ok_or(PairError::InvalidDeviceName)?;
        let endpoint = self
            .inner
            .endpoint(link.relay.as_ref())
            .await
            .map_err(PairError::State)?;
        let timeout = self.inner.config.connect_timeout;
        let begun = tokio::time::timeout(timeout, async {
            let addr = host_addr(link.host_id, link.relay.as_ref(), &link.addrs);
            let conn = endpoint
                .connect(addr, PAIRING_ALPN)
                .await
                .map_err(|_| PairError::Unreachable)?;
            let (mut send, mut recv) = conn.open_bi().await.map_err(|_| PairError::Unreachable)?;
            let request = json!({
                "secret": data_encoding::BASE64URL_NOPAD.encode(&link.secret),
                "name": device_name,
            });
            frames::write_json(&mut send, &request)
                .await
                .map_err(|_| PairError::Unreachable)?;
            let _ = send.finish();
            let answer = frames::read_json(&mut recv, CONTROL_FRAME_BYTES)
                .await
                .map_err(|_| PairError::Unreachable)?;
            Ok::<_, PairError>((conn, recv, answer))
        })
        .await;
        let (conn, recv, answer) = match begun {
            Ok(Ok(begun)) => begun,
            Ok(Err(e)) => {
                endpoint.close().await;
                return Err(e);
            }
            Err(_) => {
                endpoint.close().await;
                return Err(PairError::Unreachable);
            }
        };
        let error = match answer.as_ref().and_then(status_of) {
            Some("pending") => None,
            Some("rejected") => Some(PairError::SecretRejected),
            Some("expired") => Some(PairError::Expired),
            Some("invalid") => Some(PairError::Protocol),
            None => Some(PairError::Unreachable),
            Some(_) => Some(PairError::Protocol),
        };
        if let Some(error) = error {
            conn.close(code(CLOSE_DONE), b"done");
            endpoint.close().await;
            return Err(error);
        }
        Ok(PairingAttempt {
            machine_id: link.host_id.to_string(),
            machine_name: link.machine_name.clone(),
            fingerprint: link::fingerprint(&link.host_id, &self.inner.id, &link.secret),
            live: Some(AttemptLive {
                client: self.inner.clone(),
                endpoint,
                conn,
                recv,
                link,
            }),
        })
    }

    /// A connection object for a paired machine that connects on first
    /// [`RemoteConnection::open`] (no network activity yet). Repeated calls for the same
    /// machine return the same underlying connection.
    pub fn connection(&self, machine_id: &str) -> Result<RemoteConnection, ConnectError> {
        let id: EndpointId = machine_id
            .parse()
            .map_err(|_| ConnectError::UnknownMachine)?;
        let machine = self
            .inner
            .machine(&id)
            .ok_or(ConnectError::UnknownMachine)?;
        let mut connections = lock(&self.inner.connections);
        if let Some(shared) = connections.get(&id).and_then(Weak::upgrade) {
            if !shared.is_closed() {
                return Ok(RemoteConnection { shared });
            }
        }
        let shared = Arc::new(Shared {
            client: self.inner.clone(),
            machine: id,
            state: watch::channel(ConnectionState::Offline {
                last_connected_ms: machine.last_connected_ms,
            })
            .0,
            current: tokio::sync::Mutex::new(None),
            closed: std::sync::atomic::AtomicBool::new(false),
            generation: std::sync::atomic::AtomicU64::new(0),
        });
        connections.insert(id, Arc::downgrade(&shared));
        Ok(RemoteConnection { shared })
    }

    /// Like [`Client::connection`], then connects now; fails when the machine is
    /// unreachable or refuses this device.
    pub async fn connect(&self, machine_id: &str) -> Result<RemoteConnection, ConnectError> {
        let connection = self.connection(machine_id)?;
        connection.shared.ensure().await?;
        Ok(connection)
    }
}

struct AttemptLive {
    client: Arc<ClientInner>,
    endpoint: Endpoint,
    conn: Connection,
    recv: RecvStream,
    link: PairingLink,
}

/// A pairing request the host accepted and that waits for the owner's decision.
pub struct PairingAttempt {
    /// The host's endpoint id (lowercase hex).
    pub machine_id: String,
    /// The machine name from the link.
    pub machine_name: String,
    /// The fingerprint to compare with the desktop, e.g. `4F2A 91C7 D03E`.
    pub fingerprint: String,
    live: Option<AttemptLive>,
}

impl std::fmt::Debug for PairingAttempt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PairingAttempt")
            .field("machine_id", &self.machine_id)
            .field("machine_name", &self.machine_name)
            .field("fingerprint", &self.fingerprint)
            .finish_non_exhaustive()
    }
}

impl PairingAttempt {
    /// Waits for the owner's decision. Only `allowed` stores the machine.
    pub async fn finish(mut self) -> Result<Machine, PairError> {
        let mut live = self.live.take().expect("attempt is live until finished");
        let answer = tokio::time::timeout(
            DECISION_WAIT,
            frames::read_json(&mut live.recv, CONTROL_FRAME_BYTES),
        )
        .await;
        let result = match answer {
            Ok(Ok(Some(value))) => match status_of(&value) {
                Some("allowed") => Ok(()),
                Some("denied") => Err(PairError::Denied),
                Some("expired") => Err(PairError::Expired),
                Some("cancelled") => Err(PairError::Cancelled),
                _ => Err(PairError::Protocol),
            },
            Ok(_) => Err(PairError::Unreachable),
            Err(_) => Err(PairError::Expired),
        };
        live.conn.close(code(CLOSE_DONE), b"done");
        live.endpoint.close().await;
        result?;
        let machine = Machine {
            machine_id: live.link.host_id.to_string(),
            name: live.link.machine_name.clone(),
            relay_url: live.link.relay.as_ref().map(|r| r.to_string()),
            addrs: live.link.addrs.clone(),
            paired_at_ms: now_ms(),
            last_connected_ms: None,
        };
        {
            let mut machines = lock(&live.client.machines);
            machines.retain(|m| m.machine_id != machine.machine_id);
            machines.push(machine.clone());
        }
        live.client.persist().map_err(PairError::State)?;
        Ok(machine)
    }

    /// Abandons the attempt and closes the connection.
    pub async fn cancel(mut self) {
        if let Some(live) = self.live.take() {
            live.conn.close(code(CLOSE_DONE), b"cancelled");
            live.endpoint.close().await;
        }
    }
}

impl Drop for PairingAttempt {
    fn drop(&mut self) {
        if let Some(live) = self.live.take() {
            live.conn.close(code(CLOSE_DONE), b"cancelled");
        }
    }
}

struct Live {
    endpoint: Endpoint,
    relay: Option<RelayUrl>,
    conn: Option<Connection>,
}

struct Shared {
    client: Arc<ClientInner>,
    machine: EndpointId,
    state: watch::Sender<ConnectionState>,
    current: tokio::sync::Mutex<Option<Live>>,
    closed: std::sync::atomic::AtomicBool,
    generation: std::sync::atomic::AtomicU64,
}

impl Drop for Shared {
    // The path monitor holds a connection handle; close explicitly so dropping the last
    // `RemoteConnection` ends the QUIC connection instead of leaving it to idle out.
    fn drop(&mut self) {
        if let Some(conn) = self.current.get_mut().as_mut().and_then(|l| l.conn.take()) {
            conn.close(code(CLOSE_DONE), b"dropped");
        }
    }
}

fn selected_path(conn: &Connection) -> PathKind {
    path_kind(&conn.paths())
}

fn path_kind(paths: &iroh::endpoint::PathList<'_>) -> PathKind {
    match paths.iter().find(|p| p.is_selected()) {
        Some(p) if p.is_ip() => PathKind::Direct,
        Some(p) if p.is_relay() => PathKind::Relay,
        _ => PathKind::Unknown,
    }
}

fn refusal(error: &ConnectionError) -> Option<ConnectError> {
    match error {
        ConnectionError::ApplicationClosed(close) => {
            let value = close.error_code.into_inner();
            if value == u64::from(CLOSE_NOT_PAIRED) || value == u64::from(CLOSE_REVOKED) {
                Some(ConnectError::Rejected)
            } else if value == u64::from(crate::CLOSE_BUSY) {
                Some(ConnectError::Busy)
            } else {
                None
            }
        }
        _ => None,
    }
}

impl Shared {
    fn is_closed(&self) -> bool {
        self.closed.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn offline(&self) -> ConnectionState {
        ConnectionState::Offline {
            last_connected_ms: self
                .client
                .machine(&self.machine)
                .and_then(|m| m.last_connected_ms),
        }
    }

    async fn close(&self) {
        self.closed.store(true, std::sync::atomic::Ordering::SeqCst);
        self.generation
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let live = self.current.lock().await.take();
        if let Some(live) = live {
            if let Some(conn) = live.conn {
                conn.close(code(CLOSE_DONE), b"closed");
            }
            live.endpoint.close().await;
        }
        let offline = self.offline();
        self.state.send_replace(offline);
    }

    /// Returns the live connection, reconnecting when there is none.
    async fn ensure(self: &Arc<Self>) -> Result<Connection, ConnectError> {
        if self.is_closed() {
            return Err(ConnectError::Closed);
        }
        let mut current = self.current.lock().await;
        if let Some(conn) = current.as_ref().and_then(|l| l.conn.as_ref()) {
            if conn.close_reason().is_none() {
                return Ok(conn.clone());
            }
        }
        let Some(machine) = self.client.machine(&self.machine) else {
            return Err(ConnectError::UnknownMachine);
        };
        let relay = machine.relay_url.as_deref().and_then(link::parse_relay);
        self.state.send_replace(ConnectionState::Connecting);
        // The endpoint's relay is fixed at bind; rebind when the stored relay changed.
        if current.as_ref().is_none_or(|l| l.relay != relay) {
            if let Some(old) = current.take() {
                old.endpoint.close().await;
            }
            let endpoint = match self.client.endpoint(relay.as_ref()).await {
                Ok(endpoint) => endpoint,
                Err(e) => {
                    self.state.send_replace(self.offline());
                    return Err(ConnectError::State(e));
                }
            };
            *current = Some(Live {
                endpoint,
                relay: relay.clone(),
                conn: None,
            });
        }
        let live = current.as_mut().expect("bound above");
        live.conn = None;
        match self.dial(&live.endpoint, relay.as_ref(), &machine).await {
            Ok(conn) => {
                live.conn = Some(conn.clone());
                Ok(conn)
            }
            Err(e) => {
                let state = if matches!(e, ConnectError::Rejected) {
                    ConnectionState::Rejected
                } else {
                    self.offline()
                };
                self.state.send_replace(state);
                Err(e)
            }
        }
    }

    async fn dial(
        self: &Arc<Self>,
        endpoint: &Endpoint,
        relay: Option<&RelayUrl>,
        machine: &Machine,
    ) -> Result<Connection, ConnectError> {
        let timeout = self.client.config.connect_timeout;
        let addr = host_addr(self.machine, relay, &machine.addrs);
        let conn = tokio::time::timeout(timeout, endpoint.connect(addr, RPC_ALPN))
            .await
            .map_err(|_| ConnectError::Unreachable)?
            .map_err(|_| ConnectError::Unreachable)?;
        // The hello doubles as the host's authorization: an unpaired device is closed
        // with CLOSE_NOT_PAIRED instead.
        let hello = tokio::time::timeout(timeout, async {
            let mut uni = conn
                .accept_uni()
                .await
                .map_err(|e| refusal(&e).unwrap_or(ConnectError::Unreachable))?;
            frames::read_json(&mut uni, CONTROL_FRAME_BYTES)
                .await
                .map_err(|_| ConnectError::Protocol)?
                .ok_or(ConnectError::Protocol)
        })
        .await
        .map_err(|_| ConnectError::Unreachable)?;
        let hello = match hello {
            Ok(hello) => hello,
            Err(e) => {
                conn.close(code(CLOSE_DONE), b"done");
                return Err(e);
            }
        };
        self.apply_hello(&hello);
        let generation = self
            .generation
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        self.state.send_replace(ConnectionState::Connected {
            path: selected_path(&conn),
        });
        // Follow path changes and the end of the connection.
        let weak = Arc::downgrade(self);
        let watched = conn.clone();
        tokio::spawn(async move {
            {
                let mut paths = watched.paths_stream();
                while let Some(list) = paths.next().await {
                    let kind = path_kind(&list);
                    let Some(shared) = weak.upgrade() else { return };
                    if shared.generation.load(std::sync::atomic::Ordering::SeqCst) != generation {
                        return;
                    }
                    shared.state.send_if_modified(|s| {
                        let next = ConnectionState::Connected { path: kind };
                        let changed = *s != next;
                        *s = next;
                        changed
                    });
                }
            }
            let reason = watched.closed().await;
            let Some(shared) = weak.upgrade() else { return };
            if shared.generation.load(std::sync::atomic::Ordering::SeqCst) == generation {
                let state = if refusal(&reason).is_some_and(|e| matches!(e, ConnectError::Rejected))
                {
                    ConnectionState::Rejected
                } else {
                    shared.offline()
                };
                shared.state.send_replace(state);
            }
        });
        Ok(conn)
    }

    fn apply_hello(&self, hello: &Value) {
        let id = self.machine.to_string();
        {
            let mut machines = lock(&self.client.machines);
            let Some(machine) = machines.iter_mut().find(|m| m.machine_id == id) else {
                return;
            };
            machine.last_connected_ms = Some(now_ms());
            match hello.get("relay") {
                Some(Value::Null) => machine.relay_url = None,
                Some(Value::String(url)) if link::parse_relay(url).is_some() => {
                    machine.relay_url = Some(url.clone())
                }
                _ => {}
            }
            if let Some(addrs) = hello.get("addrs").and_then(Value::as_array) {
                let addrs: Vec<SocketAddr> = addrs
                    .iter()
                    .filter_map(|a| a.as_str()?.parse().ok())
                    .take(link::LINK_ADDRS)
                    .collect();
                if !addrs.is_empty() {
                    machine.addrs = addrs;
                }
            }
        }
        let _ = self.client.persist();
    }
}

/// A connection to one paired machine. Cheap to clone; all clones share one QUIC
/// connection, which is re-established lazily by [`RemoteConnection::open`] after loss.
#[derive(Clone)]
pub struct RemoteConnection {
    shared: Arc<Shared>,
}

impl RemoteConnection {
    /// The machine's endpoint id (lowercase hex).
    pub fn machine_id(&self) -> String {
        self.shared.machine.to_string()
    }

    /// A receiver of the connection state.
    pub fn state(&self) -> watch::Receiver<ConnectionState> {
        self.shared.state.subscribe()
    }

    /// Opens a new tunnelled stream to a fresh local service connection on the host,
    /// reconnecting first if needed. Speak the service's framed JSON-RPC over it.
    pub async fn open(&self) -> Result<RemoteStream, ConnectError> {
        for _ in 0..2 {
            let conn = self.shared.ensure().await?;
            match conn.open_bi().await {
                Ok((send, recv)) => return Ok(RemoteStream { send, recv }),
                Err(e) => {
                    if let Some(refused) = refusal(&e) {
                        return Err(refused);
                    }
                    // The connection died under us; reconnect once.
                }
            }
        }
        Err(ConnectError::Unreachable)
    }

    /// Closes the connection; later [`RemoteConnection::open`] calls fail with
    /// [`ConnectError::Closed`].
    pub async fn close(&self) {
        self.shared.close().await;
    }
}

/// One tunnelled bidirectional stream. Shutting down the write half finishes the QUIC
/// send stream (the service sees end of input); dropping the stream closes it.
pub struct RemoteStream {
    send: SendStream,
    recv: RecvStream,
}

impl RemoteStream {
    /// The underlying iroh streams.
    pub fn into_parts(self) -> (SendStream, RecvStream) {
        (self.send, self.recv)
    }
}

impl AsyncRead for RemoteStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().recv).poll_read(cx, buf)
    }
}

impl AsyncWrite for RemoteStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        AsyncWrite::poll_write(Pin::new(&mut self.get_mut().send), cx, buf)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        AsyncWrite::poll_flush(Pin::new(&mut self.get_mut().send), cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        AsyncWrite::poll_shutdown(Pin::new(&mut self.get_mut().send), cx)
    }
}
