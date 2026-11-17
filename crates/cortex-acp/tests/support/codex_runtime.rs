//! Offline codex-acp fixture shared by every Rust entry binary's tests.
//!
//! Each binary that can write the native Codex launcher names itself as the
//! launcher's shebang interpreter, so each one must run that launcher. The
//! fake `node` behaves like codex-acp: it runs `$CODEX_PATH app-server` and
//! answers `session/new` with the models that App Server listed, or with an
//! error when App Server did not start. No network, account or engine is used.
#![allow(dead_code)]
use serde_json::Value;
use std::{
    fs,
    io::{Read, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

pub const MODEL: &str = "gpt-fixture-1";
const VERSION: &str = "1.12.0";
const CODEX: &str = r#"#!/bin/sh
case "$*" in
*"features list")
 for name in multi_agent multi_agent_v2 plugins shell_tool view_image; do
  printf '%s stable false\n' "$name"
 done ;;
*"mcp list --json")
 # Like codex-cli 0.166.0 (observed offline): an inline `-c mcp_servers.<name>={...}`
 # layer defines an enabled server; this fixture's home configures none.
 listed= sep=
 for arg; do
  case "$arg" in
  mcp_servers.*=\{*) name=${arg#mcp_servers.}; listed="$listed$sep{\"name\":\"${name%%=*}\",\"enabled\":true}"; sep=, ;;
  esac
 done
 printf '[%s]\n' "$listed" ;;
"debug models") printf '%s\n' '{"models": [{"slug": "gpt-fixture-1", "visibility": "list", "supported_reasoning_levels": [{"effort": "medium"}]}]}' ;;
*" app-server"|app-server)
 IFS= read -r line
 case "$line" in
 *model/list*) printf '%s\n' '{"id":1,"result":{"data":[{"id":"gpt-fixture-1"}]}}' ;;
 *) exit 92 ;;
 esac
 # Like codex-cli 0.164.0 (observed offline): a thread started without a
 # trust layer for App Server's cwd persists that project's trust into the
 # Codex home's config.toml.
 while IFS= read -r line; do
  case "$line" in
  *thread/start*)
   cwd=$(pwd -P)
   home=${CODEX_HOME:-$HOME/.codex}
   trusted=
   for arg; do
    [ "$arg" = "projects={\"$cwd\"={trust_level=\"trusted\"}}" ] && trusted=1
   done
   [ -n "$trusted" ] || printf '\n[projects."%s"]\ntrust_level = "trusted"\n' "$cwd" >> "$home/config.toml"
   printf '%s\n' "$home/config.toml" > "$home/thread-start-observed"
   state=untrusted
   [ -z "$trusted" ] || state=trusted
   printf '%s %s\n' "$state" "$cwd" >> "$home/thread-starts.log"
   printf '%s\n' '{"id":2,"result":{"thread":{"id":"fixture-thread"}}}' ;;
  esac
 done ;;
*) exit 91 ;;
esac
"#;
const OPTIONS: &str = r#"[{"id":"model","name":"Model","type":"select","category":"model","currentValue":"gpt-fixture-1","options":[{"value":"gpt-fixture-1","name":"GPT Fixture 1"}]},{"id":"thought_level","name":"Thought_Level","type":"select","category":"thought_level","currentValue":"medium","options":[{"value":"medium","name":"Medium"}]}]"#;
const NODE: &str = r#"#!/bin/sh
[ "$NO_BROWSER" = 1 ] && [ -n "$CODEX_PATH" ] || exit 98
while IFS= read -r line; do
 id=${line#*\"id\"}
 id=${id#*:}
 id=${id%%,*}
 id=${id%%\}*}
 id=${id# }
 case "$line" in
 *session/prompt*) exit 95 ;;
 *initialize*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{}}' ;;
 *session/new*)
  models=$(printf '%s\n' '{"id":1,"method":"model/list"}' '{"id":2,"method":"thread/start","params":{"ephemeral":true}}' | "$CODEX_PATH" app-server 2>/dev/null)
  case "$models" in
  *gpt-fixture-1*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"sessionId":"fixture-session","configOptions":@OPTIONS@}}' ;;
  *) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"error":{"code":-32603,"message":"Codex App Server exited"}}' ;;
  esac ;;
 *session/set_config_option*) printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"configOptions":@OPTIONS@}}' ;;
 esac
done
"#;

