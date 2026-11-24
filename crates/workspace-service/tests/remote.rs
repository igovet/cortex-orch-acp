//! Remote access end to end: a real service on a temporary socket hosts the remote
//! endpoint on loopback (no relay, no network), a `workspace_remote::Client` pairs with it
//! in-process, and the tunnel carries the service's own JSON-RPC under the remote policy.
#[path = "support/hermetic.rs"]
mod hermetic;
use serde_json::{json, Value};
use std::{
    io::Write as _,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use workspace_remote::{Client, ClientConfig, ConnectionState};
use workspace_service::{connect, read_frame, Config, LaunchSpec, Service};

const LIMIT: Duration = Duration::from_secs(90);
const STEP: Duration = Duration::from_secs(15);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "workspace-remote-e2e-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&p).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::create_dir(p.join("projects")).unwrap();
        Self(p)
    }
    fn config(&self) -> Config {
        Config {
            runtime_dir: self.0.join("runtime"),
            state_dir: self.0.join("state"),
            allowed_root: self.0.join("projects"),
            launch: LaunchSpec::bare(),
            request_timeout: Duration::from_secs(5),
            shutdown_timeout: Duration::from_millis(150),
            event_capacity: 64,
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn send<S: AsyncWrite + Unpin>(stream: &mut S, id: &str, method: &str, params: Value) {
    let frame = workspace_protocol::encode_frame(
        &json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
    )
    .unwrap();
    stream.write_all(&frame).await.unwrap();
    stream.flush().await.unwrap();
}

async fn receive<S: AsyncRead + Unpin>(stream: &mut S) -> std::io::Result<Value> {
    tokio::time::timeout(STEP, read_frame(stream))
        .await
        .map_err(|_| std::io::Error::other("test_read_timeout"))?
}

/// One request on a fresh stream.
async fn call<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    method: &str,
    params: Value,
) -> Value {
    send(stream, "1", method, params).await;
    let reply = receive(stream).await.unwrap();
    assert_eq!(reply["id"], "1", "{reply}");
    reply
}

/// The owner's own request on the local socket.
async fn owner(service: &Service, method: &str, params: Value) -> Value {
    let mut stream = connect(service.socket_path()).await.unwrap();
    let reply = call(&mut stream, method, params).await;
    assert!(reply.get("error").is_none(), "{method}: {reply}");
    reply["result"].clone()
}

async fn until(service: &Service, check: impl Fn(&Value) -> bool) -> Value {
    for _ in 0..500 {
        let status = owner(service, "remote.status", json!({})).await;
        if check(&status) {
            return status;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("remote status never reached the expected state");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_paired_phone_reaches_the_service_only_through_the_remote_policy() {
    tokio::time::timeout(LIMIT, scenario())
        .await
        .expect("remote end-to-end scenario exceeded its time bound");
}

async fn assert_owner_sees_failed_approval(
    service: &Service,
    f: &Fixture,
    phone: &Client,
    link: &str,
) {
    let attempt = phone.pair_begin(link, "Test Phone").await.unwrap();
    let pending = until(service, |s| {
        s["pending"].as_array().is_some_and(|p| !p.is_empty())
    })
    .await;
    let path = f.config().state_dir.join("remote/remote-devices.json");
    std::fs::create_dir(&path).unwrap();
    let mut local = connect(service.socket_path()).await.unwrap();
    let response = call(
        &mut local,
        "remote.pairing.decide",
        json!({
            "requestId": pending["pending"][0]["requestId"], "allow": true
        }),
    )
    .await;
    assert_eq!(response["error"]["message"], "remote_state_unavailable");
    assert!(matches!(
        attempt.finish().await,
        Err(workspace_remote::PairError::Cancelled)
    ));
    let status = owner(service, "remote.status", json!({})).await;
    assert_eq!(status["devices"], json!([]));
    std::fs::remove_dir(path).unwrap();
}

#[tokio::test]
async fn damaged_remote_state_keeps_the_endpoint_closed_and_the_file_intact() {
    tokio::time::timeout(LIMIT, async {
        for (file, contents) in [
            ("remote-devices.json", b"not JSON".as_slice()),
            ("remote-settings.json", br#"{"enabled":"not a bool"}"#.as_slice()),
        ] {
            let f = Fixture::new();
            let service: Arc<Service> = Service::bind(f.config()).await.unwrap();
            let dir = f.config().state_dir.join("remote");
            std::fs::create_dir(&dir).unwrap();
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
            let write_private = |name: &str, bytes: &[u8]| {
                std::fs::OpenOptions::new().create_new(true).write(true).mode(0o600)
                    .open(dir.join(name)).unwrap().write_all(bytes).unwrap();
            };
            if file == "remote-devices.json" {
                write_private("remote-settings.json", br#"{"enabled":true}"#);
            }
            write_private(file, contents);
            let reserved = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
            let addr = reserved.local_addr().unwrap();
            drop(reserved);
            service.start_remote_with(vec![addr], None).await;
            let _still_free = std::net::UdpSocket::bind(addr).expect("endpoint must not bind");
            assert_eq!(std::fs::read(dir.join(file)).unwrap(), contents);
            assert!(!dir.join("remote-host.key").exists());
            let runner = tokio::spawn(service.clone().serve());
            let status = owner(&service, "remote.status", json!({})).await;
            assert_eq!(status["available"], false);
            assert_eq!(status["enabled"], false);
            assert_eq!(status["reason"], "Remote access could not start: its saved state is damaged. It stays off until the file is fixed.");
            service.stop().await.unwrap();
            tokio::time::timeout(STEP, runner).await.unwrap().unwrap().unwrap();
        }
    }).await.unwrap();
}

async fn scenario() {
    let f = Fixture::new();
    let address = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let bind_address = address.local_addr().unwrap();
    drop(address);
    let service: Arc<Service> = Service::bind(f.config()).await.unwrap();
    service.start_remote_with(vec![bind_address], None).await;
    let runner = tokio::spawn(service.clone().serve());

    // The owner sees remote access available and off, and the methods advertised.
    let methods = owner(&service, "service.capabilities", json!({})).await["methods"].clone();
    for method in [
        "remote.status",
        "remote.enable",
        "remote.pairing.start",
        "remote.pairing.cancel",
        "remote.pairing.decide",
        "remote.devices.revoke",
        "remote.relay.set",
    ] {
        assert!(
            methods.as_array().unwrap().iter().any(|m| m == method),
            "{method}"
        );
    }
    let status = owner(&service, "remote.status", json!({})).await;
    assert_eq!(status["available"], true, "{status}");
    assert_eq!(status["enabled"], false);
    assert_eq!(status["relay"], json!({"url":null,"source":"none"}));
    assert_eq!(status["pairing"], Value::Null);
    let enabled = owner(&service, "remote.enable", json!({"enabled":true})).await;
    assert_eq!(enabled["enabled"], true);
    let offer = owner(&service, "remote.pairing.start", json!({})).await["pairing"].clone();
    let link = offer["link"].as_str().unwrap().to_owned();
    assert!(offer["qr"].as_array().is_some_and(|rows| !rows.is_empty()));
    assert!(offer["expiresAt"].as_u64().is_some());

    // The phone pairs; the owner allows it from the pending list.
    let phone_state = f.0.join("phone");
    let mut config = ClientConfig::new(&phone_state);
    config.bind_addrs = vec!["127.0.0.1:0".parse().unwrap()];
    config.connect_timeout = Duration::from_secs(5);
    let client = Client::new(config).unwrap();
    assert_owner_sees_failed_approval(&service, &f, &client, &link).await;
    let offer = owner(&service, "remote.pairing.start", json!({})).await;
    let link = offer["pairing"]["link"].as_str().unwrap();
    let attempt = client.pair_begin(link, "Test Phone").await.unwrap();
    let pending = until(&service, |s| {
        s["pending"].as_array().is_some_and(|p| !p.is_empty())
    })
    .await;
    let request = pending["pending"][0].clone();
    let mut keys: Vec<_> = request.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(
        keys,
        ["deviceName", "fingerprint", "requestId", "requestedAt"]
    );
    assert_eq!(request["deviceName"], "Test Phone");
    assert_eq!(request["fingerprint"], attempt.fingerprint);
    owner(
        &service,
        "remote.pairing.decide",
        json!({"requestId":request["requestId"],"allow":true}),
    )
    .await;
    let machine = attempt.finish().await.unwrap();
    assert_eq!(machine.machine_id, enabled["machineId"]);
    let connection = client.connection(&machine.machine_id).unwrap();

    // Ordinary reads go through the tunnel to the real service.
    let mut stream = connection.open().await.unwrap();
    let status = call(&mut stream, "service.status", json!({})).await;
    assert_eq!(status["result"]["transport"], "unix", "{status}");
    assert_eq!(
        status["result"]["protocol"]["major"],
        workspace_protocol::WIRE_MAJOR
    );
    assert_eq!(
        status["result"]["protocol"]["minor"],
        workspace_protocol::WIRE_MINOR
    );
    // Owner-only and machine-bound methods are answered by the tunnel, not forwarded.
    for (method, params) in [
        ("remote.status", json!({})),
        (
            "remote.devices.revoke",
            json!({"deviceId":client.device_id()}),
        ),
        (
            "terminal.open",
            json!({"projectId":"p","requestId":"r","cols":80,"rows":24}),
        ),
        ("service.stop", json!({})),
        (
            "machine.call",
            json!({"op":"repo.file.save","projectId":"p","params":{}}),
        ),
        ("machine.call", json!({"op":"repo.file.lease.acquire","projectId":"p","params":{"file":"a.txt"}})),
        ("machine.call", json!({"op":"repo.file.lease.renew","projectId":"p","params":{"file":"a.txt"}})),
        ("machine.call", json!({"op":"repo.file.lease.release","projectId":"p","params":{"file":"a.txt"}})),
        ("machine.call", json!({"op":"repo.file.lease.status","projectId":"p","params":{"file":"a.txt"}})),
        ("service.subscribe.code", json!({})),
        ("machine.call", json!({"op":"repo.lsp.open","projectId":"p","params":{"file":"a.rs","text":"fn main() {}","version":1}})),
        ("machine.call", json!({"op":"repo.lsp.change","projectId":"p","params":{}})),
        ("machine.call", json!({"op":"repo.lsp.definition","projectId":"p","params":{}})),
        ("machine.call", json!({"op":"repo.lsp.close","projectId":"p","params":{}})),
        ("machine.call", json!({"op":"repo.lsp.status","projectId":"p","params":{}})),
        ("machine.call", json!({"op":"repo.lsp.settings","params":{}})),
        ("machine.call", json!({"op":"machine.update","params":{"patch":{"rustAnalyzerPath":"/does-not-exist"}}})),
        ("brand.new", json!({})),
    ] {
        let reply = call(&mut stream, method, params).await;
        assert_eq!(
            reply["error"]["message"], "remote_operation_refused",
            "{method}: {reply}"
        );
    }
    // The stream stays usable after a refusal.
    let projects = call(&mut stream, "projects.list", json!({})).await;
    assert_eq!(projects["result"], json!([]), "{projects}");
    drop(stream);

    // A subscription through the tunnel receives the service's events.
    let mut subscription = connection.open().await.unwrap();
    let ack = call(&mut subscription, "service.subscribe", json!({})).await;
    assert_eq!(ack["result"]["subscribed"], true, "{ack}");
    owner(&service, "remote.pairing.cancel", json!({})).await;
    owner(&service, "remote.pairing.start", json!({})).await;
    loop {
        let event = receive(&mut subscription).await.expect("an event arrives");
        assert_eq!(event["method"], "service.event", "{event}");
        if event["params"]["type"] == "remote_state" {
            break;
        }
    }
    let device =
        until(&service, |s| s["devices"][0]["connected"] == true).await["devices"][0].clone();
    assert_eq!(device["deviceId"], client.device_id());
    assert_eq!(device["name"], "Test Phone");
    assert!(device["pairedAt"].as_u64().is_some());

    // Even a failed save immediately cuts off the device, while keeping a local row
    // from which the owner can retry. Damage only the mode, leaving the old grant on disk.
    let registry = f.config().state_dir.join("remote/remote-devices.json");
    let previous = std::fs::read(&registry).unwrap();
    std::fs::set_permissions(&registry, std::fs::Permissions::from_mode(0o644)).unwrap();
    let params = json!({"deviceId":client.device_id()});
    let mut local = connect(service.socket_path()).await.unwrap();
    let failed = call(&mut local, "remote.devices.revoke", params.clone()).await;
    assert_eq!(failed["error"]["message"], "remote_state_unavailable");
    drop(local);
    let retry_row = owner(&service, "remote.status", json!({})).await;
    assert_eq!(retry_row["devices"].as_array().unwrap().len(), 1);
    assert_eq!(retry_row["devices"][0]["deviceId"], client.device_id());
    assert_eq!(retry_row["devices"][0]["connected"], false);
    assert_eq!(std::fs::read(&registry).unwrap(), previous);
    loop {
        match receive(&mut subscription).await {
            Ok(event) => assert_eq!(event["method"], "service.event", "{event}"),
            Err(error) => {
                assert_ne!(
                    error.to_string(),
                    "test_read_timeout",
                    "the subscription stayed open"
                );
                break;
            }
        }
    }
    let mut refused = false;
    for _ in 0..3 {
        match connection.open().await {
            Err(workspace_remote::ConnectError::Rejected) => {
                refused = true;
                break;
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
            Ok(mut stream) => {
                // A stream racing the close must not reach the service.
                let frame = workspace_protocol::encode_frame(
                    &json!({"jsonrpc":"2.0","id":"1","method":"service.status","params":{}}),
                )
                .unwrap();
                if stream.write_all(&frame).await.is_ok() && stream.flush().await.is_ok() {
                    assert!(
                        receive(&mut stream).await.is_err(),
                        "a revoked device was served"
                    );
                }
            }
        }
    }
    assert!(refused, "a revoked device was not refused");
    assert_eq!(*connection.state().borrow(), ConnectionState::Rejected);

    std::fs::set_permissions(&registry, std::fs::Permissions::from_mode(0o600)).unwrap();
    let revoked = owner(&service, "remote.devices.revoke", params.clone()).await;
    assert_eq!(revoked["devices"], json!([]));

    service.stop().await.unwrap();
    tokio::time::timeout(STEP, runner)
        .await
        .expect("the service did not stop")
        .unwrap()
        .unwrap();
    drop(service);

    let restarted: Arc<Service> = Service::bind(f.config()).await.unwrap();
    restarted.start_remote_with(vec![bind_address], None).await;
    let runner = tokio::spawn(restarted.clone().serve());
    let status = owner(&restarted, "remote.status", json!({})).await;
    assert_eq!(status["enabled"], true);
    assert_eq!(status["machineId"], machine.machine_id);
    assert_eq!(status["devices"], json!([]));
    let mut local = connect(restarted.socket_path()).await.unwrap();
    let unknown = call(&mut local, "remote.devices.revoke", params).await;
    assert_eq!(unknown["error"]["message"], "unknown_device");
    drop(local);

    let mut config = ClientConfig::new(&phone_state);
    config.bind_addrs = vec!["127.0.0.1:0".parse().unwrap()];
    config.connect_timeout = Duration::from_secs(5);
    let fresh = Client::new(config)
        .unwrap()
        .connection(&machine.machine_id)
        .unwrap();
    assert!(matches!(
        fresh.open().await,
        Err(workspace_remote::ConnectError::Rejected)
    ));
    restarted.stop().await.unwrap();
    tokio::time::timeout(STEP, runner)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
