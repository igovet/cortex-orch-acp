//! Real service binary/socket; disposable HOME/XDG, no engine or Python helper.
#[path = "support/hermetic.rs"]
mod hermetic;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::PermissionsExt,
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
        let root = std::env::temp_dir().join(format!(
            "cw-cs-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let f = Self { root, child: None };
        for name in [
            "home", "config", "state", "data", "bin", "runtime", "projects",
        ] {
            f.directory(&f.root.join(name));
        }
        f
    }
    fn workspace(&self) -> PathBuf {
        workspace_protocol::channel::current().state_dir(&self.root.join("state"))
    }
    fn config(&self) -> PathBuf {
        if workspace_protocol::channel::current().is_dev() {
            workspace_protocol::channel::current().config_dir(&self.root.join("config"))
        } else {
            self.root.join("config")
        }
    }
    fn mapped(&self, name: &str) -> PathBuf {
        for (prefix, root) in [
            ("config/", self.config()),
            ("workspace/", self.workspace()),
            (
                "state/",
                if workspace_protocol::channel::current().is_dev() {
                    self.workspace().join("state")
                } else {
                    self.root.join("state")
                },
            ),
            (
                "data/",
                if workspace_protocol::channel::current().is_dev() {
                    self.workspace().join("data")
                } else {
                    self.root.join("data")
                },
            ),
        ] {
            if let Some(rest) = name.strip_prefix(prefix) {
                return root.join(rest);
            }
        }
        self.root.join(name)
    }
    fn directory(&self, p: &Path) {
        for d in p
            .ancestors()
            .take_while(|p| p.starts_with(&self.root))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            fs::create_dir_all(d).unwrap();
            fs::set_permissions(d, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
    fn write(&self, p: &Path, bytes: &[u8], mode: u32) {
        self.directory(p.parent().unwrap());
        fs::write(p, bytes).unwrap();
        fs::set_permissions(p, fs::Permissions::from_mode(mode)).unwrap();
    }
    fn expand(&self, v: &Value) -> Value {
        serde_json::from_str(&v.to_string().replace("@ROOT@", self.root.to_str().unwrap())).unwrap()
    }
    fn prepare(&self, case: &Value) {
        for d in case["directories"].as_array().unwrap() {
            self.directory(&self.mapped(d.as_str().unwrap()));
        }
        for (name, raw) in case["files"].as_object().unwrap() {
            self.write(
                &self.mapped(name),
                self.expand(raw).as_str().unwrap().as_bytes(),
                if name.starts_with("bin/") {
                    0o700
                } else {
                    0o600
                },
            );
        }
    }
    fn token(&self) -> String {
        format!(
            "sha256:{:x}",
            Sha256::digest(fs::read(self.config().join("cortex/connections.json")).unwrap())
        )
    }
    async fn start(&mut self) {
        self.child = Some(
            Command::new(env!("CARGO_BIN_EXE_workspace-service"))
                .args([
                    self.root.join("projects"),
                    Path::new(env!("CARGO_MANIFEST_DIR"))
                        .parent()
                        .unwrap()
                        .parent()
                        .unwrap()
                        .to_owned(),
                ])
                .env_clear()
                .env("HOME", self.root.join("home"))
                .env("TMPDIR", std::env::temp_dir())
                .env("XDG_CONFIG_HOME", self.root.join("config"))
                .env("XDG_STATE_HOME", self.root.join("state"))
                .env("XDG_DATA_HOME", self.root.join("data"))
                .env("XDG_RUNTIME_DIR", self.root.join("runtime"))
                .env("PATH", self.root.join("bin"))
                .env("TZ", "UTC")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        tokio::time::timeout(Duration::from_secs(5), async {
            while !self.socket().exists() {
                assert!(
                    self.child.as_mut().unwrap().try_wait().unwrap().is_none(),
                    "service exited"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let status = self.rpc("service.status", json!({})).await;
        assert!(status.get("result").is_some(), "{status}");
    }
    fn socket(&self) -> PathBuf {
        workspace_protocol::channel::current()
            .runtime_dir(&self.root.join("runtime"))
            .join("service.sock")
    }
    async fn rpc(&self, method: &str, params: Value) -> Value {
        tokio::time::timeout(Duration::from_secs(5), async {
            let mut c = connect(&self.socket()).await.unwrap();
            write_frame(
                &mut c,
                &json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}),
            )
            .await
            .unwrap();
            let v = read_frame(&mut c).await.unwrap();
            assert_eq!(v["id"], 1);
            v
        })
        .await
        .unwrap()
    }
    async fn read(&self, params: Value) -> Value {
        let reply = self
            .rpc("machine.call", json!({"op":"config.read","params":params}))
            .await;
        assert!(reply.get("error").is_none(), "{reply}");
        reply["result"].clone()
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
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn snapshot(root: &Path) -> BTreeMap<PathBuf, (u32, Vec<u8>)> {
    let mut out = BTreeMap::new();
    if !root.exists() {
        return out;
    }
    for e in fs::read_dir(root).unwrap() {
        let p = e.unwrap().path();
        let m = fs::symlink_metadata(&p).unwrap();
        out.insert(
            p.clone(),
            (
                m.permissions().mode(),
                if m.is_file() {
                    fs::read(&p).unwrap()
                } else {
                    vec![]
                },
            ),
        );
        if m.is_dir() {
            out.extend(snapshot(&p));
        }
    }
    out
}
#[tokio::test]
async fn full_socket_reply_matches_the_frozen_oracle_and_delivers_diagnostics() {
    let corpus: Value =
        serde_json::from_str(include_str!("fixtures/config-read-dispatch-python.json")).unwrap();
    for case in corpus["cases"].as_array().unwrap() {
        let mut f = Fixture::new();
        f.prepare(case);
        f.start().await;
        let before = snapshot(&f.config());
        let home = snapshot(&f.root.join("home"));
        let raw = f
            .read(json!({"root":"/not-a-trusted-root","refresh":true,"action":"refresh"}))
            .await;
        assert_eq!(raw["ok"], true, "{}: {raw}", case["name"]);
        let diagnostics = raw["result"]["diagnostics"].as_array().unwrap();
        // Assert the complete additive diagnostic contract independently of the
        // Python settings capture, including unknown usage for non-native Codex.
        let registry: Value = serde_json::from_str(
            case["files"]["config/cortex/connections.json"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        let record = &registry["connections"][case["ids"][0].as_str().unwrap()];
        let mut expected_diagnostics = vec![];
        if record["engine"] == "codex-acp"
            && record["credential_ref"]["kind"] != "engine_native_dir"
        {
            expected_diagnostics.push(json!({"connection_id":case["ids"][0],
                "source":"codex_rollout","code":"native_directory_unavailable"}));
        }
        expected_diagnostics.push(json!({"engine":"opencode","source":"setup_discovery",
            "code":"version_probe_disabled"}));
        if !case["keyring"].is_null() {
            expected_diagnostics.push(json!({"connection_id":case["ids"][0],
                "source":"keyring","code":"keyring_not_observed"}));
        }
        assert_eq!(diagnostics, &expected_diagnostics, "{}", case["name"]);
        let mut want = f.expand(&case["before"]);
        let token = f.token();
        assert_eq!(want["tokens"]["connections"], "sha256:@CONNECTION_BYTES@");
        want = serde_json::from_str(
            &want
                .to_string()
                .replace("sha256:@CONNECTION_BYTES@", &token),
        )
        .unwrap();
        assert_eq!(
            want["config"]["setup"]["opencode"]["status"],
            "not_downloaded"
        );
        want["config"]["setup"]["opencode"]["status"] = json!("discovery_unavailable");
        want["diagnostics"] = json!(expected_diagnostics);
        // Nothing in these fixtures is launchable, so no background model fetch is started or reported.
        want["catalogueRefresh"] = json!({"running":false,"failed":[]});
        assert_eq!(raw, json!({"ok":true,"result":want}), "{}", case["name"]);
        assert_eq!(before, snapshot(&f.config()));
        assert_eq!(home, snapshot(&f.root.join("home")));
        f.stop().await;
    }
}
#[tokio::test]
async fn first_run_and_malformed_storage_never_create_settings_or_fallback() {
    let mut f = Fixture::new();
    f.start().await;
    assert!(!f.config().join("cortex").exists());
    let empty = f.read(json!({})).await;
    assert_eq!(empty["ok"], true, "{empty}");
    assert_eq!(empty["result"]["connections"], json!([]));
    assert_eq!(empty["result"]["tokens"]["connections"], "absent");
    assert!(!f.config().join("cortex").exists());
    let p = f.config().join("cortex/connections.json");
    f.write(&p, b"not-json fixture-private-data", 0o600);
    let bad = f.read(json!({})).await;
    assert_eq!(bad["ok"], false, "{bad}");
    assert!(bad.get("result").is_none());
    assert!(!bad.to_string().contains("fixture-private-data"));
    assert_eq!(fs::read(&p).unwrap(), b"not-json fixture-private-data");
    f.stop().await;
}
