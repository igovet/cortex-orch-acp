#[path = "support/hermetic.rs"]
mod hermetic;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
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
        let root = std::env::temp_dir().join(format!(
            "cw-rdr-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        Self(root)
    }
    fn directory(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        for directory in path
            .ancestors()
            .take_while(|p| p.starts_with(&self.0))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            fs::create_dir_all(directory).unwrap();
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).unwrap();
        }
        path
    }
    fn write(&self, name: &str, bytes: &[u8]) {
        let path = self.0.join(name);
        self.directory(Path::new(name).parent().unwrap().to_str().unwrap());
        fs::write(&path, bytes).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    fn json(&self, name: &str, value: &Value) {
        self.write(name, &serde_json::to_vec(value).unwrap());
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
    serde_json::from_str(include_str!("fixtures/config-readers-python.json")).unwrap()
}
fn snapshot(path: &Path) -> BTreeMap<PathBuf, (u32, Vec<u8>)> {
    let mut out = BTreeMap::new();
    for entry in fs::read_dir(path).unwrap() {
        let p = entry.unwrap().path();
        let m = fs::symlink_metadata(&p).unwrap();
        let data = if m.is_symlink() {
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
        out.insert(p.clone(), (m.permissions().mode(), data));
        if m.is_dir() {
            out.extend(snapshot(&p));
        }
    }
    out
}
fn prepare(case: &Value) -> (Fixture, Value) {
    let f = Fixture::new();
    let raw = f.expand(case["raw"].as_str().unwrap());
    f.write("config/cortex/connections.json", raw.as_bytes());
    f.directory("home/.codex");
    f.json(
        "home/.codex/sessions/2026/11/13/rollout.jsonl",
        &json!({"payload":{"rate_limits":case["limits"]}}),
    );
    f.json(
        "state/cortex/quota/cn_000000000001.json",
        &json!({"primary":{"used":99}}),
    );
    f.json("state/cortex/quota/cn_000000000002.json", &case["cache"]);
    f.json(
        "state/cortex/context_windows/cn_000000000001.json",
        &case["contexts"],
    );
    let input = serde_json::from_str(&f.expand(&case["input"].to_string())).unwrap();
    (f, input)
}
#[test]
fn native_connection_management_matches_the_recorded_oracle_reads() {
    let corpus = corpus();
    let cases = corpus["metadata"].as_array().unwrap();
    assert_eq!(cases.len(), 30);
    for case in cases {
        let f = Fixture::new();
        if let Some(raw) = case["raw"].as_str() {
            f.write("config/cortex/connections.json", raw.as_bytes());
        }
        let before = snapshot(&f.0);
        let actual = f
            .reader()
            .connection_management()
            .unwrap_or_else(|_| json!({"refused":true}));
        assert_eq!(actual, case["expected"], "{}", case["name"]);
        assert_eq!(before, snapshot(&f.0), "read mutated fixture");
        let text = actual.to_string();
        assert!(!text.contains("credential_ref"));
        assert!(!text.contains("/fixture"));
    }
}
#[test]
fn native_filesystem_readers_feed_complete_oracle_settings_envelopes() {
    let corpus = corpus();
    let cases = corpus["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 4);
    for case in cases {
        let (f, input) = prepare(case);
        let before = snapshot(&f.0);
        let actual = f
            .reader()
            .settings_with_readers(&input, 1800000000.0)
            .unwrap();
        assert!(actual.diagnostics.is_empty(), "{:?}", actual.diagnostics);
        let raw = fs::read(f.0.join("config/cortex/connections.json")).unwrap();
        let mut expected = case["expected"].clone();
        expected["tokens"]["connections"] = json!(format!("sha256:{:x}", Sha256::digest(raw)));
        assert_eq!(actual.view, expected, "{}", case["name"]);
        assert_eq!(before, snapshot(&f.0));
        assert!(input["surfaces"].get("connections").is_none());
    }
}
#[test]
fn unsafe_or_corrupt_metadata_refuses_without_repair_or_fallback() {
    for kind in [
        "symlink",
        "hardlink",
        "public",
        "directory",
        "fifo",
        "parent_symlink",
        "large",
    ] {
        let f = Fixture::new();
        let root = f.directory("config/cortex");
        let target = root.join("connections.json");
        match kind {
            "symlink" => {
                f.write("secret", b"{}");
                symlink(f.0.join("secret"), &target).unwrap();
            }
            "hardlink" => {
                f.write("secret", b"{}");
                fs::hard_link(f.0.join("secret"), &target).unwrap();
            }
            "public" => {
                f.write("config/cortex/connections.json", b"{}");
                fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
            }
            "directory" => {
                fs::create_dir(&target).unwrap();
            }
            "fifo" => {
                use std::ffi::CString;
                let p = CString::new(target.as_os_str().as_encoded_bytes()).unwrap();
                assert_eq!(unsafe { libc::mkfifo(p.as_ptr(), 0o600) }, 0);
            }
            "parent_symlink" => {
                f.directory("elsewhere");
                f.write("elsewhere/connections.json", b"{}");
                fs::remove_dir(&root).unwrap();
                symlink(f.0.join("elsewhere"), &root).unwrap();
            }
            "large" => {
                f.write(
                    "config/cortex/connections.json",
                    &vec![b' '; 4 * 1024 * 1024 + 1],
                );
            }
            _ => unreachable!(),
        }
        // Snapshot deliberately does not open a FIFO.
        assert!(f.reader().connection_management().is_err(), "{kind}");
        assert!(
            fs::symlink_metadata(&target).is_ok(),
            "{kind}: unexpectedly repaired"
        );
    }
}
#[test]
fn advisory_failures_remain_unknown_and_retained_without_cli_or_cache_writes() {
    let case = &corpus()["cases"][3];
    for kind in [
        "quota_corrupt",
        "quota_public",
        "context_corrupt",
        "noncanonical",
        "duplicate",
        "sessions_symlink",
        "native_symlink",
        "owned",
    ] {
        let (f, input) = prepare(case);
        let path = f.0.join("config/cortex/connections.json");
        let mut registry: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        match kind {
            "quota_corrupt" => f.write("state/cortex/quota/cn_000000000002.json", b"{"),
            "quota_public" => fs::set_permissions(
                f.0.join("state/cortex/quota/cn_000000000002.json"),
                fs::Permissions::from_mode(0o644),
            )
            .unwrap(),
            "context_corrupt" => {
                f.write("state/cortex/context_windows/cn_000000000001.json", b"[]")
            }
            "noncanonical" => {
                registry["connections"]["cn_000000000001"]["credential_ref"]["path"] =
                    json!(f.0.join("elsewhere"));
                f.json("config/cortex/connections.json", &registry);
            }
            "duplicate" => {
                registry["connections"]["cn_000000000002"]["credential_ref"] =
                    registry["connections"]["cn_000000000001"]["credential_ref"].clone();
                f.json("config/cortex/connections.json", &registry);
            }
            "sessions_symlink" => {
                fs::rename(f.0.join("home/.codex/sessions"), f.0.join("elsewhere")).unwrap();
                symlink(f.0.join("elsewhere"), f.0.join("home/.codex/sessions")).unwrap();
            }
            "native_symlink" => {
                fs::rename(f.0.join("home/.codex"), f.0.join("elsewhere")).unwrap();
                symlink(f.0.join("elsewhere"), f.0.join("home/.codex")).unwrap();
            }
            "owned" => {
                let new = f.directory("state/cortex/connections/cn_000000000001/codex-home");
                fs::rename(f.0.join("home/.codex/sessions"), new.join("sessions")).unwrap();
                registry["connections"]["cn_000000000001"]["credential_ref"] =
                    json!({"kind":"engine_native_dir","owned":true,"path":new});
                f.json("config/cortex/connections.json", &registry);
            }
            _ => unreachable!(),
        }
        let before = snapshot(&f.0);
        let actual = f
            .reader()
            .settings_with_readers(&input, 1800000000.0)
            .unwrap();
        let usage = actual.view["usage"].as_array().unwrap();
        let engine = if kind.starts_with("quota") {
            "claude"
        } else {
            "codex"
        };
        let observed = usage.iter().find(|v| v["engine"] == engine).unwrap();
        if kind == "context_corrupt" {
            // Non-object cache is unknown (not a fabricated context size).
            assert!(actual.view["config"]["catalogue"]["codex-acp"]["models"][0]
                .get("contextWindow")
                .is_none());
        } else if kind == "owned" {
            assert_eq!(observed["usedPercent"], 56.0);
        } else {
            assert!(observed["usedPercent"].is_null(), "{kind}");
        }
        if [
            "quota_corrupt",
            "quota_public",
            "noncanonical",
            "duplicate",
            "native_symlink",
        ]
        .contains(&kind)
        {
            assert!(!actual.diagnostics.is_empty(), "{kind}");
        }
        assert_eq!(before, snapshot(&f.0), "{kind}");
    }
}
#[test]
fn invalid_entry_bindings_and_clock_refuse_before_advisory_reads() {
    let (f, mut input) = prepare(&corpus()["cases"][0]);
    let reader = f.reader();
    assert!(reader.settings_with_readers(&input, f64::NAN).is_err());
    input["entries"][0]["connection_id"] = json!("../../escape");
    assert!(reader.settings_with_readers(&input, 1800000000.0).is_err());
    assert!(reader
        .settings_with_readers(&json!({}), 1800000000.0)
        .is_err());
}
#[test]
fn rollout_selection_is_bounded_and_never_substitutes_stale_cache() {
    for kind in ["expired", "two_days", "tail", "file_symlink", "missing"] {
        let (f, input) = prepare(&corpus()["cases"][0]);
        let rollout = "home/.codex/sessions/2026/11/13/rollout.jsonl";
        match kind {
            "expired" => f.json(
                rollout,
                &json!({"payload":{"rate_limits":{"secondary":{
                "used_percent":88,"resets_at":1800000000,"window_minutes":10080}}}}),
            ),
            "two_days" => {
                f.write("home/.codex/sessions/2026/11/14/empty.jsonl", b"{}");
                f.write("home/.codex/sessions/2026/11/15/empty.jsonl", b"{}");
            }
            "tail" => {
                let line = fs::read(f.0.join(rollout)).unwrap();
                let mut bytes = vec![b'x'; 300 * 1024];
                bytes.push(b'\n');
                bytes.extend(line);
                f.write(rollout, &bytes);
            }
            "file_symlink" => {
                fs::rename(f.0.join(rollout), f.0.join("elsewhere")).unwrap();
                symlink(f.0.join("elsewhere"), f.0.join(rollout)).unwrap();
            }
            "missing" => {
                fs::remove_dir_all(f.0.join("state")).unwrap();
                fs::remove_dir_all(f.0.join("home/.codex/sessions")).unwrap();
            }
            _ => unreachable!(),
        }
        let before = snapshot(&f.0);
        let read = f
            .reader()
            .settings_with_readers(&input, 1800000000.0)
            .unwrap();
        let usage = read.view["usage"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["engine"] == "codex")
            .unwrap();
        if kind == "tail" {
            assert_eq!(usage["usedPercent"], 53.0);
        } else {
            assert!(usage["usedPercent"].is_null(), "{kind}");
        }
        assert_eq!(before, snapshot(&f.0), "{kind}");
    }
}
#[test]
fn reader_fixture_sources_are_current() {
    let fixture = corpus();
    assert_eq!(fixture["evidence"]["process_attempts"], 0);
    assert_eq!(fixture["evidence"]["real_engine_calls"], 0);
}
