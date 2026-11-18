//! The host half: runs inside the desktop machine service.
//!
//! # State directory
//!
//! [`HostConfig::state_dir`] holds three private (0600) files:
//!
//! - `remote-host.key`: the 32-byte iroh secret key (the machine's endpoint id),
//! - `remote-devices.json`: the paired-device registry,
//! - `remote-settings.json`: `enabled`, the relay override and the last bound port.
//!
//! Remote access is off until [`Host::set_enabled`]`(true)`; while off no endpoint is
//! bound. The flag persists across restarts.

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iroh::endpoint::{Connection, Incoming, RecvStream, SendStream};
use iroh::{Endpoint, EndpointId, RelayUrl, SecretKey};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use subtle::ConstantTimeEq;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::sync::{oneshot, watch, OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use workspace_protocol::FRAME_BYTES;

use crate::link::{self, PairingLink};
use crate::{
    code, files, frames, now_ms, random_bytes, RemoteError, CLOSE_BUSY, CLOSE_DONE,
    CLOSE_NOT_PAIRED, CLOSE_REPLACED, CLOSE_REVOKED, CONTROL_FRAME_BYTES, IO_TIMEOUT,
    MAX_CONNECTED_DEVICES, MAX_CONNECTIONS_PER_DEVICE, MAX_HANDSHAKES, MAX_PAIRING_CONNECTIONS,
    MAX_STREAMS_PER_DEVICE, PAIRING_ALPN, PAIRING_ATTEMPTS, PAIRING_TTL, RPC_ALPN, STREAM_BUSY,
    STREAM_INVALID_FRAME, STREAM_LOCAL_FAILED,
};

const KEY_FILE: &str = "remote-host.key";
const REGISTRY_FILE: &str = "remote-devices.json";
const SETTINGS_FILE: &str = "remote-settings.json";
/// The JSON-RPC error message for a request the method policy refused.
pub const REFUSED_MESSAGE: &str = "remote_operation_refused";

/// A boxed, sendable future.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// A bidirectional byte stream to the local machine service.
pub trait LocalIo: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin + ?Sized> LocalIo for T {}

/// A fresh connection to the local machine service.
pub type LocalStream = Box<dyn LocalIo>;

/// Opens one fresh local service connection per tunnelled stream.
pub type LocalConnector = Arc<dyn Fn() -> BoxFuture<io::Result<LocalStream>> + Send + Sync>;

/// Decides whether a remote device may call a JSON-RPC `method` with `params`
/// (`Value::Null` when the request has no `params`).
pub type MethodPolicy = Arc<dyn Fn(&str, &Value) -> bool + Send + Sync>;

/// Configuration of a [`Host`].
#[derive(Clone)]
pub struct HostConfig {
    /// Private directory for the key, registry and settings (created 0700 if absent).
    pub state_dir: PathBuf,
    /// The machine's display name (1..=64 bytes, no control characters), shown to the
    /// device in the pairing link and hello.
    pub machine_name: String,
    /// The default relay URL (`http(s)://…`), or `None` for no relay. A persisted
    /// override ([`Host::set_relay`]) takes precedence.
    pub default_relay: Option<String>,
    /// Opens a local service connection.
    pub connector: LocalConnector,
    /// Method policy for tunnelled JSON-RPC requests.
    pub policy: MethodPolicy,
    /// Pairing secret lifetime and owner-decision ceiling; defaults to [`PAIRING_TTL`].
    pub pairing_ttl: Duration,
    /// Explicit bind addresses. Empty (the default) binds `0.0.0.0` and `[::]` on the
    /// last persisted port (a free one the first time, or when it is taken).
    pub bind_addrs: Vec<SocketAddr>,
}

impl HostConfig {
    /// A configuration with no relay, the default TTL and default bind addresses.
    pub fn new<C, F, P>(
        state_dir: impl Into<PathBuf>,
        machine_name: impl Into<String>,
        connector: C,
        policy: P,
    ) -> Self
    where
        C: Fn() -> F + Send + Sync + 'static,
        F: Future<Output = io::Result<LocalStream>> + Send + 'static,
        P: Fn(&str, &Value) -> bool + Send + Sync + 'static,
    {
        HostConfig {
            state_dir: state_dir.into(),
            machine_name: machine_name.into(),
            default_relay: None,
            connector: Arc::new(move || Box::pin(connector())),
            policy: Arc::new(policy),
            pairing_ttl: PAIRING_TTL,
            bind_addrs: Vec::new(),
        }
    }
}

/// Where the effective relay comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RelaySource {
    /// [`HostConfig::default_relay`].
    Default,
    /// The persisted per-machine override.
    Override,
    /// No relay is configured.
    None,
}

