use cortex_workspace_desktop::{client::Bridge, policy::authorize};
use serde_json::{json, Value};
use std::{os::unix::fs::PermissionsExt, path::PathBuf, sync::Arc, time::Duration};
use workspace_service::{Config, LaunchSpec, Service};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "cw-desktop-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::create_dir(root.join("project")).unwrap();
        Self(root)
    }
    fn config(&self) -> Config {
        self.config_with("journal")
    }
    fn config_with(&self, mode: &str) -> Config {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../..")
            .canonicalize()
            .unwrap();
        Config {
            runtime_dir: self.0.join("runtime"),
            state_dir: self.0.join("state"),
            allowed_root: self.0.join("project"),
            launch: LaunchSpec::new(
                PathBuf::from("/usr/bin/python3"),
                vec![
                    "-B".into(),
                    repo.join("crates/workspace-service/tests/fake_acp.py")
                        .display()
                        .to_string(),
                    mode.into(),
                    repo.join("src/cortex_runtime/workspace_contract/schema.json")
                        .display()
                        .to_string(),
                ],
            )
            .unwrap(),
            request_timeout: Duration::from_secs(2),
            shutdown_timeout: Duration::from_millis(150),
            event_capacity: 16,
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn call(op: &str, message: Value) -> cortex_workspace_desktop::policy::Call {
    authorize(
        "main",
        "tauri://localhost/",
        json!({"operation":op,"message":message}),
    )
    .unwrap()
}
fn query(id: &str, op: &str) -> Value {
    json!({"protocol":{"major":workspace_protocol::WIRE_MAJOR,"minor":workspace_protocol::WIRE_MINOR},"requestId":"read","projectId":id,"operation":op})
}


fn isolated_terminal_test(name: &str) -> bool {
    if std::env::var("CW_PTY_ISOLATED_TEST").as_deref() == Ok(name) { return true; }
    let fixture = Fixture::new();
    let home = fixture.0.join("shell-home");
    std::fs::create_dir(&home).unwrap();
    // Re-exec only this test: never alter the process-wide environment while sibling
    // tests run, and never load/save the owner's shell rc or history.
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name])
        .env("CW_PTY_ISOLATED_TEST", name)
        .env("HOME", &home).env("SHELL", "/bin/sh")
        .env("XDG_CONFIG_HOME", &home).env("XDG_STATE_HOME", &home)
        .env("XDG_CACHE_HOME", &home)
        .env_remove("ENV").env_remove("BASH_ENV")
        .output().unwrap().status;
    assert!(status.success(), "isolated terminal test failed");
    false
}

#[tokio::test]
async fn terminal_bridge_forwards_only_the_pty_surface() {
    if !isolated_terminal_test("terminal_bridge_forwards_only_the_pty_surface") { return; }
    let f = Fixture::new();
    let service = Service::bind(f.config()).await.unwrap();
    let project = service.register(&f.0.join("project"), "PTY").await.unwrap();
    let id = project["projectId"].as_str().unwrap();
    let serving = tokio::spawn(service.clone().serve());
    let bridge = Bridge::new(service.socket_path().into());
    let page = bridge.request(call("terminal.open", json!({"projectId":id,"cols":80,"rows":24}))).await.unwrap();
    let session = &page["sessionId"];
    let reply = bridge.request(call("terminal.write",json!({"projectId":id,"sessionId":session,"data":"printf 'bridge_%s' PTY\n"}))).await.unwrap();
    assert!(reply["written"].as_u64().unwrap() > 0);
    bridge.request(call("terminal.resize",json!({"projectId":id,"sessionId":session,"cols":90,"rows":30}))).await.unwrap();
    let page = bridge.request(call("terminal.read",json!({"projectId":id,"sessionId":session,"after":0}))).await.unwrap();
    assert_eq!(&page["sessionId"],session);
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let page = bridge.request(call("terminal.read",json!({"projectId":id,"sessionId":session,"after":0}))).await.unwrap();
            let bytes: Vec<u8> = serde_json::from_value(page["data"].clone()).unwrap();
            if String::from_utf8_lossy(&bytes).contains("bridge_PTY") { break; }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }).await.unwrap();
    assert_eq!(service.status().await["children"],0);
    bridge.request(call("terminal.close",json!({"projectId":id,"sessionId":session}))).await.unwrap();
    service.stop().await.unwrap(); serving.await.unwrap().unwrap();
}

