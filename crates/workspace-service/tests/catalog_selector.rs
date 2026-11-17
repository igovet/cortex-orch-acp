#[path = "support/hermetic.rs"]
mod hermetic;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::{symlink, PermissionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, UNIX_EPOCH},
};
use workspace_service::{config_read_model::ConfigurationReader, native_routes::RouteEnvironment};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let p = PathBuf::from(format!(
            "/tmp/cw-cs-{}-{}",
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
        fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
        p
    }
    fn install(&self, case: &Value) {
        for (i, (name, raw)) in case["files"].as_object().unwrap().iter().enumerate() {
            let p = self.write(name, raw.as_str().unwrap().as_bytes());
            fs::File::open(p)
                .unwrap()
                .set_times(
                    fs::FileTimes::new()
                        .set_modified(UNIX_EPOCH + Duration::from_secs(1800000000 + i as u64)),
                )
                .unwrap();
        }
    }
    fn reader(&self, override_: bool) -> ConfigurationReader {
        ConfigurationReader::new(RouteEnvironment {
            home: self.0.join("home"),
            config: self.0.join("config"),
            state: self.0.join("state"),
            routing_override: override_.then(|| self.0.join("override.json")),
        })
        .unwrap()
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
    serde_json::from_str(include_str!("fixtures/catalog-selector-python.json")).unwrap()
}
fn snapshot(root: &Path) -> BTreeMap<PathBuf, (u32, Vec<u8>)> {
    let mut out = BTreeMap::new();
    for e in fs::read_dir(root).unwrap() {
        let p = e.unwrap().path();
        let m = fs::symlink_metadata(&p).unwrap();
        let b = if m.is_symlink() {
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
        out.insert(p.clone(), (m.permissions().mode(), b));
        if m.is_dir() {
            out.extend(snapshot(&p));
        }
    }
    out
}
#[test]
fn catalog_selector_and_composed_settings_match_the_oracle() {
    let c = corpus();
    assert_eq!(c["cases"].as_array().unwrap().len(), 68);
    for case in c["cases"].as_array().unwrap() {
        let f = Fixture::new();
        f.install(case);
        let root = f.0.join("workspace");
        let path = root.join(".cortex/routing.json");
        let project = (case["project"] == true).then_some((root.as_path(), path.as_path()));
        let r = f.reader(case["override"] == true);
        let before = snapshot(&f.0);
        // The native selector now makes policy-disabled routes unselectable too.
        // Keep the rest of the historical Python observation byte-for-byte.
        let mut expected = f.expand(&case["expected"]);
        for route in expected["routes"]["routes"].as_array_mut().unwrap() {
            route["selectable"] = json!(route["selectable"] == true && route["disabled"] != true);
        }
        assert_eq!(
            r.catalog_selector(project).unwrap(),
            expected,
            "catalog: {}",
            case["name"]
        );
        let input = json!({"surfaces":f.expand(&case["extra_surfaces"]),"workspace":{},"discovery":null,
            "routes":{"routes":[{"engine":"forged"}]},"entries":[{"connection_id":"forged"}]});
        let actual = r
            .settings_with_catalog_readers(&input, 1800000000.0, project)
            .unwrap();
        assert_eq!(
            actual.view,
            f.expand(&case["settings"]),
            "settings: {}",
            case["name"]
        );
        assert_eq!(before, snapshot(&f.0), "read effects: {}", case["name"]);
    }
}
#[test]
fn captured_sources_and_explicit_effect_deltas_are_retained() {
    let c = corpus();
    assert_eq!(c["evidence"]["process_attempts"], 0);
    assert_eq!(c["evidence"]["real_engine_calls"], 0);
    assert_eq!(c["evidence"]["deltas"].as_array().unwrap().len(), 2);
}
#[test]
fn disabled_engine_routes_are_not_selectable() {
    let c = corpus();
    let case = c["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == "stored-configured")
        .unwrap();
    let f = Fixture::new();
    f.install(case);
    let reader = f.reader(false);
    let before = reader.catalog_selector(None).unwrap();
    let engine = before["routes"]["routes"][0]["engine"].as_str().unwrap();
    let path = "config/cortex/routing.json";
    let mut rules: Value = serde_json::from_slice(&fs::read(f.0.join(path)).unwrap()).unwrap();
    rules["deny"] = json!([format!("{engine}/*")]);
    f.write(path, &serde_json::to_vec(&rules).unwrap());
    let after = reader.catalog_selector(None).unwrap();
    let routes = after["routes"]["routes"].as_array().unwrap();
    assert!(!routes.is_empty());
    for route in routes.iter().filter(|r| r["engine"] == engine) {
        assert_eq!(route["selectable"], false);
        assert_eq!(route["in_selector"], false);
    }
}

#[test]
fn unsafe_or_unbound_catalog_degrades_only_its_connection_without_repair() {
    let c = corpus();
    let base = c["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == "stored-configured")
        .unwrap();
    let name = "state/cortex/catalog/cn_000000000001/cs_000000000001.json";
    for variant in [
        "malformed",
        "public",
        "symlink",
        "hardlink",
        "directory",
        "fifo",
        "oversize",
        "wrong-snapshot",
        "wrong-engine",
        "wrong-connection",
        "wrong-route-id",
        "bad-evidence",
        "missing-time",
        "bad-filename",
        "public-parent",
        "wrong-entry-snapshot",
        "ambiguous-mtime",
    ] {
        let f = Fixture::new();
        f.install(base);
        let p = f.0.join(name);
        match variant {
            "malformed" => {
                f.write(name, b"{");
            }
            "public" => fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap(),
            "symlink" => {
                fs::remove_file(&p).unwrap();
                symlink("/never-follow", &p).unwrap();
            }
            "hardlink" => fs::hard_link(&p, f.0.join("linked")).unwrap(),
            "directory" => {
                fs::remove_file(&p).unwrap();
                fs::create_dir(&p).unwrap();
            }
            "fifo" => {
                fs::remove_file(&p).unwrap();
                let c = std::ffi::CString::new(p.as_os_str().as_encoded_bytes()).unwrap();
                assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
            }
            "oversize" => {
                fs::OpenOptions::new()
                    .write(true)
                    .open(&p)
                    .unwrap()
                    .set_len(8 * 1024 * 1024 + 1)
                    .unwrap();
            }
            "public-parent" => {
                fs::set_permissions(p.parent().unwrap(), fs::Permissions::from_mode(0o755)).unwrap()
            }
            "bad-filename" => fs::rename(&p, p.with_file_name("cs_invalid.json")).unwrap(),
            "ambiguous-mtime" => {
                let mut d: Value = serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
                d["snapshot_id"] = json!("cs_000000000002");
                d["entries"][0]["catalog_snapshot_id"] = d["snapshot_id"].clone();
                let p2 = f.write(
                    "state/cortex/catalog/cn_000000000001/cs_000000000002.json",
                    &serde_json::to_vec(&d).unwrap(),
                );
                let t = fs::metadata(&p).unwrap().modified().unwrap();
                fs::File::open(&p2)
                    .unwrap()
                    .set_times(fs::FileTimes::new().set_modified(t))
                    .unwrap();
            }
            other => {
                let mut d: Value = serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
                match other {
                    "wrong-snapshot" => d["snapshot_id"] = json!("cs_999999999999"),
                    "wrong-entry-snapshot" => {
                        d["entries"][0]["catalog_snapshot_id"] = json!("cs_999999999999")
                    }
                    "wrong-engine" => d["entries"][0]["route"]["engine"] = json!("opencode"),
                    "wrong-connection" => {
                        d["entries"][0]["connection_id"] = json!("cn_999999999999")
                    }
                    "wrong-route-id" => d["entries"][0]["route"]["id"] = json!("xr_999999999999"),
                    "bad-evidence" => {
                        d["entries"][0]["evidence"]["catalog_obtained"]["source"] =
                            json!("invented")
                    }
                    "missing-time" => {
                        d.as_object_mut().unwrap().remove("created_at");
                    }
                    _ => unreachable!(),
                }
                f.write(name, &serde_json::to_vec(&d).unwrap());
            }
        }
        // Avoid opening the FIFO in the byte-snapshot helper.
        let before = if variant == "fifo" {
            None
        } else {
            Some(snapshot(&f.0))
        };
        let r = f.reader(false);
        // Python read_route_catalog parity: the refused snapshot is that
        // connection's own catalog problem, never a refused settings read.
        let catalog = r
            .catalog_selector(None)
            .unwrap_or_else(|e| panic!("{variant}: {e:?}"));
        let problems = catalog["routes"]["catalog_problems"].as_array().unwrap();
        assert_eq!(problems.len(), 1, "{variant}");
        assert_eq!(problems[0]["connection_id"], "cn_000000000001", "{variant}");
        for key in ["code", "message"] {
            assert!(
                problems[0][key].as_str().is_some_and(|s| !s.is_empty()),
                "{variant}: {key}"
            );
        }
        assert!(
            catalog["routes"]["routes"].as_array().unwrap().is_empty(),
            "{variant}"
        );
        assert!(
            catalog["entries"].as_array().unwrap().is_empty(),
            "{variant}"
        );
        let view = r
            .settings_with_catalog_readers(
                &json!({"surfaces":{},"workspace":{}}),
                1800000000.0,
                None,
            )
            .unwrap()
            .view;
        assert_eq!(
            view["config"]["catalogue"]["codex-acp"]["failed"], true,
            "{variant}"
        );
        assert!(view["config"]["catalogue"]["codex-acp"]["models"]
            .as_array()
            .unwrap()
            .is_empty());
        if let Some(before) = before {
            assert_eq!(before, snapshot(&f.0), "{variant}");
        }
    }
}
#[test]
fn missing_roots_stay_absent_and_supplied_selectability_is_not_used() {
    let f = Fixture::new();
    let before = snapshot(&f.0);
    let r = f.reader(false);
    assert!(r.catalog_selector(None).unwrap()["entries"]
        .as_array()
        .unwrap()
        .is_empty());
    let view = r
        .settings_with_catalog_readers(
            &json!({"surfaces":{},"workspace":{},"entries":[{"route_id":"forged"}],
        "routes":{"routes":[{"selectable":true,"engine":"codex-acp","model":"forged"}]}}),
            1800000000.0,
            None,
        )
        .unwrap()
        .view;
    assert!(view["config"]["catalogue"]["codex-acp"]["models"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(before, snapshot(&f.0));
}