/// The effective relay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayStatus {
    /// The relay URL, or `None` when relays are disabled.
    pub url: Option<String>,
    /// Where it comes from.
    pub source: RelaySource,
}

/// The active pairing offer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingStatus {
    /// The `cortex-pair:` link (contains the single-use secret; show only locally).
    pub link: String,
    /// QR matrix of `link`: square rows of `'1'` (dark) / `'0'` (light), no quiet zone.
    pub qr: Vec<String>,
    /// Expiry as Unix milliseconds.
    pub expires_at_ms: u64,
}

/// A device that presented the correct secret and waits for the owner's decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingRequest {
    /// Opaque id for [`Host::decide`].
    pub request_id: String,
    /// The name the device sent.
    pub device_name: String,
    /// The device's endpoint id (lowercase hex).
    pub device_id: String,
    /// The fingerprint the device also shows, e.g. `4F2A 91C7 D03E`.
    pub fingerprint: String,
    /// Unix milliseconds.
    pub requested_at_ms: u64,
}

/// A paired device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceStatus {
    /// The device's endpoint id (lowercase hex).
    pub device_id: String,
    /// The device's name at pairing.
    pub name: String,
    /// Unix milliseconds.
    pub paired_at_ms: u64,
    /// Unix milliseconds of the last RPC connection, if any.
    pub last_seen_ms: Option<u64>,
    /// Whether the device has an RPC connection now.
    pub connected: bool,
}