#[tokio::test]
async fn framed_bridge_detach_does_not_stop_service_or_child() {
    let f = Fixture::new();
    let service = Service::bind(f.config()).await.unwrap();
    let project = service.register(&f.0.join("project"), "One").await.unwrap();
    let id = project["projectId"].as_str().unwrap().to_owned();
    let serving = tokio::spawn(service.clone().serve());
    let bridge = Arc::new(Bridge::new(service.socket_path().into()));
    // Listing never starts a child; activation is an explicit identity-only operation.
    assert_eq!(
        bridge
            .request(call("projects.list", json!({})))
            .await
            .unwrap()[0]["activation"],
        Value::Null
    );
    assert_eq!(service.status().await["children"], 0);
    bridge
        .request(call("projects.activate", json!({"projectId":id})))
        .await
        .unwrap();
    let caps = bridge
        .request(call(
            "capabilities.get",
            json!({"protocol":{"major":workspace_protocol::WIRE_MAJOR,"minor":workspace_protocol::WIRE_MINOR},"requestId":"caps"}),
        ))
        .await
        .unwrap();
    assert!(caps["buildIdentity"]
        .as_str()
        .unwrap()
        .ends_with(&workspace_protocol::channel::current().name));
    assert_eq!(
        bridge.rpc("service.capabilities", json!({})).await.unwrap()["channel"],
        workspace_protocol::channel::current().name
    );
    assert!(caps["operations"]
        .as_array()
        .unwrap()
        .contains(&json!("tasks.create")));
    let snapshot = bridge
        .request(call("project.snapshot", query(&id, "project.snapshot")))
        .await
        .unwrap();
    assert_eq!(snapshot["projectId"], id);
    let created = bridge.request(call("tasks.create",json!({
        "protocol":{"major":workspace_protocol::WIRE_MAJOR,"minor":workspace_protocol::WIRE_MINOR},"requestId":"write-once","projectId":id,"target":"write-once",
        "operation":"tasks.create","expectedVersion":"0","ownerEpoch":null,"actorId":"untrusted",
        "payload":{"description":"one"}
    }))).await.unwrap();
    assert_ne!(created["actor"], "untrusted"); // service derives the peer actor
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let subscriber = tokio::spawn({
        let bridge = bridge.clone();
        async move {
            bridge
                .subscribe(|event| {
                    let _ = tx.send(event);
                })
                .await
        }
    });
    let observed = tokio::time::timeout(Duration::from_secs(5), async {
        let mut invalidated = false;
        while let Some(event) = rx.recv().await {
            if event["type"] == "invalidated" {
                invalidated = true;
            }
            if invalidated && event["state"] == "connected" {
                return true;
            }
        }
        false
    })
    .await
    .unwrap();
    assert!(observed);
    subscriber.abort();
    assert!(subscriber.await.unwrap_err().is_cancelled());
    drop(bridge); // same detach boundary as closing a native window
    assert_eq!(service.status().await["children"], 1);
    assert_eq!(service.project_status(&id).await.unwrap()["exited"], false);
    let another = Bridge::new(service.socket_path().into());
    assert_eq!(
        another
            .request(call("project.snapshot", query(&id, "project.snapshot")))
            .await
            .unwrap()["projectId"],
        id
    );
    assert_eq!(
        std::fs::read_to_string(f.0.join("project/fake.trace"))
            .unwrap()
            .lines()
            .filter(|line| *line == "tasks.create")
            .count(),
        1
    );
    service.stop().await.unwrap();
    serving.await.unwrap().unwrap();
}

