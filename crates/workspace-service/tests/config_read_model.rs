#[path = "support/hermetic.rs"]
mod hermetic;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::{symlink, PermissionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
use workspace_service::{config_read_model::*, native_routes::RouteEnvironment};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = PathBuf::from(format!(
            "/tmp/cw-cfg-rs-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        Self(root)
    }
    fn directory(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir_all(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn write(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}
fn snapshot(path: &Path) -> BTreeMap<PathBuf, (u32, Vec<u8>)> {
    let mut out = BTreeMap::new();
    for entry in fs::read_dir(path).unwrap() {
        let p = entry.unwrap().path();
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
            Vec::new()
        };
        out.insert(p.clone(), (m.permissions().mode(), bytes));
        if m.is_dir() {
            out.extend(snapshot(&p));
        }
    }
    out
}
#[test]
fn config_foundation_matches_the_oracle_captures() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/config-read-python.json")).unwrap();
    assert_eq!(fixture["cases"].as_array().unwrap().len(), 193);
    for case in fixture["cases"].as_array().unwrap() {
        let v = &case["input"];
        let actual = match case["kind"].as_str().unwrap() {
            "profiles" => {
                parse_profiles((!v.is_null()).then_some(v)).unwrap_or_else(|e| json!({"error":e}))
            }
            "connections" => parse_connections((!v.is_null()).then_some(v))
                .unwrap_or_else(|_| json!({"refused":true})),
            "adapter" => adapter_version(
                v["engine"].as_str().unwrap(),
                v["version"].as_str().unwrap(),
            ),
            "manifest" => json!(adapter_manifest(v)),
            "fingerprint" => {
                let launch = v["launch"].as_array().map(|a| {
                    a.iter()
                        .map(|v| v.as_str().unwrap().to_owned())
                        .collect::<Vec<_>>()
                });
                json!(route_fingerprint(
                    v["engine"].as_str().unwrap(),
                    v["provider"].as_str().unwrap(),
                    v["endpoint"].as_str(),
                    launch.as_deref()
                ))
            }
            "usage" => usage_windows(
                v["engine"].as_str().unwrap(),
                &v["windows"],
                v["now"].as_f64().unwrap(),
            ),
            "context" => json!(context_window(&v["windows"], v["model"].as_str())),
            "login" => {
                let f = Fixture::new();
                let d = f.directory("native");
                let name = if v["engine"] == "codex-acp" {
                    "auth.json"
                } else {
                    ".credentials.json"
                };
                let marker = d.join(name);
                match v["shape"].as_str().unwrap() {
                    "regular" | "unsafe-mode" => {
                        fs::write(&marker, "fixture-not-a-credential").unwrap();
                    }
                    "symlink" => symlink(f.0.join("absent"), &marker).unwrap(),
                    "directory" => fs::create_dir(&marker).unwrap(),
                    "missing" => (),
                    _ => panic!("unknown shape"),
                }
                if v["shape"] == "unsafe-mode" {
                    fs::set_permissions(&d, fs::Permissions::from_mode(0o755)).unwrap();
                }
                let before = snapshot(&f.0);
                let value = json!(native_login_marker(v["engine"].as_str().unwrap(), &d));
                assert_eq!(snapshot(&f.0), before);
                value
            }
            _ => panic!("unhandled capture"),
        };
        assert_eq!(actual, case["expected"], "{}", case["name"]);
    }
}
#[test]
fn explicit_root_reader_is_nonmutating_and_refuses_unsafe_inputs() {
    let f = Fixture::new();
    let home = f.directory("home");
    let config = f.directory("config");
    let state = f.directory("state");
    let environment = RouteEnvironment {
        home: home.clone(),
        config: config.clone(),
        state: state.clone(),
        routing_override: None,
    };
    let reader = ConfigurationReader::new(environment.clone()).unwrap();
    let empty = snapshot(&f.0);
    assert_eq!(reader.profiles().unwrap()["document"]["format"], 2);
    assert_eq!(reader.connections().unwrap()["connections"], json!({}));
    assert_eq!(
        reader
            .usage_cache("cn_123456789abc", "claude-agent-acp", 100.0)
            .unwrap(),
        json!([])
    );
    assert_eq!(
        reader.context("cn_123456789abc", Some("model")).unwrap(),
        None
    );
    assert_eq!(snapshot(&f.0), empty);
    let c = f.directory("config/cortex");
    let quota = f.directory("state/cortex/quota");
    let contexts = f.directory("state/cortex/context_windows");
    let s = state.join("cortex");
    fs::set_permissions(s, fs::Permissions::from_mode(0o700)).unwrap();
    write(
        &c.join("routing-profiles.json"),
        &json!({"format":1,"profiles":{"legacy":{"description":"","rules":{}}}}),
    );
    write(
        &c.join("connections.json"),
        &json!({"schema_version":1,"connections":{}}),
    );
    write(
        &quota.join("cn_123456789abc.json"),
        &json!({"seven_day":{"used":0,"resets_at":200}}),
    );
    write(
        &contexts.join("cn_123456789abc.json"),
        &json!({"model":128000}),
    );
    let before = snapshot(&f.0);
    assert_eq!(reader.profiles().unwrap()["migrated_from"], 1);
    assert_eq!(reader.connections().unwrap()["connections"], json!({}));
    assert_eq!(
        reader
            .usage_cache("cn_123456789abc", "claude-agent-acp", 100.0)
            .unwrap()[0]["usedPercent"],
        0.0
    );
    assert_eq!(
        reader.context("cn_123456789abc", Some("model")).unwrap(),
        Some(128000)
    );
    assert!(reader.context("../escape", None).is_err());
    assert_eq!(snapshot(&f.0), before);
    fs::set_permissions(
        c.join("routing-profiles.json"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert!(reader.profiles().is_err());
    fs::remove_file(c.join("routing-profiles.json")).unwrap();
    symlink(home.join("not-there"), c.join("routing-profiles.json")).unwrap();
    assert!(reader.profiles().is_err());
    fs::remove_file(c.join("routing-profiles.json")).unwrap();
    fs::create_dir(c.join("routing-profiles.json")).unwrap();
    assert!(reader.profiles().is_err());
    let mut relative = environment.clone();
    relative.config = PathBuf::from("relative");
    assert!(ConfigurationReader::new(relative).is_err());
    let mut project = environment;
    project.config = f.directory("repo/.cortex");
    assert!(ConfigurationReader::new(project).is_err());
}
