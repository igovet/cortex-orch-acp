//! The actual service binary starts an engine helper (its own model-catalogue
//! read at startup) in the connection's engine filesystem view: a synthetic
//! native Codex account, private sentinels beside it and a scripted no-model
//! peer that reports what it can see. No real engine, account or network.
//! Without user namespaces (inside a Cortex command view) the debug service
//! starts the peer without a view, so the view checks are not run unless
//! `CORTEX_REQUIRE_NAMESPACE_FIXTURES` demands them.
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
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}
impl Fixture {
    fn write(&self, path: &Path, content: &[u8], mode: u32) {
        for p in path
            .parent()
            .unwrap()
            .ancestors()
            .take_while(|p| p.starts_with(&self.root))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            fs::create_dir_all(p).unwrap();
            fs::set_permissions(p, fs::Permissions::from_mode(0o700)).unwrap();
        }
        fs::write(path, content).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }
    fn state(&self) -> PathBuf {
        let state = workspace_protocol::channel::current().state_dir(&self.root.join("state"));
        if workspace_protocol::channel::current().is_dev() {
            state.join("state")
        } else {
            self.root.join("state")
        }
    }
    fn config(&self) -> PathBuf {
        if workspace_protocol::channel::current().is_dev() {
            workspace_protocol::channel::current().config_dir(&self.root.join("config"))
        } else {
            self.root.join("config")
        }
    }
    fn socket(&self) -> PathBuf {
        workspace_protocol::channel::current()
            .runtime_dir(&self.root.join("runtime"))
            .join("service.sock")
    }
    async fn read(&self) -> Value {
        tokio::time::timeout(Duration::from_secs(30), async {
            let mut socket = connect(&self.socket()).await.unwrap();
            write_frame(
                &mut socket,
                &json!({"jsonrpc":"2.0","id":1,"method":"machine.call","params":{"op":"config.read"}}),
            )
            .await
            .unwrap();
            let reply = read_frame(&mut socket).await.unwrap();
            assert_eq!(reply["result"]["ok"], true, "{reply}");
            reply["result"]["result"].clone()
        })
        .await
        .unwrap()
    }
}

fn namespaces() -> bool {
    let available = Command::new("/usr/bin/bwrap")
        .args(["--unshare-pid", "--ro-bind", "/usr", "/usr", "--symlink", "usr/lib", "/lib"])
        .args(["--symlink", "usr/lib64", "/lib64", "--symlink", "usr/bin", "/bin"])
        .args(["--proc", "/proc", "--dev", "/dev", "--", "/usr/bin/true"])
        .env_clear()
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if std::env::var_os("CORTEX_REQUIRE_NAMESPACE_FIXTURES").is_some() {
        assert!(available, "engine view service fixtures require user namespaces");
    }
    available
}