#[tokio::test]
async fn native_editor_uses_only_the_registered_root_with_fake_code() {
    // Isolate PATH in a subprocess instead of mutating this parallel test process.
    if std::env::var_os("CW_EDITOR_FIXTURE").is_none() {
        let fixture = Fixture::new();
        let bin = fixture.0.join("bin");
        std::fs::create_dir(&bin).unwrap();
        let code = bin.join("code");
        std::fs::write(&code, "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$CW_EDITOR_RECEIPT\"\n").unwrap();
        std::fs::set_permissions(&code, std::fs::Permissions::from_mode(0o700)).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "native_editor_uses_only_the_registered_root_with_fake_code"])
            .env("CW_EDITOR_FIXTURE", "1")
            .env("CW_EDITOR_RECEIPT", fixture.0.join("editor-receipt"))
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .output().unwrap();
        assert!(output.status.success(), "isolated editor fixture failed");
        return;
    }
    let fixture = Fixture::new();
    let mut config = fixture.config();
    config.launch = config.launch.with_tools(helper_path()).unwrap();
    let service = Service::bind(config).await.unwrap();
    let registered = fixture.0.join("project").canonicalize().unwrap();
    git_repo(&registered);
    let project = service.register(&registered, "Fixture").await.unwrap();
    let serving = tokio::spawn(service.clone().serve());
    let bridge = Bridge::new(service.socket_path().into());
    let capabilities = bridge.request(call("service.capabilities", json!({}))).await.unwrap();
    assert!(capabilities["methods"].as_array().unwrap().contains(&json!("native.open_editor")));
    assert!(capabilities["methods"].as_array().unwrap().contains(&json!("native.open_editor.vscode")));
    let result = bridge.request(call("native.open_editor", json!({"projectId":project["projectId"]}))).await.unwrap();
    assert_eq!(result["outcome"], "opened");
    let receipt = std::fs::read_to_string(std::env::var_os("CW_EDITOR_RECEIPT").unwrap()).unwrap();
    assert_eq!(receipt.lines().collect::<Vec<_>>(), vec!["--reuse-window", "--", registered.to_str().unwrap()]);
    // A separate checkout must reach the launcher as its own folder, not the main project.
    let tree = fixture.0.join("separate");
    assert!(std::process::Command::new("git").args(["worktree", "add", "-b", "topic"])
        .arg(&tree).current_dir(&registered).output().unwrap().status.success());
    let snapshot = bridge.request(call("machine.call", json!({"op":"repo.snapshot","projectId":project["projectId"]}))).await.unwrap();
    let tree_id = snapshot["result"]["checkouts"].as_array().unwrap().iter()
        .find(|c| c["kind"] == "worktree").unwrap()["id"].clone();
    let result = bridge.request(call("native.open_editor", json!({"projectId":project["projectId"],"checkoutId":tree_id}))).await.unwrap();
    assert_eq!(result["outcome"], "opened");
    let receipt = std::fs::read_to_string(std::env::var_os("CW_EDITOR_RECEIPT").unwrap()).unwrap();
    assert_eq!(receipt.lines().collect::<Vec<_>>(), vec!["--reuse-window", "--", tree.to_str().unwrap()]);
    // A file opens at its line inside the checkout the service resolved; the page only named it.
    let checkout = format!("{}:main", project["projectId"].as_str().unwrap());
    let opened = bridge.request(call("native.open_editor", json!({"projectId":project["projectId"],"checkoutId":checkout,"file":"a.txt","line":7}))).await.unwrap();
    assert_eq!(opened["outcome"], "opened");
    let receipt = std::fs::read_to_string(std::env::var_os("CW_EDITOR_RECEIPT").unwrap()).unwrap();
    let target = format!("{}:7", registered.join("a.txt").display());
    assert_eq!(receipt.lines().collect::<Vec<_>>(), vec!["--reuse-window", registered.to_str().unwrap(), "--goto", &target]);
    let escaped = bridge.request(call("native.open_editor", json!({"projectId":project["projectId"],"file":"../outside"}))).await.unwrap_err();
    assert_eq!(escaped, "refused:That path is not inside this checkout.");
    service.stop().await.unwrap();
    serving.await.unwrap().unwrap();
}

fn helper_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap().join("crates/workspace-service/machine_tools.py")
}

fn git_repo(root: &std::path::Path) {
    std::fs::write(root.join("a.txt"), "one\n").unwrap();
    for args in [&["init", "-q", "-b", "main"][..], &["-c", "user.name=T", "-c", "user.email=t@example.invalid", "add", "."], &["-c", "user.name=T", "-c", "user.email=t@example.invalid", "commit", "-qm", "one"]] {
        assert!(std::process::Command::new("git").args(args).current_dir(root).status().unwrap().success());
    }
}

