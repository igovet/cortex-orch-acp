#[path = "support/hermetic.rs"]
mod hermetic;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    os::{
        fd::AsRawFd,
        unix::fs::{symlink, PermissionsExt},
    },
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
use workspace_service::{
    config_read_model::{ConfigurationReader, SetupEnvironment},
    native_routes::RouteEnvironment,
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "cw-sd-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o700)).unwrap();
        Self(p)
    }
    fn write(&self, name: &str, raw: &[u8]) -> PathBuf {
        let p = self.0.join(name);
        for dir in p
            .parent()
            .unwrap()
            .ancestors()
            .take_while(|p| p.starts_with(&self.0))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            fs::create_dir_all(dir).unwrap();
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
        }
        fs::write(&p, raw).unwrap();
        let exec = name.starts_with("bin/") || name.ends_with("/node") || name.contains("/vendor/");
        fs::set_permissions(
            &p,
            fs::Permissions::from_mode(if exec { 0o700 } else { 0o600 }),
        )
        .unwrap();
        p
    }
    fn reader(&self) -> ConfigurationReader {
        ConfigurationReader::new(RouteEnvironment {
            home: self.0.join("home"),
            config: self.0.join("config"),
            state: self.0.join("state"),
            routing_override: None,
        })
        .unwrap()
    }
    fn env(&self) -> SetupEnvironment {
        SetupEnvironment {
            data: self.0.join("data"),
            path: vec![self.0.join("bin")],
            node_override: None,
            bundle: None,
        }
    }
    fn expand(&self, v: &Value) -> Value {
        serde_json::from_str(&v.to_string().replace("@ROOT@", self.0.to_str().unwrap())).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn corpus() -> Value {
    serde_json::from_str(include_str!("fixtures/setup-discovery-python.json")).unwrap()
}
fn snapshot(root: &Path) -> BTreeMap<PathBuf, (u32, Vec<u8>)> {
    let mut out = BTreeMap::new();
    for e in fs::read_dir(root).unwrap() {
        let p = e.unwrap().path();
        let m = fs::symlink_metadata(&p).unwrap();
        let bytes = if m.is_symlink() {
            fs::read_link(&p)
                .unwrap()
                .as_os_str()
                .as_encoded_bytes()
                .to_vec()
        } else if m.is_file() {
            fs::read(&p).unwrap()
        } else {
            vec![]
        };
        out.insert(p.clone(), (m.permissions().mode(), bytes));
        if m.is_dir() {
            out.extend(snapshot(&p));
        }
    }
    out
}
#[test]
fn setup_and_full_composition_match_the_oracle() {
    let c = corpus();
    assert_eq!(c["cases"].as_array().unwrap().len(), 54);
    for case in c["cases"].as_array().unwrap() {
        let f = Fixture::new();
        for (name, raw) in case["files"].as_object().unwrap() {
            f.write(name, f.expand(raw).as_str().unwrap().as_bytes());
        }
        let _held = if case["held_lock"] == true {
            let file = OpenOptions::new()
                .read(true)
                .open(f.0.join("config/cortex/automatic-setup.lock"))
                .unwrap();
            assert_eq!(
                unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
                0
            );
            Some(file)
        } else {
            None
        };
        let mut env = f.env();
        if case["bundle"] == true {
            env.bundle = Some(f.0.join("bundle"));
        }
        if case["node_override"] == true {
            env.node_override = Some(f.0.join("bin/node"));
            env.path.clear();
        }
        let before = snapshot(&f.0);
        let actual = f.reader().setup_discovery(&env).unwrap();
        let mut expected = f.expand(&case["expected"]["discovery"]);
        // The retained Python optional version producer is deliberately disabled
        // by the read-effect contract. Absence must not be invented as success.
        for item in expected["engines"].as_array_mut().unwrap() {
            if matches!(item["automatic_setup"].as_str(), Some("downloading" | "connecting" | "refreshing_models")) {
                // Frozen captures know only the global writer lock.
                item["automatic_setup"] = json!("interrupted");
            }
            if item["engine"] == "opencode" && item["already_connected"].is_null() {
                assert_eq!(item["status"], "not_downloaded");
                item["status"] = json!("discovery_unavailable");
            }
        }
        assert_eq!(actual.view, expected, "{}", case["name"]);
        let diagnostic =
            json!({"engine":"opencode","source":"setup_discovery","code":"version_probe_disabled"});
        assert_eq!(actual.diagnostics, vec![diagnostic.clone()]);
        let full = f.reader().settings_with_setup_readers(
            &json!({"discovery":{"engines":[{"engine":"codex-acp","status":"forged"}]},"workspace":{"mode":"forged"},"surfaces":{}}),
            1800000000.0,None,&f.0.join("workspace"),&env).unwrap();
        let mut want = f.expand(&case["expected"]["view"]);
        if want["config"]["setup"]["opencode"]["status"] == "not_downloaded" {
            want["config"]["setup"]["opencode"]["status"] = json!("discovery_unavailable");
        }
        for item in expected["engines"].as_array().unwrap() {
            if let (Some(engine), Some(phase)) = (item["engine"].as_str(), item["automatic_setup"].as_str()) {
                want["config"]["setup"][engine]["automaticPhase"] = json!(phase);
            }
        }
        assert_eq!(full.view, want, "{}", case["name"]);
        assert!(full.diagnostics.contains(&diagnostic));
        assert_eq!(before, snapshot(&f.0), "nonmutation {}", case["name"]);
    }
}
#[test]
fn provenance_and_no_process_capture() {
    let c = corpus();
    for key in ["process_attempts", "real_engine_calls"] {
        assert_eq!(c["evidence"][key], 0);
    }
    for key in ["disposable_home", "fake_executables"] {
        assert_eq!(c["evidence"][key], true);
    }
}
#[test]
fn staged_setup_failure_is_visible_without_reclassifying_the_connection() {
    let f = Fixture::new();
    let observe = |status: &[u8]| {
        f.write("config/cortex/first-run-codex-acp.json", status);
        f.reader().settings_with_setup_readers(
            &json!({"surfaces":{}}), 1800000000.0, None, &f.0.join("workspace"), &f.env(),
        ).unwrap().view["config"]["setup"]["codex-acp"].clone()
    };
    let current = observe(br#"{"phase":"setup_failed","stage":"checking_saved_catalog","reason":"unsafe_storage"}"#);
    assert_eq!(current["automaticPhase"], "setup_failed");
    assert_eq!(current["automaticStage"], "checking_saved_catalog");
    assert_eq!(current["reason"], "unsafe_storage");
    let historical = observe(br#"{"phase":"setup_failed"}"#);
    assert_eq!(historical["status"], current["status"]);
    assert_eq!(historical["automaticPhase"], "setup_failed");
    assert!(historical["automaticStage"].is_null());
    assert!(historical["reason"].is_null());
    let unsafe_data = observe(br#"{"phase":"setup_failed","stage":"/private/path","reason":"secret contents"}"#);
    assert!(unsafe_data["automaticStage"].is_null());
    assert!(unsafe_data["reason"].is_null());
}
#[test]
fn unsafe_status_roots_and_locks_refuse_without_partial_settings_or_repair() {
    for variant in [
        "status-link",
        "status-hardlink",
        "status-fifo",
        "status-directory",
        "status-public",
        "status-oversize",
        "status-utf8",
        "lock-link",
        "lock-hardlink",
        "lock-fifo",
        "lock-public",
        "data-link",
        "data-relative",
        "data-parent",
        "data-project",
        "bundle-link",
        "manifest-link",
        "manifest-fifo",
        "package-parent-link",
        "native-link",
        "native-public",
        "marker-link",
    ] {
        let f = Fixture::new();
        let status = f.write(
            "config/cortex/first-run-codex-acp.json",
            br#"{"phase":"downloading"}"#,
        );
        let lock = f.write("config/cortex/automatic-setup-codex-acp.lock", b"");
        let manifest = f.write("data/cortex/engines/codex-acp/2.3.0/node_modules/@agentclientprotocol/codex-acp/package.json",br#"{"version":"2.3.0","bin":"dist/entry.js"}"#);
        f.write("data/cortex/engines/codex-acp/2.3.0/node_modules/@agentclientprotocol/codex-acp/dist/entry.js",b"// inert");
        f.write("bin/node", b"#!/bin/sh\nexit 98\n");
        let marker = f.write("home/.codex/auth.json", b"not-to-be-read");
        let mut env = f.env();
        let target = if variant.starts_with("status-") {
            status.clone()
        } else if variant.starts_with("lock-") {
            lock.clone()
        } else {
            manifest.clone()
        };
        match variant {
            "status-link" | "lock-link" | "manifest-link" => {
                fs::remove_file(&target).unwrap();
                symlink(f.write("other", b"{}"), &target).unwrap();
            }
            "status-hardlink" | "lock-hardlink" => {
                fs::hard_link(&target, f.0.join("alias")).unwrap();
            }
            "status-fifo" | "lock-fifo" | "manifest-fifo" => {
                fs::remove_file(&target).unwrap();
                let p = std::ffi::CString::new(target.as_os_str().as_encoded_bytes()).unwrap();
                assert_eq!(unsafe { libc::mkfifo(p.as_ptr(), 0o600) }, 0);
            }
            "status-directory" => {
                fs::remove_file(&target).unwrap();
                fs::create_dir(&target).unwrap();
            }
            "status-public" | "lock-public" => {
                fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap()
            }
            "status-oversize" => {
                OpenOptions::new()
                    .write(true)
                    .open(&target)
                    .unwrap()
                    .set_len(4 * 1024 * 1024 + 1)
                    .unwrap();
            }
            "status-utf8" => fs::write(&target, [0xff]).unwrap(),
            "data-relative" => env.data = PathBuf::from("relative"),
            "data-parent" => env.data = f.0.join("data/../data"),
            "data-project" => {
                f.write("data/.git", b"");
            }
            "data-link" => {
                fs::rename(f.0.join("data"), f.0.join("real-data")).unwrap();
                symlink(f.0.join("real-data"), f.0.join("data")).unwrap();
            }
            "bundle-link" => {
                symlink(f.0.join("data"), f.0.join("bundle")).unwrap();
                env.bundle = Some(f.0.join("bundle"));
            }
            "package-parent-link" => {
                let p = manifest.parent().unwrap();
                fs::rename(p, p.with_file_name("real")).unwrap();
                symlink(p.with_file_name("real"), p).unwrap();
            }
            "native-link" => {
                fs::rename(f.0.join("home/.codex"), f.0.join("native")).unwrap();
                symlink(f.0.join("native"), f.0.join("home/.codex")).unwrap();
            }
            "native-public" => {
                fs::set_permissions(marker.parent().unwrap(), fs::Permissions::from_mode(0o755))
                    .unwrap()
            }
            "marker-link" => {
                fs::remove_file(&marker).unwrap();
                symlink(f.write("other", b"secret"), marker).unwrap();
            }
            _ => unreachable!(),
        }
        // Status encoding/JSON is a diagnostic failure, not a storage success.
        let expected_diagnostic = variant == "status-utf8";
        if expected_diagnostic {
            assert_eq!(
                f.reader().setup_discovery(&env).unwrap().view["engines"][0]["automatic_setup"],
                "setup_failed"
            );
            continue;
        }
        // Snapshot FIFOs without opening them (all regular files remain unchanged).
        let before = snapshot(&f.0);
        assert!(f.reader().setup_discovery(&env).is_err(), "{variant}");
        assert!(
            f.reader()
                .settings_with_setup_readers(
                    &json!({"surfaces":{}}),
                    1800000000.0,
                    None,
                    &f.0.join("workspace"),
                    &env
                )
                .is_err(),
            "{variant}"
        );
        assert_eq!(before, snapshot(&f.0), "{variant}");
    }
}
#[test]
fn missing_state_is_not_created_and_upstream_failures_propagate() {
    let f = Fixture::new();
    let before = snapshot(&f.0);
    let setup = f.reader().setup_discovery(&f.env()).unwrap();
    assert_eq!(setup.view["engines"][0]["status"], "not_downloaded");
    f.reader()
        .settings_with_setup_readers(
            &json!({"surfaces":{}}),
            1800000000.0,
            None,
            &f.0.join("workspace"),
            &f.env(),
        )
        .unwrap();
    assert_eq!(before, snapshot(&f.0));
    f.write("config/cortex/connections.json", b"{");
    assert!(f.reader().setup_discovery(&f.env()).is_err());
    assert!(f
        .reader()
        .settings_with_setup_readers(
            &json!({"surfaces":{}}),
            1800000000.0,
            None,
            &f.0.join("workspace"),
            &f.env()
        )
        .is_err());
    assert!(f
        .reader()
        .settings_with_setup_readers(
            &Value::Null,
            1800000000.0,
            None,
            &f.0.join("workspace"),
            &f.env()
        )
        .is_err());
}
