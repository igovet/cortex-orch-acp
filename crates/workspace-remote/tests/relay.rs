//! A live check of one relay: a phone that knows no direct address of the machine pairs and
//! exchanges a request through the relay alone. It needs the network and a running relay, so it
//! runs only when `CORTEX_RELAY_TEST_URL` names one, for example:
//!
//!   CORTEX_RELAY_TEST_URL=https://relay.example.org \
//!     cargo test -p workspace-remote --test relay -- --nocapture
//!
//! Without the variable the test reports itself skipped and proves nothing.

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use workspace_remote::link::PairingLink;
use workspace_remote::{Client, ClientConfig, ConnectionState, Host, HostConfig, LocalStream};

async fn frame_in<R: tokio::io::AsyncRead + Unpin>(reader: &mut R) -> std::io::Result<Value> {
    let mut prefix = [0u8; 4];
    reader.read_exact(&mut prefix).await?;
    let mut body = vec![0u8; u32::from_be_bytes(prefix) as usize];
    reader.read_exact(&mut body).await?;
    serde_json::from_slice(&body).map_err(std::io::Error::other)
}

async fn frame_out<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    value: &Value,
) -> std::io::Result<()> {
    writer
        .write_all(&workspace_protocol::encode_frame(value).unwrap())
        .await?;
    writer.flush().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_phone_reaches_the_machine_through_the_relay_alone() {
    let Ok(relay) = std::env::var("CORTEX_RELAY_TEST_URL") else {
        eprintln!("skipped: CORTEX_RELAY_TEST_URL is not set, so no relay was checked");
        return;
    };
    tokio::time::timeout(Duration::from_secs(120), scenario(relay))
        .await
        .expect("the relay scenario exceeded its time bound");
}

async fn scenario(relay: String) {
    let root = std::env::temp_dir().join(format!("workspace-remote-relay-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();

    // The machine: a local service that echoes each request's params.
    let mut config = HostConfig::new(
        root.join("host"),
        "Relay check",
        || async {
            let (near, mut far) = tokio::io::duplex(64 * 1024);
            tokio::spawn(async move {
                while let Ok(request) = frame_in(&mut far).await {
                    let reply =
                        json!({"jsonrpc":"2.0","id":request["id"],"result":request["params"]});
                    if frame_out(&mut far, &reply).await.is_err() {
                        break;
                    }
                }
            });
            Ok(Box::new(near) as LocalStream)
        },
        |_: &str, _: &Value| true,
    );
    config.default_relay = Some(relay.clone());
    let host = Host::start(config).await.unwrap();
    host.set_enabled(true).await.unwrap();
    let offer = host.pairing_start().await.unwrap();

    // The phone gets the link without any direct address: only the relay can carry it.
    let mut link = PairingLink::decode(&offer.link).unwrap();
    assert_eq!(
        link.relay
            .as_ref()
            .map(|url| url.to_string().trim_end_matches('/').to_owned()),
        Some(relay.trim_end_matches('/').to_owned()),
        "the link names the relay"
    );
    link.addrs.clear();
    let link = link.encode().unwrap();

    let mut phone = ClientConfig::new(root.join("phone"));
    phone.connect_timeout = Duration::from_secs(30);
    let client = Client::new(phone).unwrap();
    let attempt = client
        .pair_begin(&link, "Relay check phone")
        .await
        .expect("pairing reaches the machine through the relay");
    let request = loop {
        if let Some(request) = host.status().pending.first().cloned() {
            break request;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(request.fingerprint, attempt.fingerprint);
    host.decide(&request.request_id, true).unwrap();
    let machine = attempt.finish().await.unwrap();
    assert!(machine.addrs.is_empty() || machine.relay_url.is_some());

    let connection = client.connection(&machine.machine_id).unwrap();
    let mut stream = connection
        .open()
        .await
        .expect("the paired phone connects through the relay");
    frame_out(
        &mut stream,
        &json!({"jsonrpc":"2.0","id":"1","method":"echo","params":{"through":"relay"}}),
    )
    .await
    .unwrap();
    let reply = frame_in(&mut stream).await.unwrap();
    assert_eq!(reply["result"], json!({"through":"relay"}), "{reply}");
    let state = connection.state().borrow().clone();
    eprintln!("relay {relay}: request answered; connection state {state:?}");
    assert!(
        matches!(state, ConnectionState::Connected { .. }),
        "{state:?}"
    );

    connection.close().await;
    host.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}