#[tokio::test]
async fn an_owner_save_goes_through_the_real_helper_bound_to_its_revision_and_confined() {
    let f = Fixture::new();
    let mut config = f.config();
    config.launch = config.launch.with_tools(helper_path()).unwrap();
    let service = Service::bind(config).await.unwrap();
    let root = f.0.join("project").canonicalize().unwrap();
    git_repo(&root);
    let project = service.register(&root, "Fixture").await.unwrap();
    let id = project["projectId"].clone();
    let serving = tokio::spawn(service.clone().serve());
    let bridge = Bridge::new(service.socket_path().into());
    let machine = |op: &str, params: Value| call("machine.call", json!({"op":op,"projectId":id,"params":params}));

    let read = bridge.request(machine("repo.file", json!({"file":"a.txt"}))).await.unwrap();
    assert_eq!(read["ok"], true);
    let revision = read["result"]["revision"].as_str().unwrap().to_owned();
    assert_eq!(read["result"]["text"], "one\n");
    let saved = bridge.request(machine("repo.file.save", json!({"file":"a.txt","text":"owner\n","revision":revision}))).await.unwrap();
    assert_eq!(saved["result"]["outcome"], "saved");
    assert_eq!(std::fs::read(root.join("a.txt")).unwrap(), b"owner\n");

    // The revision the owner saw is stale now, so the same save changes nothing.
    std::fs::write(root.join("a.txt"), "worker\n").unwrap();
    let conflict = bridge.request(machine("repo.file.save", json!({"file":"a.txt","text":"late\n","revision":saved["result"]["revision"]}))).await.unwrap();
    assert_eq!(conflict["result"]["outcome"], "conflict");
    assert_ne!(conflict["result"]["revision"], saved["result"]["revision"]);
    assert_eq!(std::fs::read(root.join("a.txt")).unwrap(), b"worker\n");

    let outside = f.0.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("secret"), "secret").unwrap();
    let escape = bridge.request(machine("repo.file.save", json!({"file":"../outside/secret","text":"x","revision":null}))).await.unwrap();
    assert_eq!((escape["ok"].clone(), escape["code"].clone()), (json!(false), json!("invalid_path")));
    std::os::unix::fs::symlink(&outside, root.join("through")).unwrap();
    let linked = bridge.request(machine("repo.file.save", json!({"file":"through/secret","text":"x","revision":null}))).await.unwrap();
    assert_eq!((linked["ok"].clone(), linked["code"].clone()), (json!(false), json!("symlink_refused")));
    assert_eq!(std::fs::read(outside.join("secret")).unwrap(), b"secret");
    service.stop().await.unwrap();
    serving.await.unwrap().unwrap();
}

#[tokio::test]
async fn a_refused_registration_names_its_reason_and_can_create_the_folder() {
    let f = Fixture::new();
    let service = Service::bind(f.config()).await.unwrap();
    let serving = tokio::spawn(service.clone().serve());
    let bridge = Bridge::new(service.socket_path().into());
    let wanted = f.0.join("project/new-folder");
    let register = |create: Option<bool>| {
        let mut message = json!({"root":wanted,"displayName":"New"});
        if let Some(create) = create { message["create"] = json!(create); }
        call("projects.register", message)
    };
    assert_eq!(bridge.request(register(None)).await.unwrap_err(), "project_folder_missing");
    assert!(!wanted.exists());
    let created = bridge.request(register(Some(true))).await.unwrap();
    assert_eq!(created["displayName"], "New");
    assert!(wanted.is_dir());
    let outside = bridge.request(call("projects.register", json!({"root":f.0.join("state"),"displayName":"Private"}))).await.unwrap_err();
    assert!(outside.contains("private"), "{outside}");
    service.stop().await.unwrap();
    serving.await.unwrap().unwrap();
}

