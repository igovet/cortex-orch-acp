#[path = "support/hermetic.rs"]
mod hermetic;
use serde_json::{json, Value};
use std::{
    os::unix::fs::{symlink, PermissionsExt},
    path::{Path, PathBuf},
    time::Duration,
};
use workspace_service::{
    connect, native_runtime::UnqualifiedRoutes, read_frame, write_frame, Config, LaunchSpec,
    Service,
};

/// The native host is the only execution host; these tests exercise the service around it.
struct Native;
impl Native {
    async fn bind(config: Config) -> std::io::Result<std::sync::Arc<Service>> {
        Service::bind_native(config, std::sync::Arc::new(UnqualifiedRoutes)).await
    }
}

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
    fn config(&self) -> Config {
        Config {
            runtime_dir: self.0.join("runtime"),
            state_dir: self.0.join("state"),
            allowed_root: self.0.join("projects"),
            launch: LaunchSpec::bare(),
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
async fn activated() -> (Fixture, std::sync::Arc<Service>, String) {
    let f = Fixture::new();
    let s = Native::bind(f.config()).await.unwrap();
    let p = s.register(&f.0.join("projects/one"), "One").await.unwrap();
    assert!(p["activation"].is_null());
    let id = p["projectId"].as_str().unwrap().to_owned();
    s.activate(&id).await.unwrap();
    (f, s, id)
}
#[tokio::test]
async fn storage_refusal_keeps_project_registered_and_explains_reads_and_activation() {
    use cortex_core::{OpenMode, ProjectKey, Store, StoreChannel};

    let f = Fixture::new();
    let s = Native::bind(f.config()).await.unwrap();
    let root = f.0.join("projects/one");
    let registered = s.register(&root, "Saved project").await.unwrap();
    let project = registered["projectId"].as_str().unwrap();
    let channel = if workspace_protocol::channel::current().is_dev() {
        StoreChannel::Dev
    } else {
        StoreChannel::Release
    };
    let key = ProjectKey::new(&root, channel).unwrap();
    let store = Store::open(key.clone(), OpenMode::Initialize).unwrap();
    store
        .write(|db| Ok(db.pragma_update(None, "user_version", 10)?))
        .unwrap();
    drop(store);
    let original = std::fs::read(key.database()).unwrap();

    assert_eq!(
        s.activate(project).await.unwrap_err().to_string(),
        "unsupported_storage"
    );
    let status = s.project_status(project).await.unwrap();
    assert_eq!(status["state"], "failed");
    assert_eq!(status["reason"], "unsupported_storage");
    let guidance = status["message"].as_str().unwrap();
    assert!(guidance.contains("format 10 from the retired Python Cortex runtime"));
    assert_eq!(s.projects().await[0]["projectId"], project);

    let server = tokio::spawn(s.clone().serve());
    let mut client = connect(s.socket_path()).await.unwrap();
    for (id, method, params) in [
        (
            1,
            "native.query",
            json!({"protocol":{"major":2,"mapping":2},"requestId":"storage-read",
            "projectId":project,"operation":"tasks.page","payload":{"target":project,"cursor":null,"limit":20}}),
        ),
        (2, "projects.activate", json!({"projectId":project})),
    ] {
        write_frame(
            &mut client,
            &json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
        )
        .await
        .unwrap();
        let reply = read_frame(&mut client).await.unwrap();
        assert_eq!(reply["id"], id);
        assert_eq!(
            reply["error"]["data"]["reason"], "unsupported_storage",
            "{reply}"
        );
        assert_eq!(reply["error"]["data"]["kind"], "project_store");
        assert_eq!(reply["error"]["message"], guidance);
    }

    for (id, op, params) in [
        (3, "project.config.read", json!({})),
        (
            4,
            "project.config.save",
            json!({"surface":"project_ceiling","value":{},
            "tokens":{"project_ceiling":"absent"}}),
        ),
    ] {
        write_frame(
            &mut client,
            &json!({"jsonrpc":"2.0","id":id,"method":"machine.call",
            "params":{"op":op,"projectId":project,"params":params}}),
        )
        .await
        .unwrap();
        let reply = read_frame(&mut client).await.unwrap();
        assert_eq!(reply["id"], id);
        assert_eq!(reply["result"]["code"], "unsupported_storage", "{reply}");
        assert_eq!(reply["result"]["message"], guidance);
    }
    assert_eq!(std::fs::read(key.database()).unwrap(), original);
    assert!(!key.directory().join(".migration-backups").exists());
    drop(client);
    s.stop().await.unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn workspace_activation_upgrades_and_recovers_legacy_work_under_the_same_fences() {
    use cortex_core::{lifecycle::TaskMachine, OpenMode, ProjectKey, Store, StoreChannel};

    let f = Fixture::new();
    let s = Native::bind(f.config()).await.unwrap();
    let root = f.0.join("projects/one");
    let registered = s.register(&root, "Saved project").await.unwrap();
    let project = registered["projectId"].as_str().unwrap();
    let channel = if workspace_protocol::channel::current().is_dev() {
        StoreChannel::Dev
    } else {
        StoreChannel::Release
    };
    let key = ProjectKey::new(&root, channel).unwrap();
    let store = Store::open(key.clone(), OpenMode::Initialize).unwrap();
    TaskMachine::new(&store)
        .create("legacy", "Preserve the stopped work", "python-owner")
        .unwrap();
    store
        .write(|db| {
            db.execute(
                "UPDATE runtime_tasks SET state='PAUSED' WHERE task_id='legacy'",
                [],
            )?;
            db.execute(
                "INSERT INTO runtime_events(task_id,type,actor,payload_json,created_at)
                VALUES ('legacy','CHECK_EXECUTION_STARTED','runtime',?,'0')",
                [json!({"command_ref":"legacy-check","revision":0}).to_string()],
            )?;
            db.pragma_update(None, "user_version", 11)?;
            Ok(())
        })
        .unwrap();
    drop(store);

    assert_eq!(s.activate(project).await.unwrap()["executionHost"], "rust");
    assert_eq!(s.project_status(project).await.unwrap()["ready"], true);
    let backup_dir = key.directory().join(".migration-backups");
    assert_eq!(std::fs::read_dir(backup_dir).unwrap().count(), 1);
    let store = Store::open(key, OpenMode::LiveReadOnly).unwrap();
    store
        .read(|db| {
            assert_eq!(
                db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?,
                12
            );
            assert_eq!(
                db.query_row(
                    "SELECT state,blocker FROM runtime_tasks WHERE task_id='legacy'",
                    [],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                )?,
                ("PAUSED".into(), "legacy_effect_outcome_unknown".into())
            );
            assert_eq!(
                db.query_row(
                    "SELECT count(*) FROM runtime_events WHERE task_id='legacy'
            AND type='NATIVE_TASK_ADOPTED' AND json_extract(payload_json,'$.replayed')=0",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                1
            );
            Ok(())
        })
        .unwrap();
    drop(store);
    s.stop().await.unwrap();
}

#[tokio::test]
async fn serve_joins_background_owners_before_returning() {
    use std::{future::Future, sync::Arc, task::Poll};
    let f = Fixture::new();
    let service = Native::bind(f.config()).await.unwrap();
    let weak = Arc::downgrade(&service);
    let mut serving = Box::pin(service.clone().serve());
    // Poll once without yielding to the spawned background owners. Their abort
    // requests must be observed, not merely enqueued, before serve returns.
    std::future::poll_fn(|cx| {
        assert!(serving.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    service.stop().await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), &mut serving)
        .await
        .unwrap()
        .unwrap();
    drop(serving);
    drop(service);
    assert!(
        weak.upgrade().is_none(),
        "background task still owns the service"
    );
    let reopened = Native::bind(f.config()).await.unwrap();
    reopened.stop().await.unwrap();
}

#[tokio::test]
async fn native_queries_and_small_deltas_use_one_core_generation_without_a_child_read() {
    let f = Fixture::new();
    let mut config = f.config();
    config.event_capacity = 128;
    let s = Native::bind(config).await.unwrap();
    let root = f.0.join("projects/one");
    let p = s.register(&root, "One").await.unwrap();
    let id = p["projectId"].as_str().unwrap().to_owned();
    s.activate(&id).await.unwrap();
    let channel = if workspace_protocol::channel::current().is_dev() {
        cortex_core::StoreChannel::Dev
    } else {
        cortex_core::StoreChannel::Release
    };
    let store = cortex_core::Store::open(
        cortex_core::ProjectKey::new(&root, channel).unwrap(),
        cortex_core::OpenMode::Initialize,
    )
    .unwrap();
    store.write(|db|{
        db.execute("INSERT INTO workspace_tasks(task_id,project_id,description,created_at) VALUES ('native-task',?,'Initial','now')",[&id])?;
        Ok(())
    }).unwrap();
    let query = json!({"protocol":{"major":2,"mapping":2},"requestId":"native-read","projectId":id,"operation":"tasks.page","payload":{"target":id,"cursor":null,"limit":50}});
    let reply = s.native_query(query.clone()).await.unwrap();
    assert_eq!(reply["data"]["value"]["items"][0]["description"], "Initial");
    assert_eq!(
        s.native_query(query).await.unwrap()["generation"],
        reply["generation"]
    );
    let mut events = s.subscribe();
    let serving = tokio::spawn(s.clone().serve());
    store.write(|db|{
        db.execute("UPDATE workspace_tasks SET description='Changed',version=version+1 WHERE task_id='native-task'",[])?;
        cortex_core::store::append_public_tx(db,&id,"native-task","task",2)?;
        Ok(())
    }).unwrap();
    let delta = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let event = s.receive_event(&mut events).await.unwrap();
            if event["type"] == "native_delta" {
                break event;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(delta["batch"]["generation"], reply["generation"]);
    assert_eq!(
        delta["batch"]["changes"][0]["operation"]["data"]["data"]["description"],
        "Changed"
    );
    assert!(serde_json::to_vec(&delta).unwrap().len() < 4096);
    s.stop().await.unwrap();
    serving.await.unwrap().unwrap();
}
#[tokio::test]
async fn dev_refuses_unowned_dev_store_and_rechecks_activation_without_writes() {
    let f = Fixture::new();
    let s = Native::bind(f.config()).await.unwrap();
    let root = f.0.join("projects/one");
    std::fs::create_dir(root.join(".cortex-dev")).unwrap();
    std::fs::set_permissions(
        root.join(".cortex-dev"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
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
    let s = Native::bind(f.config()).await.unwrap();
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
    std::fs::set_permissions(
        root.join(".cortex-workspace-dev.json"),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    s.stop().await.unwrap();
    drop(s);
    let again = Native::bind(f.config()).await.unwrap();
    again
        .activate(p["projectId"].as_str().unwrap())
        .await
        .unwrap();
    assert!(marker.is_file());
    assert!(!root.join(".cortex-dev-owner.json").exists());
    assert!(!root.join(".cortex-workspace-dev.json").exists());
    assert_eq!(
        serde_json::from_str::<Value>(
            &std::fs::read_to_string(root.join(".cortex-dev/legacy-workspace.json")).unwrap()
        )
        .unwrap(),
        old
    );
    again
        .activate(p["projectId"].as_str().unwrap())
        .await
        .unwrap();
    let mut files: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    files.sort();
    assert_eq!(files, vec![".cortex-dev"]);
    again.stop().await.unwrap();
}

#[tokio::test]
async fn dev_opens_beside_release_without_touching_release() {
    let f = Fixture::new();
    let s = Native::bind(f.config()).await.unwrap();
    let root = f.0.join("projects/one");
    std::fs::create_dir(root.join(".cortex")).unwrap();
    std::fs::write(root.join(".cortex/cortex.sqlite3"), "release-canary").unwrap();
    let before = std::fs::metadata(root.join(".cortex"))
        .unwrap()
        .modified()
        .unwrap();
    let p = s.register(&root, "Shared folder").await.unwrap();
    s.activate(p["projectId"].as_str().unwrap()).await.unwrap();
    assert!(root.join(".cortex-dev").is_dir());
    assert_eq!(
        std::fs::read_to_string(root.join(".cortex/cortex.sqlite3")).unwrap(),
        "release-canary"
    );
    assert_eq!(
        std::fs::metadata(root.join(".cortex"))
            .unwrap()
            .modified()
            .unwrap(),
        before
    );
    assert_eq!(std::fs::read_dir(root.join(".cortex")).unwrap().count(), 1);
    s.stop().await.unwrap();
}

#[tokio::test]
async fn lock_modes_stale_socket_and_real_same_user_peers() {
    let f = Fixture::new();
    let s = Native::bind(f.config()).await.unwrap();
    assert!(Native::bind(f.config()).await.is_err());
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
    let again = Native::bind(f.config()).await.unwrap();
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
            {
            }
        }
    }

    let f = Fixture::new();
    let s = Native::bind(f.config()).await.unwrap();
    let mut pipe = [-1; 2];
    assert_eq!(
        unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC) },
        0
    );
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
    let forked = Forked {
        pid,
        release: Some(write),
    };
    assert!(Native::bind(f.config()).await.is_err());
    s.stop().await.unwrap();
    drop(s);
    let again = Native::bind(f.config()).await.unwrap();
    assert!(Native::bind(f.config()).await.is_err());
    drop(forked);
    assert!(Native::bind(f.config()).await.is_err());
    again.stop().await.unwrap();
}

#[tokio::test]
async fn unsafe_runtime_and_stale_endpoint_are_never_replaced() {
    let f = Fixture::new();
    let runtime = f.0.join("runtime");
    std::fs::create_dir(&runtime).unwrap();
    // World-writable: refused, never repaired.
    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o777)).unwrap();
    assert!(Native::bind(f.config()).await.is_err());
    assert_eq!(
        std::fs::metadata(&runtime).unwrap().permissions().mode() & 0o777,
        0o777
    );
    // Owned and merely permissive (0755): made private (0700) before use.
    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o755)).unwrap();
    let repaired = Native::bind(f.config()).await.unwrap();
    assert_eq!(
        std::fs::metadata(&runtime).unwrap().permissions().mode() & 0o777,
        0o700
    );
    repaired.stop().await.unwrap();
    drop(repaired);
    let _ = std::fs::remove_file(runtime.join("service.sock"));
    let _ = std::fs::remove_file(runtime.join("service.lock"));
    let target = f.0.join("keep");
    std::fs::write(&target, "do not touch").unwrap();
    symlink(&target, runtime.join("service.sock")).unwrap();
    assert!(Native::bind(f.config()).await.is_err());
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "do not touch");
    assert!(std::fs::symlink_metadata(runtime.join("service.sock"))
        .unwrap()
        .file_type()
        .is_symlink());
}
#[tokio::test]
async fn registration_is_inert_and_rejects_private_and_escape() {
    let f = Fixture::new();
    let s = Native::bind(f.config()).await.unwrap();
    std::fs::create_dir(f.0.join("projects/one/.cortex")).unwrap();
    for p in ["projects/one/.cortex", "state", "projects/../state"] {
        assert!(s.register(&f.0.join(p), "Bad").await.is_err(), "{p}");
    }
    // A release store can coexist; it is not the dev channel store.
    let p = s.register(&f.0.join("projects/one"), "One").await.unwrap();
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
    let s = Native::bind(f.config()).await.unwrap();
    symlink(f.0.join("projects/one"), f.0.join("projects/link")).unwrap();
    let linked = s
        .register(&f.0.join("projects/link"), "Linked")
        .await
        .unwrap();
    assert_eq!(
        linked["canonicalRoot"],
        f.0.join("projects/one").to_str().unwrap()
    );
    let wanted = f.0.join("projects/new/deep");
    assert_eq!(
        s.register(&wanted, "New").await.unwrap_err().to_string(),
        "project_folder_missing"
    );
    assert!(!wanted.exists());
    assert_eq!(
        s.register_folder(&wanted, "New", true).await.unwrap()["canonicalRoot"],
        wanted.to_str().unwrap()
    );
    assert!(wanted.is_dir());
    let file = f.0.join("projects/file");
    std::fs::write(&file, "x").unwrap();
    assert_eq!(
        s.register(&file, "File").await.unwrap_err().to_string(),
        "project_folder_not_directory"
    );
    assert_eq!(
        s.register(Path::new("relative/path"), "Rel")
            .await
            .unwrap_err()
            .to_string(),
        "absolute_path_required"
    );
    s.stop().await.unwrap();
}
#[tokio::test]
async fn a_machine_wide_root_still_refuses_private_and_read_only_folders() {
    let f = Fixture::new();
    let mut config = f.config();
    config.allowed_root = PathBuf::from("/");
    let s = Native::bind(config).await.unwrap();
    let outside = f.0.join("outside-the-old-root");
    std::fs::create_dir(&outside).unwrap();
    assert!(s.register(&outside, "Outside").await.is_ok());
    for private in ["state", "runtime"] {
        assert_eq!(
            s.register(&f.0.join(private), "P")
                .await
                .unwrap_err()
                .to_string(),
            "project_path_refused",
            "{private}"
        );
    }
    assert_eq!(
        s.register(Path::new("/usr"), "Usr")
            .await
            .unwrap_err()
            .to_string(),
        "project_folder_not_writable"
    );
    s.stop().await.unwrap();
}
#[tokio::test]
async fn a_missing_folder_never_stops_the_service_and_recovers_when_it_returns() {
    let (f, s, p) = activated().await;
    s.stop().await.unwrap();
    drop(s);
    let root = f.0.join("projects/one");
    let parked = f.0.join("parked");
    std::fs::rename(&root, &parked).unwrap();
    let again = Native::bind(f.config()).await.unwrap();
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
/// The owner's upgrade: an older build recorded the activation but wrote its own legacy marker
/// and kept its data in `.cortex`. The new build has no reservation and no dev store for it.
async fn legacy_activated(f: &Fixture) -> (PathBuf, String) {
    let s = Native::bind(f.config()).await.unwrap();
    let root = f.0.join("projects/one");
    let id = s.register(&root, "One").await.unwrap()["projectId"]
        .as_str()
        .unwrap()
        .to_owned();
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
    let again = Native::bind(f.config()).await.unwrap();
    let runner = tokio::spawn(again.clone().serve());
    assert!(
        ready_within(&again, &id, 8).await,
        "{}",
        again.project_status(&id).await.unwrap()
    );
    assert!(root.join(".cortex-dev/owner.json").is_file() && root.join(".cortex-dev").is_dir());
    // Legacy data is left exactly as it was: never adopted, moved or migrated.
    assert_eq!(
        std::fs::read_to_string(root.join(".cortex-workspace-dev.json")).unwrap(),
        "legacy-marker"
    );
    assert_eq!(
        std::fs::read_to_string(root.join(".cortex/cortex.sqlite3")).unwrap(),
        "legacy-data"
    );
    assert_eq!(std::fs::read_dir(root.join(".cortex")).unwrap().count(), 1);
    again.stop().await.unwrap();
    runner.await.unwrap().unwrap();
}
#[tokio::test]
async fn an_unreserved_existing_dev_store_is_still_refused_with_a_reason() {
    let f = Fixture::new();
    let (root, id) = legacy_activated(&f).await;
    std::fs::create_dir(root.join(".cortex-dev")).unwrap();
    std::fs::set_permissions(
        root.join(".cortex-dev"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    std::fs::write(root.join(".cortex-dev/cortex.sqlite3"), "unowned").unwrap();
    let again = Native::bind(f.config()).await.unwrap();
    let runner = tokio::spawn(again.clone().serve());
    tokio::time::sleep(Duration::from_millis(400)).await;
    let status = again.project_status(&id).await.unwrap();
    assert_eq!(status["state"], "failed");
    assert!(status["reason"]
        .as_str()
        .unwrap()
        .starts_with("dev_project_refused"));
    assert!(!root.join(".cortex-dev-owner.json").exists());
    assert_eq!(
        std::fs::read_to_string(root.join(".cortex-dev/cortex.sqlite3")).unwrap(),
        "unowned"
    );
    again.stop().await.unwrap();
    runner.await.unwrap().unwrap();
}

#[tokio::test]
async fn removing_a_project_preserves_files_and_readding_preserves_identity() {
    let (f, s, p) = activated().await;
    let root = f.0.join("projects/one");
    std::fs::write(root.join("owner.txt"), "keep").unwrap();
    assert_eq!(s.remove(&p).await.unwrap()["outcome"], "removed");
    assert!(s.projects().await.as_array().unwrap().is_empty());
    assert_eq!(
        std::fs::read_to_string(root.join("owner.txt")).unwrap(),
        "keep"
    );
    assert_eq!(s.register(&root, "Again").await.unwrap()["projectId"], p);
    s.stop().await.unwrap();
}
#[tokio::test]
async fn explicit_stop_outlives_its_requesting_connection() {
    let (_f, s, _p) = activated().await;
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
    assert_eq!(s.status().await["stopping"], true);
}
#[tokio::test]
async fn restart_rereads_locators_not_commands_or_prompts() {
    let (f, s, p) = activated().await;
    s.stop().await.unwrap();
    drop(s);
    let again = Native::bind(f.config()).await.unwrap();
    // Recovery runs beside the socket, so a slow project never delays the service answering.
    let runner = tokio::spawn(again.clone().serve());
    assert!(ready_within(&again, &p, 8).await);
    assert_eq!(again.project_status(&p).await.unwrap()["recovery"], "fresh");
    // Native creation uses the authenticated actor, never the client's field.
    let result = again.workspace(json!({"protocol":{"major":1,"minor":0},"requestId":"draft","projectId":p,
        "actorId":"forged","operation":"tasks.create","target":p,"expectedVersion":"0","ownerEpoch":null,
        "payload":{"description":"test"}})).await.unwrap();
    assert_eq!(result["outcome"], "recorded");
    let channel = if workspace_protocol::channel::current().is_dev() {
        cortex_core::StoreChannel::Dev
    } else {
        cortex_core::StoreChannel::Release
    };
    let store = cortex_core::Store::open(
        cortex_core::ProjectKey::new(f.0.join("projects/one"), channel).unwrap(),
        cortex_core::OpenMode::LiveReadOnly,
    )
    .unwrap();
    store
        .read(|db| {
            assert!(db
                .query_row("SELECT actor_id FROM workspace_requests", [], |r| r
                    .get::<_, String>(0))?
                .starts_with("uid:"));
            // Nothing was started or prompted by the restart.
            assert_eq!(
                db.query_row(
                    "SELECT COUNT(*) FROM workspace_tasks WHERE execution_id IS NOT NULL",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                0
            );
            Ok(())
        })
        .unwrap();
    again.stop().await.unwrap();
    runner.await.unwrap().unwrap();
}
#[tokio::test]
async fn a_notification_never_cancels_a_partial_inbound_frame() {
    use tokio::io::AsyncWriteExt;
    let (f, s, _p) = activated().await;
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
    // A second project becoming active publishes a service event mid-frame.
    let other = f.0.join("projects/two");
    std::fs::create_dir(&other).unwrap();
    let two = s.register(&other, "Two").await.unwrap();
    s.activate(two["projectId"].as_str().unwrap())
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
    let writer = tokio::spawn(async move {
        write_frame(&mut send, &value).await.unwrap();
    });
    let actual = tokio::time::timeout(std::time::Duration::from_secs(10), read_frame(&mut receive))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(actual, expected);
    writer.await.unwrap();
}

#[tokio::test]
async fn bounded_frames_and_closed_dispatch() {
    let f = Fixture::new();
    let s = Native::bind(f.config()).await.unwrap();
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
    c.write_all(&((workspace_protocol::FRAME_BYTES as u32 + 1).to_be_bytes()))
        .await
        .unwrap();
    assert!(read_frame(&mut c).await.is_err());
    s.stop().await.unwrap();
    runner.await.unwrap().unwrap();
}
#[tokio::test]
async fn a_caller_past_the_connection_bound_waits_for_a_slot_instead_of_being_dropped() {
    let f = Fixture::new();
    let s = Native::bind(f.config()).await.unwrap();
    let server = tokio::spawn(s.clone().serve());
    // Idle callers hold every connection the service serves at once.
    let mut held = Vec::new();
    for _ in 0..workspace_service::CONNECTION_LIMIT {
        held.push(connect(s.socket_path()).await.unwrap());
    }
    let mut late = connect(s.socket_path()).await.unwrap();
    write_frame(
        &mut late,
        &json!({"jsonrpc":"2.0","id":"late","method":"service.status","params":{}}),
    )
    .await
    .unwrap();
    let reply = tokio::spawn(async move { read_frame(&mut late).await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !reply.is_finished(),
        "a caller past the bound was answered or dropped"
    );
    drop(held.pop());
    let reply = tokio::time::timeout(Duration::from_secs(3), reply)
        .await
        .expect("the waiting caller is served once a connection ends")
        .unwrap()
        .expect("the waiting caller is answered, not disconnected");
    assert_eq!(reply["id"], "late");
    assert!(reply.get("result").is_some());
    drop(held);
    s.stop().await.unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(3), server).await;
}
