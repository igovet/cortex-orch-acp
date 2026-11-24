//! In-process tests with two real iroh endpoints on loopback, no relay and no network.

use std::future::Future;
use std::io::Write as _;
use std::net::SocketAddr;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use workspace_remote::link::{qr_rows, PairingLink};
use workspace_remote::{
    Client, ClientConfig, ConnectError, ConnectionState, Host, HostConfig, LocalStream, PairError,
    RemoteError, INCOMPATIBLE_VERSION_CODE, PAIRING_ALPN, RPC_ALPN, VERSION_ALPN,
};

const LOOPBACK: &str = "127.0.0.1:0";
const LIMIT: Duration = Duration::from_secs(60);

async fn bounded<F: Future<Output = ()>>(f: F) {
    tokio::time::timeout(LIMIT, f)
        .await
        .expect("test exceeded its time bound");
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> TempDir {
        let mut random = [0u8; 8];
        getrandom_fill(&mut random);
        let hex: String = random.iter().map(|b| format!("{b:02x}")).collect();
        let path = std::env::temp_dir().join(format!("workspace-remote-{tag}-{hex}"));
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        TempDir(path)
    }
    fn sub(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn getrandom_fill(out: &mut [u8]) {
    // Uniqueness only; the time and a counter are enough for temp names.
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
        ^ (u64::from(std::process::id()) << 32)
        ^ COUNTER
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15);
    for (i, b) in out.iter_mut().enumerate() {
        *b = (seed >> ((i % 8) * 8)) as u8;
    }
}

/// What the fake service saw.
#[derive(Clone, Default)]
struct Service {
    methods: Arc<Mutex<Vec<String>>>,
    connections: Arc<Mutex<usize>>,
}

impl Service {
    fn methods(&self) -> Vec<String> {
        self.methods.lock().unwrap().clone()
    }
}

async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> std::io::Result<Option<Value>> {
    let mut prefix = [0u8; 4];
    match reader.read_exact(&mut prefix).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let n = u32::from_be_bytes(prefix) as usize;
    let mut body = vec![0u8; n];
    reader.read_exact(&mut body).await?;
    Ok(Some(serde_json::from_slice(&body).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, e)
    })?))
}

async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, value: &Value) -> std::io::Result<()> {
    writer
        .write_all(&workspace_protocol::encode_frame(value).unwrap())
        .await?;
    writer.flush().await
}

/// A duplex-backed fake machine service: `echo` returns its params; `service.subscribe`
/// acknowledges and then pushes three `service.event` frames.
fn fake_connector(
    service: Service,
) -> impl Fn() -> std::pin::Pin<Box<dyn Future<Output = std::io::Result<LocalStream>> + Send>>
       + Send
       + Sync
       + 'static {
    move || {
        let service = service.clone();
        Box::pin(async move {
            let (ours, theirs) = tokio::io::duplex(1 << 16);
            *service.connections.lock().unwrap() += 1;
            tokio::spawn(async move {
                let (mut reader, mut writer) = tokio::io::split(theirs);
                while let Ok(Some(request)) = read_frame(&mut reader).await {
                    let method = request["method"].as_str().unwrap_or_default().to_string();
                    service.methods.lock().unwrap().push(method.clone());
                    let id = request["id"].clone();
                    if method == "service.subscribe" {
                        if write_frame(
                            &mut writer,
                            &json!({"jsonrpc":"2.0","id":id,"result":{"subscribed":true}}),
                        )
                        .await
                        .is_err()
                        {
                            return;
                        }
                        for n in 0..3 {
                            let event =
                                json!({"jsonrpc":"2.0","method":"service.event","params":{"n":n}});
                            if write_frame(&mut writer, &event).await.is_err() {
                                return;
                            }
                        }
                    } else {
                        let reply =
                            json!({"jsonrpc":"2.0","id":id,"result":request["params"].clone()});
                        if write_frame(&mut writer, &reply).await.is_err() {
                            return;
                        }
                    }
                }
            });
            Ok(Box::new(ours) as LocalStream)
        })
    }
}

fn host_config(dir: &Path, service: &Service, ttl: Duration) -> HostConfig {
    let mut config = HostConfig::new(
        dir,
        "Test Machine",
        fake_connector(service.clone()),
        |method: &str, _params: &Value| !method.starts_with("forbidden."),
    );
    config.pairing_ttl = ttl;
    config.bind_addrs = vec![LOOPBACK.parse().unwrap()];
    config
}

fn client(dir: &Path) -> Client {
    let mut config = ClientConfig::new(dir);
    config.bind_addrs = vec![LOOPBACK.parse().unwrap()];
    config.connect_timeout = Duration::from_secs(5);
    Client::new(config).unwrap()
}