/// Serializable snapshot of the host's remote-access state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostStatus {
    /// Whether remote access is on (an endpoint is bound).
    pub enabled: bool,
    /// The machine's endpoint id (lowercase hex); stable across restarts.
    pub endpoint_id: String,
    /// The effective relay.
    pub relay: RelayStatus,
    /// The active pairing offer, if any.
    pub pairing: Option<PairingStatus>,
    /// Requests waiting for [`Host::decide`].
    pub pending: Vec<PendingRequest>,
    /// Paired devices.
    pub devices: Vec<DeviceStatus>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Settings {
    enabled: bool,
    relay_override: Option<String>,
    port: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceRecord {
    device_id: String,
    name: String,
    paired_at_ms: u64,
    last_seen_ms: Option<u64>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Registry {
    devices: Vec<DeviceRecord>,
}

struct ActivePairing {
    secret: [u8; 32],
    status: PairingStatus,
    deadline: Instant,
    attempts: u32,
    generation: u64,
}

struct PendingEntry {
    info: PendingRequest,
    decide: Option<oneshot::Sender<bool>>,
}

struct LiveDevice {
    conns: Vec<(u64, Connection)>,
    streams: Arc<Semaphore>,
}

struct Running {
    endpoint: Endpoint,
    accept: JoinHandle<()>,
}

struct State {
    settings: Settings,
    devices: Vec<DeviceRecord>,
    pairing: Option<ActivePairing>,
    pending: BTreeMap<String, PendingEntry>,
    live: HashMap<EndpointId, LiveDevice>,
    running: Option<Running>,
}

struct Inner {
    name: String,
    default_relay: Option<RelayUrl>,
    connector: LocalConnector,
    policy: MethodPolicy,
    ttl: Duration,
    bind_addrs: Vec<SocketAddr>,
    key: SecretKey,
    id: EndpointId,
    registry_path: PathBuf,
    settings_path: PathBuf,
    state: Mutex<State>,
    /// Serializes enable/disable/relay changes.
    lifecycle: tokio::sync::Mutex<()>,
    /// Orders registry and settings writes so a stale snapshot never wins.
    writes: Mutex<()>,
    changes: watch::Sender<u64>,
    sequence: AtomicU64,
    handshakes: Arc<Semaphore>,
    pairings: Arc<Semaphore>,
}

/// The host half of remote access. Dropping it closes the endpoint without changing
/// the persisted `enabled` flag; prefer [`Host::shutdown`] for a graceful close.
pub struct Host {
    inner: Arc<Inner>,
}

impl Host {
    /// Opens the state directory, creating the identity key on first use, and binds the
    /// endpoint if remote access was left enabled.
    pub async fn start(config: HostConfig) -> Result<Host, RemoteError> {
        let name = link::validate_name(&config.machine_name)
            .ok_or(RemoteError::InvalidConfig("machine name"))?;
        let default_relay = match &config.default_relay {
            None => None,
            Some(text) => Some(
                link::parse_relay(text).ok_or(RemoteError::InvalidConfig("default relay url"))?,
            ),
        };
        files::ensure_private_dir(&config.state_dir)?;
        let key = files::load_or_create_key(&config.state_dir.join(KEY_FILE))?;
        let settings_path = config.state_dir.join(SETTINGS_FILE);
        let registry_path = config.state_dir.join(REGISTRY_FILE);
        let settings: Settings = files::read_json(&settings_path)?.unwrap_or_default();
        let registry: Registry = files::read_json(&registry_path)?.unwrap_or_default();
        let enabled = settings.enabled;
        let inner = Arc::new(Inner {
            name,
            default_relay,
            connector: config.connector,
            policy: config.policy,
            ttl: config.pairing_ttl,
            bind_addrs: config.bind_addrs,
            id: key.public(),
            key,
            registry_path,
            settings_path,
            state: Mutex::new(State {
                settings,
                devices: registry.devices,
                pairing: None,
                pending: BTreeMap::new(),
                live: HashMap::new(),
                running: None,
            }),
            lifecycle: tokio::sync::Mutex::new(()),
            writes: Mutex::new(()),
            changes: watch::channel(0).0,
            sequence: AtomicU64::new(0),
            handshakes: Arc::new(Semaphore::new(MAX_HANDSHAKES)),
            pairings: Arc::new(Semaphore::new(MAX_PAIRING_CONNECTIONS)),
        });
        if enabled {
            let _guard = inner.lifecycle.lock().await;
            inner.bind().await?;
        }
        Ok(Host { inner })
    }

    /// The machine's endpoint id (lowercase hex).
    pub fn endpoint_id(&self) -> String {
        self.inner.id.to_string()
    }

    /// A receiver whose value increases whenever [`Host::status`] may have changed.
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.inner.changes.subscribe()
    }

    /// Turns remote access on (binds the endpoint) or off (closes every connection,
    /// cancels pairing and pending requests, unbinds). Persisted.
    pub async fn set_enabled(&self, enabled: bool) -> Result<(), RemoteError> {
        let inner = &self.inner;
        let _guard = inner.lifecycle.lock().await;
        if enabled {
            if inner.lock().running.is_none() {
                inner.bind().await?;
            }
        } else {
            inner.unbind().await;
        }
        inner.lock().settings.enabled = enabled;
        inner.persist_settings()?;
        inner.changed();
        Ok(())
    }

    /// Sets (`Some(url)`) or clears (`None`, back to the configured default) the
    /// persisted relay override. When remote access is on, the endpoint is rebound with
    /// the new relay, which closes current connections and cancels the pairing offer.
    pub async fn set_relay(&self, url: Option<&str>) -> Result<(), RemoteError> {
        let url = match url {
            None => None,
            Some(text) => {
                link::parse_relay(text).ok_or(RemoteError::InvalidConfig("relay url"))?;
                Some(text.to_string())
            }
        };
        let inner = &self.inner;
        let _guard = inner.lifecycle.lock().await;
        inner.lock().settings.relay_override = url;
        inner.persist_settings()?;
        if inner.lock().running.is_some() {
            inner.unbind().await;
            inner.bind().await?;
        }
        inner.changed();
        Ok(())
    }

    /// Starts a pairing offer, replacing any previous one, and returns its link and QR
    /// matrix. Fails with [`RemoteError::Disabled`] while remote access is off.
    pub async fn pairing_start(&self) -> Result<PairingStatus, RemoteError> {
        let inner = &self.inner;
        let endpoint = inner
            .lock()
            .running
            .as_ref()
            .map(|r| r.endpoint.clone())
            .ok_or(RemoteError::Disabled)?;
        let mut secret = [0u8; 32];
        random_bytes(&mut secret);
        let text = PairingLink {
            host_id: inner.id,
            relay: inner.effective_relay().0,
            addrs: advertised_addrs(&endpoint).await,
            secret,
            machine_name: inner.name.clone(),
        }
        .encode()
        .map_err(|_| RemoteError::InvalidConfig("pairing link"))?;
        let status = PairingStatus {
            qr: link::qr_rows(&text),
            link: text,
            expires_at_ms: now_ms() + inner.ttl.as_millis() as u64,
        };
        let generation = inner.sequence.fetch_add(1, Ordering::Relaxed);
        let deadline = Instant::now() + inner.ttl;
        {
            let mut state = inner.lock();
            if state.running.is_none() {
                return Err(RemoteError::Disabled);
            }
            state.pairing = Some(ActivePairing {
                secret,
                status: status.clone(),
                deadline,
                attempts: 0,
                generation,
            });
        }
        // Notify at expiry so subscribers refresh. The expired offer stays in memory
        // (hidden from status) so that its secret is answered `expired`, until it is
        // replaced, presented, cancelled or remote access is turned off.
        let weak = Arc::downgrade(inner);
        tokio::spawn(async move {
            tokio::time::sleep_until(deadline).await;
            if let Some(inner) = weak.upgrade() {
                let current = inner
                    .lock()
                    .pairing
                    .as_ref()
                    .is_some_and(|p| p.generation == generation);
                if current {
                    inner.changed();
                }
            }
        });
        inner.changed();
        Ok(status)
    }

    /// Withdraws the pairing offer, if any. Pending requests are unaffected.
    pub fn pairing_cancel(&self) {
        if self.inner.lock().pairing.take().is_some() {
            self.inner.changed();
        }
    }

    /// Records the owner's decision on a pending request. Only `allow == true` adds the
    /// device to the registry.
    pub fn decide(&self, request_id: &str, allow: bool) -> Result<(), RemoteError> {
        let entry = self
            .inner
            .lock()
            .pending
            .remove(request_id)
            .ok_or(RemoteError::UnknownRequest)?;
        if let Some(decide) = entry.decide {
            let _ = decide.send(allow);
        }
        self.inner.changed();
        Ok(())
    }

    /// Removes a paired device and immediately closes its connections.
    pub fn revoke(&self, device_id: &str) -> Result<(), RemoteError> {
        let inner = &self.inner;
        {
            let mut state = inner.lock();
            let before = state.devices.len();
            state.devices.retain(|d| d.device_id != device_id);
            if state.devices.len() == before {
                return Err(RemoteError::UnknownDevice);
            }
            if let Ok(id) = device_id.parse::<EndpointId>() {
                if let Some(live) = state.live.remove(&id) {
                    for (_, conn) in live.conns {
                        conn.close(code(CLOSE_REVOKED), b"revoked");
                    }
                }
            }
        }
        let result = inner.persist_registry();
        inner.changed();
        result
    }

    /// A snapshot of the remote-access state.
    pub fn status(&self) -> HostStatus {
        let inner = &self.inner;
        let (url, source) = inner.effective_relay();
        let state = inner.lock();
        let pairing = state
            .pairing
            .as_ref()
            .filter(|p| Instant::now() < p.deadline)
            .map(|p| p.status.clone());
        HostStatus {
            enabled: state.running.is_some(),
            endpoint_id: inner.id.to_string(),
            relay: RelayStatus {
                url: url.map(|u| u.to_string()),
                source,
            },
            pairing,
            pending: state.pending.values().map(|p| p.info.clone()).collect(),
            devices: state
                .devices
                .iter()
                .map(|d| DeviceStatus {
                    device_id: d.device_id.clone(),
                    name: d.name.clone(),
                    paired_at_ms: d.paired_at_ms,
                    last_seen_ms: d.last_seen_ms,
                    connected: d
                        .device_id
                        .parse::<EndpointId>()
                        .ok()
                        .and_then(|id| state.live.get(&id))
                        .is_some_and(|l| !l.conns.is_empty()),
                })
                .collect(),
        }
    }

    /// The direct socket addresses the bound endpoint currently advertises (empty when
    /// off).
    pub async fn direct_addrs(&self) -> Vec<SocketAddr> {
        let endpoint = self
            .inner
            .lock()
            .running
            .as_ref()
            .map(|r| r.endpoint.clone());
        match endpoint {
            Some(endpoint) => advertised_addrs(&endpoint).await,
            None => Vec::new(),
        }
    }

    /// Closes the endpoint and all connections without changing the persisted flag.
    pub async fn shutdown(&self) {
        let _guard = self.inner.lifecycle.lock().await;
        self.inner.unbind().await;
        self.inner.changed();
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        let mut state = self.inner.lock();
        if let Some(running) = state.running.take() {
            running.accept.abort();
            for (_, live) in state.live.drain() {
                for (_, conn) in live.conns {
                    conn.close(code(CLOSE_REVOKED), b"shutdown");
                }
            }
            state.pending.clear();
            state.pairing = None;
        }
    }
}

/// The addresses worth advertising: routable ones first, loopback last, at most
/// [`link::LINK_ADDRS`].
async fn advertised_addrs(endpoint: &Endpoint) -> Vec<SocketAddr> {
    let mut addrs = crate::direct_addrs(endpoint).await;
    addrs.sort_by_key(|a| (a.ip().is_loopback(), a.is_ipv6()));
    addrs.dedup();
    addrs.truncate(link::LINK_ADDRS);
    addrs
}

impl Inner {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn changed(&self) {
        self.changes.send_modify(|v| *v = v.wrapping_add(1));
    }

    fn effective_relay(&self) -> (Option<RelayUrl>, RelaySource) {
        let override_url = self.lock().settings.relay_override.clone();
        if let Some(url) = override_url.as_deref().and_then(link::parse_relay) {
            return (Some(url), RelaySource::Override);
        }
        match &self.default_relay {
            Some(url) => (Some(url.clone()), RelaySource::Default),
            None => (None, RelaySource::None),
        }
    }

    fn persist_settings(&self) -> Result<(), RemoteError> {
        let _order = self.writes.lock().unwrap_or_else(|e| e.into_inner());
        let settings = self.lock().settings.clone();
        files::write_json(&self.settings_path, &settings)
    }

    fn persist_registry(&self) -> Result<(), RemoteError> {
        let _order = self.writes.lock().unwrap_or_else(|e| e.into_inner());
        let registry = Registry {
            devices: self.lock().devices.clone(),
        };
        files::write_json(&self.registry_path, &registry)
    }

    /// Binds the endpoint and starts the accept loop. Caller holds `lifecycle`.
    async fn bind(self: &Arc<Self>) -> Result<(), RemoteError> {
        let relay = self.effective_relay().0;
        let alpns = vec![PAIRING_ALPN.to_vec(), RPC_ALPN.to_vec()];
        let endpoint = if self.bind_addrs.is_empty() {
            let port = self.lock().settings.port.unwrap_or(0);
            let binds = |port| {
                [
                    (SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)), true),
                    (SocketAddr::from((Ipv6Addr::UNSPECIFIED, port)), false),
                ]
            };
            match crate::bind_endpoint(&self.key, relay.as_ref(), &binds(port), alpns.clone()).await
            {
                Ok(endpoint) => endpoint,
                Err(_) if port != 0 => {
                    crate::bind_endpoint(&self.key, relay.as_ref(), &binds(0), alpns).await?
                }
                Err(e) => return Err(e),
            }
        } else {
            let binds: Vec<_> = self.bind_addrs.iter().map(|a| (*a, true)).collect();
            crate::bind_endpoint(&self.key, relay.as_ref(), &binds, alpns).await?
        };
        if self.bind_addrs.is_empty() {
            let port = endpoint
                .bound_sockets()
                .iter()
                .find(|a| a.is_ipv4())
                .map(|a| a.port());
            let changed = {
                let mut state = self.lock();
                let changed = port.is_some() && state.settings.port != port;
                if changed {
                    state.settings.port = port;
                }
                changed
            };
            if changed {
                self.persist_settings()?;
            }
        }
        let accept = tokio::spawn(accept_loop(Arc::downgrade(self), endpoint.clone()));
        self.lock().running = Some(Running { endpoint, accept });
        Ok(())
    }

    /// Closes everything. Caller holds `lifecycle`.
    async fn unbind(&self) {
        let running = {
            let mut state = self.lock();
            state.pairing = None;
            // Dropping the decision senders answers pending devices with `cancelled`.
            state.pending.clear();
            for (_, live) in state.live.drain() {
                for (_, conn) in live.conns {
                    conn.close(code(CLOSE_REVOKED), b"remote_access_disabled");
                }
            }
            state.running.take()
        };
        if let Some(running) = running {
            running.accept.abort();
            let _ = tokio::time::timeout(Duration::from_secs(3), running.endpoint.close()).await;
        }
    }
}

