//! Native login/status/registration and API-key setup share the real Python oracle.
//! Real service/helper/registry code; disposable roots and executable fakes only.
#[path = "support/hermetic.rs"]
mod hermetic;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
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
    bindings: Vec<(String, String)>,
}
impl Fixture {
    fn new(index: usize) -> Self {
        let root = PathBuf::from(format!("/tmp/cw-ec-{}-{index}", std::process::id()));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        for leaf in ["home", "bin", "config", "state", "runtime", "projects"] {
            fs::create_dir(root.join(leaf)).unwrap();
            fs::set_permissions(root.join(leaf), fs::Permissions::from_mode(0o700)).unwrap();
        }
        Self {
            root,
            child: None,
            bindings: vec![],
        }
    }
    fn config(&self) -> PathBuf {
        if workspace_protocol::channel::current().is_dev() {
            workspace_protocol::channel::current().config_dir(&self.root.join("config"))
        } else {
            self.root.join("config")
        }
    }
    fn data(&self) -> PathBuf {
        if workspace_protocol::channel::current().is_dev() {
            workspace_protocol::channel::current()
                .state_dir(&self.root.join("state"))
                .join("data")
        } else {
            // The retained release child environment does not forward XDG_DATA_HOME.
            self.root.join("home/.local/share")
        }
    }
    fn socket(&self) -> PathBuf {
        workspace_protocol::channel::current()
            .runtime_dir(&self.root.join("runtime"))
            .join("service.sock")
    }
    fn normalize(&self, text: &str) -> String {
        let mut text = text.to_owned();
        for (actual, captured) in &self.bindings {
            text = text.replace(actual, captured);
        }
        text.replace(self.config().to_str().unwrap(), "$ROOT/config")
            .replace(self.data().to_str().unwrap(), "$ROOT/home/.local/share")
            .replace(self.root.to_str().unwrap(), "$ROOT")
            .replace("codex-linux-x64", "codex-linux-$ARCH")
            .replace("codex-linux-arm64", "codex-linux-$ARCH")
            .replace("claude-agent-sdk-linux-x64", "claude-agent-sdk-linux-$ARCH")
            .replace(
                "claude-agent-sdk-linux-arm64",
                "claude-agent-sdk-linux-$ARCH",
            )
            .replace("x86_64-unknown-linux-musl", "$TRIPLE")
            .replace("aarch64-unknown-linux-musl", "$TRIPLE")
    }
    fn bind_registration(&mut self, reply: &Value, expected: &Value) {
        let id = reply["result"]["connectionId"].as_str().unwrap();
        assert!(regex::Regex::new(r"^cn_[a-f0-9]{12}$")
            .unwrap()
            .is_match(id));
        if self.bindings.iter().any(|(actual, _)| actual == id) {
            return;
        }
        let captured = expected["reply"]["result"]["connectionId"]
            .as_str()
            .unwrap();
        let raw = fs::read_to_string(self.config().join("cortex/connections.json")).unwrap();
        let registry: Value = serde_json::from_str(&raw).unwrap();
        let r = &registry["connections"][id];
        assert_eq!(r["id"], id);
        let updated = r["updated_at"].as_str().unwrap();
        let timestamp = r["created_at"].as_str().unwrap();
        assert!(
            regex::Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{6}\+00:00$")
                .unwrap()
                .is_match(timestamp)
        );
        let expected_registry: Value = serde_json::from_str(
            expected["files"]["config/cortex/connections.json"]["content"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        self.bindings.push((id.into(), captured.into()));
        assert!(
            regex::Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{6}\+00:00$")
                .unwrap()
                .is_match(updated)
        );
        self.bindings.push((
            updated.into(),
            expected_registry["connections"][captured]["updated_at"]
                .as_str()
                .unwrap()
                .into(),
        ));
        self.bindings.push((
            timestamp.into(),
            expected_registry["connections"][captured]["created_at"]
                .as_str()
                .unwrap()
                .into(),
        ));
    }
    fn initialize(&self, files: &Value) {
        let (arch, triple) = match std::env::consts::ARCH {
            "x86_64" => ("x64", "x86_64-unknown-linux-musl"),
            "aarch64" => ("arm64", "aarch64-unknown-linux-musl"),
            other => panic!("unsupported fixture architecture: {other}"),
        };
        for (name, value) in files.as_object().unwrap() {
            let name = name.replace("$ARCH", arch).replace("$TRIPLE", triple);
            let path = if let Some(rest) = name.strip_prefix("home/.local/share/") {
                self.data().join(rest)
            } else if let Some(rest) = name.strip_prefix("config/") {
                self.config().join(rest)
            } else {
                self.root.join(&name)
            };
            let parent = path.parent().unwrap();
            fs::create_dir_all(parent).unwrap();
            for p in parent.ancestors().take_while(|p| p.starts_with(&self.root)) {
                fs::set_permissions(p, fs::Permissions::from_mode(0o700)).unwrap();
            }
            if value["directory"] == true {
                fs::create_dir_all(&path).unwrap();
            } else {
                fs::write(
                    &path,
                    value["content"]
                        .as_str()
                        .unwrap()
                        .replace("$ROOT", self.root.to_str().unwrap()),
                )
                .unwrap();
            }
            fs::set_permissions(
                path,
                fs::Permissions::from_mode(value["mode"].as_u64().unwrap() as u32),
            )
            .unwrap();
        }
    }
    async fn start(&mut self) {
        let checkout = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_owned();
        self.spawn_at(&checkout).await;
    }
    async fn spawn_at(&mut self, checkout: &Path) {
        self.child = Some(
            Command::new(env!("CARGO_BIN_EXE_workspace-service"))
                .arg(self.root.join("projects"))
                .arg(checkout)
                .env_clear()
                .env("HOME", self.root.join("home"))
                .env("PATH", self.root.join("bin"))
                .env("XDG_CONFIG_HOME", self.root.join("config"))
                .env("XDG_STATE_HOME", self.root.join("state"))
                .env("XDG_DATA_HOME", self.root.join("unforwarded-data"))
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
    async fn wire(&self, method: &str, params: Value) -> Value {
        tokio::time::timeout(Duration::from_secs(10), async {
            let mut stream = connect(&self.socket()).await.unwrap();
            write_frame(
                &mut stream,
                &json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}),
            )
            .await
            .unwrap();
            let reply = read_frame(&mut stream).await.unwrap();
            assert_eq!(reply["id"], 1);
            reply
        })
        .await
        .unwrap()
    }
    fn snapshot(&self) -> Value {
        fn walk(f: &Fixture, path: &Path, logical: &str, result: &mut Map<String, Value>) {
            if !path.exists() {
                return;
            }
            let mut entries: Vec<_> = fs::read_dir(path)
                .unwrap()
                .map(|p| p.unwrap().path())
                .collect();
            entries.sort();
            for p in entries {
                let name = format!("{logical}/{}", p.file_name().unwrap().to_str().unwrap());
                let mode = fs::symlink_metadata(&p).unwrap().permissions().mode() & 0o777;
                assert!(!p.is_symlink());
                if p.is_dir() {
                    result.insert(name.clone(), json!({"directory":true,"mode":mode}));
                    walk(f, &p, &name, result);
                } else {
                    let mut content = fs::read_to_string(&p).unwrap();
                    if p.file_name().unwrap() == "connections.json" {
                        let registry: Value = serde_json::from_str(&content).unwrap();
                        for r in registry["connections"].as_object().unwrap().values() {
                            let canonical = serde_json::to_string(&json!({
                                "engine":r["engine"],"provider":r["provider"],"endpoint":r["endpoint"],
                                "launch_command":r["launch_command"]})).unwrap();
                            let digest = format!("{:x}", Sha256::digest(canonical.as_bytes()));
                            assert_eq!(r["approved_fingerprint"], digest);
                            let normalized = f.normalize(&canonical);
                            content = content.replace(
                                &digest,
                                &format!("{:x}", Sha256::digest(normalized.as_bytes())),
                            );
                        }
                    }
                    result.insert(
                        f.normalize(&name),
                        json!({"content":f.normalize(&content),"mode":mode}),
                    );
                }
            }
        }
        let mut result = Map::new();
        walk(self, &self.config(), "config", &mut result);
        walk(
            self,
            &self.root.join("home/.codex"),
            "home/.codex",
            &mut result,
        );
        walk(
            self,
            &self.root.join("home/.claude"),
            "home/.claude",
            &mut result,
        );
        Value::Object(result)
    }
    fn lines(&self, path: &str) -> Value {
        json!(fs::read_to_string(self.root.join(path))
            .unwrap_or_default()
            .lines()
            .collect::<Vec<_>>())
    }
    async fn stop(&mut self) {
        let reply = self.wire("service.stop", json!({})).await;
        assert!(reply.get("error").is_none());
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

/// A keyring backend is never observed: exercise production socket dispatch with the same
/// synthetic keyring inputs, with and without a complete bundled runtime.
#[tokio::test]
async fn keyring_absence_preserves_state_until_backend_is_observed() {
    let corpus: Value =
        serde_json::from_str(include_str!("fixtures/settings-secrets-python.json")).unwrap();
    let cases: Vec<_> = corpus["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["variant"].get("keyring").is_some())
        .collect();
    assert_eq!(cases.len(), 12);
    for (index, case) in cases.into_iter().enumerate() {
        let mut f = Fixture::new(1000 + index);
        f.initialize(&case["before"]);
        let initial = f.snapshot();
        // Native refusal is independent of the bundled runtime contents.
        let incomplete = f.root.join("incomplete-checkout");
        fs::create_dir_all(incomplete.join(".git")).unwrap();
        for infrastructure in ["complete-checkout", "incomplete-checkout"] {
            match infrastructure {
                "incomplete-checkout" => f.spawn_at(&incomplete).await,
                _ => f.start().await,
            }
            let wire = f
                .wire(
                    "machine.call",
                    json!({"op":case["op"],"params":case["params"]}),
                )
                .await;
            if case["variant"]["stale"] == true || case["variant"]["invalid_identifier"] == true {
                assert_eq!(wire["result"], case["reply"], "{}", case["name"]);
                assert!(wire.get("error").is_none());
            } else {
                // Explicit native refusal; never an observed or guessed credential state.
                assert!(wire.get("error").is_none(), "{}: {wire}", case["name"]);
                assert_eq!(
                    wire["result"]["code"], "config_action_unavailable",
                    "{}: {wire}",
                    case["name"]
                );
            }
            let after = f.snapshot();
            assert_eq!(
                after["config/cortex/connections.json"], initial["config/cortex/connections.json"],
                "unobserved credential state: {}",
                case["name"]
            );
            // Stale/invalid native refusals retain their established lock/ignore effects.
            let expected = if case["reply"]["ok"] == false {
                &case["after"]
            } else {
                &initial
            };
            assert_eq!(after, *expected, "unobserved effects: {}", case["name"]);
            f.stop().await;
        }
    }
}

#[tokio::test]
async fn engine_connection_preserves_complete_offline_replies_and_effects() {
    let corpus: Value =
        serde_json::from_str(include_str!("fixtures/engine-connection-python.json")).unwrap();
    for (index, case) in corpus["cases"].as_array().unwrap().iter().enumerate() {
        let mut f = Fixture::new(2000 + index);
        f.initialize(&case["files"]);
        f.start().await;
        for (params, expected) in case["requests"]
            .as_array()
            .unwrap()
            .iter()
            .zip(case["observations"].as_array().unwrap())
        {
            // The old corpus injected an exception inside the Python provider,
            // not inside the product. Native requests ignore that untrusted field.
            // Compare its native effect to the same real successful Python action;
            // native uncertain-write/no-cleanup behavior is fault-tested in units.
            let mut observation = expected.clone();
            if params["_fixture_unknown_after_effect"] == true {
                observation["reply"] = corpus["cases"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|c| c["name"] == format!("{}-add", params["engine"].as_str().unwrap()))
                    .unwrap()["observations"][0]["reply"]
                    .clone();
            }
            let expected = &observation;
            let wire = f
                .wire(
                    "machine.call",
                    json!({"op":"engine.connection","params":params}),
                )
                .await;
            let reply = if wire.get("error").is_some() {
                json!({"transportError":wire["error"]})
            } else {
                wire["result"].clone()
            };
            if matches!(params["action"].as_str(), Some("recheck" | "add_key"))
                && reply["ok"] == true
            {
                f.bind_registration(&reply, expected);
            }
            let reply: Value =
                serde_json::from_str(&f.normalize(&serde_json::to_string(&reply).unwrap()))
                    .unwrap();
            assert_eq!(
                serde_json::to_vec(&reply).unwrap(),
                serde_json::to_vec(&expected["reply"]).unwrap(),
                "reply bytes: {}",
                case["name"]
            );
            assert_eq!(f.snapshot(), expected["files"], "files: {}", case["name"]);
            assert_eq!(
                f.lines("status-called"),
                expected["statusCalls"],
                "status argv: {}",
                case["name"]
            );
        }
        if case["name"].as_str().unwrap().ends_with("-login-ready") {
            let wire = f.wire("machine.call", json!({"op":"engine.connection","params":{
                "action":"login_command", "engine":case["requests"][0]["engine"],
                "program":"/untrusted/client-program", "args":["execute"], "path":"/untrusted"
            }})).await;
            let reply: Value = serde_json::from_str(
                &f.normalize(&serde_json::to_string(&wire["result"]).unwrap()),
            )
            .unwrap();
            assert_eq!(reply, case["observations"][0]["reply"]);
            assert_eq!(f.snapshot(), case["observations"][0]["files"]);
            assert_eq!(f.lines("status-called"), json!([]));
        }
        if case["name"].as_str().unwrap().ends_with("-recheck-ready") {
            let bytes = fs::read(f.config().join("cortex/connections.json")).unwrap();
            let wire = f.wire("machine.call", json!({"op":"engine.connection","params":{
                "action":"recheck", "engine":case["requests"][0]["engine"],
                "program":"/untrusted/client-program", "args":["execute"], "path":"/untrusted",
                "credential_ref":{"path":"/untrusted"}, "revision":"stale"
            }})).await;
            let reply: Value = serde_json::from_str(
                &f.normalize(&serde_json::to_string(&wire["result"]).unwrap()),
            )
            .unwrap();
            assert_eq!(reply, case["observations"][0]["reply"]);
            assert_eq!(
                fs::read(f.config().join("cortex/connections.json")).unwrap(),
                bytes
            );
            let expected_args = if case["requests"][0]["engine"] == "codex-acp" {
                "login status"
            } else {
                "auth status"
            };
            assert_eq!(
                f.lines("status-called"),
                json!([expected_args, expected_args, expected_args])
            );
        }
        // No project was registered/activated: setup cannot acquire a scheduler owner.
        let status = f.wire("service.status", json!({})).await;
        assert!(status.get("error").is_none());
        assert!(!f.root.join("projects/.cortex").exists());
        assert!(!f.root.join("projects/.cortex-dev").exists());
        f.stop().await;
    }
}
