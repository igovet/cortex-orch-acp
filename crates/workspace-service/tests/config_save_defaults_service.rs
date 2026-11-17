//! Native socket saves: complete offline Python replies and exact file bytes/modes.
//! The native host alone answers, even on refusals.
#[path = "support/hermetic.rs"]
mod hermetic;
use serde_json::{json, Value};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::Duration,
};
use workspace_service::{connect, read_frame, write_frame};
struct Fixture {
    root: PathBuf,
    child: Option<Child>,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = PathBuf::from(format!(
            "/tmp/cw-mds-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        for name in [
            "home", "config", "state", "data", "runtime", "bin", "projects",
        ] {
            fs::create_dir(root.join(name)).unwrap();
            fs::set_permissions(root.join(name), fs::Permissions::from_mode(0o700)).unwrap();
        }
        Self { root, child: None }
    }
    fn config(&self) -> PathBuf {
        if workspace_protocol::channel::current().is_dev() {
            workspace_protocol::channel::current().config_dir(&self.root.join("config"))
        } else {
            self.root.join("config")
        }
    }
    fn state(&self) -> PathBuf {
        workspace_protocol::channel::current().state_dir(&self.root.join("state"))
    }
    fn socket(&self) -> PathBuf {
        workspace_protocol::channel::current()
            .runtime_dir(&self.root.join("runtime"))
            .join("service.sock")
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
        assert!(reply.get("error").is_none(), "{reply}");
        reply["result"].clone()
    }
    async fn start(&mut self, routing_override: bool) {
        self.child = Some(
            Command::new(env!("CARGO_BIN_EXE_workspace-service"))
                .arg(self.root.join("projects"))
                .arg(
                    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                        .parent()
                        .unwrap()
                        .parent()
                        .unwrap(),
                )
                .env_clear()
                .env("HOME", self.root.join("home"))
                .envs(
                    routing_override
                        .then(|| ("CORTEX_ROUTING", self.config().join("cortex/override.json"))),
                )
                .env("PATH", self.root.join("bin"))
                .env("XDG_CONFIG_HOME", self.root.join("config"))
                .env("XDG_STATE_HOME", self.root.join("state"))
                .env("XDG_DATA_HOME", self.root.join("data"))
                .env("XDG_RUNTIME_DIR", self.root.join("runtime"))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                assert!(self.child.as_mut().unwrap().try_wait().unwrap().is_none());
                if connect(&self.socket()).await.is_ok() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    fn initialize(&self, case: &Value) {
        for (key, path) in [
            ("initial", self.config().join("cortex/defaults.json")),
            ("registry", self.config().join("cortex/connections.json")),
            ("workspace", self.state().join("defaults.json")),
            ("ignore", self.config().join("cortex/.gitignore")),
        ] {
            if let Some(s) = case[key].as_str() {
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o700))
                    .unwrap();
                fs::write(&path, s).unwrap();
                fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
            } else {
                match fs::remove_file(&path) {
                    Ok(()) => (),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                    Err(e) => panic!("{e}"),
                }
            }
        }
    }
    fn files(&self) -> Value {
        let mut v = json!({});
        for (key, path) in [
            ("owner", self.config().join("cortex/defaults.json")),
            ("workspace", self.state().join("defaults.json")),
            ("ignore", self.config().join("cortex/.gitignore")),
        ] {
            v[key] = match fs::read_to_string(&path) {
                Ok(body) => {
                    json!({"body":body,"mode":fs::metadata(path).unwrap().permissions().mode() & 0o777})
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Value::Null,
                Err(e) => panic!("{e}"),
            };
        }
        v
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
fn bind(v: &Value, config: &Path, state: &Path, root: &Path) -> Value {
    match v {
        Value::String(s) => json!(s
            .replace("$CONFIG", config.to_str().unwrap())
            .replace("$WORKSPACE", state.to_str().unwrap())
            .replace("$ROOT", root.to_str().unwrap())),
        Value::Array(a) => json!(a
            .iter()
            .map(|v| bind(v, config, state, root))
            .collect::<Vec<_>>()),
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, v)| (k.clone(), bind(v, config, state, root)))
                .collect(),
        ),
        v => v.clone(),
    }
}
fn expected_wire(reply: &Value, f: &Fixture) -> Value {
    if reply["message"] == "AttributeError" {
        // Observed on the real retained-provider socket before cutover: the
        // Python wrapper did not catch AttributeError and emitted no stdout.
        json!({"jsonrpc":"2.0","id":1,"error":{"code":-32000,"message":"machine_tool_failed"}})
    } else {
        json!({"jsonrpc":"2.0","id":1,"result":bind(reply, &f.config(), &f.state(), &f.root)})
    }
}
async fn save_observation(f: &Fixture, observation: &Value, name: &Value) {
    let mut params = observation["params"].clone();
    if params.is_object() {
        params["configRoot"] = json!("/not-an-authorized-target");
        params["refresh"] = json!(true);
    }
    let reply = f
        .wire("machine.call", json!({"op":"config.save","params":params}))
        .await;
    assert_eq!(reply, expected_wire(&observation["reply"], f), "{name}");
}
#[tokio::test]
async fn native_defaults_socket_matches_full_oracle_replies_and_files() {
    let corpus: Value =
        serde_json::from_str(include_str!("fixtures/machine-defaults-python.json")).unwrap();
    let mut f = Fixture::new();
    f.start(false).await;
    let mut calls = 0;
    for case in corpus["cases"].as_array().unwrap() {
        f.initialize(case);
        for observation in case["observations"].as_array().unwrap() {
            save_observation(&f, observation, &case["name"]).await;
            assert_eq!(f.files(), observation["files"], "{}", case["name"]);
            calls += 1;
        }
    }
    assert_eq!(calls, 56);
    for params in [Value::Null, json!([]), json!(1), json!("malformed")] {
        let reply = f
            .wire("machine.call", json!({"op":"config.save","params":params}))
            .await;
        assert_eq!(
            reply,
            expected_wire(&json!({"message":"AttributeError"}), &f)
        );
    }
    for surface in [Value::Null, json!("unknown"), json!([]), json!({})] {
        let reply = f
            .rpc(
                "machine.call",
                json!({"op":"config.save","params":{"surface":surface}}),
            )
            .await;
        assert_eq!(
            reply,
            json!({"ok":false,"code":"unsupported_action","message":"This machine does not support that yet."})
        );
    }
    assert!(!f.root.join("projects/.cortex").exists());
    assert!(!f.root.join("projects/.cortex-dev").exists());
    f.stop().await;
}
#[tokio::test]
async fn native_routing_mcp_socket_matches_full_oracle_replies_and_files() {
    let corpus: Value =
        serde_json::from_str(include_str!("fixtures/machine-save-python.json")).unwrap();
    let mut calls = 0;
    for case in corpus["cases"].as_array().unwrap() {
        let mut f = Fixture::new();
        // Invalid saved registries exercise the save boundary, not startup admission.
        f.start(case["override"] == true).await;
        let folder = f.config().join("cortex");
        fs::create_dir_all(&folder).unwrap();
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o700)).unwrap();
        for (name, body) in case["initial"].as_object().unwrap() {
            let path = folder.join(name);
            fs::write(&path, body.as_str().unwrap()).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        for item in case["stateFiles"].as_array().into_iter().flatten() {
            let base = if workspace_protocol::channel::current().is_dev() {
                f.state().join("state")
            } else {
                f.root.join("state")
            };
            let path = base.join("cortex").join(item["path"].as_str().unwrap());
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            for parent in path
                .parent()
                .unwrap()
                .ancestors()
                .take_while(|p| p.starts_with(&f.root))
            {
                fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).unwrap();
            }
            fs::write(&path, item["raw"].as_str().unwrap()).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            fs::File::open(&path)
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(
                    std::time::UNIX_EPOCH + Duration::from_secs(item["mtime"].as_u64().unwrap()),
                ))
                .unwrap();
        }
        for observation in case["observations"].as_array().unwrap() {
            save_observation(&f, observation, &case["name"]).await;
            let mut files = json!({});
            for entry in fs::read_dir(&folder).unwrap() {
                let p = entry.unwrap().path();
                let name = p.file_name().unwrap().to_str().unwrap();
                if name == "connections.lock" {
                    continue;
                }
                files[name] = json!({"body":fs::read_to_string(&p).unwrap(),
                    "mode":fs::metadata(&p).unwrap().permissions().mode()&0o777});
            }
            assert_eq!(
                files,
                bind(&observation["files"], &f.config(), &f.state(), &f.root),
                "{} files",
                case["name"]
            );
            assert!(!f.state().join("defaults.json").exists());
            calls += 1;
        }
        assert!(!f.root.join("projects/.cortex").exists());
        assert!(!f.root.join("projects/.cortex-dev").exists());
        f.stop().await;
    }
    assert_eq!(calls, 168);
}
