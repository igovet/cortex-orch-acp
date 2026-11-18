//! The Rust Paseo setup commands against a disposable HOME: the Git-install
//! bootstrap stages a non-Python runtime and writes the Rust launch record,
//! upgrades a managed Python-argv record and refuses an unrelated one.
use serde_json::{json, Value};
use std::{
    fs,
    os::unix::fs::{symlink, DirBuilderExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Output},
};

const TOOL: &str = env!("CARGO_BIN_EXE_cortex-paseo-tools");
const SHIM: &str = "integrations/paseo/server/acp-load-shim.mjs";

struct Fixture {
    root: PathBuf,
    bin: PathBuf,
}
impl Fixture {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("cortex-paseo-setup-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        fs::DirBuilder::new()
            .mode(0o700)
            .create(root.join("home"))
            .unwrap();
        // Both binaries beside one another, as a release build leaves them.
        let bin = root.join("bin");
        fs::DirBuilder::new().mode(0o700).create(&bin).unwrap();
        for name in ["cortex-acp", "cortex-paseo-tools"] {
            fs::copy(TOOL, bin.join(name)).unwrap();
        }
        Self { root, bin }
    }
    fn home(&self) -> PathBuf {
        self.root.join("home")
    }
    fn source() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap()
    }
    fn run(&self, args: &[&str]) -> Output {
        output(
            Command::new(self.bin.join("cortex-paseo-tools"))
                .args(args)
                .env_clear()
                .env("HOME", self.home())
                .env("PATH", "")
                .env("CORTEX_ENGINE_UPDATE", "off")
                .env("CORTEX_ENGINE_FOLLOW", "off")
                .current_dir("/"),
        )
    }
    fn bootstrap(&self) -> Output {
        self.run(&[
            "bootstrap-paseo-git-install",
            "--source-root",
            Self::source().to_str().unwrap(),
        ])
    }
    fn record_path(&self) -> PathBuf {
        self.home().join(".config/cortex/paseo-plugin-launch.json")
    }
    fn version() -> String {
        let identity: Value = serde_json::from_slice(
            &fs::read(Self::source().join("package-identity.json")).unwrap(),
        )
        .unwrap();
        identity["version"].as_str().unwrap().to_owned()
    }
    fn state_root(&self) -> PathBuf {
        let host = self.run(&["config", "--host"]);
        assert!(host.status.success());
        let host: Value = serde_json::from_slice(&host.stdout).unwrap();
        PathBuf::from(host["state_home"].as_str().unwrap())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
/// Run a command, retrying while a sibling test thread still holds a freshly
/// written executable open (ETXTBSY on fork).
fn output(command: &mut Command) -> Output {
    for _ in 0..100 {
        match command.output() {
            Err(e) if e.raw_os_error() == Some(26) => {
                std::thread::sleep(std::time::Duration::from_millis(50))
            }
            other => return other.unwrap(),
        }
    }
    command.output().unwrap()
}
fn ok(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "status {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn bootstrap_stages_a_python_free_runtime_and_records_the_rust_entry() {
    let f = Fixture::new("stage");
    let out = ok(&f.bootstrap());
    let runtime = f
        .state_root()
        .join("paseo-runtime")
        .join(Fixture::version());
    assert_eq!(out["runtime_root"], runtime.display().to_string());
    assert_eq!(out["launch_config"]["created"], true);

    // Payload: only what the Rust entries and the shim read; no src/.
    for relative in [
        "package-identity.json",
        "pinned-dependencies.json",
        SHIM,
        "integrations/paseo/server/replay-spool.mjs",
        "bin/cortex-acp",
        "bin/cortex-paseo-tools",
    ] {
        assert!(runtime.join(relative).is_file(), "{relative}");
    }
    assert!(!runtime.join("src").exists());
    assert_eq!(fs::metadata(&runtime).unwrap().mode() & 0o7777, 0o700);
    assert_eq!(
        fs::metadata(runtime.join("bin/cortex-acp")).unwrap().mode() & 0o7777,
        0o700
    );
    assert_eq!(
        fs::metadata(runtime.join("package-identity.json"))
            .unwrap()
            .mode()
            & 0o7777,
        0o600
    );
    let mut stack = vec![runtime.clone()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            assert!(
                path.extension().is_none_or(|e| e != "py"),
                "no Python file is staged: {}",
                path.display()
            );
            if path.is_dir() {
                stack.push(path);
            }
        }
    }

    // The record: Rust argv plus the runtime root, private, shim in the runtime.
    let record_path = f.record_path();
    assert_eq!(fs::metadata(&record_path).unwrap().mode() & 0o7777, 0o600);
    let record: Value = serde_json::from_slice(&fs::read(&record_path).unwrap()).unwrap();
    assert_eq!(
        record["argv"],
        json!([runtime.join("bin/cortex-acp").display().to_string()])
    );
    assert_eq!(
        record["env"]["CORTEX_ACP_RUNTIME_ROOT"],
        runtime.display().to_string()
    );
    assert_eq!(record["env"]["HOME"], f.home().display().to_string());
    assert_eq!(record["shim"], runtime.join(SHIM).display().to_string());
    assert!(!record.to_string().contains("python"));

    // Idempotent: a rerun changes nothing and reports the existing record.
    let again = ok(&f.bootstrap());
    assert_eq!(again["launch_config"]["created"], false);
    assert_eq!(
        fs::read(&record_path).unwrap(),
        serde_json::to_vec_pretty(&record)
            .unwrap()
            .into_iter()
            .chain(*b"\n")
            .collect::<Vec<u8>>()
    );
}