#[tokio::test]
async fn the_service_starts_its_catalogue_helper_in_the_connection_view() {
    if !namespaces() {
        println!("NOT_RUN engine view service check: user namespaces unavailable");
        return;
    }
    let root = PathBuf::from(format!("/tmp/cw-ev-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let mut f = Fixture { root, child: None };
    for name in ["config", "state", "runtime", "bin", "assets/.git", "projects"] {
        f.write(&f.root.join(name).join(".keep"), b"", 0o600);
    }
    std::os::unix::fs::symlink("/usr/bin/git", f.root.join("bin/git")).unwrap();
    let root = f.root.to_str().unwrap().to_owned();
    // The synthetic native account and the owner's other files around it.
    f.write(&f.root.join("home/.codex/auth.json"), b"synthetic-auth", 0o600);
    f.write(&f.root.join("home/.codex/sessions/owner.jsonl"), b"owner", 0o600);
    f.write(&f.root.join("home/owner-sentinel"), b"owner", 0o600);
    f.write(&f.root.join("outside-sentinel"), b"host", 0o600);
    let producer: Value =
        serde_json::from_str(include_str!("fixtures/config-catalog-producer-python.json")).unwrap();
    let case = &producer["cases"][0];
    let registry = case["initial"]["config/cortex/connections.json"]["content"]
        .as_str()
        .unwrap()
        .replace("@ROOT@", &root);
    let folder = f.config().join("cortex");
    f.write(&folder.join("connections.json"), registry.as_bytes(), 0o600);
    f.write(&folder.join("connections.lock"), b"", 0o600);
    // The fixture peer answers the model listing; it reports what it sees into
    // its own `~/.cache`, which on the host is the connection's private cache.
    let peer = case["peer"]
        .as_str()
        .unwrap()
        .replace("@ROOT@/bin/seen", "$HOME/.cache/seen")
        .replacen(
            "#!/bin/sh\n",
            &format!(
                "#!/bin/sh\nr() {{ printf '%s\\n' \"$1\" >> \"$HOME/.cache/probe\"; }}\n\
                 if [ -e '{root}/home/owner-sentinel' ]; then r leak-owner-home; else r owner-home-hidden; fi\n\
                 if [ -e '{root}/outside-sentinel' ]; then r leak-host; else r host-hidden; fi\n\
                 if [ -e '{root}/config/cortex' ] || [ -e '{root}/state' ]; then r leak-cortex-state; else r cortex-state-hidden; fi\n\
                 if [ \"$(/usr/bin/cat \"$CODEX_HOME/auth.json\" 2>/dev/null)\" = synthetic-auth ]; then r native-auth-visible; else r native-auth-missing; fi\n\
                 if [ -e \"$CODEX_HOME/sessions\" ]; then r leak-owner-sessions; else r owner-sessions-hidden; fi\n\
                 if [ \"$(/usr/bin/ls /proc | /usr/bin/grep -c '^[0-9]')\" -lt 10 ]; then r pid-private; else r pid-shared; fi\n\
                 printf ' refreshed' >> \"$CODEX_HOME/auth.json\"\n\
                 echo private > \"$CODEX_HOME/cortex-own-file\"\n\
                 echo private > '{root}/outside-new' 2>/dev/null\n"
            ),
            1,
        );
    f.write(&f.root.join("bin/peer"), peer.as_bytes(), 0o700);
    f.child = Some(
        Command::new(env!("CARGO_BIN_EXE_workspace-service"))
            .arg(f.root.join("projects"))
            .arg(f.root.join("assets"))
            .env_clear()
            .env("HOME", f.root.join("home"))
            .env("XDG_CONFIG_HOME", f.root.join("config"))
            .env("XDG_STATE_HOME", f.root.join("state"))
            .env("XDG_RUNTIME_DIR", f.root.join("runtime"))
            .env("PATH", f.root.join("bin"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        while connect(&f.socket()).await.is_err() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let mut done = Value::Null;
    for _ in 0..600 {
        done = f.read().await;
        if done["catalogueRefresh"]["running"] == false {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(done["catalogueRefresh"]["failed"], json!([]), "{done}");
    assert_eq!(
        done["config"]["catalogue"]["codex-acp"]["models"]
            .as_array()
            .unwrap()
            .len(),
        2,
        "{done}"
    );
    let private = f.state().join("cortex/engine-private/cn_000000000001/cache");
    let probe = fs::read_to_string(private.join("probe")).expect("the peer ran in its view");
    let seen: Vec<&str> = probe.lines().collect();
    assert_eq!(
        seen,
        [
            "owner-home-hidden",
            "host-hidden",
            "cortex-state-hidden",
            "native-auth-visible",
            "owner-sessions-hidden",
            "pid-private"
        ],
        "{probe}"
    );
    assert!(private.join("seen").is_file());
    // The native sign-in file is the owner's own, rewritten in place; everything
    // else the peer wrote stayed private to its view.
    assert_eq!(
        fs::read_to_string(f.root.join("home/.codex/auth.json")).unwrap(),
        "synthetic-auth refreshed"
    );
    assert!(!f.root.join("home/.codex/cortex-own-file").exists());
    assert!(f
        .state()
        .join("cortex/engine-private/cn_000000000001/codex-home/cortex-own-file")
        .is_file());
    assert!(!f.root.join("outside-new").exists());
    assert!(!f.root.join("home/.cache/probe").exists());
    println!("ENGINE_VIEW_SERVICE {}", seen.join(" "));
}
