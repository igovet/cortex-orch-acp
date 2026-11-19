//! The phone's native side against a real machine service on a temporary socket: pairing,
//! the open machine's bridge, the phone's own refusals, and forgetting a machine. Loopback
//! only: no relay, no network, no web view.
#[path = "../../../../crates/workspace-service/tests/support/hermetic.rs"]
mod hermetic;
use cortex_workspace_android::phone::Phone;
use serde_json::{json, Value};
use std::{
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use workspace_service::{connect, read_frame, write_frame, Config, LaunchSpec, Service};

const LIMIT: Duration = Duration::from_secs(120);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("workspace-phone-e2e-{}", std::process::id()));
        std::fs::create_dir(&p).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::create_dir(p.join("projects")).unwrap();
        std::fs::create_dir(p.join("phone")).unwrap();
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

/// The owner's own request on the local socket.
async fn owner(service: &Service, method: &str, params: Value) -> Value {
    let mut stream = connect(service.socket_path()).await.unwrap();
    write_frame(
        &mut stream,
        &json!({"jsonrpc":"2.0","id":"1","method":method,"params":params}),
    )
    .await
    .unwrap();
    let reply = read_frame(&mut stream).await.unwrap();
    assert!(reply.get("error").is_none(), "{method}: {reply}");
    reply["result"].clone()
}

async fn eventually<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    for _ in 0..600 {
        if let Some(found) = check() {
            return found;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("never happened: {what}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_phone_pairs_opens_a_machine_and_forgets_it() {
    tokio::time::timeout(LIMIT, scenario())
        .await
        .expect("the phone scenario exceeded its time bound");
}

async fn scenario() {
    let f = Fixture::new();
    let service: Arc<Service> = Service::bind(f.config()).await.unwrap();
    service
        .start_remote_with(vec!["127.0.0.1:0".parse().unwrap()], None)
        .await;
    let runner = tokio::spawn(service.clone().serve());
    let machine_id = owner(&service, "remote.enable", json!({"enabled":true})).await["machineId"]
        .as_str()
        .unwrap()
        .to_owned();
    let link = owner(&service, "remote.pairing.start", json!({})).await["pairing"]["link"]
        .as_str()
        .unwrap()
        .to_owned();

    let events: Arc<Mutex<Vec<(String, Value)>>> = Arc::default();
    let seen = events.clone();
    let phone = Phone::open(
        &f.0.join("phone"),
        "Test Phone",
        Arc::new(move |name, payload| seen.lock().unwrap().push((name.to_owned(), payload))),
    )
    .unwrap();
    assert_eq!(phone.state(), json!({"active":null,"machines":[]}));

    // A link that is not one is refused before anything is sent.
    assert_eq!(
        phone.pair_begin("https://example.org").await.unwrap_err(),
        "invalid_link"
    );
    // Waiting with nothing begun is a cancelled pairing, not a hang.
    assert_eq!(phone.pair_finish("x").await.unwrap_err(), "cancelled");

    // The phone presents itself; the computer shows the same fingerprint and allows it.
    let begun = phone.pair_begin(&link).await.unwrap();
    let fingerprint = begun["fingerprint"].as_str().unwrap().to_owned();
    let mut pending = Value::Null;
    for _ in 0..400 {
        pending = owner(&service, "remote.status", json!({})).await["pending"][0].clone();
        if !pending.is_null() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(pending["deviceName"], "Test Phone", "{pending}");
    assert_eq!(pending["fingerprint"], fingerprint);
    let waiting = {
        let phone = phone.clone();
        tokio::spawn(async move { phone.pair_finish("Workstation").await })
    };
    owner(
        &service,
        "remote.pairing.decide",
        json!({"requestId":pending["requestId"],"allow":true}),
    )
    .await;
    assert_eq!(
        waiting.await.unwrap().unwrap(),
        json!({"machineId":machine_id})
    );

    // The first machine becomes the open one, under the name the owner typed, and connects.
    let state = eventually("the machine connects", || {
        let state = phone.state();
        (state["machines"][0]["state"] == "connected").then_some(state)
    })
    .await;
    assert_eq!(state["active"], machine_id.as_str());
    assert_eq!(state["machines"][0]["name"], "Workstation");
    assert_eq!(state["machines"][0]["path"], "direct", "{state}");
    assert!(events
        .lock()
        .unwrap()
        .iter()
        .any(|(name, _)| name == "phone-state"));

    // Its subscription reports the connection to the interface, named by machine.
    eventually("the transport reports connected", || {
        events
            .lock()
            .unwrap()
            .iter()
            .any(|(name, payload)| {
                name == "workspace-transport"
                    && payload["machineId"] == machine_id.as_str()
                    && payload["event"]["type"] == "connection"
                    && payload["event"]["state"] == "connected"
            })
            .then_some(())
    })
    .await;

    // A request reaches the real service; one for another machine does not leave the phone.
    let status = phone
        .request(&machine_id, "service.status".into(), json!({}))
        .await
        .unwrap();
    assert_eq!(status["transport"], "unix", "{status}");
    assert_eq!(
        phone
            .request("someone-else", "service.status".into(), json!({}))
            .await
            .unwrap_err(),
        "machine_not_open"
    );
    // What a phone must not send is refused on the phone itself.
    for (operation, message) in [
        ("remote.status", json!({})),
        (
            "terminal.open",
            json!({"projectId":"p","requestId":"r","cols":80,"rows":24}),
        ),
        ("native.pick_folder", json!({})),
        (
            "machine.call",
            json!({"op":"repo.file.save","projectId":"p","params":{}}),
        ),
    ] {
        let refusal = phone.request(&machine_id, operation.into(), message).await;
        assert!(refusal.is_err(), "{operation} was sent: {refusal:?}");
    }
    let projects = phone
        .request(&machine_id, "projects.list".into(), json!({}))
        .await
        .unwrap();
    assert_eq!(projects, json!([]));

    // The open machine survives a restart of the app.
    drop(phone);
    let phone = Phone::open(&f.0.join("phone"), "Test Phone", Arc::new(|_, _| ())).unwrap();
    let state = eventually("the machine reconnects after a restart", || {
        let state = phone.state();
        (state["machines"][0]["state"] == "connected").then_some(state)
    })
    .await;
    assert_eq!(state["active"], machine_id.as_str());
    assert_eq!(state["machines"][0]["name"], "Workstation");
    assert_eq!(
        phone.rename(&machine_id, "\u{7}").unwrap_err(),
        "invalid_name"
    );
    phone.rename(&machine_id, "Desk").unwrap();
    assert_eq!(phone.state()["machines"][0]["name"], "Desk");
    assert_eq!(phone.select("someone-else").unwrap_err(), "unknown_machine");

    // Forgetting the only machine leaves the phone with nothing open.
    phone.forget(&machine_id).await.unwrap();
    assert_eq!(phone.state(), json!({"active":null,"machines":[]}));
    assert_eq!(
        phone
            .request(&machine_id, "service.status".into(), json!({}))
            .await
            .unwrap_err(),
        "machine_not_open"
    );

    service.stop().await.unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(15), runner).await;
}