#[tokio::test]
async fn the_bridge_carries_a_folder_search_and_the_reason_a_new_folder_is_refused() {
    let f = Fixture::new();
    let mut config = f.config();
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    config.launch = config.launch.with_tools(repo.join("crates/workspace-service/machine_tools.py")).unwrap();
    let service = Service::bind(config).await.unwrap();
    let serving = tokio::spawn(service.clone().serve());
    let bridge = Bridge::new(service.socket_path().into());
    std::fs::create_dir_all(f.0.join("project/cedar-notes/.git")).unwrap();
    let found = bridge.request(call("directories.search", json!({"session":"pick-1","query":"cedar"}))).await.unwrap();
    assert_eq!(found["entries"], json!([{"path":f.0.join("project/cedar-notes"),"git":true}]));
    let cancelled = bridge.request(call("directories.cancel", json!({"session":"pick-1"}))).await.unwrap();
    assert_eq!(cancelled["outcome"], "not_running");
    let refused = bridge.request(call("projects.register", json!({"root":f.0.join("project/cedar-notes"),"displayName":"Cedar","newFolder":true}))).await.unwrap_err();
    assert_eq!(refused, "refused:A folder with this name is already there.");
    service.stop().await.unwrap();
    serving.await.unwrap().unwrap();
}

#[tokio::test]
async fn a_project_that_will_not_start_never_disconnects_the_machine() {
    let f = Fixture::new();
    let service = Service::bind(f.config_with("once")).await.unwrap();
    let project = service.register(&f.0.join("project"), "One").await.unwrap();
    let id = project["projectId"].as_str().unwrap().to_owned();
    let serving = tokio::spawn(service.clone().serve());
    let bridge = Arc::new(Bridge::new(service.socket_path().into()));
    bridge.request(call("projects.activate", json!({"projectId":id}))).await.unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let subscriber = tokio::spawn({
        let bridge = bridge.clone();
        async move { bridge.subscribe(|event| { let _ = tx.send(event); }).await }
    });
    loop {
        let event: Value = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.unwrap().unwrap();
        if event["state"] == "connected" { break; }
    }
    // The child exits here and every restart of it exits during startup.
    assert!(bridge.request(call("tasks.get", json!({"protocol":{"major":workspace_protocol::WIRE_MAJOR,"minor":workspace_protocol::WIRE_MINOR},"requestId":"read","projectId":id,"operation":"tasks.get","target":"x"}))).await.is_err());
    tokio::time::sleep(Duration::from_secs(3)).await;
    let mut seen = vec![];
    while let Ok(event) = rx.try_recv() { seen.push(event); }
    assert!(!seen.iter().any(|e| e["state"] == "disconnected"), "{seen:?}");
    let caps = bridge.request(call("capabilities.get", json!({"protocol":{"major":workspace_protocol::WIRE_MAJOR,"minor":workspace_protocol::WIRE_MINOR},"requestId":"caps"}))).await.unwrap();
    assert_eq!(caps["operations"], json!([]));
    let status = bridge.request(call("projects.status", json!({"projectId":id}))).await.unwrap();
    assert_eq!(status["ready"], false);
    assert_eq!(status["state"], "failed");
    subscriber.abort();
    let _ = subscriber.await;
    service.stop().await.unwrap();
    serving.await.unwrap().unwrap();
}

#[tokio::test]
async fn missing_service_can_recover_without_replaying_any_request() {
    let f = Fixture::new();
    let bridge = Bridge::new(f.0.join("runtime/service.sock"));
    assert!(bridge
        .request(call("projects.list", json!({})))
        .await
        .is_err());
    let service = Service::bind(f.config()).await.unwrap();
    let serving = tokio::spawn(service.clone().serve());
    assert_eq!(
        bridge
            .request(call("projects.list", json!({})))
            .await
            .unwrap(),
        json!([])
    );
    service.stop().await.unwrap();
    serving.await.unwrap().unwrap();
}

#[tokio::test]
async fn mismatched_rpc_identity_and_oversized_frame_fail_closed() {
    for oversized in [false, true] {
        let f = Fixture::new();
        let socket = f.0.join("test.sock");
        let server = tokio::net::UnixListener::bind(&socket).unwrap();
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
        let responder = tokio::spawn(async move {
            let (mut peer, _) = server.accept().await.unwrap();
            let _ = workspace_service::read_frame(&mut peer).await.unwrap();
            if oversized {
                use tokio::io::AsyncWriteExt;
                peer.write_all(&(2_000_000u32).to_be_bytes()).await.unwrap();
            } else {
                workspace_service::write_frame(
                    &mut peer,
                    &json!({"jsonrpc":"2.0","id":"wrong","result":{}}),
                )
                .await
                .unwrap();
            }
        });
        let bridge = Bridge::new(socket);
        assert!(bridge
            .request(call("service.status", json!({})))
            .await
            .is_err());
        responder.await.unwrap();
    }
}
