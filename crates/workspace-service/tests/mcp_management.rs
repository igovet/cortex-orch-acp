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
use workspace_service::{config_read_model::ConfigurationReader, native_routes::RouteEnvironment};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = PathBuf::from(format!(
            "/tmp/cw-mcp-rs-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }
    fn directory(&self, name: &str) -> PathBuf {
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
        path
    }
    fn write(&self, raw: &[u8]) -> PathBuf {
        let path = self.directory("config/cortex").join("mcp-servers.json");
        fs::write(&path, raw).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        path
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
    fn expand(&self, text: &str) -> String {
        text.replace("@ROOT@", self.0.to_str().unwrap())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn corpus() -> Value {
    serde_json::from_str(include_str!("fixtures/mcp-management-python.json")).unwrap()
}
fn snapshot(root: &Path) -> BTreeMap<PathBuf, (u32, Vec<u8>)> {
    let mut out = BTreeMap::new();
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        let m = fs::symlink_metadata(&path).unwrap();
        let data = if m.is_file() {
            fs::read(&path).unwrap()
        } else if m.is_symlink() {
            fs::read_link(&path)
                .unwrap()
                .as_os_str()
                .as_encoded_bytes()
                .to_vec()
        } else {
            vec![]
        };
        out.insert(path.clone(), (m.permissions().mode(), data));
        if m.is_dir() {
            out.extend(snapshot(&path));
        }
    }
    out
}
#[test]
fn mcp_management_matches_the_frozen_oracle_reads() {
    let fixture = corpus();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 67);
    for case in cases {
        let f = Fixture::new();
        if let Some(raw) = case["raw"].as_str() {
            f.write(raw.as_bytes());
        }
        let before = snapshot(&f.0);
        let got = f.reader().mcp_management();
        let expected: Value =
            serde_json::from_str(&f.expand(&case["expected"].to_string())).unwrap();
        assert_eq!(
            f.reader().mcp_management_observation().unwrap(),
            expected,
            "{}",
            case["name"]
        );
        if expected["ok"] == false {
            assert!(got.is_err(), "{}", case["name"]);
        } else {
            assert_eq!(got.unwrap(), expected["result"], "{}", case["name"]);
        }
        assert_eq!(
            snapshot(&f.0),
            before,
            "read mutated files: {}",
            case["name"]
        );
    }
}
#[test]
fn mcp_reader_is_wired_to_complete_settings_projection() {
    let fixture = corpus();
    let cases = fixture["compositions"].as_array().unwrap();
    assert_eq!(cases.len(), 67);
    for case in cases {
        let f = Fixture::new();
        if let Some(raw) = case["raw"].as_str() {
            f.write(raw.as_bytes());
        }
        let mut input: Value = serde_json::from_str(&f.expand(&case["input"].to_string())).unwrap();
        // Must use the real MCP producer, not trust a supplied completed view.
        input["surfaces"]["mcp_servers"] = json!({"invalid":"must be replaced"});
        let before = snapshot(&f.0);
        let result = f
            .reader()
            .settings_with_readers(&input, 1800000000.0)
            .unwrap();
        let expected: Value =
            serde_json::from_str(&f.expand(&case["expected"].to_string())).unwrap();
        assert_eq!(result.view, expected, "{}", case["name"]);
        assert!(result.diagnostics.is_empty());
        assert_eq!(snapshot(&f.0), before);
    }
}
#[test]
fn private_storage_refusals_do_not_repair_or_read_other_targets() {
    for kind in [
        "symlink",
        "hardlink",
        "public",
        "directory",
        "fifo",
        "oversize",
        "parent-symlink",
        "public-parent",
    ] {
        let f = Fixture::new();
        let path = f.write(br#"{"servers":{}}"#);
        match kind {
            "symlink" => {
                fs::remove_file(&path).unwrap();
                let target = f.0.join("target");
                fs::write(&target, "private sentinel").unwrap();
                symlink(target, &path).unwrap();
            }
            "hardlink" => fs::hard_link(&path, f.0.join("link")).unwrap(),
            "public" => fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap(),
            "directory" => {
                fs::remove_file(&path).unwrap();
                fs::create_dir(&path).unwrap();
            }
            "fifo" => {
                fs::remove_file(&path).unwrap();
                let p = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
                assert_eq!(unsafe { libc::mkfifo(p.as_ptr(), 0o600) }, 0);
            }
            "oversize" => fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .unwrap()
                .set_len(4 * 1024 * 1024 + 1)
                .unwrap(),
            "parent-symlink" => {
                fs::rename(f.0.join("config/cortex"), f.0.join("real-cortex")).unwrap();
                symlink(f.0.join("real-cortex"), f.0.join("config/cortex")).unwrap();
            }
            "public-parent" => {
                fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o755))
                    .unwrap()
            }
            _ => unreachable!(),
        }
        let before = snapshot(&f.0);
        assert!(f.reader().mcp_management().is_err(), "{kind}");
        assert_eq!(snapshot(&f.0), before, "{kind}");
    }
}
#[test]
fn malformed_and_pathological_mcp_remain_safe_and_secret_free() {
    let f = Fixture::new();
    for raw in [
        br#"{"servers":{"s":{"env":"private-sentinel"}}}"#.as_slice(),
        br#"{"servers":{"s":{"command":"/no-execution","allowed_tools":["inspect"],"runtime_dir_env":[{}],"env":{"TOKEN":"private-sentinel"}}}}"#.as_slice(),
        br#"{"servers":{"s":{"command":"/no-execution","allowed_tools":["inspect"],"env":[{"name":"TOKEN","value":"private-sentinel"}]}}}"#.as_slice(),
    ] {
        f.write(raw);
        let before = snapshot(&f.0);
        let view = f.reader().mcp_management().unwrap();
        assert!(!view.to_string().contains("private-sentinel"));
        assert_eq!(snapshot(&f.0), before);
    }
    for raw in [
        vec![0xff],
        vec![b'['; 200],
        br#"{"servers":{},"secret":"private-sentinel"} garbage"#.to_vec(),
    ] {
        f.write(&raw);
        assert!(f.reader().mcp_management().is_err());
    }
    f.write(b"{");
    // A malformed file is not replaced with the caller-supplied successful view.
    assert!(f.reader().settings_with_readers(
        &json!({"workspace":{},"entries":[],"surfaces":{"mcp_servers":{"ok":true,"result":{}}}}), 0.0).is_err());
}
#[test]
fn capture_provenance_is_current_and_no_engine_was_invoked() {
    let fixture = corpus();
    assert_eq!(fixture["evidence"]["real_engine_calls"], 0);
    assert_eq!(fixture["evidence"]["process_attempts"], 0);
}
#[test]
fn containment_is_validated_displayed_and_never_unsupported() {
    let f = Fixture::new();
    f.write(
        br#"{"version":1,"containment":"none","servers":{
        "own":{"command":"/opt/tool","allowed_tools":["read"],"containment":"contained"},
        "inherits":{"command":"/opt/tool","allowed_tools":["read"]},
        "bad":{"command":"/opt/tool","allowed_tools":["read"],"containment":"off"}}}"#,
    );
    let view = f.reader().mcp_management().unwrap();
    assert_eq!(view["containment"], "none");
    let servers = view["servers"].as_array().unwrap();
    assert_eq!(servers[0]["config"]["containment"], "contained");
    assert!(servers.iter().all(|s| s["unsupported_keys"] == json!([])));
    assert_eq!(servers[0]["valid"], true);
    assert_eq!(servers[1]["valid"], true);
    assert!(servers[1]["config"].get("containment").is_none());
    assert_eq!(servers[2]["valid"], false);
    assert!(servers[2]["problem"]["message"]
        .as_str()
        .unwrap()
        .ends_with("bad.containment must be \"contained\" or \"none\"."));
    // The frozen editable list (also the profile policy keys) is unchanged.
    assert!(!view["editable_keys"]
        .as_array()
        .unwrap()
        .contains(&json!("containment")));
    for bad in [r#""off""#, "true", "null", r#"["none"]"#] {
        f.write(format!(r#"{{"version":1,"containment":{bad},"servers":{{}}}}"#).as_bytes());
        let observation = f.reader().mcp_management_observation().unwrap();
        assert_eq!(observation["ok"], false, "{bad}");
        assert_eq!(observation["error"]["code"], "invalid_mcp_registry");
    }
    // Absent: no registry default is reported.
    f.write(br#"{"version":1,"servers":{}}"#);
    assert!(f
        .reader()
        .mcp_management()
        .unwrap()
        .get("containment")
        .is_none());
}
