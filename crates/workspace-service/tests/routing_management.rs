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
        let path = std::env::temp_dir().join(format!(
            "cw-route-{}-{}",
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
    fn write(&self, name: &str, raw: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        self.directory(
            path.parent()
                .unwrap()
                .strip_prefix(&self.0)
                .unwrap()
                .to_str()
                .unwrap(),
        );
        fs::write(&path, raw).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        path
    }
    fn reader(&self, env: bool) -> ConfigurationReader {
        ConfigurationReader::new(RouteEnvironment {
            home: self.0.join("home"),
            config: self.0.join("config"),
            state: self.0.join("state"),
            routing_override: env.then(|| self.0.join("override.json")),
        })
        .unwrap()
    }
    fn expand(&self, v: &Value) -> Value {
        serde_json::from_str(&v.to_string().replace("@ROOT@", self.0.to_str().unwrap())).unwrap()
    }
    fn install(&self, case: &Value) {
        for (p, v) in case["files"].as_object().unwrap() {
            self.write(
                p,
                v.as_str()
                    .unwrap()
                    .replace("@ROOT@", self.0.to_str().unwrap())
                    .as_bytes(),
            );
            if let Some(seconds) = case["mtimes"][p].as_u64() {
                let time = UNIX_EPOCH + Duration::from_secs(seconds);
                fs::File::open(self.0.join(p))
                    .unwrap()
                    .set_times(fs::FileTimes::new().set_modified(time))
                    .unwrap();
            }
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn corpus() -> Value {
    serde_json::from_str(include_str!("fixtures/routing-management-python.json")).unwrap()
}
fn snapshot(root: &Path) -> BTreeMap<PathBuf, (u32, Vec<u8>)> {
    let mut out = BTreeMap::new();
    for e in fs::read_dir(root).unwrap() {
        let p = e.unwrap().path();
        let m = fs::symlink_metadata(&p).unwrap();
        let bytes = if m.is_file() {
            fs::read(&p).unwrap()
        } else if m.is_symlink() {
            fs::read_link(&p)
                .unwrap()
                .as_os_str()
                .as_encoded_bytes()
                .to_vec()
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
fn routing_sources_and_complete_settings_match_the_frozen_oracle() {
    let c = corpus();
    assert_eq!(c["cases"].as_array().unwrap().len(), 31);
    assert_eq!(c["compositions"].as_array().unwrap().len(), 31);
    for case in c["compositions"].as_array().unwrap() {
        let f = Fixture::new();
        f.install(case);
        let root = f.0.join("workspace");
        let path = root.join(".cortex/routing.json");
        let project = (case["project"] == true).then_some((root.as_path(), path.as_path()));
        let r = f.reader(case["override"] == true);
        let before = snapshot(&f.0);
        let expected = f.expand(&case["native_expected"]);
        assert_eq!(
            r.routing_management(project).unwrap(),
            expected,
            "{}",
            case["name"]
        );
        let mut observation = f.expand(&case["input"]);
        observation["surfaces"]["routing"] = json!({"invalid":"must not be trusted"});
        let settings = r
            .settings_with_routing_readers(&observation, 1800000000.0, project)
            .unwrap();
        assert_eq!(
            settings.view,
            f.expand(&case["settings"]),
            "{}",
            case["name"]
        );
        assert!(settings.diagnostics.is_empty());
        assert_eq!(snapshot(&f.0), before, "mutation: {}", case["name"]);
    }
}
/// A pure alias entry (the Claude adapter's `default`) names no concrete model, so the native
/// reader reports no observed efforts for it; the rest of the frozen observation is kept.
fn without_alias(mut v: Value) -> Value {
    fn walk(v: &mut Value) {
        match v {
            Value::Object(o) => {
                if let Some(Value::Object(engines)) = o.get_mut("model_efforts") {
                    for models in engines.values_mut() {
                        if let Some(m) = models.as_object_mut() {
                            m.retain(|k, _| !k.eq_ignore_ascii_case("default"));
                        }
                    }
                    engines.retain(|_, m| m.as_object().is_none_or(|m| !m.is_empty()));
                }
                o.values_mut().for_each(walk);
            }
            Value::Array(a) => a.iter_mut().for_each(walk),
            _ => {}
        }
    }
    walk(&mut v);
    v
}
#[test]
fn observed_efforts_and_placeholder_fallback_match_the_oracle() {
    let c = corpus();
    assert_eq!(c["catalogs"].as_array().unwrap().len(), 20);
    for case in c["catalogs"].as_array().unwrap() {
        let f = Fixture::new();
        f.install(case);
        let before = snapshot(&f.0);
        assert_eq!(
            f.reader(false).routing_management(None).unwrap(),
            without_alias(f.expand(&case["expected"])),
            "{}",
            case["name"]
        );
        assert_eq!(snapshot(&f.0), before);
    }
}
#[test]
fn provenance_and_explicit_diagnostic_deltas_are_retained() {
    let c = corpus();
    assert_eq!(c["evidence"]["process_attempts"], 0);
    assert_eq!(c["evidence"]["disposable_home"], true);
    let mut changed = 0;
    for case in c["cases"].as_array().unwrap() {
        let mut original = case["expected"].clone();
        let native = &case["native_expected"];
        for (a, b) in original["sources"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .zip(native["sources"].as_array().unwrap())
        {
            if a != b {
                assert_eq!(a["error"]["code"], "invalid_routing_overrides");
                assert!(
                    b["error"]["message"] == "Saved routing has an invalid shape."
                        || b["error"]["message"] == "Saved routing is not valid UTF-8 JSON."
                );
                a["error"]["message"] = b["error"]["message"].clone();
                changed += 1;
            }
        }
        assert_eq!(original, *native);
    }
    assert_eq!(changed, 9);
}
#[test]
fn unsafe_routing_sources_do_not_expose_targets_or_merge_lower_rules() {
    for kind in ["symlink", "directory", "fifo", "oversize", "parent-symlink"] {
        let f = Fixture::new();
        let user = f.write(
            "config/cortex/routing.json",
            br#"{"model_policy_version":2,"deny":["user"]}"#,
        );
        let project = f.write("workspace/.cortex/routing.json", b"{}");
        match kind {
            "symlink" => {
                fs::remove_file(&project).unwrap();
                symlink(&user, &project).unwrap();
            }
            "directory" => {
                fs::remove_file(&project).unwrap();
                fs::create_dir(&project).unwrap();
            }
            "fifo" => {
                fs::remove_file(&project).unwrap();
                let p = std::ffi::CString::new(project.as_os_str().as_encoded_bytes()).unwrap();
                assert_eq!(unsafe { libc::mkfifo(p.as_ptr(), 0o600) }, 0);
            }
            "oversize" => fs::OpenOptions::new()
                .write(true)
                .open(&project)
                .unwrap()
                .set_len(workspace_protocol::FRAME_BYTES as u64 + 1)
                .unwrap(),
            "parent-symlink" => {
                fs::rename(f.0.join("workspace/.cortex"), f.0.join("target")).unwrap();
                symlink(f.0.join("target"), f.0.join("workspace/.cortex")).unwrap();
            }
            _ => unreachable!(),
        }
        let root = f.0.join("workspace");
        let r = f.reader(false);
        let before = if kind == "fifo" {
            None
        } else {
            Some(snapshot(&f.0))
        };
        let got = r.routing_management(Some((&root, &project)));
        if kind == "oversize" || kind == "parent-symlink" {
            assert!(got.is_err(), "{kind}");
        } else {
            let got = got.unwrap();
            if kind == "symlink" {
                assert_eq!(got["effective_source"], "project");
                assert_eq!(got["effective_valid"], false);
                assert_eq!(got["effective_rules"], Value::Null);
                assert!(!got["sources"][0].to_string().contains("user"));
            } else {
                assert_eq!(got["effective_source"], "user");
                assert_eq!(got["effective_rules"]["deny"], json!(["user"]));
            }
        }
        if let Some(before) = before {
            assert_eq!(snapshot(&f.0), before);
        }
    }
}
#[test]
fn malformed_or_unsafe_catalogs_remain_unknown_without_fallback() {
    let c = corpus();
    let case = &c["catalogs"][0];
    for kind in [
        "invalid-json",
        "fingerprint",
        "bad-evidence",
        "directory-link",
        "public-directory",
        "hardlink",
        "oversize",
    ] {
        let f = Fixture::new();
        f.install(case);
        let p =
            f.0.join("state/cortex/catalog/cn_000000000001/cs_000000000001.json");
        match kind {
            "invalid-json" => {
                fs::write(&p, "{").unwrap();
            }
            "fingerprint" | "bad-evidence" => {
                let mut v: Value = serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
                if kind == "fingerprint" {
                    v["entries"][0]["route"]["id"] = json!("xr_forged");
                } else {
                    v["entries"][0]["evidence"]["catalog_obtained"]["source"] = json!("manual");
                }
                fs::write(&p, serde_json::to_vec(&v).unwrap()).unwrap();
            }
            "directory-link" => {
                let dir = p.parent().unwrap();
                fs::rename(dir, f.0.join("private-target")).unwrap();
                symlink(f.0.join("private-target"), dir).unwrap();
            }
            "public-directory" => {
                fs::set_permissions(p.parent().unwrap(), fs::Permissions::from_mode(0o755)).unwrap()
            }
            "hardlink" => fs::hard_link(&p, f.0.join("linked-snapshot")).unwrap(),
            "oversize" => fs::OpenOptions::new()
                .write(true)
                .open(&p)
                .unwrap()
                .set_len(4 * 1024 * 1024 + 1)
                .unwrap(),
            _ => unreachable!(),
        }
        let before = snapshot(&f.0);
        // Python _routing_model_efforts: a damaged connection catalog only
        // withholds that connection's levels; an unsafe directory is never read.
        let got = f.reader(false).routing_management(None);
        assert_eq!(
            got.unwrap_or_else(|e| panic!("{kind}: {e:?}"))["builtin"]["model_efforts"],
            json!({}),
            "{kind}"
        );
        assert_eq!(snapshot(&f.0), before);
    }
}
#[test]
fn explicit_channel_path_and_invalid_roots_do_not_consult_process_environment() {
    let f = Fixture::new();
    let release = f.write(
        "workspace/.cortex/routing.json",
        br#"{"model_policy_version":2,"deny":["release"]}"#,
    );
    let dev = f.write(
        "workspace/.cortex-dev/routing.json",
        br#"{"model_policy_version":2,"deny":["dev"]}"#,
    );
    let r = f.reader(false);
    let root = f.0.join("workspace");
    assert_eq!(
        r.routing_management(Some((&root, &dev))).unwrap()["effective_rules"]["deny"],
        json!(["dev"])
    );
    assert_eq!(
        r.routing_management(Some((&root, &release))).unwrap()["effective_rules"]["deny"],
        json!(["release"])
    );
    assert!(r
        .routing_management(Some((Path::new("relative"), &dev)))
        .is_err());
    assert!(r
        .routing_management(Some((&root, Path::new("relative"))))
        .is_err());
}