async fn wait_pending(host: &Host) -> workspace_remote::PendingRequest {
    for _ in 0..500 {
        if let Some(p) = host.status().pending.first() {
            return p.clone();
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("no pending request appeared");
}

async fn pair(host: &Host, client: &Client) -> String {
    let offer = host.pairing_start().await.unwrap();
    let attempt = client.pair_begin(&offer.link, "Test Phone").await.unwrap();
    let pending = wait_pending(host).await;
    assert_eq!(
        pending.fingerprint, attempt.fingerprint,
        "both sides show the same fingerprint"
    );
    assert_eq!(pending.device_id, client.device_id());
    assert_eq!(pending.device_name, "Test Phone");
    host.decide(&pending.request_id, true).unwrap();
    let machine = attempt.finish().await.unwrap();
    assert_eq!(machine.machine_id, host.endpoint_id());
    machine.machine_id
}

async fn call<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    id: u64,
    method: &str,
    params: Value,
) -> Value {
    write_frame(
        stream,
        &json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
    )
    .await
    .unwrap();
    read_frame(stream).await.unwrap().expect("reply")
}

async fn wait_state(
    conn: &workspace_remote::RemoteConnection,
    want: impl Fn(&ConnectionState) -> bool,
) -> ConnectionState {
    let mut rx = conn.state();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let current = rx.borrow_and_update().clone();
        if want(&current) {
            return current;
        }
        if tokio::time::timeout_at(deadline, rx.changed())
            .await
            .is_err()
        {
            panic!("state not reached, last {current:?}");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pair_allow_then_tunnel_requests_and_subscription() {
    bounded(async {
        let dirs = TempDir::new("tunnel");
        let service = Service::default();
        let host = Host::start(host_config(
            &dirs.sub("host"),
            &service,
            Duration::from_secs(60),
        ))
        .await
        .unwrap();
        host.set_enabled(true).await.unwrap();
        let phone = client(&dirs.sub("phone"));
        let machine_id = pair(&host, &phone).await;
        let status = host.status();
        assert!(status.pending.is_empty());
        assert!(status.pairing.is_none(), "the secret was consumed");
        assert_eq!(status.devices.len(), 1);
        assert_eq!(status.devices[0].device_id, phone.device_id());
        assert_eq!(phone.machines().len(), 1);
        assert_eq!(phone.machines()[0].name, "Test Machine");

        let conn = phone.connect(&machine_id).await.unwrap();
        let state = wait_state(&conn, |s| matches!(s, ConnectionState::Connected { .. })).await;
        // Loopback without a relay can only be a direct path (or not yet selected).
        assert_eq!(
            state,
            ConnectionState::Connected {
                path: workspace_remote::PathKind::Direct
            }
        );
        let mut stream = conn.open().await.unwrap();
        let reply = call(&mut stream, 7, "echo", json!({"hello":"world"})).await;
        assert_eq!(
            reply,
            json!({"jsonrpc":"2.0","id":7,"result":{"hello":"world"}})
        );

        let mut events = conn.open().await.unwrap();
        let ack = call(&mut events, 1, "service.subscribe", json!({})).await;
        assert_eq!(ack["result"]["subscribed"], json!(true));
        for n in 0..3 {
            let event = read_frame(&mut events).await.unwrap().unwrap();
            assert_eq!(
                event,
                json!({"jsonrpc":"2.0","method":"service.event","params":{"n":n}})
            );
        }
        assert!(host.status().devices[0].connected);
        assert!(host.status().devices[0].last_seen_ms.is_some());
        assert!(phone.machines()[0].last_connected_ms.is_some());
        assert_eq!(service.methods(), vec!["echo", "service.subscribe"]);

        // Closing the device side ends the service connection.
        stream.shutdown().await.unwrap();
        assert!(read_frame(&mut stream).await.unwrap().is_none());
        conn.close().await;
        host.shutdown().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refused_method_is_answered_locally_and_never_forwarded() {
    bounded(async {
        let dirs = TempDir::new("policy");
        let service = Service::default();
        let host = Host::start(host_config(&dirs.sub("host"), &service, Duration::from_secs(60))).await.unwrap();
        host.set_enabled(true).await.unwrap();
        let phone = client(&dirs.sub("phone"));
        let machine_id = pair(&host, &phone).await;
        let conn = phone.connect(&machine_id).await.unwrap();
        let mut stream = conn.open().await.unwrap();
        let refused = call(&mut stream, "abc".len() as u64, "forbidden.terminal", json!({"cmd":"rm"})).await;
        assert_eq!(
            refused,
            json!({"jsonrpc":"2.0","id":3,"error":{"code":-32000,"message":"remote_operation_refused"}})
        );
        // The stream stays usable for allowed requests.
        let reply = call(&mut stream, 4, "echo", json!(1)).await;
        assert_eq!(reply["result"], json!(1));
        assert_eq!(service.methods(), vec!["echo"]);
        conn.close().await;
        host.shutdown().await;
    })
    .await;
}

fn with_secret(link: &str, secret: [u8; 32]) -> String {
    let mut decoded = PairingLink::decode(link).unwrap();
    decoded.secret = secret;
    decoded.encode().unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_reused_expired_and_denied_pairings_leave_no_device() {
    bounded(async {
        let dirs = TempDir::new("pairfail");
        let service = Service::default();
        let host = Host::start(host_config(
            &dirs.sub("host"),
            &service,
            Duration::from_millis(1500),
        ))
        .await
        .unwrap();
        host.set_enabled(true).await.unwrap();
        let phone = client(&dirs.sub("phone"));

        // Wrong secret.
        let offer = host.pairing_start().await.unwrap();
        let wrong = with_secret(&offer.link, [7u8; 32]);
        assert!(matches!(
            phone.pair_begin(&wrong, "Phone").await,
            Err(PairError::SecretRejected)
        ));

        // Denied (the offer survived one wrong attempt).
        let attempt = phone.pair_begin(&offer.link, "Phone").await.unwrap();
        let pending = wait_pending(&host).await;
        host.decide(&pending.request_id, false).unwrap();
        assert!(matches!(attempt.finish().await, Err(PairError::Denied)));
        assert!(matches!(
            host.decide(&pending.request_id, true),
            Err(RemoteError::UnknownRequest)
        ));

        // Reused secret.
        assert!(matches!(
            phone.pair_begin(&offer.link, "Phone").await,
            Err(PairError::SecretRejected)
        ));

        // Expired secret.
        let offer = host.pairing_start().await.unwrap();
        tokio::time::sleep(Duration::from_millis(1700)).await;
        assert!(host.status().pairing.is_none());
        let result = phone.pair_begin(&offer.link, "Phone").await;
        assert!(matches!(result, Err(PairError::Expired)), "{result:?}");
        // The expired secret is gone afterwards.
        assert!(matches!(
            phone.pair_begin(&offer.link, "Phone").await,
            Err(PairError::SecretRejected)
        ));

        // No decision within the ceiling.
        let offer = host.pairing_start().await.unwrap();
        let attempt = phone.pair_begin(&offer.link, "Phone").await.unwrap();
        assert!(matches!(attempt.finish().await, Err(PairError::Expired)));
        assert!(host.status().pending.is_empty());

        // Five wrong attempts cancel the offer.
        let offer = host.pairing_start().await.unwrap();
        for n in 0..5u8 {
            let wrong = with_secret(&offer.link, [n; 32]);
            assert!(matches!(
                phone.pair_begin(&wrong, "Phone").await,
                Err(PairError::SecretRejected)
            ));
        }
        assert!(host.status().pairing.is_none());
        assert!(matches!(
            phone.pair_begin(&offer.link, "Phone").await,
            Err(PairError::SecretRejected)
        ));

        // Invalid device name never reaches the host.
        let offer = host.pairing_start().await.unwrap();
        assert!(matches!(
            phone.pair_begin(&offer.link, "bad\u{7}name").await,
            Err(PairError::InvalidDeviceName)
        ));
        assert!(matches!(
            phone.pair_begin("cortex-pair:AAAA", "Phone").await,
            Err(PairError::InvalidLink(_))
        ));

        // A replaced offer invalidates the previous link.
        let newer = host.pairing_start().await.unwrap();
        assert_ne!(newer.link, offer.link);
        assert!(matches!(
            phone.pair_begin(&offer.link, "Phone").await,
            Err(PairError::SecretRejected)
        ));

        assert!(host.status().devices.is_empty());
        assert!(phone.machines().is_empty());
        host.shutdown().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revoke_closes_live_connection_and_rejects_reconnect() {
    bounded(async {
        let dirs = TempDir::new("revoke");
        let service = Service::default();
        let host = Host::start(host_config(
            &dirs.sub("host"),
            &service,
            Duration::from_secs(60),
        ))
        .await
        .unwrap();
        host.set_enabled(true).await.unwrap();
        let phone = client(&dirs.sub("phone"));
        let machine_id = pair(&host, &phone).await;
        let conn = phone.connect(&machine_id).await.unwrap();
        let mut events = conn.open().await.unwrap();
        let ack = call(&mut events, 1, "service.subscribe", json!({})).await;
        assert_eq!(ack["result"]["subscribed"], json!(true));
        for _ in 0..3 {
            read_frame(&mut events).await.unwrap().unwrap();
        }

        host.revoke(&phone.device_id()).unwrap();
        assert!(matches!(
            host.revoke(&phone.device_id()),
            Err(RemoteError::UnknownDevice)
        ));
        let after = tokio::time::timeout(Duration::from_secs(10), read_frame(&mut events))
            .await
            .unwrap();
        assert!(
            !matches!(after, Ok(Some(_))),
            "live stream must end on revoke"
        );
        wait_state(&conn, |s| *s == ConnectionState::Rejected).await;

        assert!(matches!(conn.open().await, Err(ConnectError::Rejected)));
        assert!(matches!(
            phone.connect(&machine_id).await,
            Err(ConnectError::Rejected)
        ));
        assert!(host.status().devices.is_empty());

        // Forget removes the machine and closes the connection object.
        phone.forget(&machine_id).await.unwrap();
        assert!(phone.machines().is_empty());
        assert!(matches!(conn.open().await, Err(ConnectError::Closed)));
        assert!(matches!(
            phone.connect(&machine_id).await,
            Err(ConnectError::UnknownMachine)
        ));
        host.shutdown().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabled_by_default_nothing_listens_and_state_survives_restart() {
    bounded(async {
        let dirs = TempDir::new("enable");
        let service = Service::default();
        let host_dir = dirs.sub("host");
        let host = Host::start(host_config(&host_dir, &service, Duration::from_secs(60)))
            .await
            .unwrap();
        let id = host.endpoint_id();
        assert!(!host.status().enabled);
        assert!(host.direct_addrs().await.is_empty());
        assert!(matches!(
            host.pairing_start().await,
            Err(RemoteError::Disabled)
        ));

        host.set_enabled(true).await.unwrap();
        let phone = client(&dirs.sub("phone"));
        let machine_id = pair(&host, &phone).await;
        let conn = phone.connect(&machine_id).await.unwrap();
        let mut stream = conn.open().await.unwrap();
        assert_eq!(
            call(&mut stream, 1, "echo", json!("x")).await["result"],
            json!("x")
        );
        let bound: Vec<SocketAddr> = host.direct_addrs().await;
        assert!(!bound.is_empty());

        host.set_enabled(false).await.unwrap();
        assert!(!host.status().enabled);
        assert!(!host.status().devices[0].connected);
        let after = tokio::time::timeout(Duration::from_secs(10), read_frame(&mut stream))
            .await
            .unwrap();
        assert!(
            !matches!(after, Ok(Some(_))),
            "disabling closes device connections"
        );
        // The UDP port is released: nothing listens any more.
        for addr in &bound {
            std::net::UdpSocket::bind(addr).expect("port released after disable");
        }
        assert!(matches!(
            phone.connect(&machine_id).await,
            Err(ConnectError::Unreachable)
        ));

        host.set_enabled(true).await.unwrap();
        assert_eq!(host.endpoint_id(), id);
        host.shutdown().await;
        drop(host);

        let restarted = Host::start(host_config(&host_dir, &service, Duration::from_secs(60)))
            .await
            .unwrap();
        let status = restarted.status();
        assert!(status.enabled, "enabled flag persisted");
        assert_eq!(status.endpoint_id, id);
        assert_eq!(status.devices.len(), 1);
        assert_eq!(status.devices[0].device_id, phone.device_id());
        restarted.set_enabled(false).await.unwrap();
        drop(restarted);
        let again = Host::start(host_config(&host_dir, &service, Duration::from_secs(60)))
            .await
            .unwrap();
        assert!(!again.status().enabled, "disabled flag persisted");

        // Relay override persists and wins over the default; clearing returns to the default.
        again
            .set_relay(Some("https://relay.example.test"))
            .await
            .unwrap();
        let relay = again.status().relay;
        assert_eq!(relay.url.as_deref(), Some("https://relay.example.test/"));
        assert_eq!(relay.source, workspace_remote::RelaySource::Override);
        again.set_relay(None).await.unwrap();
        assert_eq!(
            again.status().relay.source,
            workspace_remote::RelaySource::None
        );
        assert!(matches!(
            again.set_relay(Some("ftp://nope")).await,
            Err(RemoteError::InvalidConfig(_))
        ));

        // A client restarted on its state dir keeps its identity and machines.
        let device = phone.device_id();
        drop(phone);
        let phone = client(&dirs.sub("phone"));
        assert_eq!(phone.device_id(), device);
        assert_eq!(phone.machines().len(), 1);
        phone.rename(&machine_id, "Desk").unwrap();
        assert_eq!(client(&dirs.sub("phone")).machines()[0].name, "Desk");
    })
    .await;
}

fn mode(path: &Path) -> u32 {
    std::fs::symlink_metadata(path)
        .unwrap()
        .permissions()
        .mode()
        & 0o777
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn state_files_are_private_and_symlinks_or_loose_modes_are_refused() {
    bounded(async {
        let dirs = TempDir::new("perms");
        let service = Service::default();
        let host_dir = dirs.sub("host");
        let host = Host::start(host_config(&host_dir, &service, Duration::from_secs(60)))
            .await
            .unwrap();
        host.set_enabled(true).await.unwrap();
        let phone_dir = dirs.sub("phone");
        let phone = client(&phone_dir);
        pair(&host, &phone).await;
        host.shutdown().await;
        drop(host);
        for file in [
            "remote-host.key",
            "remote-devices.json",
            "remote-settings.json",
        ] {
            assert_eq!(mode(&host_dir.join(file)), 0o600, "{file}");
        }
        for file in ["remote-device.key", "remote-machines.json"] {
            assert_eq!(mode(&phone_dir.join(file)), 0o600, "{file}");
        }
        assert_eq!(
            std::fs::metadata(host_dir.join("remote-host.key"))
                .unwrap()
                .len(),
            32
        );
        assert_eq!(mode(&host_dir), 0o700);

        let start = |dir: PathBuf| {
            let service = service.clone();
            async move { Host::start(host_config(&dir, &service, Duration::from_secs(60))).await }
        };

        // A key file with group/other bits is refused, not repaired.
        let key = host_dir.join("remote-host.key");
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            start(host_dir.clone()).await,
            Err(RemoteError::InsecureState(_))
        ));
        assert_eq!(mode(&key), 0o644);
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(start(host_dir.clone()).await.is_ok());

        // A symlinked key is refused.
        let elsewhere = dirs.sub("elsewhere.key");
        std::fs::rename(&key, &elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &key).unwrap();
        assert!(matches!(
            start(host_dir.clone()).await,
            Err(RemoteError::InsecureState(_))
        ));
        std::fs::remove_file(&key).unwrap();
        std::fs::rename(&elsewhere, &key).unwrap();

        // A loose or symlinked registry is refused.
        let registry = host_dir.join("remote-devices.json");
        std::fs::set_permissions(&registry, std::fs::Permissions::from_mode(0o640)).unwrap();
        assert!(matches!(
            start(host_dir.clone()).await,
            Err(RemoteError::InsecureState(_))
        ));
        std::fs::set_permissions(&registry, std::fs::Permissions::from_mode(0o600)).unwrap();
        let moved = dirs.sub("registry.json");
        std::fs::rename(&registry, &moved).unwrap();
        std::os::unix::fs::symlink(&moved, &registry).unwrap();
        assert!(matches!(
            start(host_dir.clone()).await,
            Err(RemoteError::InsecureState(_))
        ));

        // A symlinked state directory is refused.
        let linked = dirs.sub("linked-state");
        std::os::unix::fs::symlink(&phone_dir, &linked).unwrap();
        let mut config = ClientConfig::new(&linked);
        config.bind_addrs = vec![LOOPBACK.parse().unwrap()];
        assert!(matches!(
            Client::new(config),
            Err(RemoteError::InsecureState(_))
        ));

        // The device key follows the same rules.
        let device_key = phone_dir.join("remote-device.key");
        std::fs::set_permissions(&device_key, std::fs::Permissions::from_mode(0o604)).unwrap();
        assert!(matches!(
            Client::new(ClientConfig::new(&phone_dir)),
            Err(RemoteError::InsecureState(_))
        ));
        // A wrong-length key is diagnosed as damaged rather than replaced.
        std::fs::set_permissions(&device_key, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::write(&device_key, b"short").unwrap();
        assert!(matches!(
            Client::new(ClientConfig::new(&phone_dir)),
            Err(RemoteError::CorruptState(_))
        ));
        assert_eq!(std::fs::read(device_key).unwrap(), b"short");
    })
    .await;
}

#[test]
fn link_round_trip_rejections_fingerprint_and_qr() {
    let host = iroh::SecretKey::generate().public();
    let device = iroh::SecretKey::generate().public();
    let link = PairingLink {
        host_id: host,
        relay: Some("https://relay.example.test".parse().unwrap()),
        addrs: vec![
            "192.168.1.20:4433".parse().unwrap(),
            "[fe80::1]:4433".parse().unwrap(),
        ],
        secret: [9u8; 32],
        machine_name: "Desk Machine".into(),
    };
    let text = link.encode().unwrap();
    assert!(text.starts_with("cortex-pair:"));
    assert!(text["cortex-pair:".len()..]
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
    assert_eq!(PairingLink::decode(&text).unwrap(), link);
    assert_eq!(PairingLink::decode(&format!("  {text}\n")).unwrap(), link);
    let bare = PairingLink {
        relay: None,
        addrs: vec![],
        ..link.clone()
    };
    assert_eq!(PairingLink::decode(&bare.encode().unwrap()).unwrap(), bare);

    use workspace_remote::LinkError;
    let malformed = |t: &str| matches!(PairingLink::decode(t), Err(LinkError::Malformed(_)));
    assert!(malformed(""));
    assert!(malformed("https://example.test"));
    assert!(malformed(&text.replacen(
        "cortex-pair:",
        "cortex-pairing:",
        1
    )));
    assert!(malformed(&format!("{text}*")));
    assert!(malformed(&text[..text.len() - 6]));
    // Change one character in the middle: the checksum catches it.
    let mut chars: Vec<char> = text.chars().collect();
    let i = chars.len() / 2;
    chars[i] = if chars[i] == 'A' { 'B' } else { 'A' };
    assert!(malformed(&chars.into_iter().collect::<String>()));
    let unknown = format!(
        "cortex-pair:{}",
        data_encoding::BASE64URL_NOPAD.encode(&[2u8, 1, 2, 3, 4, 5, 6, 7])
    );
    assert_eq!(
        PairingLink::decode(&unknown),
        Err(LinkError::UnsupportedVersion(2))
    );
    assert_eq!(
        PairingLink::decode(&unknown).unwrap_err().code(),
        "unsupported_pairing_link_version"
    );
    let long_name = PairingLink {
        machine_name: "x".repeat(65),
        ..link.clone()
    };
    assert!(long_name.encode().is_err());

    let a = workspace_remote::fingerprint(&host, &device, &[9u8; 32]);
    assert_eq!(a, workspace_remote::fingerprint(&host, &device, &[9u8; 32]));
    assert_ne!(a, workspace_remote::fingerprint(&host, &device, &[8u8; 32]));
    assert_ne!(a, workspace_remote::fingerprint(&device, &host, &[9u8; 32]));
    assert_eq!(a.len(), 14);
    let groups: Vec<&str> = a.split(' ').collect();
    assert_eq!(groups.len(), 3);
    assert!(groups.iter().all(|g| g.len() == 4
        && g.bytes()
            .all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b))));

    let qr = qr_rows(&text);
    assert!(!qr.is_empty());
    assert!(qr
        .iter()
        .all(|row| row.len() == qr.len() && row.bytes().all(|b| b == b'0' || b == b'1')));
    assert!(qr.iter().any(|row| row.contains('1')));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revoke_ends_an_in_flight_reply_and_prevents_later_frames() {
    bounded(async {
        let dirs = TempDir::new("revoke-in-flight");
        let received = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let (finished, mut observed) = tokio::sync::mpsc::channel(1);
        let mut config = HostConfig::new(
            dirs.sub("host"),
            "Test machine",
            {
                let received = received.clone();
                let release = release.clone();
                move || {
                    let received = received.clone();
                    let release = release.clone();
                    let finished = finished.clone();
                    async move {
                        let (tunnel, mut local) = tokio::io::duplex(65536);
                        tokio::spawn(async move {
                            let first = read_frame(&mut local).await.unwrap().unwrap();
                            assert_eq!(first["method"], "in.flight");
                            received.notify_one();
                            release.notified().await;
                            let write = write_frame(&mut local, &json!({"id":1,"result":"processed"})).await;
                            let next = read_frame(&mut local).await;
                            finished.send((write.is_ok(), next)).await.unwrap();
                        });
                        Ok(Box::new(tunnel) as LocalStream)
                    }
                }
            },
            |_, _| true,
        );
        config.bind_addrs = vec![LOOPBACK.parse().unwrap()];
        let host = Host::start(config).await.unwrap();
        host.set_enabled(true).await.unwrap();
        let phone = client(&dirs.sub("phone"));
        let id = pair(&host, &phone).await;
        let conn = phone.connect(&id).await.unwrap();
        let mut stream = conn.open().await.unwrap();
        write_frame(&mut stream, &json!({"jsonrpc":"2.0","id":1,"method":"in.flight"})).await.unwrap();
        received.notified().await;

        let started = std::time::Instant::now();
        host.revoke(&phone.device_id()).unwrap();
        let returned_after = started.elapsed();
        let stale_write = write_frame(&mut stream, &json!({"jsonrpc":"2.0","id":2,"method":"after.revoke"})).await;
        release.notify_one();
        let result = tokio::time::timeout(Duration::from_secs(5), read_frame(&mut stream)).await.unwrap();
        wait_state(&conn, |s| *s == ConnectionState::Rejected).await;
        let rejected_after = started.elapsed();
        let (service_wrote, next) = observed.recv().await.unwrap();
        eprintln!(
            "revoke returned in {returned_after:?}, client observed rejection in {rejected_after:?}; \
             stale write accepted locally={}, service reply write={}, client reply={result:?}",
            stale_write.is_ok(), service_wrote,
        );
        assert!(!matches!(result, Ok(Some(_))), "the already-started request has no reply");
        assert!(next.unwrap().is_none(), "no subsequent frame may reach the service");
        assert!(matches!(conn.open().await, Err(ConnectError::Rejected)));
        conn.close().await;
        host.shutdown().await;
    }).await;
}

fn write_private(path: &Path, bytes: &[u8]) {
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
}

/// Give a test client a route without using the pairing flow or changing its identity.
fn routed_client(dir: &Path, host_id: iroh::EndpointId, addrs: &[SocketAddr]) -> Client {
    drop(client(dir));
    let record = json!({"machines":[{
        "machineId": host_id.to_string(), "name": "Stored machine", "relayUrl": null,
        "addrs": addrs, "pairedAtMs": 0, "lastConnectedMs": null,
    }]});
    write_private(
        &dir.join("remote-machines.json"),
        &serde_json::to_vec(&record).unwrap(),
    );
    client(dir)
}

async fn test_endpoint(alpns: &[&[u8]]) -> iroh::Endpoint {
    iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
        .relay_mode(iroh::RelayMode::Disabled)
        .clear_address_lookup()
        .clear_ip_transports()
        .bind_addr_with_opts(LOOPBACK.parse::<SocketAddr>().unwrap(), Default::default())
        .unwrap()
        .alpns(alpns.iter().map(|alpn| alpn.to_vec()).collect())
        .bind()
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_device_that_was_never_paired_is_rejected() {
    bounded(async {
        let dirs = TempDir::new("never-paired");
        let service = Service::default();
        let host = Host::start(host_config(
            &dirs.sub("host"),
            &service,
            Duration::from_secs(60),
        ))
        .await
        .unwrap();
        host.set_enabled(true).await.unwrap();
        let id = host.endpoint_id();
        let outsider = routed_client(
            &dirs.sub("outsider"),
            id.parse().unwrap(),
            &host.direct_addrs().await,
        );
        let connection = outsider.connection(&id).unwrap();
        assert!(matches!(
            connection.open().await,
            Err(ConnectError::Rejected)
        ));
        assert_eq!(*connection.state().borrow(), ConnectionState::Rejected);
        connection.close().await;
        assert!(host.status().devices.is_empty());
        assert!(service.methods().is_empty());
        host.shutdown().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_approval_does_not_grant_access_and_failed_revoke_does_not_restore_it() {
    bounded(async {
        let dirs = TempDir::new("failed-registry-write");
        let dir = dirs.sub("host");
        let service = Service::default();
        let host = Host::start(host_config(&dir, &service, Duration::from_secs(60)))
            .await
            .unwrap();
        host.set_enabled(true).await.unwrap();
        let phone_dir = dirs.sub("phone");
        let phone = client(&phone_dir);
        let id = host.endpoint_id();
        let offer = host.pairing_start().await.unwrap();
        let attempt = phone.pair_begin(&offer.link, "Phone").await.unwrap();
        let pending = wait_pending(&host).await;
        let registry = dir.join("remote-devices.json");
        std::fs::create_dir(&registry).unwrap();
        assert!(matches!(
            host.decide(&pending.request_id, true),
            Err(RemoteError::InsecureState(_))
        ));
        assert!(matches!(attempt.finish().await, Err(PairError::Cancelled)));
        assert!(host.status().devices.is_empty());
        assert!(host.status().pending.is_empty());
        assert!(phone.machines().is_empty());
        drop(phone);

        // Same device key and route; even an attempted RPC without local pairing is rejected.
        let phone = routed_client(&phone_dir, id.parse().unwrap(), &host.direct_addrs().await);
        assert!(matches!(
            phone.connect(&id).await,
            Err(ConnectError::Rejected)
        ));

        std::fs::remove_dir(&registry).unwrap();
        assert_eq!(pair(&host, &phone).await, id);
        let conn = phone.connect(&id).await.unwrap();
        let mut stream = conn.open().await.unwrap();
        assert_eq!(call(&mut stream, 1, "echo", json!(1)).await["result"], 1);
        std::fs::rename(&registry, dir.join("previous-registry.json")).unwrap();
        std::fs::create_dir(&registry).unwrap();
        assert!(matches!(
            host.revoke(&phone.device_id()),
            Err(RemoteError::InsecureState(_))
        ));
        let retry_rows = host.status().devices;
        assert_eq!(retry_rows.len(), 1);
        assert_eq!(retry_rows[0].device_id, phone.device_id());
        assert!(!retry_rows[0].connected);
        let result = tokio::time::timeout(Duration::from_secs(5), read_frame(&mut stream))
            .await
            .unwrap();
        assert!(!matches!(result, Ok(Some(_))));
        assert!(matches!(conn.open().await, Err(ConnectError::Rejected)));
        conn.close().await;
        host.shutdown().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn version_probe_is_constants_only_even_with_pairing_and_devices() {
    bounded(async {
        let dirs = TempDir::new("probe-privacy");
        let service = Service::default();
        let host = Host::start(host_config(&dirs.sub("host"), &service, Duration::from_secs(60)))
            .await.unwrap();
        host.set_enabled(true).await.unwrap();
        let phone = client(&dirs.sub("phone"));
        pair(&host, &phone).await;
        let offer = host.pairing_start().await.unwrap();
        let link = PairingLink::decode(&offer.link).unwrap();
        let addr = iroh::EndpointAddr::from_parts(
            link.host_id,
            link.addrs.into_iter().map(iroh::TransportAddr::Ip),
        );
        let probe = test_endpoint(&[]).await;
        let conn = probe.connect(addr.clone(), VERSION_ALPN).await.unwrap();
        let (mut send, mut recv) = conn.open_bi().await.unwrap();
        AsyncWriteExt::write_all(&mut send, b"\x01").await.unwrap();
        send.finish().unwrap();
        let frame = recv.read_to_end(260).await.unwrap();
        assert_eq!(
            workspace_protocol::decode_frame(&frame).unwrap(),
            json!({
                "supported": [
                    std::str::from_utf8(PAIRING_ALPN).unwrap(),
                    std::str::from_utf8(RPC_ALPN).unwrap()
                ],
                "wire": format!("{}.{}", workspace_protocol::WIRE_MAJOR, workspace_protocol::WIRE_MINOR),
            })
        );
        conn.close(0u32.into(), b"done");

        let conn = probe.connect(addr, VERSION_ALPN).await.unwrap();
        let (mut send, mut recv) = conn.open_bi().await.unwrap();
        AsyncWriteExt::write_all(&mut send, b"\x01\x01").await.unwrap();
        send.finish().unwrap();
        assert!(!matches!(recv.read_to_end(260).await, Ok(reply) if !reply.is_empty()));
        assert!(host.status().pairing.is_some());
        assert_eq!(host.status().devices.len(), 1);
        assert_eq!(*service.connections.lock().unwrap(), 0);
        probe.close().await;
        host.shutdown().await;
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_pair_and_rpc_dials_distinguish_mismatch_from_no_probe() {
    bounded(async {
        let dirs = TempDir::new("probe-mismatch");
        for (case, alpns, advertised, incompatible) in [
            ("different", vec![VERSION_ALPN], vec!["cortex-workspace/pair/2", "cortex-workspace/rpc/2"], true),
            ("old", vec![b"cortex-workspace/rpc/0".as_slice()], vec![], false),
            ("same", vec![VERSION_ALPN], vec![
                std::str::from_utf8(PAIRING_ALPN).unwrap(),
                std::str::from_utf8(RPC_ALPN).unwrap(),
            ], false),
        ] {
            let machine = test_endpoint(&alpns).await;
            let link = PairingLink {
                host_id: machine.id(),
                relay: None,
                addrs: machine.bound_sockets(),
                secret: [1; 32],
                machine_name: "Fixture".into(),
            };
            let reply = json!({"supported": advertised, "wire": "2.0"});
            let responder = alpns.contains(&VERSION_ALPN).then(|| {
                let machine = machine.clone();
                tokio::spawn(async move {
                    let mut answered = 0;
                    while answered < 2 {
                        let incoming = machine.accept().await.unwrap();
                        let Ok(conn) = incoming.await else {
                            // The original unsupported dial reaches this endpoint too.
                            continue;
                        };
                        answered += 1;
                        assert_eq!(conn.alpn(), VERSION_ALPN);
                        let (mut send, mut recv) = conn.accept_bi().await.unwrap();
                        assert_eq!(recv.read_to_end(1).await.unwrap(), b"\x01");
                        write_frame(&mut send, &reply).await.unwrap();
                        send.finish().unwrap();
                        conn.closed().await;
                    }
                })
            });
            let phone = routed_client(&dirs.sub(case), link.host_id, &link.addrs);
            let pair_error = phone.pair_begin(&link.encode().unwrap(), "Phone").await.unwrap_err();
            let conn = phone.connection(&link.host_id.to_string()).unwrap();
            let Err(connect_error) = conn.open().await else {
                panic!("RPC should not have connected: {case}");
            };
            if incompatible {
                assert_eq!(pair_error.to_string(), INCOMPATIBLE_VERSION_CODE);
                assert_eq!(connect_error.to_string(), INCOMPATIBLE_VERSION_CODE);
                assert!(matches!(pair_error, PairError::IncompatibleVersion(info)
                    if info.wire == "2.0" && info.supported == ["cortex-workspace/pair/2", "cortex-workspace/rpc/2"]));
                assert!(matches!(connect_error, ConnectError::IncompatibleVersion(info) if info.wire == "2.0"));
                assert_eq!(*conn.state().borrow(), ConnectionState::IncompatibleVersion);
            } else {
                assert!(matches!(pair_error, PairError::Unreachable), "{case}: {pair_error}");
                assert!(matches!(connect_error, ConnectError::Unreachable), "{case}: {connect_error}");
            }
            if let Some(responder) = responder {
                responder.await.unwrap();
            }
            conn.close().await;
            machine.close().await;
        }
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_or_malformed_frames_close_the_stream_unforwarded() {
    bounded(async {
        let dirs = TempDir::new("frames");
        let service = Service::default();
        let host = Host::start(host_config(
            &dirs.sub("host"),
            &service,
            Duration::from_secs(60),
        ))
        .await
        .unwrap();
        host.set_enabled(true).await.unwrap();
        let phone = client(&dirs.sub("phone"));
        let machine_id = pair(&host, &phone).await;
        let conn = phone.connect(&machine_id).await.unwrap();

        let oversized = ((workspace_protocol::FRAME_BYTES + 1) as u32).to_be_bytes();
        let not_json = {
            let mut f = 8u32.to_be_bytes().to_vec();
            f.extend_from_slice(b"not json");
            f
        };
        let no_method = workspace_protocol::encode_frame(&json!({"jsonrpc":"2.0","id":1})).unwrap();
        let zero = 0u32.to_be_bytes().to_vec();
        for bad in [oversized.to_vec(), not_json, no_method, zero] {
            let mut stream = conn.open().await.unwrap();
            stream.write_all(&bad).await.unwrap();
            stream.write_all(&[0u8; 64]).await.unwrap();
            let result = tokio::time::timeout(Duration::from_secs(10), read_frame(&mut stream))
                .await
                .unwrap();
            assert!(
                !matches!(result, Ok(Some(_))),
                "bad frame must close the stream"
            );
        }
        assert!(service.methods().is_empty(), "nothing reached the service");

        // The connection itself is still fine.
        let mut stream = conn.open().await.unwrap();
        assert_eq!(
            call(&mut stream, 1, "echo", json!(true)).await["result"],
            json!(true)
        );
        assert_eq!(service.methods(), vec!["echo"]);
        conn.close().await;
        host.shutdown().await;
    })
    .await;
}
