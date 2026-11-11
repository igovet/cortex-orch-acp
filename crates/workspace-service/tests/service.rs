use serde_json::{json, Value};
use std::{
    os::unix::fs::{symlink, PermissionsExt},
    path::{Path, PathBuf},
    time::Duration,
};
use workspace_service::{connect, read_frame, write_frame, Config, LaunchSpec, Service};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "workspace-service-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&p).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o700)).unwrap();
        for name in ["projects", "projects/one"] {
            std::fs::create_dir(p.join(name)).unwrap();
        }
        Self(p)
    }
    fn config(&self, mode: &str) -> Config {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        Config {
            runtime_dir: self.0.join("runtime"),
            state_dir: self.0.join("state"),
            allowed_root: self.0.join("projects"),
            launch: LaunchSpec::new(
                PathBuf::from("/usr/bin/python3"),
                vec![
                    "-B".into(),
                    root.join("tests/fake_acp.py").display().to_string(),
                    mode.into(),
                    root.join("../../src/cortex_runtime/workspace_contract/schema.json")
                        .canonicalize()
                        .unwrap()
                        .display()
                        .to_string(),
                ],
            )
            .unwrap(),
            request_timeout: Duration::from_secs(2),
            shutdown_timeout: Duration::from_millis(150),
            event_capacity: 2,
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn query(project: &str, operation: &str, target: Value) -> Value {
    json!({"protocol":{"major":1,"minor":0},"requestId":"read","projectId":project,"operation":operation,"target":target})
}
async fn activated(mode: &str) -> (Fixture, std::sync::Arc<Service>, String) {
    let f = Fixture::new();
    let s = Service::bind(f.config(mode)).await.unwrap();
    let p = s.register(&f.0.join("projects/one"), "One").await.unwrap();
    assert!(p["activation"].is_null());
    let id = p["projectId"].as_str().unwrap().to_owned();
    s.activate(&id).await.unwrap();
    (f, s, id)
}
#[tokio::test]
async fn dev_refuses_unowned_dev_store_and_rechecks_activation_without_writes() {
    let f = Fixture::new();
    let s = Service::bind(f.config("normal")).await.unwrap();
    let root = f.0.join("projects/one");
    std::fs::create_dir(root.join(".cortex-dev")).unwrap();
    std::fs::set_permissions(root.join(".cortex-dev"), std::fs::Permissions::from_mode(0o700)).unwrap();
    let before = std::fs::metadata(&root).unwrap().modified().unwrap();
    std::fs::write(root.join(".cortex-dev/cortex.sqlite3"), "production-canary").unwrap();
    assert!(s
        .register(&root, "Production")
        .await
        .unwrap_err()
        .to_string()
        .contains("dev_project_refused"));
    assert!(!root.join(".cortex-dev-owner.json").exists());
    assert_eq!(
        std::fs::metadata(&root).unwrap().modified().unwrap(),
        before
    );
    assert_eq!(
        std::fs::read_to_string(root.join(".cortex-dev/cortex.sqlite3")).unwrap(),
        "production-canary"
    );
    assert_eq!(s.projects().await, json!([]));
    let fresh = f.0.join("projects/fresh");
    std::fs::create_dir(&fresh).unwrap();
    let p = s.register(&fresh, "Fresh").await.unwrap();
    assert!(fresh.join(".cortex-dev/owner.json").is_file());
    // A replaced dev directory after registration cannot be adopted.
    std::fs::rename(fresh.join(".cortex-dev"), fresh.join("reserved-store")).unwrap();
    std::fs::create_dir(fresh.join(".cortex-dev")).unwrap();
    std::fs::set_permissions(
        fresh.join(".cortex-dev"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    let reservation = std::fs::read(fresh.join("reserved-store/owner.json")).unwrap();
    std::fs::write(fresh.join(".cortex-dev/cortex.sqlite3"), "keep").unwrap();
    assert!(s.activate(p["projectId"].as_str().unwrap()).await.is_err());
    assert_eq!(s.status().await["children"], 0);
    assert_eq!(
        std::fs::read(fresh.join("reserved-store/owner.json")).unwrap(),
        reservation
    );
    assert_eq!(
        std::fs::read_to_string(fresh.join(".cortex-dev/cortex.sqlite3")).unwrap(),
        "keep"
    );
    s.stop().await.unwrap();
}

#[tokio::test]
async fn project_metadata_is_inside_the_channel_store_and_legacy_markers_move_once() {
    use std::os::unix::fs::MetadataExt;
    let f = Fixture::new();
    let s = Service::bind(f.config("normal")).await.unwrap();
    let root = f.0.join("projects/one");
    let p = s.register(&root, "One").await.unwrap();
    s.activate(p["projectId"].as_str().unwrap()).await.unwrap();
    assert!(!root.join(".cortex-dev-owner.json").exists());
    let marker = root.join(".cortex-dev/owner.json");
    assert!(marker.is_file());
    // Simulate the exact two-marker layout produced by older dev builds.
    std::fs::rename(&marker, root.join(".cortex-dev-owner.json")).unwrap();
    let meta = std::fs::metadata(root.join(".cortex-dev")).unwrap();
    let old = json!({"channel":"dev","root":root,"uid":unsafe{libc::geteuid()},"store":[meta.dev(),meta.ino()]});
    std::fs::write(root.join(".cortex-workspace-dev.json"), old.to_string()).unwrap();
    std::fs::set_permissions(root.join(".cortex-workspace-dev.json"), std::fs::Permissions::from_mode(0o600)).unwrap();
    s.stop().await.unwrap();
    drop(s);
    let again = Service::bind(f.config("normal")).await.unwrap();
    again.activate(p["projectId"].as_str().unwrap()).await.unwrap();
    assert!(marker.is_file());
    assert!(!root.join(".cortex-dev-owner.json").exists());
    assert!(!root.join(".cortex-workspace-dev.json").exists());
    assert_eq!(serde_json::from_str::<Value>(&std::fs::read_to_string(root.join(".cortex-dev/legacy-workspace.json")).unwrap()).unwrap(), old);
    again.activate(p["projectId"].as_str().unwrap()).await.unwrap();
    let mut files: Vec<_> = std::fs::read_dir(&root).unwrap().map(|e| e.unwrap().file_name()).collect();
    files.sort();
    assert_eq!(files, vec![".cortex-dev", "fake.trace"]); // trace belongs to the scripted test peer
    again.stop().await.unwrap();
}

#[tokio::test]
async fn dev_opens_beside_release_without_touching_release() {
    let f = Fixture::new();
    let s = Service::bind(f.config("normal")).await.unwrap();
    let root = f.0.join("projects/one");
    std::fs::create_dir(root.join(".cortex")).unwrap();
    std::fs::write(root.join(".cortex/cortex.sqlite3"), "release-canary").unwrap();
    let before = std::fs::metadata(root.join(".cortex")).unwrap().modified().unwrap();
    let p = s.register(&root, "Shared folder").await.unwrap();
    s.activate(p["projectId"].as_str().unwrap()).await.unwrap();
    assert!(root.join(".cortex-dev").is_dir());
    assert_eq!(std::fs::read_to_string(root.join(".cortex/cortex.sqlite3")).unwrap(), "release-canary");
    assert_eq!(std::fs::metadata(root.join(".cortex")).unwrap().modified().unwrap(), before);
    assert_eq!(std::fs::read_dir(root.join(".cortex")).unwrap().count(), 1);
    s.stop().await.unwrap();
}

#[tokio::test]
async fn lock_modes_stale_socket_and_real_same_user_peers() {
    let f = Fixture::new();
    let s = Service::bind(f.config("normal")).await.unwrap();
    assert!(Service::bind(f.config("normal")).await.is_err());
    assert_eq!(
        std::fs::metadata(s.socket_path().parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(s.socket_path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let client = connect(s.socket_path()).await.unwrap();
    assert_eq!(client.peer_cred().unwrap().uid(), unsafe {
        libc::geteuid()
    });
    s.stop().await.unwrap();
    drop(client);
    drop(s);
    let again = Service::bind(f.config("normal")).await.unwrap();
    again.stop().await.unwrap();
}
#[tokio::test]
async fn endpoint_drop_releases_the_lock_before_a_forked_child_execs() {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    struct Forked {
        pid: libc::pid_t,
        release: Option<OwnedFd>,
    }
    impl Drop for Forked {
        fn drop(&mut self) {
            self.release.take();
            while unsafe { libc::waitpid(self.pid, std::ptr::null_mut(), 0) } == -1
                && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
            {}
        }
    }

    let f = Fixture::new();
    let s = Service::bind(f.config("normal")).await.unwrap();
    let mut pipe = [-1; 2];
    assert_eq!(unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC) }, 0);
    let read = unsafe { OwnedFd::from_raw_fd(pipe[0]) };
    let write = unsafe { OwnedFd::from_raw_fd(pipe[1]) };
    let pid = unsafe { libc::fork() };
    assert!(pid >= 0);
    if pid == 0 {
        // Only async-signal-safe calls between fork and _exit. Keep inherited
        // descriptors open, just as a concurrently spawning peer does before exec.
        unsafe {
            libc::close(write.as_raw_fd());
            let mut byte = 0u8;
            libc::read(read.as_raw_fd(), (&mut byte as *mut u8).cast(), 1);
            libc::_exit(0);
        }
    }
    drop(read);
    let forked = Forked { pid, release: Some(write) };
    assert!(Service::bind(f.config("normal")).await.is_err());
    s.stop().await.unwrap();
    drop(s);
    let again = Service::bind(f.config("normal")).await.unwrap();
    assert!(Service::bind(f.config("normal")).await.is_err());
    drop(forked);
    assert!(Service::bind(f.config("normal")).await.is_err());
    again.stop().await.unwrap();
}

#[tokio::test]
async fn unsafe_runtime_and_stale_endpoint_are_never_replaced() {
    let f = Fixture::new();
    let runtime = f.0.join("runtime");
    std::fs::create_dir(&runtime).unwrap();
    assert!(Service::bind(f.config("normal")).await.is_err()); // not 0700
    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700)).unwrap();
    let target = f.0.join("keep");
    std::fs::write(&target, "do not touch").unwrap();
    symlink(&target, runtime.join("service.sock")).unwrap();
    assert!(Service::bind(f.config("normal")).await.is_err());
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "do not touch");
    assert!(std::fs::symlink_metadata(runtime.join("service.sock"))
        .unwrap()
        .file_type()
        .is_symlink());
}
#[tokio::test]
async fn registration_is_inert_and_rejects_private_and_escape() {
    let f = Fixture::new();
    let s = Service::bind(f.config("normal")).await.unwrap();
    std::fs::create_dir(f.0.join("projects/one/.cortex")).unwrap();
    for p in [
        "projects/one/.cortex",
        "state",
        "projects/../state",
    ] {
        assert!(s.register(&f.0.join(p), "Bad").await.is_err(), "{p}");
    }
    // A release store can coexist; it is not the dev channel store.
    let p = s.register(&f.0.join("projects/one"), "One").await.unwrap();
    assert_eq!(s.status().await["children"], 0);
    assert!(p["activation"].is_null());
    s.activate(p["projectId"].as_str().unwrap()).await.unwrap();
    let projects = s.projects().await;
    assert!(projects[0]["activation"]["actorId"]
        .as_str()
        .unwrap()
        .starts_with("uid:"));
    assert!(projects[0]["activation"]["recordedAt"].as_u64().is_some());
    s.stop().await.unwrap();
}
async fn ready_within(s: &Service, id: &str, seconds: u64) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds);
    while tokio::time::Instant::now() < deadline {
        if s.project_status(id).await.unwrap()["ready"] == true {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}
#[tokio::test]
async fn registration_resolves_links_and_names_a_missing_folder() {
    let f = Fixture::new();
    let s = Service::bind(f.config("normal")).await.unwrap();
    symlink(f.0.join("projects/one"), f.0.join("projects/link")).unwrap();
    let linked = s.register(&f.0.join("projects/link"), "Linked").await.unwrap();
    assert_eq!(linked["canonicalRoot"], f.0.join("projects/one").to_str().unwrap());
    let wanted = f.0.join("projects/new/deep");
    assert_eq!(s.register(&wanted, "New").await.unwrap_err().to_string(), "project_folder_missing");
    assert!(!wanted.exists());
    assert_eq!(s.register_folder(&wanted, "New", true).await.unwrap()["canonicalRoot"], wanted.to_str().unwrap());
    assert!(wanted.is_dir());
    let file = f.0.join("projects/file");
    std::fs::write(&file, "x").unwrap();
    assert_eq!(s.register(&file, "File").await.unwrap_err().to_string(), "project_folder_not_directory");
    assert_eq!(s.register(Path::new("relative/path"), "Rel").await.unwrap_err().to_string(), "absolute_path_required");
    s.stop().await.unwrap();
}
#[tokio::test]
async fn a_machine_wide_root_still_refuses_private_and_read_only_folders() {
    let f = Fixture::new();
    let mut config = f.config("normal");
    config.allowed_root = PathBuf::from("/");
    let s = Service::bind(config).await.unwrap();
    let outside = f.0.join("outside-the-old-root");
    std::fs::create_dir(&outside).unwrap();
    assert!(s.register(&outside, "Outside").await.is_ok());
    for private in ["state", "runtime"] {
        assert_eq!(s.register(&f.0.join(private), "P").await.unwrap_err().to_string(), "project_path_refused", "{private}");
    }
    assert_eq!(s.register(Path::new("/usr"), "Usr").await.unwrap_err().to_string(), "project_folder_not_writable");
    s.stop().await.unwrap();
}
#[tokio::test]
async fn a_missing_folder_never_stops_the_service_and_recovers_when_it_returns() {
    let (f, s, p) = activated("normal").await;
    s.stop().await.unwrap();
    drop(s);
    let root = f.0.join("projects/one");
    let parked = f.0.join("parked");
    std::fs::rename(&root, &parked).unwrap();
    let again = Service::bind(f.config("normal")).await.unwrap();
    let runner = tokio::spawn(again.clone().serve());
    tokio::time::sleep(Duration::from_millis(300)).await;
    let status = again.project_status(&p).await.unwrap();
    assert_eq!(status["ready"], false);
    assert_eq!(status["reason"], "project_folder_missing");
    std::fs::rename(&parked, &root).unwrap();
    assert!(ready_within(&again, &p, 8).await);
    again.stop().await.unwrap();
    runner.await.unwrap().unwrap();
}
#[tokio::test]
async fn activation_never_holds_the_service_while_the_child_starts() {
    let f = Fixture::new();
    let s = Service::bind(f.config("slow_init")).await.unwrap();
    let p = s.register(&f.0.join("projects/one"), "One").await.unwrap();
    let id = p["projectId"].as_str().unwrap().to_owned();
    let activation = tokio::spawn({
        let (s, id) = (s.clone(), id.clone());
        async move { s.activate(&id).await }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    tokio::time::timeout(Duration::from_millis(300), s.status()).await.expect("status must answer during activation");
    let during = tokio::time::timeout(Duration::from_millis(300), s.project_status(&id)).await.expect("status must answer during activation").unwrap();
    assert_eq!(during["state"], "starting");
    activation.await.unwrap().unwrap();
    assert_eq!(s.project_status(&id).await.unwrap()["ready"], true);
    s.stop().await.unwrap();
}
/// The owner's upgrade: an older build recorded the activation but wrote its own legacy marker
/// and kept its data in `.cortex`. The new build has no reservation and no dev store for it.
async fn legacy_activated(f: &Fixture) -> (PathBuf, String) {
    let s = Service::bind(f.config("normal")).await.unwrap();
    let root = f.0.join("projects/one");
    let id = s.register(&root, "One").await.unwrap()["projectId"].as_str().unwrap().to_owned();
    s.activate(&id).await.unwrap();
    s.stop().await.unwrap();
    std::fs::remove_file(root.join(".cortex-dev/owner.json")).unwrap();
    std::fs::remove_dir_all(root.join(".cortex-dev")).unwrap();
    std::fs::write(root.join(".cortex-workspace-dev.json"), "legacy-marker").unwrap();
    std::fs::create_dir(root.join(".cortex")).unwrap();
    std::fs::write(root.join(".cortex/cortex.sqlite3"), "legacy-data").unwrap();
    (root, id)
}
#[tokio::test]
async fn an_activated_project_from_an_older_build_comes_back_after_the_upgrade_restart() {
    let f = Fixture::new();
    let (root, id) = legacy_activated(&f).await;
    let again = Service::bind(f.config("normal")).await.unwrap();
    let runner = tokio::spawn(again.clone().serve());
    assert!(ready_within(&again, &id, 8).await, "{}", again.project_status(&id).await.unwrap());
    assert!(root.join(".cortex-dev/owner.json").is_file() && root.join(".cortex-dev").is_dir());
    // Legacy data is left exactly as it was: never adopted, moved or migrated.
    assert_eq!(std::fs::read_to_string(root.join(".cortex-workspace-dev.json")).unwrap(), "legacy-marker");
    assert_eq!(std::fs::read_to_string(root.join(".cortex/cortex.sqlite3")).unwrap(), "legacy-data");
    assert_eq!(std::fs::read_dir(root.join(".cortex")).unwrap().count(), 1);
    again.stop().await.unwrap();
    runner.await.unwrap().unwrap();
}
#[tokio::test]
async fn an_unreserved_existing_dev_store_is_still_refused_with_a_reason() {
    let f = Fixture::new();
    let (root, id) = legacy_activated(&f).await;
    std::fs::create_dir(root.join(".cortex-dev")).unwrap();
    std::fs::set_permissions(root.join(".cortex-dev"), std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(root.join(".cortex-dev/cortex.sqlite3"), "unowned").unwrap();
    let again = Service::bind(f.config("normal")).await.unwrap();
    let runner = tokio::spawn(again.clone().serve());
    tokio::time::sleep(Duration::from_millis(400)).await;
    let status = again.project_status(&id).await.unwrap();
    assert_eq!(status["state"], "failed");
    assert!(status["reason"].as_str().unwrap().starts_with("dev_project_refused"));
    assert!(!root.join(".cortex-dev-owner.json").exists());
    assert_eq!(std::fs::read_to_string(root.join(".cortex-dev/cortex.sqlite3")).unwrap(), "unowned");
    again.stop().await.unwrap();
    runner.await.unwrap().unwrap();
}
#[tokio::test]
async fn a_refused_activation_keeps_a_reason_and_no_child() {
    let f = Fixture::new();
    let s = Service::bind(f.config("refuse_session")).await.unwrap();
    let p = s.register(&f.0.join("projects/one"), "One").await.unwrap();
    let id = p["projectId"].as_str().unwrap();
    assert_eq!(s.activate(id).await.unwrap_err().to_string(), "acp_request_refused");
    let status = s.project_status(id).await.unwrap();
    assert_eq!(status["state"], "failed");
    assert_eq!(status["reason"], "acp_request_refused");
    assert_eq!(s.status().await["children"], 0);
    s.stop().await.unwrap();
}
#[tokio::test]
async fn an_exited_child_is_restarted_by_the_running_service() {
    let (_f, s, p) = activated("exit").await;
    let runner = tokio::spawn(s.clone().serve());
    assert!(s.workspace(query(&p, "tasks.get", json!("x"))).await.is_err());
    assert!(ready_within(&s, &p, 8).await, "the project must come back without the owner");
    assert_eq!(s.project_status(&p).await.unwrap()["recovery"], "unknown");
    s.stop().await.unwrap();
    runner.await.unwrap().unwrap();
}
#[tokio::test]
async fn a_child_that_answers_late_is_not_abandoned() {
    let (_f, s, p) = activated("stall_once").await;
    let runner = tokio::spawn(s.clone().serve());
    let pid_before = s.workspace(query(&p, "tasks.get", json!("x"))).await;
    assert!(pid_before.is_err(), "the stalled request itself stays unknown");
    assert!(ready_within(&s, &p, 8).await, "a slow reply must not end the child");
    assert_eq!(s.status().await["children"], 1);
    assert_eq!(s.project_status(&p).await.unwrap()["exited"], false);
    s.workspace(query(&p, "tasks.get", json!("after"))).await.unwrap();
    s.stop().await.unwrap();
    runner.await.unwrap().unwrap();
}
#[tokio::test]
async fn exact_correlation_and_stderr_drain() {
    let (_f, s, p) = activated("reverse").await;
    let (a, b) = tokio::join!(
        s.workspace(query(&p, "tasks.get", json!("a"))),
        s.workspace(query(&p, "tasks.get", json!("b")))
    );
    assert_eq!(a.unwrap()["target"], "a");
    assert_eq!(b.unwrap()["target"], "b");
    s.stop().await.unwrap();
    let (_f, s, p) = activated("stderr").await;
    assert_eq!(
        s.workspace(query(&p, "tasks.get", json!("x")))
            .await
            .unwrap()["drained"],
        true
    );
    s.stop().await.unwrap();
}
#[tokio::test]
async fn unsupported_host_requests_are_refused_without_stopping_the_reader() {
    let (_f, s, p) = activated("host").await;
    assert_eq!(
        s.workspace(query(&p, "tasks.get", json!("permission")))
            .await
            .unwrap()["refused"],
        true
    );
    assert_eq!(s.project_status(&p).await.unwrap()["ready"], true);
    s.stop().await.unwrap();
}
#[tokio::test]
async fn scripted_host_peer_correlates_replies_across_interleaved_requests() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let f = Fixture::new();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut peer = tokio::process::Command::new("/usr/bin/python3")
        .args(["-B", root.join("tests/fake_acp.py").to_str().unwrap(), "host"])
        .current_dir(f.0.join("projects/one"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut input = peer.stdin.take().unwrap();
    let mut output = BufReader::new(peer.stdout.take().unwrap()).lines();
    tokio::time::timeout(Duration::from_secs(5), async {
        for request in [
            json!({"jsonrpc":"2.0","id":1,"method":"_cortex/workspace","params":{"operation":"tasks.get"}}),
            json!({"jsonrpc":"2.0","id":2,"method":"_cortex/workspace","params":{"operation":"events.page"}}),
            json!({"jsonrpc":"2.0","id":3,"method":"_cortex/workspace","params":{"operation":"tasks.get"}}),
            json!({"jsonrpc":"2.0","id":"permission-3","error":{"code":-32000}}),
            json!({"jsonrpc":"2.0","id":"permission-1","error":{"code":-32601}}),
        ] {
            input.write_all(format!("{request}\n").as_bytes()).await.unwrap();
        }
        let host: Value = serde_json::from_str(&output.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(host["id"], "permission-1");
        let events: Value = serde_json::from_str(&output.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(events["id"], 2);
        assert_eq!(events["result"]["items"], json!([]));
        let other_host: Value = serde_json::from_str(&output.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(other_host["id"], "permission-3");
        let other_task: Value = serde_json::from_str(&output.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(other_task["id"], 3);
        assert_eq!(other_task["result"]["refused"], false);
        let task: Value = serde_json::from_str(&output.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(task["id"], 1);
        assert_eq!(task["result"]["refused"], true);
        drop(input);
        assert!(peer.wait().await.unwrap().success());
    }).await.unwrap();
}

#[tokio::test]
async fn oversized_stdout_is_invalidated_but_still_drained() {
    let (f, s, p) = activated("oversized").await;
    assert!(s
        .workspace(query(&p, "tasks.get", json!("oversized")))
        .await
        .is_err());
    for _ in 0..100 {
        if f.0.join("projects/one/oversize-drained").is_file() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(f.0.join("projects/one/oversize-drained").is_file());
    assert_eq!(s.project_status(&p).await.unwrap()["ready"], false);
    s.stop().await.unwrap();
}
#[tokio::test]
async fn pending_queue_overflow_never_returns_success() {
    let (_f, s, p) = activated("hang").await;
    let mut calls = tokio::task::JoinSet::new();
    for _ in 0..70 {
        let service = s.clone();
        let project = p.clone();
        calls.spawn(async move {
            service
                .workspace(query(&project, "tasks.get", json!("blocked")))
                .await
        });
    }
    while let Some(call) = calls.join_next().await {
        assert!(call.unwrap().is_err());
    }
    assert_eq!(s.project_status(&p).await.unwrap()["ready"], false);
    s.stop().await.unwrap();
}
#[tokio::test]
async fn protocol_error_exit_and_timeout_invalidate_readiness() {
    for mode in ["exit", "bad_id", "hang"] {
        let (_f, s, p) = activated(mode).await;
        assert!(s
            .workspace(query(&p, "tasks.get", json!("x")))
            .await
            .is_err());
        assert_eq!(s.project_status(&p).await.unwrap()["ready"], false);
        s.stop().await.unwrap();
    }
}
#[tokio::test]
async fn removing_a_project_preserves_files_and_readding_preserves_identity() {
    let (f, s, p) = activated("normal").await;
    let root = f.0.join("projects/one");
    std::fs::write(root.join("owner.txt"), "keep").unwrap();
    assert_eq!(s.remove(&p).await.unwrap()["outcome"], "removed");
    assert!(s.projects().await.as_array().unwrap().is_empty());
    assert_eq!(std::fs::read_to_string(root.join("owner.txt")).unwrap(), "keep");
    assert_eq!(s.status().await["children"], 0);
    assert_eq!(s.register(&root, "Again").await.unwrap()["projectId"], p);
    s.stop().await.unwrap();
}
#[tokio::test]
async fn detach_does_not_cancel_or_drop_child_stdin() {
    let (f, s, p) = activated("normal").await;
    let runner = tokio::spawn(s.clone().serve());
    let mut c = connect(s.socket_path()).await.unwrap();
    write_frame(
        &mut c,
        &json!({"jsonrpc":"2.0","id":1,"method":"service.status","params":{}}),
    )
    .await
    .unwrap();
    assert_eq!(read_frame(&mut c).await.unwrap()["id"], 1);
    drop(c);
    assert_eq!(s.status().await["children"], 1);
    let reply = s
        .workspace(query(&p, "tasks.get", json!("after")))
        .await
        .unwrap();
    assert_eq!(reply["unexpectedEnvironment"], json!([]));
    // The bundled maintenance helper reads this to keep dev builds off GitHub releases.
    assert_eq!(reply["channel"], "dev");
    assert_eq!(reply["devEnvironment"], true);
    let pid = reply["pid"].as_i64().unwrap() as i32;
    assert_eq!(unsafe { libc::kill(pid, 0) }, 0);
    let trace = std::fs::read_to_string(f.0.join("projects/one/fake.trace")).unwrap();
    assert!(!trace.lines().any(|l| l == "session/cancel"));
    s.stop().await.unwrap();
    runner.await.unwrap().unwrap();
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    let trace = std::fs::read_to_string(f.0.join("projects/one/fake.trace")).unwrap();
    assert!(trace.lines().any(|l| l == "session/cancel"));
}
#[tokio::test]
async fn explicit_stop_outlives_its_requesting_connection() {
    let (_f, s, _p) = activated("normal").await;
    let server = tokio::spawn(s.clone().serve());
    let mut client = connect(s.socket_path()).await.unwrap();
    write_frame(
        &mut client,
        &json!({"jsonrpc":"2.0","id":1,"method":"service.stop","params":{}}),
    )
    .await
    .unwrap();
    drop(client);
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(s.status().await["children"], 0);
    assert_eq!(s.status().await["stopping"], true);
}
#[tokio::test]
async fn overflow_requires_snapshot_resynchronization() {
    let (_f, s, p) = activated("flood").await;
    let mut sub = s.subscribe();
    s.workspace(query(&p, "tasks.get", json!("x")))
        .await
        .unwrap();
    assert_eq!(
        s.receive_event(&mut sub).await.unwrap()["type"],
        "resync_required"
    );
    assert!(s
        .workspace(query(&p, "tasks.get", json!("x")))
        .await
        .is_err());
    s.resynchronize(&p).await.unwrap();
    assert_eq!(s.project_status(&p).await.unwrap()["ready"], true);
    s.stop().await.unwrap();
}
#[tokio::test]
async fn restart_rereads_locators_not_commands_or_prompts() {
    let (f, s, p) = activated("normal").await;
    s.workspace(
        json!({"protocol":{"major":1,"minor":0},"requestId":"start","projectId":p,
        "actorId":"forged","operation":"tasks.start","target":"task","expectedVersion":"1",
        "ownerEpoch":null,"payload":{}}),
    )
    .await
    .unwrap();
    s.stop().await.unwrap();
    drop(s);
    let again = Service::bind(f.config("normal")).await.unwrap();
    // Recovery runs beside the socket, so a slow project never delays the service answering.
    let runner = tokio::spawn(again.clone().serve());
    assert!(ready_within(&again, &p, 8).await);
    assert_eq!(again.status().await["children"], 1);
    assert_eq!(
        again.project_status(&p).await.unwrap()["recovery"],
        "unknown"
    );
    // Fake reports the authenticated actor; client-supplied actor cannot survive forwarding.
    let result = again.workspace(json!({"protocol":{"major":1,"minor":0},"requestId":"draft","projectId":p,
        "actorId":"forged","operation":"tasks.create","target":p,"expectedVersion":"0","ownerEpoch":null,
        "payload":{"description":"test"}})).await.unwrap();
    assert!(result["actor"].as_str().unwrap().starts_with("uid:"));
    let trace = std::fs::read_to_string(f.0.join("projects/one/fake.trace")).unwrap();
    assert_eq!(trace.lines().filter(|l| *l == "tasks.start").count(), 1);
    assert_eq!(trace.lines().filter(|l| *l == "session/prompt").count(), 0);
    again.stop().await.unwrap();
    runner.await.unwrap().unwrap();
}
#[tokio::test]
async fn public_journal_polling_is_independent_of_windows() {
    let (_f, s, _p) = activated("journal").await;
    let mut sub = s.subscribe();
    let event = tokio::time::timeout(Duration::from_secs(2), s.receive_event(&mut sub))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(event["type"], "public_events");
    assert_eq!(event["page"]["items"][0]["eventId"], "event-1");
    s.stop().await.unwrap();
}
#[tokio::test]
async fn a_notification_never_cancels_a_partial_inbound_frame() {
    use tokio::io::AsyncWriteExt;
    let (_f, s, p) = activated("single").await;
    let server = tokio::spawn(s.clone().serve());
    let mut client = connect(s.socket_path()).await.unwrap();
    write_frame(
        &mut client,
        &json!({"jsonrpc":"2.0","id":1,"method":"service.subscribe","params":{}}),
    )
    .await
    .unwrap();
    assert_eq!(read_frame(&mut client).await.unwrap()["id"], 1);
    let frame = workspace_protocol::encode_frame(
        &json!({"jsonrpc":"2.0","id":2,"method":"service.status","params":{}}),
    )
    .unwrap();
    client.write_all(&frame[..2]).await.unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    s.workspace(query(&p, "tasks.get", json!("notify")))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    client.write_all(&frame[2..]).await.unwrap();
    loop {
        let message = tokio::time::timeout(Duration::from_secs(2), read_frame(&mut client))
            .await
            .unwrap()
            .unwrap();
        if message["id"] == 2 {
            break;
        }
        assert_eq!(message["method"], "service.event");
    }
    s.stop().await.unwrap();
    server.await.unwrap().unwrap();
}
#[tokio::test]
async fn screenshot_sized_local_frames_round_trip() {
    let value = json!({"attachments":[{"name":"screen.png","mimeType":"image/png",
        "data":"A".repeat((workspace_protocol::ATTACHMENT_BYTES + 2) / 3 * 4)}]});
    let (mut send, mut receive) = tokio::io::duplex(1024);
    let expected = value.clone();
    let writer = tokio::spawn(async move { write_frame(&mut send, &value).await.unwrap(); });
    let actual = tokio::time::timeout(std::time::Duration::from_secs(10), read_frame(&mut receive)).await.unwrap().unwrap();
    assert_eq!(actual, expected);
    writer.await.unwrap();
}

#[tokio::test]
async fn bounded_frames_and_closed_dispatch() {
    let f = Fixture::new();
    let s = Service::bind(f.config("normal")).await.unwrap();
    let runner = tokio::spawn(s.clone().serve());
    let mut c = connect(s.socket_path()).await.unwrap();
    write_frame(
        &mut c,
        &json!({"jsonrpc":"2.0","id":"x","method":"session/prompt","params":{}}),
    )
    .await
    .unwrap();
    assert!(read_frame(&mut c).await.unwrap().get("error").is_some());
    write_frame(
        &mut c,
        &json!({"jsonrpc":"2.0","id":2,"method":"service.stop","params":{"unexpected":true}}),
    )
    .await
    .unwrap();
    assert!(read_frame(&mut c).await.unwrap().get("error").is_some());
    assert_eq!(s.status().await["stopping"], false);
    use tokio::io::AsyncWriteExt;
    c.write_all(&((workspace_protocol::FRAME_BYTES as u32 + 1).to_be_bytes())).await.unwrap();
    assert!(read_frame(&mut c).await.is_err());
    s.stop().await.unwrap();
    runner.await.unwrap().unwrap();
}