async fn accept_loop(inner: std::sync::Weak<Inner>, endpoint: Endpoint) {
    while let Some(incoming) = endpoint.accept().await {
        let Some(strong) = inner.upgrade() else {
            return;
        };
        let Ok(permit) = strong.handshakes.clone().try_acquire_owned() else {
            incoming.refuse();
            continue;
        };
        let endpoint = endpoint.clone();
        tokio::spawn(async move {
            let _ = handle_incoming(strong, endpoint, incoming, permit).await;
        });
    }
}

async fn handle_incoming(
    inner: Arc<Inner>,
    endpoint: Endpoint,
    incoming: Incoming,
    permit: OwnedSemaphorePermit,
) -> Option<()> {
    let accepting = incoming.accept().ok()?;
    let conn = tokio::time::timeout(IO_TIMEOUT, accepting)
        .await
        .ok()?
        .ok()?;
    drop(permit);
    if conn.alpn() == PAIRING_ALPN {
        serve_pairing(inner, conn).await;
    } else if conn.alpn() == RPC_ALPN {
        serve_rpc(inner, endpoint, conn).await;
    } else {
        conn.close(code(CLOSE_NOT_PAIRED), b"unknown_alpn");
    }
    Some(())
}

// ---------------------------------------------------------------------------------------
// Pairing
// ---------------------------------------------------------------------------------------