fn private(path: &Path, directory: bool) {
    if directory {
        fs::create_dir_all(path).unwrap();
    }
    let mode = if directory { 0o700 } else { 0o600 };
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}
fn write(path: &Path, bytes: &[u8], mode: u32) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}
fn files(root: &Path, name: &str) -> Vec<PathBuf> {
    let mut found = vec![];
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).into_iter().flatten().flatten() {
            let path = entry.path();
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                pending.push(path);
            } else if kind.is_file()
                && path.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                    n == name
                        || (name == "cs_*.json" && n.starts_with("cs_") && n.ends_with(".json"))
                })
            {
                found.push(path);
            }
        }
    }
    found
}

/// A disposable home with a native Codex login marker and a pinned, owned
/// synthetic codex-acp install whose vendored Codex is the fake App Server.
pub struct Fixture(pub PathBuf);
impl Fixture {
    pub fn new(tag: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = PathBuf::from(format!(
            "/var/tmp/cortex-codex-runtime-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        private(&root, true);
        let f = Self(root);
        for directory in ["home", "home/.codex", "payload", "bin"] {
            private(&f.0.join(directory), true);
        }
        write(&f.0.join("home/.codex/auth.json"), b"{}\n", 0o600);
        write(
            &f.0.join("payload/pinned-dependencies.json"),
            format!(
                r#"{{"schema_version": 1, "portable_contract_version": 1, "engines": {{"codex-acp": {{"pinned_package_version": "{VERSION}"}}}}}}"#
            )
            .as_bytes(),
            0o600,
        );
        // The synthetic 1.12.0 bundle whose patch rules the native producer
        // already qualifies against Python (config-catalog-producer corpus).
        let corpus: Value = serde_json::from_str(include_str!(
            "../../../workspace-service/tests/fixtures/config-catalog-producer-python.json"
        ))
        .unwrap();
        let case = corpus["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "codex-pinned-hoisted")
            .unwrap();
        let modules = f.modules();
        let source = "home/engines/node_modules/";
        for name in [
            "@agentclientprotocol/codex-acp/package.json",
            "@agentclientprotocol/codex-acp/dist/index.js",
            "@agentclientprotocol/codex-acp/dist/xdg-open",
            "@openai/codex/package.json",
        ] {
            let item = &case["initial"][format!("{source}{name}")];
            write(
                &modules.join(name),
                item["content"].as_str().unwrap().as_bytes(),
                item["mode"].as_u64().unwrap() as u32,
            );
        }
        write(
            &modules.join("@openai/codex-linux-x64/vendor/x86_64-unknown-linux-musl/bin/codex"),
            CODEX.as_bytes(),
            0o700,
        );
        write(
            &f.node(),
            NODE.replace("@OPTIONS@", OPTIONS).as_bytes(),
            0o700,
        );
        f
    }
    fn modules(&self) -> PathBuf {
        self.0
            .join("data/cortex/engines/codex-acp")
            .join(VERSION)
            .join("node_modules")
    }
    fn node(&self) -> PathBuf {
        self.0.join("bin/node")
    }
    /// Make the fake codex-acp also serve task sessions. A launch whose cwd
    /// is a task project (it holds `scenario.txt`) first starts App Server
    /// once, as the real adapter's ephemeral title thread does: through
    /// `$CODEX_PATH` when Cortex pins it, else through the bundled Codex
    /// (the adapter's own fallback). The session itself is the no-paid fake
    /// engine `peer` in `desktop-auto` mode. Catalogue launches list
    /// `model`, which that fake engine also offers.
    pub fn serve_task_sessions(&self, peer: &Path, model: &str) {
        let task = format!(
            "if [ -f scenario.txt ]; then\n printf '%s\\n' '{{\"id\":1,\"method\":\"model/list\"}}' '{{\"id\":2,\"method\":\"thread/start\",\"params\":{{\"ephemeral\":true}}}}' | \"${{CODEX_PATH:-{}}}\" app-server >/dev/null 2>&1\n exec '{}' desktop-auto\nfi\n",
            self.codex().display(),
            peer.display()
        );
        let options = OPTIONS.replace(&format!("\"{MODEL}\""), &format!("\"{model}\""));
        let node = NODE
            .replacen("#!/bin/sh\n", &format!("#!/bin/sh\n{task}"), 1)
            .replace("@OPTIONS@", &options);
        write(&self.node(), node.as_bytes(), 0o700);
    }
    /// Every regular file named `name` below the fixture root.
    pub fn find(&self, name: &str) -> Vec<PathBuf> {
        files(&self.0, name)
    }
    pub fn codex(&self) -> PathBuf {
        self.modules()
            .join("@openai/codex-linux-x64/vendor/x86_64-unknown-linux-musl/bin/codex")
    }
    /// The entry binary's environment: only fixture roots, no engine update
    /// download (the pinned version is already owned) and no adapter follow.
    pub fn command(&self, exe: &str) -> Command {
        let mut cmd = Command::new(exe);
        cmd.env_clear()
            .env("HOME", self.0.join("home"))
            .env("XDG_CONFIG_HOME", self.0.join("home/config"))
            .env("XDG_STATE_HOME", self.0.join("home/state"))
            .env("XDG_DATA_HOME", self.0.join("data"))
            .env("PATH", "/usr/bin:/bin")
            .env("CORTEX_NODE_BINARY", self.node())
            .env("CORTEX_ENGINE_FOLLOW", "off")
            .env("CORTEX_ACP_RUNTIME_ROOT", self.0.join("payload"))
            .current_dir(&self.0);
        cmd
    }
    /// `first-run-codex-acp.json`, wherever the compiled channel placed it.
    pub fn status(&self) -> Option<Value> {
        let path = files(&self.0, "first-run-codex-acp.json").pop()?;
        serde_json::from_slice(&fs::read(path).ok()?).ok()
    }
    pub fn finished(&self) -> bool {
        self.status().is_some_and(|s| {
            !["downloading", "connecting", "refreshing_models"]
                .contains(&s["phase"].as_str().unwrap_or(""))
        })
    }
    /// The launcher's shebang line, e.g. `#!/abs/cortex-acp --native-codex-runtime`.
    pub fn launcher_header(&self) -> String {
        let launchers = files(&self.0, "codex-acp-runtime");
        assert_eq!(launchers.len(), 1, "exactly one Codex launcher");
        let text = fs::read_to_string(&launchers[0]).unwrap();
        text.lines().next().unwrap().to_owned()
    }
    /// The config.toml path the fake App Server targeted for a thread start.
    pub fn thread_start_config(&self) -> Option<PathBuf> {
        let marker = files(&self.0, "thread-start-observed").pop()?;
        Some(PathBuf::from(fs::read_to_string(marker).ok()?.trim_end()))
    }
    /// Models in every retained catalogue snapshot.
    pub fn models(&self) -> Vec<String> {
        let mut models = vec![];
        for path in files(&self.0, "cs_*.json") {
            let doc: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
            for entry in doc["entries"].as_array().into_iter().flatten() {
                if let Some(model) = entry["route"]["model"].as_str().or(entry["model"].as_str()) {
                    models.push(model.to_owned());
                }
            }
        }
        models
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Assert a completed first-run pass observed the App Server's models through
/// a launcher whose interpreter is exactly `exe`.
pub fn assert_first_run_listed_models(f: &Fixture, exe: &str) {
    let status = f.status().expect("first-run status");
    assert_eq!(status["phase"], "ready", "first-run status: {status}");
    assert_eq!(
        f.launcher_header(),
        format!("#!{exe} --native-codex-runtime")
    );
    assert!(f.models().iter().any(|m| m == MODEL), "{:?}", f.models());
}

/// Run `exe` as an ACP agent (stdin held open) until its detached first-run
/// pass finishes, then close stdin so the agent exits.
pub fn run_acp_first_run(f: &Fixture, exe: &str) {
    let mut child = f
        .command(exe)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(90);
    while !f.finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    drop(child.stdin.take());
    let status = child.wait().unwrap();
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(status.success(), "{status}: {stderr}");
}

/// The launcher artifact naming `exe` runs the native runtime mode: feature
/// and MCP checks pass, then App Server answers `model/list` on stdio.
pub fn assert_runtime_mode_lists_models(f: &Fixture, exe: &str) {
    let launcher = f.0.join("bin/codex-acp-runtime");
    let body =
        workspace_service::native_codex_runtime::artifact(Path::new(exe), &f.codex()).unwrap();
    write(&launcher, body.as_bytes(), 0o700);
    let mut child = Command::new(&launcher)
        .arg("app-server")
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"{\"id\":1,\"method\":\"model/list\"}\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(reply["result"]["data"][0]["id"], MODEL);
}
