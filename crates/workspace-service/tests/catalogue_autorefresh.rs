//! A connection without a saved model list is observed by the service on its own, as the first
//! settings read asks; a saved list is served at once (stale or not); a failed observation is
//! reported, not retried at every read but again a few seconds later. Disposable roots, an owned
//! fake engine, no interpreter.
#[path = "support/hermetic.rs"]
mod hermetic;
use serde_json::{json, Value};
use std::{
    fs,
    os::unix::fs::{symlink, PermissionsExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use workspace_service::{connect, read_frame, write_frame};

struct Fixture {
    root: PathBuf,
    child: Option<Child>,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = PathBuf::from(format!(
            "/tmp/cw-ar-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let f = Self { root, child: None };
        for name in [
            "home/.codex",
            "config",
            "state",
            "runtime",
            "bin",
            "assets/.git",
            "projects",
        ] {
            f.directory(&f.root.join(name));
        }
        symlink("/usr/bin/git", f.root.join("bin/git")).unwrap();
        f
    }
    fn directory(&self, path: &Path) {
        for p in path
            .ancestors()
            .take_while(|p| p.starts_with(&self.root))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            fs::create_dir_all(p).unwrap();
            fs::set_permissions(p, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
    fn write(&self, path: &Path, content: &[u8], mode: u32) {
        self.directory(path.parent().unwrap());
        fs::write(path, content).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }
    fn socket(&self) -> PathBuf {
        workspace_protocol::channel::current()
            .runtime_dir(&self.root.join("runtime"))
            .join("service.sock")
    }
    fn config_root(&self) -> PathBuf {
        if workspace_protocol::channel::current().is_dev() {
            workspace_protocol::channel::current().config_dir(&self.root.join("config"))
        } else {
            self.root.join("config")
        }
    }
    fn catalog(&self, id: &str) -> PathBuf {
        let state = workspace_protocol::channel::current().state_dir(&self.root.join("state"));
        let base = if workspace_protocol::channel::current().is_dev() {
            state.join("state")
        } else {
            self.root.join("state")
        };
        base.join("cortex/catalog").join(id)
    }
    /// The fixture's one codex connection and a peer that answers the model listing (or fails).
    fn engine(&self, peer_ok: bool) {
        let producer: Value =
            serde_json::from_str(include_str!("fixtures/config-catalog-producer-python.json"))
                .unwrap();
        let case = &producer["cases"][0];
        let root = self.root.to_str().unwrap();
        let registry = case["initial"]["config/cortex/connections.json"]["content"]
            .as_str()
            .unwrap()
            .replace("@ROOT@", root);
        let folder = self.config_root().join("cortex");
        self.write(&folder.join("connections.json"), registry.as_bytes(), 0o600);
        self.write(&folder.join("connections.lock"), b"", 0o600);
        let peer = if peer_ok {
            case["peer"].as_str().unwrap().replace("@ROOT@", root)
        } else {
            "#!/bin/sh\nexit 1\n".to_owned()
        };
        self.write(&self.root.join("bin/peer"), peer.as_bytes(), 0o700);
    }
    async fn start(&mut self) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_workspace-service"));
        command
            .arg(self.root.join("projects"))
            .arg(self.root.join("assets"))
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_RUNTIME_DIR", self.root.join("runtime"))
            .env("PATH", self.root.join("bin"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        self.child = Some(command.spawn().unwrap());
        tokio::time::timeout(Duration::from_secs(10), async {
            while connect(&self.socket()).await.is_err() {
                assert!(
                    self.child.as_mut().unwrap().try_wait().unwrap().is_none(),
                    "startup exited"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
    async fn config_read(&self) -> Value {
        tokio::time::timeout(Duration::from_secs(30), async {
            let mut socket = connect(&self.socket()).await.unwrap();
            write_frame(&mut socket, &json!({"jsonrpc":"2.0","id":1,"method":"machine.call","params":{"op":"config.read"}}))
                .await
                .unwrap();
            let reply = read_frame(&mut socket).await.unwrap();
            assert_eq!(reply["result"]["ok"], true, "{reply}");
            reply["result"]["result"].clone()
        })
        .await
        .unwrap()
    }
    /// Reads until the background refresh has ended; every read returns the same fixed shape.
    async fn settled(&self) -> Value {
        for _ in 0..600 {
            let read = self.config_read().await;
            if read["catalogueRefresh"]["running"] == false {
                return read;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the refresh never ended");
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop();
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn models(read: &Value) -> usize {
    read["config"]["catalogue"]["codex-acp"]["models"]
        .as_array()
        .unwrap()
        .len()
}
fn seen(f: &Fixture) -> usize {
    fs::read_to_string(f.root.join("bin/seen"))
        .map(|s| s.lines().count())
        .unwrap_or(0)
}

#[tokio::test]
async fn a_connection_without_a_saved_list_is_observed_by_the_service_on_its_own() {
    let mut f = Fixture::new();
    f.engine(true);
    f.start().await;
    // The first read already says the models are being fetched, and offers none yet.
    let first = f.config_read().await;
    assert_eq!(first["config"]["setup"]["codex-acp"]["status"], "connected");
    assert_eq!(models(&first), 0);
    assert_eq!(first["catalogueRefresh"]["running"], true, "{first}");
    // Nobody pressed Refresh models: the list arrives by itself.
    let done = f.settled().await;
    assert_eq!(done["catalogueRefresh"]["failed"], json!([]));
    assert_eq!(models(&done), 2, "{done}");
    // A saved list is not observed again by later reads.
    let calls = seen(&f);
    assert!(calls > 0);
    for _ in 0..3 {
        let read = f.config_read().await;
        assert_eq!(
            read["catalogueRefresh"],
            json!({"running": false, "failed": []})
        );
        assert_eq!(models(&read), 2);
    }
    assert_eq!(seen(&f), calls, "the saved list was observed again");
}

#[tokio::test]
async fn a_retained_list_is_served_at_once_even_when_old_and_refreshed_behind_it() {
    let mut f = Fixture::new();
    f.engine(true);
    f.start().await;
    f.settled().await;
    f.stop();
    // A restart: the saved list is a week old.
    let dir = f.catalog("cn_000000000001");
    let old = std::time::SystemTime::now() - Duration::from_secs(7 * 24 * 3600);
    for file in fs::read_dir(&dir).unwrap().flatten() {
        if file
            .file_name()
            .to_str()
            .is_some_and(|n| n.starts_with("cs_"))
        {
            fs::File::options()
                .write(true)
                .open(file.path())
                .unwrap()
                .set_modified(old)
                .unwrap();
        }
    }
    let before = seen(&f);
    f.start().await;
    let first = f.config_read().await;
    assert_eq!(
        models(&first),
        2,
        "an old list must still be shown: {first}"
    );
    assert_eq!(first["catalogueRefresh"]["running"], true);
    let done = f.settled().await;
    assert_eq!(models(&done), 2);
    assert!(seen(&f) > before, "the old list was never observed again");
}

#[tokio::test]
async fn a_failed_observation_is_reported_and_not_retried_by_every_read() {
    let mut f = Fixture::new();
    f.engine(false);
    f.start().await;
    let done = f.settled().await;
    assert_eq!(models(&done), 0);
    let failed = done["catalogueRefresh"]["failed"].as_array().unwrap();
    assert_eq!(failed.len(), 1, "{done}");
    assert_eq!(failed[0]["connectionId"], "cn_000000000001");
    assert!(
        failed[0]["reason"]
            .as_str()
            .unwrap()
            .starts_with("engine_unreachable"),
        "{done}"
    );
    // Reading again neither relaunches the engine nor forgets the failure.
    let again = f.config_read().await;
    assert_eq!(again["catalogueRefresh"]["running"], false);
    assert_eq!(
        again["catalogueRefresh"]["failed"],
        done["catalogueRefresh"]["failed"]
    );
}

/// A first launch whose refresh failed for a passing reason must not stay without that engine's
/// models for the long cooldown: a read a few seconds later observes it again, and the list appears.
#[tokio::test]
async fn a_failed_observation_is_tried_again_seconds_later_and_its_list_arrives() {
    let mut f = Fixture::new();
    f.engine(false);
    f.start().await;
    let failed = f.settled().await;
    assert_eq!(models(&failed), 0);
    assert_eq!(
        failed["catalogueRefresh"]["failed"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "{failed}"
    );
    // The cause passes (here: the engine answers now). Nobody presses Refresh models.
    f.engine(true);
    let started = std::time::Instant::now();
    let mut read = f.config_read().await;
    while models(&read) == 0 {
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "the failed connection was not tried again soon: {read}"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
        read = f.config_read().await;
    }
    assert_eq!(models(&read), 2, "{read}");
    let done = f.settled().await;
    assert_eq!(done["catalogueRefresh"]["failed"], json!([]));
}