#[test]
fn bootstrap_restores_missing_files_and_refreshes_binaries() {
    let f = Fixture::new("repair");
    ok(&f.bootstrap());
    let runtime = f
        .state_root()
        .join("paseo-runtime")
        .join(Fixture::version());
    fs::remove_file(runtime.join("pinned-dependencies.json")).unwrap();
    fs::write(f.bin.join("cortex-acp"), b"#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(f.bin.join("cortex-acp"), fs::Permissions::from_mode(0o700)).unwrap();
    ok(&f.bootstrap());
    assert!(runtime.join("pinned-dependencies.json").is_file());
    assert_eq!(
        fs::read(runtime.join("bin/cortex-acp")).unwrap(),
        b"#!/bin/sh\nexit 0\n"
    );
}

#[test]
fn a_managed_python_record_is_upgraded_not_refused() {
    let f = Fixture::new("upgrade");
    let config = f.home().join(".config");
    fs::DirBuilder::new().mode(0o700).create(&config).unwrap();
    fs::DirBuilder::new()
        .mode(0o700)
        .create(config.join("cortex"))
        .unwrap();
    let env = json!({
        "HOME": f.home().display().to_string(),
        "XDG_CONFIG_HOME": config.display().to_string(),
        "XDG_STATE_HOME": f.home().join(".local/state").display().to_string(),
    });
    let old = json!({
        "argv": ["python3", "-B", "/old/runtime/src/cortex_acp.py"],
        "env": env,
        "shim": format!("/old/runtime/{SHIM}"),
    });
    fs::write(f.record_path(), serde_json::to_vec_pretty(&old).unwrap()).unwrap();
    fs::set_permissions(f.record_path(), fs::Permissions::from_mode(0o600)).unwrap();
    let out = ok(&f.bootstrap());
    assert_eq!(out["launch_config"]["created"], false);
    let record: Value = serde_json::from_slice(&fs::read(f.record_path()).unwrap()).unwrap();
    assert_eq!(record["argv"].as_array().unwrap().len(), 1);
    assert!(record["argv"][0]
        .as_str()
        .unwrap()
        .ends_with("/bin/cortex-acp"));
    assert!(record["env"]["CORTEX_ACP_RUNTIME_ROOT"].is_string());
}

#[test]
fn an_unrelated_or_unsafe_record_is_refused_and_left_alone() {
    let f = Fixture::new("refuse");
    let config = f.home().join(".config");
    fs::DirBuilder::new().mode(0o700).create(&config).unwrap();
    fs::DirBuilder::new()
        .mode(0o700)
        .create(config.join("cortex"))
        .unwrap();
    let path = f.record_path();
    fs::write(&path, b"{\"argv\":[\"/usr/bin/other\"]}\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let refused = f.bootstrap();
    assert_eq!(refused.status.code(), Some(1));
    assert!(
        stderr(&refused).contains("refusing to overwrite unrelated launch configuration"),
        "{}",
        stderr(&refused)
    );
    assert_eq!(
        fs::read(&path).unwrap(),
        b"{\"argv\":[\"/usr/bin/other\"]}\n"
    );

    // Explicit replacement through the launch step is allowed.
    let replaced = f.run(&[
        "configure-paseo-plugin-launch",
        "--rust-binary",
        f.bin.join("cortex-acp").to_str().unwrap(),
        "--runtime-root",
        Fixture::source().to_str().unwrap(),
        "--replace",
    ]);
    assert_eq!(
        ok(&replaced)["argv"],
        json!([f.bin.join("cortex-acp").display().to_string()])
    );

    // A symlinked record and a loose-mode record are refused.
    fs::remove_file(&path).unwrap();
    symlink("/etc/hostname", &path).unwrap();
    assert!(stderr(&f.bootstrap()).contains("refusing symlinked Paseo launch configuration"));
    fs::remove_file(&path).unwrap();
    fs::write(&path, b"{}").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(stderr(&f.bootstrap()).contains("must be a regular 0600 file"));
}

#[test]
fn bootstrap_needs_both_built_binaries_and_setup_never_takes_secrets() {
    let f = Fixture::new("missing");
    fs::remove_file(f.bin.join("cortex-acp")).unwrap();
    let refused = f.bootstrap();
    assert_eq!(refused.status.code(), Some(1));
    assert!(stderr(&refused).contains("cortex-acp is not built"));
    assert!(!f.record_path().exists());

    let secret = f.run(&[
        "add-connection",
        "--engine",
        "codex-acp",
        "--api-key-file",
        "/dev/null",
    ]);
    assert_eq!(secret.status.code(), Some(1));
    assert!(stderr(&secret).contains("API-key connections"));
    assert_eq!(f.run(&["no-such-command"]).status.code(), Some(2));
    assert_eq!(f.run(&["validate-connection"]).status.code(), Some(1));
}

/// A stand-in Paseo CLI (never the real daemon): answers the five commands
/// `install-paseo-plugin` issues and records each invocation.
fn fake_paseo(f: &Fixture, home: &Path) -> PathBuf {
    let script = f.root.join("paseo-fake");
    let state = f.root.join("installed");
    let log = f.root.join("paseo.log");
    let body = format!(
        r#"#!/bin/sh
echo "$@" >> '{log}'
case "$1 $2" in
"daemon status") echo '{{"localDaemon":"running","home":"{home}"}}';;
"plugin ls")
  if [ "$3" = "--json" ]; then
    if [ -f '{state}' ]; then echo '[{{"id":"cortex-acp","status":"running","path":"{plugin}"}}]'; else echo '[]'; fi
  else echo '[{{"id":"cortex-acp","status":"running"}}]'; fi;;
"plugin install") : > '{state}';;
esac
exit 0
"#,
        log = log.display(),
        home = home.display(),
        state = state.display(),
        plugin = f.root.join("source/integrations/paseo").display(),
    );
    fs::write(&script, body).unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    script
}

