//! No-account startup boundary checks against the actual binary.
use serde_json::{json, Value};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = PathBuf::from(format!(
            "/var/tmp/cortex-acp-startup-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        for child in ["home", "payload"] {
            fs::create_dir(root.join(child)).unwrap();
            fs::set_permissions(root.join(child), fs::Permissions::from_mode(0o700)).unwrap();
        }
        Self(root)
    }
    fn run(&self, efforts: bool) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_cortex-acp"));
        cmd.env_clear()
            .env("HOME", self.0.join("home"))
            .env("XDG_CONFIG_HOME", self.0.join("home/config"))
            .env("XDG_STATE_HOME", self.0.join("home/state"))
            .env("PATH", "/usr/bin:/bin")
            .env("CORTEX_ENGINE_UPDATE", "off")
            .env("CORTEX_ENGINE_FOLLOW", "off")
            .env("CORTEX_ACP_RUNTIME_ROOT", self.0.join("payload"));
        if efforts {
            cmd.env("CORTEX_MODEL_EFFORTS", "1");
        }
        cmd.current_dir(&self.0).output().unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
#[test]
fn pinned_refusal_is_stderr_only_and_efforts_bypasses_startup_like_python() {
    let f = Fixture::new();
    fs::write(
        f.0.join("payload/pinned-dependencies.json"),
        json!({
            "schema_version":1,"portable_contract_version":99
        })
        .to_string(),
    )
    .unwrap();
    let output = f.run(false);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.starts_with(
        "cortex_acp: pinned dependency portable_contract_version mismatch: pinned 99 "
    ));
    assert!(!f.0.join("home/config").exists());
    let output = f.run(true);
    assert!(output.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        json!({})
    );
    assert!(!f.0.join("home/config").exists());
}
#[test]
fn defaults_are_private_absent_only_and_stable_on_restart() {
    let f = Fixture::new();
    fs::copy(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../pinned-dependencies.json"),
        f.0.join("payload/pinned-dependencies.json"),
    )
    .unwrap();
    let output = f.run(false);
    assert!(output.status.success(), "{:?}", output.stderr);
    assert!(output.stdout.is_empty());
    let base = f.0.join("home/config");
    let base = if workspace_protocol::channel::current().is_dev() {
        workspace_protocol::channel::current().config_dir(&base)
    } else {
        base
    };
    let path = base.join("cortex/defaults.json");
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    fs::write(&path, b"owner-invalid-json\n").unwrap();
    assert!(f.run(false).status.success());
    assert_eq!(fs::read(path).unwrap(), b"owner-invalid-json\n");
}