async fn send_status(send: &mut SendStream, status: &str) -> io::Result<()> {
    tokio::time::timeout(
        IO_TIMEOUT,
        frames::write_json(send, &json!({ "status": status })),
    )
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "write_timeout"))?
}

/// Gives the peer a moment to read the last frame, then closes.
async fn finish_and_close(conn: &Connection, send: &mut SendStream) {
    let _ = send.finish();
    let _ = tokio::time::timeout(IO_TIMEOUT, conn.closed()).await;
    conn.close(code(CLOSE_DONE), b"done");
}

async fn serve_pairing(inner: Arc<Inner>, conn: Connection) {
    let Ok(_slot) = inner.pairings.clone().try_acquire_owned() else {
        conn.close(code(CLOSE_BUSY), b"busy");
        return;
    };
    let device = conn.remote_id();
    let Ok(Ok((mut send, mut recv))) = tokio::time::timeout(IO_TIMEOUT, conn.accept_bi()).await
    else {
        conn.close(code(CLOSE_DONE), b"no_request");
        return;
    };
    let request = tokio::time::timeout(
        IO_TIMEOUT,
        frames::read_json(&mut recv, CONTROL_FRAME_BYTES),
    )
    .await;
    let parsed = match request {
        Ok(Ok(Some(value))) => parse_pairing_request(&value),
        _ => None,
    };
    let Some((secret, name)) = parsed else {
        let _ = send_status(&mut send, "invalid").await;
        finish_and_close(&conn, &mut send).await;
        return;
    };
    let admitted = inner.admit(&secret, device, name);
    let (info, deadline, decision) = match admitted {
        Ok(admitted) => admitted,
        Err(status) => {
            let _ = send_status(&mut send, status).await;
            finish_and_close(&conn, &mut send).await;
            return;
        }
    };
    let request_id = info.request_id.clone();
    if send_status(&mut send, "pending").await.is_err() {
        inner.drop_pending(&request_id);
        return;
    }
    let outcome = tokio::select! {
        decided = tokio::time::timeout_at(deadline, decision) => match decided {
            Ok(Ok(true)) => "allowed",
            Ok(Ok(false)) => "denied",
            Ok(Err(_)) => "cancelled",
            Err(_) => "expired",
        },
        _ = conn.closed() => {
            inner.drop_pending(&request_id);
            return;
        }
    };
    inner.lock().pending.remove(&request_id);
    if outcome == "allowed" {
        if inner.add_device(&info).is_err() {
            let _ = send_status(&mut send, "cancelled").await;
            finish_and_close(&conn, &mut send).await;
            return;
        }
    }
    inner.changed();
    let _ = send_status(&mut send, outcome).await;
    finish_and_close(&conn, &mut send).await;
}

