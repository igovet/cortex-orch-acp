use super::*;
use std::{
    os::unix::fs::PermissionsExt,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

struct Fixture {
    root: PathBuf,
    config: crate::Config,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = PathBuf::from(format!(
            "/tmp/cw-mrw-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let mut launch = crate::LaunchSpec::bare();
        launch.dev_config = Some(root.join("config"));
        let config = crate::Config {
            runtime_dir: root.join("run"),
            state_dir: root.join("workspace"),
            allowed_root: root.clone(),
            launch,
            request_timeout: Duration::from_secs(5),
            shutdown_timeout: Duration::from_millis(100),
            event_capacity: 16,
        };
        Self { root, config }
    }
    fn path(&self) -> PathBuf {
        self.root.join("config/cortex/routing.json")
    }
    fn write(&self, b: &[u8]) {
        let folder = self.path().parent().unwrap().to_owned();
        crate::local::private_tree(&folder).unwrap();
        fs::write(self.path(), b).unwrap();
        fs::set_permissions(self.path(), fs::Permissions::from_mode(0o600)).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn params() -> Value {
    json!({"surface":"routing","value":{"version":2},"tokens":{"routing":"absent"}})
}
fn bytes(v: &Value) -> Option<Vec<u8>> {
    v.as_array()
        .map(|a| a.iter().map(|n| n.as_u64().unwrap() as u8).collect())
}
fn bind(v: &Value, config: &Path) -> Value {
    match v {
        Value::String(s) => json!(s.replace("$CONFIG", config.to_str().unwrap())),
        Value::Array(a) => json!(a.iter().map(|v| bind(v, config)).collect::<Vec<_>>()),
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, v)| (k.clone(), bind(v, config)))
                .collect(),
        ),
        v => v.clone(),
    }
}

#[test]
fn user_routing_persistence_matches_the_frozen_oracle() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/machine-routing-write-python.json"
    ))
    .unwrap();
    assert_eq!(fixture["processAttempts"], 0);
    assert!(fixture["cases"].as_array().unwrap().len() >= 60);
    for case in fixture["cases"].as_array().unwrap() {
        let f = Fixture::new();
        if let Some(b) = bytes(&case["initial"]) {
            f.write(&b);
        }
        for o in case["observations"].as_array().unwrap() {
            let before = fs::read(f.path()).ok();
            let result = PreparedUserRouting::prepare(&f.config, &o["params"]).and_then(|p| {
                // Preparation has not persisted the candidate.
                assert_eq!(fs::read(f.path()).ok(), before);
                assert_eq!(p.body(), bytes(&o["bytes"]).unwrap().as_slice());
                p.commit()
            });
            let expected = bind(&o["reply"], &f.root.join("config"));
            match result {
                Ok(actual) => {
                    assert_eq!(expected["ok"], true, "{}", case["name"]);
                    // This sub-piece compares persistence metadata only, NOT the
                    // unimplemented full routing/catalog/profile save reply.
                    for key in ["changed", "target", "written_revision"] {
                        assert_eq!(
                            actual[key], expected["result"][key],
                            "{} {key}",
                            case["name"]
                        );
                    }
                }
                Err(e) => {
                    assert_eq!(
                        expected["ok"], false,
                        "{}: {} {}",
                        case["name"], e.code, e.message
                    );
                    assert_eq!(e.code, expected["code"], "{}", case["name"]);
                    assert_eq!(e.message, expected["message"], "{}", case["name"]);
                }
            }
            assert_eq!(
                fs::read(f.path()).ok(),
                bytes(&o["bytes"]),
                "{}",
                case["name"]
            );
            let mode = fs::metadata(f.path())
                .ok()
                .map(|m| json!(m.permissions().mode() & 0o777))
                .unwrap_or(Value::Null);
            assert_eq!(mode, o["mode"], "{}", case["name"]);
        }
        assert!(!f.config.state_dir.exists());
        assert!(!f.root.join(".cortex").exists());
    }
}

#[test]
fn user_routing_drop_conflict_and_unsafe_targets_never_write() {
    let f = Fixture::new();
    let prepared = PreparedUserRouting::prepare(&f.config, &params()).unwrap();
    drop(prepared);
    assert!(!f.path().exists());
    let prepared = PreparedUserRouting::prepare(&f.config, &params()).unwrap();
    f.write(b"{}");
    assert_eq!(prepared.commit().unwrap_err().code, "revision_conflict");
    assert_eq!(fs::read(f.path()).unwrap(), b"{}");
    let mut p = params();
    p["tokens"]["routing"] = json!(token(Some(b"{}")));
    let prepared = PreparedUserRouting::prepare(&f.config, &p).unwrap();
    let other = f.root.join("other");
    fs::rename(f.path(), &other).unwrap();
    std::os::unix::fs::symlink(&other, f.path()).unwrap();
    assert_eq!(prepared.commit().unwrap_err().code, "unsafe_storage");
    assert_eq!(fs::read(other).unwrap(), b"{}");
}

#[test]
fn user_routing_sync_failure_is_unknown_after_committed_bytes() {
    let f = Fixture::new();
    let prepared = PreparedUserRouting::prepare(&f.config, &params()).unwrap();
    let body = prepared.body().to_vec();
    let error = prepared
        .commit_with(|path, body| {
            replace_sync(path, body, |_| {
                Err(std::io::Error::other("injected sync failure").into())
            })
        })
        .unwrap_err();
    assert_eq!(error.code, "machine_outcome_unknown");
    assert_eq!(fs::read(f.path()).unwrap(), body);
    let mut entries = fs::read_dir(f.path().parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    entries.sort();
    assert_eq!(
        entries,
        vec![".gitignore", "connections.lock", "routing.json"]
    );
    assert_eq!(
        fs::read(f.path().parent().unwrap().join(".gitignore")).unwrap(),
        b"*\n"
    );
}

#[test]
fn user_routing_unsafe_lock_and_hardlinked_target_refuse() {
    let f = Fixture::new();
    f.write(b"{}");
    let peer = f.root.join("peer");
    fs::hard_link(f.path(), &peer).unwrap();
    let mut p = params();
    p["tokens"]["routing"] = json!(token(Some(b"{}")));
    assert_eq!(
        PreparedUserRouting::prepare(&f.config, &p)
            .unwrap()
            .commit()
            .unwrap_err()
            .code,
        "unsafe_storage"
    );
    assert_eq!(fs::read(peer).unwrap(), b"{}");
    let f = Fixture::new();
    crate::local::private_tree(f.path().parent().unwrap()).unwrap();
    let peer = f.root.join("lock-peer");
    fs::write(&peer, b"").unwrap();
    std::os::unix::fs::symlink(&peer, f.path().parent().unwrap().join("connections.lock")).unwrap();
    assert_eq!(
        PreparedUserRouting::prepare(&f.config, &params())
            .err()
            .unwrap()
            .code,
        "unsafe_storage"
    );
    assert!(!f.path().exists());
}

#[test]
fn user_routing_lock_serializes_competing_revision_admission() {
    let f = Fixture::new();
    let prepared = PreparedUserRouting::prepare(&f.config, &params()).unwrap();
    let cfg = f.config.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        let result = PreparedUserRouting::prepare(&cfg, &params()).and_then(|p| p.commit());
        tx.send(result.map_err(|e| e.code)).unwrap();
    });
    assert!(rx.recv_timeout(Duration::from_millis(40)).is_err());
    prepared.commit().unwrap();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap_err(),
        "revision_conflict"
    );
    thread.join().unwrap();
}