#[test]
fn install_verifies_the_plugin_hash_writes_the_record_and_migrates_only_the_owned_legacy_provider()
{
    let f = Fixture::new("install");
    let source = f.root.join("source");
    let plugin = source.join("integrations/paseo");
    fs::create_dir_all(plugin.join("node_modules/dep")).unwrap();
    fs::write(plugin.join("node_modules/dep/ignored.js"), "x").unwrap();
    fs::write(
        plugin.join("paseo-plugin.json"),
        r#"{"id":"cortex-acp","requirements":{"paseo":">=0.9.0"}}"#,
    )
    .unwrap();
    fs::write(plugin.join("index.server.ts"), "export {}\n").unwrap();
    let write_integrity = |digest: String| {
        fs::write(
            plugin.join("plugin-integrity.json"),
            json!({"algorithm":"sha256","payload_sha256":digest}).to_string(),
        )
        .unwrap();
    };
    let paseo_home = f.root.join(".paseo-fake");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&paseo_home)
        .unwrap();
    let entry = source.join("src/cortex_acp.py").display().to_string();
    let config = json!({"pluginsEnabled":true,"keep":1,
        "agents":{"providers":{"cortex":{"extends":"acp","command":["python3","-B",entry]},
                               "other":{"extends":"acp","command":["/usr/bin/other"]}}}});
    fs::write(paseo_home.join("config.json"), config.to_string()).unwrap();
    fs::set_permissions(
        paseo_home.join("config.json"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let cli = fake_paseo(&f, &paseo_home);
    let run = |extra: &[&str]| {
        let binary = f.bin.join("cortex-acp");
        let mut args = vec![
            "install-paseo-plugin",
            "--source-root",
            source.to_str().unwrap(),
            "--rust-binary",
            binary.to_str().unwrap(),
        ];
        args.extend(extra);
        output(
            Command::new(f.bin.join("cortex-paseo-tools"))
                .args(args)
                .env_clear()
                .env("HOME", f.home())
                .env("PATH", "")
                .env("CORTEX_PASEO_CLI", &cli)
                .env("CORTEX_ENGINE_UPDATE", "off")
                .current_dir("/"),
        )
    };

    // A stale integrity record refuses before the daemon is touched.
    write_integrity("0".repeat(64));
    let stale = run(&[]);
    assert_eq!(stale.status.code(), Some(1));
    assert!(
        stderr(&stale).contains("source hash does not match"),
        "{}",
        stderr(&stale)
    );
    assert!(!f.root.join("paseo.log").exists());
    assert!(!f.record_path().exists());

    write_integrity(cortex_paseo_tools::paseo_setup::plugin_payload_digest(&plugin).unwrap());
    let out = ok(&run(&[]));
    assert_eq!(out["status"], "running");
    assert_eq!(out["launch_config"]["created"], true);
    assert_eq!(
        out["launch_config"]["argv"],
        json!([f.bin.join("cortex-acp").display().to_string()])
    );
    assert_eq!(out["legacy_provider_migration"]["migrated"], true);
    let after: Value =
        serde_json::from_slice(&fs::read(paseo_home.join("config.json")).unwrap()).unwrap();
    assert!(after["agents"]["providers"].get("cortex").is_none());
    assert_eq!(
        after["agents"]["providers"]["other"]["command"],
        json!(["/usr/bin/other"])
    );
    assert_eq!(after["keep"], 1);
    let backup = out["legacy_provider_migration"]["backup"].as_str().unwrap();
    assert_eq!(fs::metadata(backup).unwrap().mode() & 0o7777, 0o600);
    let log = fs::read_to_string(f.root.join("paseo.log")).unwrap();
    assert!(log.contains("plugin install"), "{log}");
    assert!(log.contains("reload --json"), "{log}");

    // Idempotent: an installed running plugin is reloaded, nothing is migrated again.
    let again = ok(&run(&[]));
    assert_eq!(again["launch_config"]["created"], false);
    assert_eq!(again["legacy_provider_migration"]["migrated"], false);
    let log = fs::read_to_string(f.root.join("paseo.log")).unwrap();
    assert!(log.contains("plugin reload cortex-acp"), "{log}");
}

#[test]
fn the_repository_plugin_integrity_record_matches_the_native_digest() {
    // The same digest the retired Python stamp computed; a stale record would
    // make `install-paseo-plugin` refuse. Restamp after any plugin edit.
    let plugin = Fixture::source().join("integrations/paseo");
    let record: Value =
        serde_json::from_slice(&fs::read(plugin.join("plugin-integrity.json")).unwrap()).unwrap();
    assert_eq!(
        record["payload_sha256"],
        cortex_paseo_tools::paseo_setup::plugin_payload_digest(&plugin).unwrap()
    );
    let manifest: Value =
        serde_json::from_slice(&fs::read(plugin.join("paseo-plugin.json")).unwrap()).unwrap();
    let build = manifest["build"].to_string();
    assert!(!build.contains("python"), "{build}");
    assert!(
        build.contains("cortex-paseo-tools") && build.contains("bootstrap-paseo-git-install"),
        "{build}"
    );
}