fn parse_pairing_request(value: &Value) -> Option<([u8; 32], String)> {
    let object = value.as_object()?;
    if object.len() != 2 {
        return None;
    }
    let secret = data_encoding::BASE64URL_NOPAD
        .decode(object.get("secret")?.as_str()?.as_bytes())
        .ok()?;
    let secret: [u8; 32] = secret.try_into().ok()?;
    let raw = object.get("name")?.as_str()?;
    let name = link::validate_name(raw).filter(|n| n == raw)?;
    Some((secret, name))
}

type Admitted = (PendingRequest, Instant, oneshot::Receiver<bool>);

impl Inner {
    /// Checks a presented secret in constant time. On success the secret is consumed and
    /// a pending request is created; otherwise the terminal status to send.
    fn admit(
        &self,
        secret: &[u8; 32],
        device: EndpointId,
        device_name: String,
    ) -> Result<Admitted, &'static str> {
        let mut state = self.lock();
        let Some(pairing) = state.pairing.as_mut() else {
            return Err("rejected");
        };
        let matches: bool = pairing.secret.ct_eq(secret).into();
        if !matches {
            pairing.attempts += 1;
            if pairing.attempts >= PAIRING_ATTEMPTS {
                state.pairing = None;
                drop(state);
                self.changed();
            }
            return Err("rejected");
        }
        let expired = Instant::now() >= pairing.deadline;
        state.pairing = None;
        if expired {
            drop(state);
            self.changed();
            return Err("expired");
        }
        let mut id = [0u8; 16];
        random_bytes(&mut id);
        let info = PendingRequest {
            request_id: data_encoding::HEXLOWER.encode(&id),
            device_name,
            device_id: device.to_string(),
            fingerprint: link::fingerprint(&self.id, &device, secret),
            requested_at_ms: now_ms(),
        };
        let (tx, rx) = oneshot::channel();
        state.pending.insert(
            info.request_id.clone(),
            PendingEntry {
                info: info.clone(),
                decide: Some(tx),
            },
        );
        drop(state);
        self.changed();
        Ok((info, Instant::now() + self.ttl, rx))
    }

    fn drop_pending(&self, request_id: &str) {
        if self.lock().pending.remove(request_id).is_some() {
            self.changed();
        }
    }

    fn add_device(&self, info: &PendingRequest) -> Result<(), RemoteError> {
        {
            let mut state = self.lock();
            let now = now_ms();
            match state
                .devices
                .iter_mut()
                .find(|d| d.device_id == info.device_id)
            {
                Some(existing) => {
                    existing.name = info.device_name.clone();
                    existing.paired_at_ms = now;
                }
                None => state.devices.push(DeviceRecord {
                    device_id: info.device_id.clone(),
                    name: info.device_name.clone(),
                    paired_at_ms: now,
                    last_seen_ms: None,
                }),
            }
        }
        self.persist_registry()
    }

    /// Registers an RPC connection of a paired device. `None` when it must be refused.
    fn register(&self, conn: &Connection) -> Result<(u64, Arc<Semaphore>), (u32, &'static [u8])> {
        let device = conn.remote_id();
        let device_id = device.to_string();
        let mut state = self.lock();
        let Some(record) = state.devices.iter_mut().find(|d| d.device_id == device_id) else {
            return Err((CLOSE_NOT_PAIRED, b"not_paired"));
        };
        record.last_seen_ms = Some(now_ms());
        if !state.live.contains_key(&device) && state.live.len() >= MAX_CONNECTED_DEVICES {
            return Err((CLOSE_BUSY, b"too_many_devices"));
        }
        let live = state.live.entry(device).or_insert_with(|| LiveDevice {
            conns: Vec::new(),
            streams: Arc::new(Semaphore::new(MAX_STREAMS_PER_DEVICE as usize)),
        });
        while live.conns.len() >= MAX_CONNECTIONS_PER_DEVICE {
            let (_, oldest) = live.conns.remove(0);
            oldest.close(code(CLOSE_REPLACED), b"replaced");
        }
        let id = self.sequence.fetch_add(1, Ordering::Relaxed);
        live.conns.push((id, conn.clone()));
        Ok((id, live.streams.clone()))
    }

    fn unregister(&self, device: EndpointId, id: u64) {
        let mut state = self.lock();
        if let Some(live) = state.live.get_mut(&device) {
            live.conns.retain(|(c, _)| *c != id);
            if live.conns.is_empty() {
                state.live.remove(&device);
            }
        }
        drop(state);
        self.changed();
    }
}

