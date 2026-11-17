#[path = "support/hermetic.rs"]
mod hermetic;
use cortex_core::store::locks::AccessLock;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::fs::{symlink, PermissionsExt},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Barrier,
    },
    time::Duration,
};
use workspace_service::{config_refresh::refresh_connections, native_routes::RouteEnvironment};

const NOW: &str = "2026-11-13T21:30:00+00:00";
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = PathBuf::from(format!(
            "/tmp/cw-rf-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let f = Self(root);
        for p in ["home", "config/cortex", "state"] {
            f.directory(p);
        }
        f
    }
    fn directory(&self, name: &str) {
        let path = self.0.join(name);
        for p in path
            .ancestors()
            .take_while(|p| p.starts_with(&self.0))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            fs::create_dir_all(p).unwrap();
            fs::set_permissions(p, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
    fn write(&self, name: &str, bytes: &[u8]) {
        self.directory(Path::new(name).parent().unwrap().to_str().unwrap());
        let path = self.0.join(name);
        fs::write(&path, bytes).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    fn expand(&self, s: &str) -> String {
        s.replace("@ROOT@", self.0.to_str().unwrap())
    }
    fn env(&self) -> RouteEnvironment {
        RouteEnvironment {
            home: self.0.join("home"),
            config: self.0.join("config"),
            state: self.0.join("state"),
            routing_override: None,
        }
    }
    fn bytes(&self) -> Vec<u8> {
        fs::read(self.0.join("config/cortex/connections.json")).unwrap()
    }
    fn token(&self) -> String {
        token(&self.bytes())
    }
    fn refresh(&self, ids: &[String]) -> cortex_core::CoreResult<Value> {
        refresh_connections(&self.env(), ids, &self.token(), NOW, Duration::ZERO)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn token(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}
fn credential_snapshot(root: &Path) -> std::collections::BTreeMap<PathBuf, (u32, Vec<u8>)> {
    let mut result = std::collections::BTreeMap::new();
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if ["connections.json", "connections.lock"]
            .iter()
            .any(|n| path.file_name().unwrap() == *n)
            && path.parent().unwrap().ends_with("config/cortex")
        {
            continue;
        }
        let m = fs::symlink_metadata(&path).unwrap();
        let bytes = if m.is_symlink() {
            fs::read_link(&path)
                .unwrap()
                .as_os_str()
                .as_encoded_bytes()
                .to_vec()
        } else if m.is_file() {
            fs::read(&path).unwrap()
        } else {
            vec![]
        };
        result.insert(path.clone(), (m.permissions().mode(), bytes));
        if m.is_dir() {
            result.extend(credential_snapshot(&path));
        }
    }
    result
}
fn corpus() -> Value {
    serde_json::from_str(include_str!("fixtures/registry-refresh-python.json")).unwrap()
}
fn ids(case: &Value) -> Vec<String> {
    case["ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().into())
        .collect()
}
fn prepare(case: &Value) -> Fixture {
    let f = Fixture::new();
    for dir in case["directories"].as_array().unwrap() {
        f.directory(dir.as_str().unwrap());
    }
    for (path, data) in case["files"].as_object().unwrap() {
        f.write(path, data.as_str().unwrap().as_bytes());
    }
    for (path, target) in case["links"].as_object().unwrap() {
        symlink(f.expand(target.as_str().unwrap()), f.0.join(path)).unwrap();
    }
    for (path, mode) in case["modes"].as_object().unwrap() {
        fs::set_permissions(
            f.0.join(path),
            fs::Permissions::from_mode(mode.as_u64().unwrap() as u32),
        )
        .unwrap();
    }
    let target = PathBuf::from(f.expand(case["target"].as_str().unwrap()));
    if case["scenario"] == "parent-link" {
        let parent = target.parent().unwrap();
        let real = parent.with_file_name(format!(
            "{}-real",
            parent.file_name().unwrap().to_str().unwrap()
        ));
        fs::rename(parent, &real).unwrap();
        symlink(&real, parent).unwrap();
    }
    if case["scenario"] == "hardlink" {
        fs::hard_link(&target, target.with_file_name("alias")).unwrap();
    }
    f.write(
        "config/cortex/connections.json",
        f.expand(case["raw"].as_str().unwrap()).as_bytes(),
    );
    f
}
#[test]
fn explicit_refresh_matches_the_recorded_oracle_filesystem_states_and_changes() {
    let corpus = corpus();
    assert_eq!(corpus["cases"].as_array().unwrap().len(), 62);
    for case in corpus["cases"].as_array().unwrap() {
        let f = prepare(case);
        let before = f.bytes();
        let untouched = credential_snapshot(&f.0);
        let out = f
            .refresh(&ids(case))
            .unwrap_or_else(|e| panic!("{}: {e}", case["name"]));
        assert_eq!(out["connections"], case["expected"], "{}", case["name"]);
        assert_eq!(out["changed"], case["changed"], "{}", case["name"]);
        assert_eq!(out["revision"], f.token());
        assert_eq!(
            untouched,
            credential_snapshot(&f.0),
            "refresh changed credential storage"
        );
        if case["changed"] == false {
            assert_eq!(before, f.bytes(), "no-op rewrote registry");
        }
        let actual: Value = serde_json::from_slice(&f.bytes()).unwrap();
        // Python normalization adds default optional fields; this corpus sets them explicitly.
        let expected: Value = serde_json::from_str(&f.expand(&case["after"].to_string())).unwrap();
        assert_eq!(actual, expected, "{}", case["name"]);
        assert!(!out.to_string().contains("credential"));
        assert!(!out.to_string().contains(f.0.to_str().unwrap()));
        assert!(!fs::read_dir(f.0.join("config/cortex")).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp")));
    }
}
#[test]
fn capture_provenance_is_current() {
    assert_eq!(corpus()["process_attempts"], 0);
}
fn present() -> Value {
    corpus()["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "codex-acp-True-engine_native_dir-present")
        .unwrap()
        .clone()
}
#[test]
fn failures_preserve_registry_and_do_not_approve_or_lookup_credentials() {
    for scenario in [
        "stale",
        "fingerprint",
        "keyring",
        "unknown",
        "malformed",
        "unsafe",
        "link",
        "hardlink",
        "fifo",
    ] {
        let case = present();
        let f = prepare(&case);
        let mut doc: Value = serde_json::from_slice(&f.bytes()).unwrap();
        let id = ids(&case).remove(0);
        let mut expected_ids = vec![id.clone()];
        let mut expected_token = f.token();
        let expected_error = match scenario {
            "stale" => {
                expected_token = "absent".into();
                "revision_conflict"
            }
            "fingerprint" => {
                doc["connections"][&id]["approved_fingerprint"] = json!("not-approved");
                "connection_fingerprint_changed"
            }
            "keyring" => {
                doc["connections"][&id]["auth_mode"] = json!("api_key");
                doc["connections"][&id]["key_kind"] = json!("openai_api_key");
                doc["connections"][&id]["credential_ref"] =
                    json!({"kind":"os_keyring","service":"fixture","account":"fixture"});
                "keyring_refresh_unsupported"
            }
            "unknown" => {
                expected_ids.push("cn_ffffffffffff".into());
                "connection_unavailable"
            }
            "malformed" => "invalid_registry",
            _ => "native_route_settings_unreadable",
        };
        if ["fingerprint", "keyring"].contains(&scenario) {
            f.write(
                "config/cortex/connections.json",
                &serde_json::to_vec(&doc).unwrap(),
            );
        }
        if scenario == "malformed" {
            f.write("config/cortex/connections.json", b"{");
        }
        let path = f.0.join("config/cortex/connections.json");
        if scenario == "unsafe" {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        }
        if ["link", "hardlink", "fifo"].contains(&scenario) {
            let other = f.0.join("untouched");
            fs::rename(&path, &other).unwrap();
            if scenario == "link" {
                symlink(&other, &path).unwrap();
            } else if scenario == "hardlink" {
                fs::hard_link(&other, &path).unwrap();
            } else {
                use std::os::unix::ffi::OsStrExt;
                let p = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
                assert_eq!(unsafe { libc::mkfifo(p.as_ptr(), 0o600) }, 0);
            }
        }
        let before = if scenario != "fifo" {
            Some(f.bytes())
        } else {
            None
        };
        if !["stale", "fifo"].contains(&scenario) {
            expected_token = f.token();
        }
        let error = refresh_connections(
            &f.env(),
            &expected_ids,
            &expected_token,
            NOW,
            Duration::ZERO,
        )
        .unwrap_err();
        assert_eq!(error.code(), expected_error, "{scenario}");
        if let Some(before) = before {
            assert_eq!(before, f.bytes(), "{scenario} changed registry");
        }
        assert!(!error.to_string().contains("not-approved"));
    }
}
#[test]
fn lock_timeout_and_unsafe_lock_never_rewrite_registry() {
    for unsafe_lock in [false, true] {
        let f = prepare(&present());
        let path = f.0.join("config/cortex/connections.lock");
        let lock = AccessLock::acquire(&path, true, true, Duration::ZERO).unwrap();
        let before = f.bytes();
        assert_eq!(f.refresh(&ids(&present())).unwrap_err().code(), "busy");
        drop(lock);
        if unsafe_lock {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            assert_eq!(
                f.refresh(&ids(&present())).unwrap_err().code(),
                "unsafe_storage"
            );
        }
        assert_eq!(before, f.bytes());
    }
}
#[test]
fn request_preflight_never_creates_a_lock_or_changes_registry() {
    let f = prepare(&present());
    let id = ids(&present());
    for invalid in [
        vec![],
        vec!["../bad".into()],
        vec![id[0].clone(), id[0].clone()],
    ] {
        assert_eq!(
            f.refresh(&invalid).unwrap_err().code(),
            "invalid_refresh_request"
        );
    }
    assert_eq!(
        refresh_connections(&f.env(), &id, &f.token(), "not-a-time", Duration::ZERO)
            .unwrap_err()
            .code(),
        "invalid_refresh_request"
    );
    assert_eq!(
        refresh_connections(&f.env(), &id, &f.token(), NOW, Duration::from_secs(26))
            .unwrap_err()
            .code(),
        "invalid_refresh_request"
    );
    assert!(!f.0.join("config/cortex/connections.lock").exists());
}
#[test]
fn no_setup_repair_or_root_creation_and_display_stays_read_only() {
    let f = Fixture::new();
    fs::remove_dir(f.0.join("config/cortex")).unwrap();
    let id = vec!["cn_000000000001".into()];
    assert!(refresh_connections(&f.env(), &id, "absent", NOW, Duration::ZERO).is_err());
    assert!(!f.0.join("config/cortex").exists());
    let reader = workspace_service::config_read_model::ConfigurationReader::new(f.env()).unwrap();
    reader.connection_management().unwrap();
    assert!(!f.0.join("config/cortex").exists());
    f.directory("config/cortex");
    assert_eq!(
        refresh_connections(&f.env(), &id, "absent", NOW, Duration::ZERO)
            .unwrap_err()
            .code(),
        "registry_missing"
    );
    assert!(!f.0.join("config/cortex/connections.json").exists());
    fs::set_permissions(f.0.join("config/cortex"), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(refresh_connections(&f.env(), &id, "absent", NOW, Duration::ZERO).is_err());
    assert_eq!(
        fs::metadata(f.0.join("config/cortex"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
}
#[test]
fn serialized_refreshes_revalidate_after_lock_and_preserve_other_owner_fields() {
    let f = prepare(&present());
    let token = f.token();
    let env = f.env();
    let barrier = Arc::new(Barrier::new(3));
    let id = ids(&present());
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let (env, token, barrier, id) =
                (env.clone(), token.clone(), barrier.clone(), id.clone());
            std::thread::spawn(move || {
                barrier.wait();
                refresh_connections(&env, &id, &token, NOW, Duration::from_secs(2))
            })
        })
        .collect();
    barrier.wait();
    let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .find_map(|r| r.as_ref().err())
            .unwrap()
            .code(),
        "revision_conflict"
    );
    let guard = AccessLock::acquire(
        &f.0.join("config/cortex/connections.lock"),
        true,
        true,
        Duration::ZERO,
    )
    .unwrap();
    let stale = f.token();
    let env = f.env();
    let ids = id.clone();
    let t = std::thread::spawn(move || {
        refresh_connections(&env, &ids, &stale, NOW, Duration::from_secs(2))
    });
    let mut doc: Value = serde_json::from_slice(&f.bytes()).unwrap();
    doc["connections"][&id[0]]["name"] = json!("Concurrent owner");
    doc["connections"][&id[0]]["max_concurrent_sessions"] = json!(9);
    doc["connections"][&id[0]]["state"] = json!("unavailable");
    f.write(
        "config/cortex/connections.json",
        &serde_json::to_vec(&doc).unwrap(),
    );
    drop(guard);
    assert_eq!(t.join().unwrap().unwrap_err().code(), "revision_conflict");
    f.refresh(&id).unwrap();
    let after: Value = serde_json::from_slice(&f.bytes()).unwrap();
    assert_eq!(after["connections"][&id[0]]["name"], "Concurrent owner");
    assert_eq!(after["connections"][&id[0]]["max_concurrent_sessions"], 9);
    assert_eq!(after["auto_connect_disabled"], json!(["opencode"]));
    assert_eq!(after["override_history"], json!({}));
}
