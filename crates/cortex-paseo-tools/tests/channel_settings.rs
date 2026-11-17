//! Real no-account entrypoints; build cortex-acp beside the tools before running.
use cortex_core::store::{OpenMode, ProjectKey, Store, StoreChannel};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};
const TOOL: &str = env!("CARGO_BIN_EXE_cortex-paseo-tools");

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("cortex-settings-channel-{}", std::process::id()));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        Self(root)
    }
    fn write(&self, relative: &str, body: &[u8]) {
        let path = self.0.join(relative);
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path.parent().unwrap())
            .unwrap();
        fs::write(&path, body).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    fn run(&self, binary: &Path, args: &[&str], input: &str, xdg: bool, channel: &str) -> Output {
        let mut cmd = Command::new(binary);
        cmd.args(args)
            .env_clear()
            .env("HOME", self.0.join("home"))
            .env("PATH", "")
            .env("CORTEX_ENGINE_UPDATE", "off")
            .env("CORTEX_ENGINE_FOLLOW", "off")
            // A runtime override must not change either binary's compiled identity.
            .env("CORTEX_WORKSPACE_CHANNEL", channel)
            .env(
                "CORTEX_ACP_RUNTIME_ROOT",
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."),
            )
            .current_dir("/")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if xdg {
            cmd.env("XDG_CONFIG_HOME", self.0.join("home/.config"))
                .env("XDG_STATE_HOME", self.0.join("home/.local/state"));
        }
        let mut child = cmd.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "args {args:?} input {input}: status {:?}: {} {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        output
    }
    fn config(&self, request: Value, xdg: bool, channel: &str) -> Value {
        let output = self.run(
            Path::new(TOOL),
            &["config"],
            &request.to_string(),
            xdg,
            channel,
        );
        let answer: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(answer["ok"], true, "{answer}");
        answer["result"].clone()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn setup_settings_and_agent_share_compiled_channel_without_touching_other_channel() {
    let f = Fixture::new();
    let dev = workspace_protocol::channel::current().is_dev();
    let own = if dev {
        "home/.config/cortex-workspace-dev/cortex"
    } else {
        "home/.config/cortex"
    };
    let other = if dev {
        "home/.config/cortex"
    } else {
        "home/.config/cortex-workspace-dev/cortex"
    };
    let own_store = if dev { ".cortex-dev" } else { ".cortex" };
    let other_store = if dev { ".cortex" } else { ".cortex-dev" };
    let other_body = b"other-channel-must-not-be-read-or-written\n";
    f.write(&format!("{other}/routing.json"), other_body);
    f.write(&format!("{other}/defaults.json"), other_body);
    f.write(&format!("{other}/connections.json"), other_body);
    f.write(&format!("project/{other_store}/routing.json"), other_body);
    drop(
        Store::open(
            ProjectKey::new(
                f.0.join("project"),
                if dev {
                    StoreChannel::Dev
                } else {
                    StoreChannel::Release
                },
            )
            .unwrap(),
            OpenMode::Initialize,
        )
        .unwrap(),
    );
    let agent = Path::new(TOOL).parent().unwrap().join("cortex-acp");
    assert!(
        agent.is_file(),
        "build cortex-acp beside cortex-paseo-tools before this test"
    );

    // Both unset XDG and explicit XDG use the same roots. Opposite/invalid
    // runtime channel values cannot redirect either entrypoint.
    for (xdg, runtime_channel) in [(true, "release"), (false, "dev"), (true, "invalid")] {
        let output = f.run(
            Path::new(TOOL),
            &["config", "--host"],
            "",
            xdg,
            runtime_channel,
        );
        let host: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(host["config_home"], f.0.join(own).display().to_string());
        assert_eq!(host["store_name"], own_store);
        let state = if dev {
            "home/.local/state/cortex-workspace-dev/state/cortex"
        } else {
            "home/.local/state/cortex"
        };
        assert_eq!(host["state_home"], f.0.join(state).display().to_string());
        assert!(!f.0.join(own).exists() || f.0.join(own).join("routing.json").is_file());

        f.run(Path::new(TOOL), &["setup"], "", xdg, runtime_channel);
        let view = f.config(
            json!({"action":"read","surface":"routing"}),
            xdg,
            runtime_channel,
        );
        assert!(view
            .to_string()
            .contains(&f.0.join(own).join("routing.json").display().to_string()));
        let defaults = f.config(
            json!({"action":"read","surface":"user_defaults"}),
            xdg,
            runtime_channel,
        );
        let updated = f.config(json!({
            "action":"update","surface":"user_defaults","expected_revision":defaults["revision"],
            "defaults":{"language":"ru"}
        }), xdg, runtime_channel);
        assert!(updated.to_string().contains("ru"));
        let saved = fs::read(f.0.join(own).join("defaults.json")).unwrap();

        // Real ACP launch (no session/new, no accounts or paid model call).
        // Seed is absent-only: Settings' save must remain byte-for-byte intact.
        let output = f.run(&agent, &[], &format!("{}\n", json!({
            "jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{}}
        })), xdg, runtime_channel);
        let answer: Value = serde_json::from_slice(
            output
                .stdout
                .split(|b| *b == b'\n')
                .find(|line| !line.is_empty())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(answer["result"]["_meta"]["cortexChannel"], host["channel"]);
        assert_eq!(
            fs::read(f.0.join(own).join("defaults.json")).unwrap(),
            saved
        );

        // The agent's read path must also ignore the malformed other-channel
        // registry (initialize alone could hide a deferred first-run failure).
        let efforts = Command::new(&agent)
            .env_clear()
            .env("HOME", f.0.join("home"))
            .env("PATH", "")
            .env("CORTEX_MODEL_EFFORTS", "1")
            .env("CORTEX_WORKSPACE_CHANNEL", runtime_channel)
            .current_dir("/")
            .output()
            .unwrap();
        assert!(
            efforts.status.success(),
            "{}",
            String::from_utf8_lossy(&efforts.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&efforts.stdout).unwrap(),
            json!({})
        );

        let project = f.0.join("project");
        f.config(json!({
            "action":"update","surface":"routing","project":project,"target":"project",
            "expected_revision": if project.join(own_store).join("routing.json").exists() {
                json!(format!("sha256:{:x}", Sha256::digest(fs::read(project.join(own_store).join("routing.json")).unwrap())))
            } else { json!("absent") },
            "rules":{"model_policy_version":2}
        }), xdg, runtime_channel);
        assert!(project.join(own_store).join("routing.json").is_file());
        assert_eq!(
            fs::read(project.join(other_store).join("routing.json")).unwrap(),
            other_body
        );
        for name in ["routing.json", "defaults.json", "connections.json"] {
            assert_eq!(fs::read(f.0.join(other).join(name)).unwrap(), other_body);
        }
    }
}