// ---------------------------------------------------------------------------------------
// RPC tunnel
// ---------------------------------------------------------------------------------------

async fn serve_rpc(inner: Arc<Inner>, endpoint: Endpoint, conn: Connection) {
    let device = conn.remote_id();
    let (id, streams) = match inner.register(&conn) {
        Ok(registered) => registered,
        Err((close, reason)) => {
            conn.close(code(close), reason);
            return;
        }
    };
    inner.changed();
    let _ = inner.persist_registry();
    conn.set_max_concurrent_bi_streams(code(MAX_STREAMS_PER_DEVICE));
    conn.set_max_concurrent_uni_streams(code(0));
    // Hello: the current addresses and relay, so the phone refreshes its record.
    let hello = json!({
        "name": inner.name,
        "relay": inner.effective_relay().0.map(|u| u.to_string()),
        "addrs": advertised_addrs(&endpoint).await.iter().map(|a| a.to_string()).collect::<Vec<_>>(),
    });
    let hello_conn = conn.clone();
    tokio::spawn(async move {
        let sent = tokio::time::timeout(IO_TIMEOUT, async {
            let mut uni = hello_conn.open_uni().await.ok()?;
            frames::write_json(&mut uni, &hello).await.ok()?;
            uni.finish().ok()
        })
        .await;
        if !matches!(sent, Ok(Some(()))) {
            hello_conn.close(code(CLOSE_DONE), b"hello_failed");
        }
    });
    loop {
        tokio::select! {
            accepted = conn.accept_bi() => {
                let Ok((mut send, mut recv)) = accepted else { break };
                match streams.clone().try_acquire_owned() {
                    Ok(permit) => {
                        let inner = inner.clone();
                        tokio::spawn(async move {
                            tunnel(&inner, send, recv).await;
                            drop(permit);
                        });
                    }
                    Err(_) => {
                        let _ = send.reset(code(STREAM_BUSY));
                        let _ = recv.stop(code(STREAM_BUSY));
                    }
                }
            }
            _ = conn.closed() => break,
        }
    }
    inner.unregister(device, id);
}

