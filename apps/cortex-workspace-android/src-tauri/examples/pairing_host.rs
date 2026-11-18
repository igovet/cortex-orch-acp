//! A throwaway machine for checking a phone by hand: a bare machine service (no engines, no
//! projects) in a temporary folder, with remote access on. It prints a pairing link, allows the
//! first phone that presents it after showing the fingerprint, and serves until stopped.
//!
//!   cargo run -p cortex-workspace-android --example pairing_host [-- --no-relay]
//!
//! It never touches the owner's real service, state or projects.
use serde_json::{json, Value};
use std::{os::unix::fs::PermissionsExt, sync::Arc, time::Duration};
use workspace_service::{connect, read_frame, write_frame, Config, LaunchSpec, Service};

async fn owner(service: &Service, method: &str, params: Value) -> Value {
    let mut stream = connect(service.socket_path()).await.expect("local socket");
    write_frame(&mut stream, &json!({"jsonrpc":"2.0","id":"1","method":method,"params":params}))
        .await
        .expect("request");
    let reply = read_frame(&mut stream).await.expect("reply");
    assert!(reply.get("error").is_none(), "{method}: {reply}");
    reply["result"].clone()
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    let relay = if std::env::args().any(|a| a == "--no-relay") {
        None
    } else {
        workspace_remote::default_relay().map(str::to_owned)
    };
    let root = std::env::temp_dir().join(format!("workspace-pairing-host-{}", std::process::id()));
    std::fs::create_dir(&root).expect("temporary folder");
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).expect("private folder");
    std::fs::create_dir(root.join("projects")).expect("projects folder");
    let service: Arc<Service> = Service::bind(Config {
        runtime_dir: root.join("runtime"),
        state_dir: root.join("state"),
        allowed_root: root.join("projects"),
        launch: LaunchSpec::bare(),
        request_timeout: Duration::from_secs(10),
        shutdown_timeout: Duration::from_millis(300),
        event_capacity: 64,
    })
    .await
    .expect("service");
    service.start_remote_with(Vec::new(), relay).await;
    tokio::spawn(service.clone().serve());
    let status = owner(&service, "remote.enable", json!({"enabled":true})).await;
    println!("machine {} relay {}", status["machineId"], status["relay"]);
    let offer = owner(&service, "remote.pairing.start", json!({})).await;
    println!("LINK {}", offer["pairing"]["link"].as_str().unwrap_or_default());
    let mut allowed = false;
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let status = owner(&service, "remote.status", json!({})).await;
        if !allowed {
            if let Some(request) = status["pending"].get(0) {
                println!("PENDING {} fingerprint {}", request["deviceName"], request["fingerprint"]);
                owner(&service, "remote.pairing.decide", json!({"requestId":request["requestId"],"allow":true})).await;
                println!("ALLOWED");
                allowed = true;
            }
        } else {
            println!("DEVICES {}", status["devices"]);
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }
}
