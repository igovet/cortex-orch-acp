//! Cold-start/socket regressions with disposable roots and no interpreter on the child PATH.
#[path = "support/hermetic.rs"]
mod hermetic;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
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
            "/tmp/cw-pf-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let f = Self { root, child: None };
        for name in [
            "home",
            "config",
            "state",
            "runtime",
            "bin",
            "assets/.git",
            "projects/one",
            "projects/two",
            "projects/broken",
        ] {
            f.directory(&f.root.join(name));
        }
        // Git is available, but no directory on the service PATH contains an interpreter.
        symlink("/usr/bin/git", f.root.join("bin/git")).unwrap();
        assert!(Command::new("/usr/bin/git")
            .args(["init", "-q"])
            .arg(f.root.join("projects/one"))
            .status()
            .unwrap()
            .success());
        f.write(
            &f.root.join("projects/one/readme.txt"),
            b"native file\n",
            0o600,
        );
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
    async fn start(&mut self) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_workspace-service"));
        command.arg(self.root.join("projects"));
        command
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
        tokio::time::timeout(Duration::from_secs(5), async {
            while connect(&self.socket()).await.is_err() {
                assert!(
                    self.child.as_mut().unwrap().try_wait().unwrap().is_none(),
                    "native startup exited"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            self.rpc("service.capabilities", json!({})).await["machine"],
            true
        );
    }
    async fn wire(&self, method: &str, params: Value) -> Value {
        tokio::time::timeout(Duration::from_secs(5), async {
            let mut socket = connect(&self.socket()).await.unwrap();
            write_frame(
                &mut socket,
                &json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}),
            )
            .await
            .unwrap();
            let reply = read_frame(&mut socket).await.unwrap();
            assert_eq!(reply["id"], 1);
            reply
        })
        .await
        .unwrap()
    }
    async fn rpc(&self, method: &str, params: Value) -> Value {
        let reply = self.wire(method, params).await;
        assert!(reply.get("error").is_none(), "{method}: {reply}");
        reply["result"].clone()
    }
    async fn machine(&self, op: &str, project: Option<&str>, params: Value) -> Value {
        let reply = self
            .rpc(
                "machine.call",
                json!({"op":op,"projectId":project,"params":params}),
            )
            .await;
        assert_eq!(reply["ok"], true, "{reply}");
        reply["result"].clone()
    }
    async fn register(&self, name: &str) -> String {
        self.rpc(
            "projects.register",
            json!({"root":self.root.join("projects").join(name),"displayName":name,"newFolder":false}),
        )
        .await["projectId"]
            .as_str()
            .unwrap()
            .to_owned()
    }
    async fn native_requests(&self) -> String {
        let id = self.register("one").await;
        let projects = self.rpc("projects.list", json!({})).await;
        if projects
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["projectId"] == id)
            .unwrap()["activation"]
            .is_null()
        {
            self.rpc("projects.activate", json!({"projectId":id})).await;
        }
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let status = self.rpc("projects.status", json!({"projectId":id})).await;
                assert_ne!(status["state"], "failed", "{status}");
                if status["ready"] == true {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let file = self
            .machine("repo.file", Some(&id), json!({"file":"readme.txt"}))
            .await;
        assert_eq!(file["text"], "native file\n");
        let saved = self
            .machine(
                "repo.file.save",
                Some(&id),
                json!({"file":"readme.txt","text":"native file\n","revision":file["revision"]}),
            )
            .await;
        assert_eq!(saved["outcome"], "saved");
        let git = self.machine("repo.snapshot", Some(&id), json!({})).await;
        assert!(git["checkouts"].is_array(), "{git}");
        let settings = self.machine("config.read", None, json!({})).await;
        assert!(settings.is_object());
        let config_root = if workspace_protocol::channel::current().is_dev() {
            workspace_protocol::channel::current().config_dir(&self.root.join("config"))
        } else {
            self.root.join("config")
        };
        let routing = config_root.join("cortex/routing.json");
        let token = fs::read(&routing)
            .map(|bytes| format!("sha256:{:x}", Sha256::digest(bytes)))
            .unwrap_or_else(|_| "absent".into());
        self.machine(
            "config.save",
            None,
            json!({"surface":"routing","value":{"version":2},"tokens":{"routing":token}}),
        )
        .await;
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(routing).unwrap()).unwrap()
                ["model_policy_version"],
            2
        );
        self.machine(
            "config.action",
            None,
            json!({"action":"profile","verb":"read"}),
        )
        .await;
        let refreshed = self
            .machine("config.action", None, json!({"action":"refresh"}))
            .await;
        assert_eq!(refreshed["surface"], "route_catalog");
        assert_eq!(refreshed["refresh"], json!([]));
        assert_eq!(refreshed["routes"], json!([]));
        // A real prompt-free producer also works with no helper assets.
        let producer: Value =
            serde_json::from_str(include_str!("fixtures/config-catalog-producer-python.json"))
                .unwrap();
        let case = &producer["cases"][0];
        let registry = case["initial"]["config/cortex/connections.json"]["content"]
            .as_str()
            .unwrap()
            .replace("@ROOT@", self.root.to_str().unwrap());
        let registry_path = config_root.join("cortex/connections.json");
        let original = fs::read(&registry_path).ok();
        self.write(&registry_path, registry.as_bytes(), 0o600);
        self.directory(&self.root.join("home/.codex"));
        self.write(
            &self.root.join("bin/peer"),
            case["peer"]
                .as_str()
                .unwrap()
                .replace("@ROOT@", self.root.to_str().unwrap())
                .as_bytes(),
            0o700,
        );
        let observed = self
            .machine("config.action", None, json!({"action":"refresh"}))
            .await;
        assert_eq!(observed["refresh"][0]["success"], true, "{observed}");
        assert_eq!(observed["routes"].as_array().unwrap().len(), 2);
        assert_eq!(fs::read(&registry_path).unwrap(), registry.as_bytes());
        if let Some(original) = original {
            fs::write(&registry_path, original).unwrap();
        } else {
            fs::remove_file(&registry_path).unwrap();
        }
        let created = self.rpc("workspace", json!({
            "protocol":{"major":1,"minor":8},"projectId":id,"actorId":"forged",
            "requestId":"startup-draft","operation":"tasks.create","target":id,
            "expectedVersion":"0","ownerEpoch":null,
            "payload":{"description":"Offline native startup draft","checkout":{"kind":"main"}}
        })).await;
        assert!(created["target"].is_string(), "{created}");
        if !self.root.join("history-seeded").exists() {
            let root = self.root.join("projects/one");
            let channel = if workspace_protocol::channel::current().is_dev() {
                cortex_core::StoreChannel::Dev
            } else {
                cortex_core::StoreChannel::Release
            };
            let store = cortex_core::Store::open(
                cortex_core::ProjectKey::new(&root, channel).unwrap(),
                cortex_core::OpenMode::RuntimeValidate,
            )
            .unwrap();
            cortex_core::lifecycle::TaskMachine::new(&store)
                .create("history-runtime", "Offline history", "fixture")
                .unwrap();
            store.write(|db| {
                db.execute("UPDATE runtime_tasks SET state='CANCELLED' WHERE task_id=\'history-runtime\'", [])?;
                db.execute("INSERT INTO acp_host_sessions VALUES ('history-session',?,'history-runtime','{}','{}','[]','now',NULL)", [root.to_str().unwrap()])?;
                db.execute("INSERT INTO workspace_executions VALUES ('history-execution',?,'history-runtime','history-session',1,1,'now')", [created["target"].as_str().unwrap()])?;
                db.execute("UPDATE workspace_tasks SET execution_id='history-execution' WHERE task_id=?", [created["target"].as_str().unwrap()])?;
                db.execute("INSERT INTO host_transcript(session_id,seq,kind,text,created_at) VALUES ('history-session',1,'agent_message','Saved offline history','now')", [])?;
                Ok(())
            }).unwrap();
            self.write(&self.root.join("history-seeded"), b"seeded", 0o600);
        }
        for operation in ["tasks.page", "history.page"] {
            let target = if operation == "history.page" {
                created["target"].clone()
            } else {
                json!(id)
            };
            let page = self
                .rpc(
                    "native.query",
                    json!({"protocol":{"major":2,"mapping":2},"requestId":operation,"projectId":id,
                "operation":operation,"payload":{"target":target,"cursor":null,"limit":50}}),
                )
                .await;
            let items = page["data"]["value"]["items"].as_array().unwrap();
            assert!(!items.is_empty(), "{page}");
            if operation == "history.page" {
                assert!(page.to_string().contains("Saved offline history"), "{page}");
            }
        }
        // The desktop reads a conversation only when the project offers
        // history.page, and both read routes then serve the same page.
        let status = self.rpc("projects.status", json!({"projectId":id})).await;
        assert!(
            status["capabilities"]["operations"]
                .as_array()
                .unwrap()
                .contains(&json!("history.page")),
            "{status}"
        );
        let page = self
            .rpc(
                "workspace",
                json!({"protocol":{"major":1,"minor":8},"requestId":"read-history","projectId":id,
                    "operation":"history.page","target":created["target"]}),
            )
            .await;
        assert!(
            page["items"].to_string().contains("Saved offline history"),
            "{page}"
        );
        id
    }
    async fn stop(&mut self) {
        self.rpc("service.stop", json!({})).await;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(status) = self.child.as_mut().unwrap().try_wait().unwrap() {
                    assert!(status.success());
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        self.child.take();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[tokio::test]
async fn native_cold_start_without_helper_assets_keeps_all_projects() {
    let mut f = Fixture::new();
    assert!(!f.root.join("assets/src").exists());
    assert!(!f.root.join("assets/crates").exists());
    f.start().await;
    f.native_requests().await;
    f.register("two").await;
    f.register("broken").await;
    f.stop().await;
    let store =
        f.root
            .join("projects/broken")
            .join(if workspace_protocol::channel::current().is_dev() {
                ".cortex-dev"
            } else {
                ".cortex"
            });
    f.write(
        &store.join("cortex.sqlite3"),
        b"invalid fixture store",
        0o600,
    );
    f.start().await;
    let projects = f.rpc("projects.list", json!({})).await;
    assert_eq!(projects.as_array().unwrap().len(), 3, "{projects}");
    f.native_requests().await;
    f.stop().await;
}

#[tokio::test]
async fn native_requests_including_float_profile_verbs_need_no_helper() {
    let mut f = Fixture::new();
    f.start().await;
    f.native_requests().await;
    // A float profile verb is a native refusal, answered with no helper of any kind.
    let reply = f
        .wire(
            "machine.call",
            json!({"op":"config.action","params":{"action":"profile","verb":1.5}}),
        )
        .await;
    assert!(reply.get("error").is_none(), "{reply}");
    assert_eq!(reply["result"]["ok"], false, "{reply}");
    assert_eq!(reply["result"]["code"], "unsupported_action", "{reply}");
    assert!(reply.to_string().len() < 512);
    // An operation no native implementation serves is refused, never delegated.
    let unknown = f
        .wire("machine.call", json!({"op":"retained.helper","params":{}}))
        .await;
    assert!(unknown.get("result").is_none(), "{unknown}");
    assert_eq!(unknown["error"]["message"], "unsupported_operation");
    f.native_requests().await;
    f.stop().await;
}