/// Bridges one device stream to one fresh local service connection.
async fn tunnel(inner: &Inner, mut send: SendStream, mut recv: RecvStream) {
    let local = match (inner.connector)().await {
        Ok(local) => local,
        Err(_) => {
            let _ = send.reset(code(STREAM_LOCAL_FAILED));
            let _ = recv.stop(code(STREAM_LOCAL_FAILED));
            return;
        }
    };
    let (local_read, local_write) = tokio::io::split(local);
    let send = tokio::sync::Mutex::new(send);
    // Either direction may be suspended while holding the send lock, so the outcome is
    // decided first and acted on only after both futures (and the device receive
    // stream, which stops it) are dropped.
    let reset = {
        let up = upstream(&inner.policy, recv, local_write, &send);
        let down = downstream(local_read, &send);
        tokio::pin!(up, down);
        let mut up_done = false;
        loop {
            tokio::select! {
                result = &mut up, if !up_done => match result {
                    // The device finished sending; the service sees EOF and ends the stream.
                    Ok(()) => up_done = true,
                    Err(reset) => break Some(reset),
                },
                result = &mut down => break match result {
                    Ok(()) => None,
                    Err(()) => Some(STREAM_LOCAL_FAILED),
                },
            }
        }
    };
    let mut send = send.into_inner();
    match reset {
        None => {
            let _ = send.finish();
        }
        Some(reset) => {
            let _ = send.reset(code(reset));
        }
    }
}

/// Device → service. `Err(code)` resets the stream.
async fn upstream(
    policy: &MethodPolicy,
    mut recv: RecvStream,
    mut local: WriteHalf<LocalStream>,
    send: &tokio::sync::Mutex<SendStream>,
) -> Result<(), u32> {
    loop {
        let frame = match frames::read_raw(&mut recv, FRAME_BYTES).await {
            Ok(Some(frame)) => frame,
            Ok(None) => {
                let _ = local.shutdown().await;
                return Ok(());
            }
            Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                let _ = recv.stop(code(STREAM_INVALID_FRAME));
                return Err(STREAM_INVALID_FRAME);
            }
            Err(_) => return Err(STREAM_INVALID_FRAME),
        };
        let Ok(value) = workspace_protocol::decode_frame(&frame) else {
            let _ = recv.stop(code(STREAM_INVALID_FRAME));
            return Err(STREAM_INVALID_FRAME);
        };
        let Some(method) = value.get("method").and_then(Value::as_str) else {
            let _ = recv.stop(code(STREAM_INVALID_FRAME));
            return Err(STREAM_INVALID_FRAME);
        };
        let params = value.get("params").unwrap_or(&Value::Null);
        if policy(method, params) {
            local
                .write_all(&frame)
                .await
                .map_err(|_| STREAM_LOCAL_FAILED)?;
        } else {
            let id = value.get("id").cloned().unwrap_or(Value::Null);
            let refusal =
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":REFUSED_MESSAGE}});
            let mut send = send.lock().await;
            frames::write_json(&mut *send, &refusal)
                .await
                .map_err(|_| STREAM_LOCAL_FAILED)?;
        }
    }
}

/// Service → device, whole frames unchanged.
async fn downstream(
    mut local: ReadHalf<LocalStream>,
    send: &tokio::sync::Mutex<SendStream>,
) -> Result<(), ()> {
    loop {
        match frames::read_raw(&mut local, FRAME_BYTES).await {
            Ok(Some(frame)) => {
                let mut send = send.lock().await;
                send.write_all(&frame).await.map_err(|_| ())?;
            }
            Ok(None) => return Ok(()),
            Err(_) => return Err(()),
        }
    }
}
