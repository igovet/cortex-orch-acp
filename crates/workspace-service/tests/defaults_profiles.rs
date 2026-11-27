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
        let p = std::env::temp_dir().join(format!(
            "cw-dp-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o700)).unwrap();
        Self(p)
    }
    fn directory(&self, p: &Path) {
        for p in p
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
    fn write(&self, name: &str, raw: &[u8]) -> PathBuf {
        let p = self.0.join(name);
        self.directory(p.parent().unwrap());
        fs::write(&p, raw).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
        p
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
    fn install(&self, case: &Value) {
        // Match capture's insertion-order mtimes for older/newer snapshot semantics.
        let mut files = case["files"]
            .as_object()
            .unwrap()
            .iter()
            .collect::<Vec<_>>();
        files.sort_by_key(|(p, _)| *p);
        for (i, (p, v)) in files.into_iter().enumerate() {
            let p = self.write(p, v.as_str().unwrap().as_bytes());
            let t = UNIX_EPOCH + Duration::from_secs(1800000000 + i as u64);
            fs::File::open(p)
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(t))
                .unwrap();
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn corpus() -> Value {
    serde_json::from_str(include_str!("fixtures/defaults-profiles-python.json")).unwrap()
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
fn defaults_profiles_and_composed_settings_match_the_recorded_oracle() {
    let c = corpus();
    assert_eq!(c["cases"].as_array().unwrap().len(), 65);
    for case in c["cases"].as_array().unwrap() {
        let f = Fixture::new();
        f.install(case);
        let root = f.0.join("workspace");
        let path = root.join(".cortex/routing.json");
        let project = (case["project"] == true).then_some((root.as_path(), path.as_path()));
        let r = f.reader(case["override"] == true);
        let before = snapshot(&f.0);
        let expected = f.expand(&case["native_expected"]);
        assert_eq!(
            r.user_defaults_management_observation().unwrap(),
            expected["user_defaults"],
            "defaults: {}",
            case["name"]
        );
        assert_eq!(
            r.profiles_management_observation(project).unwrap(),
            expected["routing_profiles"],
            "profiles: {}",
            case["name"]
        );
        let mut input = f.expand(&case["input"]);
        for key in [
            "user_defaults",
            "routing",
            "routing_profiles",
            "mcp_servers",
            "connections",
        ] {
            input["surfaces"][key] = json!({"invalid":"must be replaced by real IO"});
        }
        let settings = r
            .settings_with_management_readers(&input, 1800000000.0, project)
            .unwrap();
        if case["settings_error"].is_null() {
            assert_eq!(
                settings.view,
                f.expand(&case["settings"]),
                "compositor: {}",
                case["name"]
            );
        } else {
            // Retained Python compositor crashes on malformed parameter scalars;
            // the independent management reads above still compare, not a Python pass.
            assert_eq!(case["settings_error"], "AttributeError");
            assert_eq!(
                expected["user_defaults"]["result"]["coordinator_resolution"]["state"],
                "unavailable"
            );
        }
        let registry: Value = case["files"]["config/cortex/connections.json"]
            .as_str()
            .map(|s| serde_json::from_str(s).unwrap())
            .unwrap_or(json!({"connections":{}}));
        let diagnostics:Vec<_>=registry["connections"].as_object().unwrap().keys().map(|id|json!({"connection_id":id,"source":"codex_rollout","code":"native_directory_unavailable"})).collect();
        assert_eq!(
            settings.diagnostics, diagnostics,
            "advisory diagnostics: {}",
            case["name"]
        );
        assert_eq!(snapshot(&f.0), before, "mutation: {}", case["name"]);
        assert!(!settings.view.to_string().contains("fixture-secret"));
    }
}
#[test]
fn fixture_provenance_and_only_declared_diagnostic_deltas() {
    let c = corpus();
    assert_eq!(c["evidence"]["process_attempts"], 0);
    assert_eq!(c["evidence"]["real_engine_calls"], 0);
    assert_eq!(c["evidence"]["fake_executables"], true);
    for case in c["cases"].as_array().unwrap() {
        let mut original = case["expected"].clone();
        for env in original.as_object_mut().unwrap().values_mut() {
            if env["ok"] == false
                && env["error"]["code"] == "invalid_profiles"
                && !env["error"]["message"]
                    .as_str()
                    .unwrap()
                    .starts_with("routing-profiles.json is not valid")
                && !env["error"]["message"]
                    .as_str()
                    .unwrap()
                    .starts_with("routing-profiles.json must contain")
            {
                env["error"]["message"] = json!(
                    "Routing profiles contain invalid metadata or settings; repair them first."
                );
                env["error"]["field"] = Value::Null;
            }
            if env["ok"] == true && env["result"]["surface"] == "routing_profiles" {
                for problem in env["result"]["problems"].as_array_mut().unwrap() {
                    if problem["code"] == "invalid_configuration" {
                        problem["message"]=json!("The current section cannot be represented as a profile; repair it first.");
                    }
                }
            }
        }
        assert_eq!(
            original, case["native_expected"],
            "undeclared delta: {}",
            case["name"]
        );
    }
}
#[test]
fn unsafe_defaults_and_profile_files_refuse_without_repairs() {
    for name in ["defaults.json", "routing-profiles.json"] {
        for kind in ["mode", "link", "hardlink", "fifo", "oversize", "directory"] {
            let f = Fixture::new();
            let p = f.write(&format!("config/cortex/{name}"), b"{}");
            match kind {
                "mode" => fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap(),
                "link" => {
                    fs::remove_file(&p).unwrap();
                    symlink(f.write("target", b"{}"), &p).unwrap();
                }
                "hardlink" => fs::hard_link(&p, f.0.join("alias")).unwrap(),
                "fifo" => {
                    fs::remove_file(&p).unwrap();
                    let p = std::ffi::CString::new(p.as_os_str().as_encoded_bytes()).unwrap();
                    assert_eq!(unsafe { libc::mkfifo(p.as_ptr(), 0o600) }, 0);
                }
                "oversize" => fs::OpenOptions::new()
                    .write(true)
                    .open(&p)
                    .unwrap()
                    .set_len(4 * 1024 * 1024 + 1)
                    .unwrap(),
                "directory" => {
                    fs::remove_file(&p).unwrap();
                    fs::create_dir(&p).unwrap();
                }
                _ => unreachable!(),
            }
            let r = f.reader(false);
            let result = if name == "defaults.json" {
                r.user_defaults_management_observation()
            } else {
                r.profiles_management_observation(None)
            };
            assert!(result.is_err(), "{name} {kind}");
            if kind == "mode" {
                assert_eq!(fs::metadata(p).unwrap().permissions().mode() & 0o777, 0o644);
            }
        }
    }
}
#[test]
fn unsafe_catalog_is_never_a_bound_coordinator_and_profiles_do_not_load_it() {
    let c = corpus();
    for kind in [
        "malformed",
        "symlink",
        "public-directory",
        "wrong-binding",
        "invalid-evidence",
    ] {
        let case = c["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "coordinator-bound")
            .unwrap();
        let f = Fixture::new();
        f.install(case);
        let name = "state/cortex/catalog/cn_000000000001/cs_000000000001.json";
        let p = f.0.join(name);
        match kind {
            "malformed" => {
                f.write(name, b"{");
            }
            "symlink" => {
                fs::remove_file(&p).unwrap();
                symlink(f.write("outside-catalog", b"{}"), &p).unwrap();
            }
            "public-directory" => {
                fs::set_permissions(p.parent().unwrap(), fs::Permissions::from_mode(0o755)).unwrap()
            }
            "wrong-binding" | "invalid-evidence" => {
                let mut v: Value = serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
                if kind == "wrong-binding" {
                    v["entries"][0]["route"]["connection_id"] = json!("cn_ffffffffffff");
                } else {
                    v["entries"][0]["evidence"]["catalog_obtained"]["adapter_version"] =
                        Value::Null;
                }
                f.write(name, v.to_string().as_bytes());
            }
            _ => unreachable!(),
        }
        // Python read_user_defaults: the snapshot's refusal makes the binding
        // unavailable with that reason; the defaults read itself still succeeds.
        let v = f
            .reader(false)
            .user_defaults_management_observation()
            .unwrap_or_else(|e| panic!("{kind}: {e:?}"));
        let resolution = &v["result"]["coordinator_resolution"];
        assert_eq!(resolution["state"], "unavailable", "{kind}");
        assert!(
            resolution["error"]["code"]
                .as_str()
                .is_some_and(|c| !c.is_empty()),
            "{kind}"
        );
        // Profile status does not depend on unrelated catalogs or coordinator binding.
        let v = f
            .reader(false)
            .profiles_management_observation(None)
            .unwrap();
        assert_eq!(v["result"]["state"], "none");
    }
}
#[test]
fn missing_roots_read_without_creating_and_unsafe_section_is_not_in_sync() {
    let f = Fixture::new();
    let before = snapshot(&f.0);
    assert_eq!(
        f.reader(false)
            .profiles_management_observation(None)
            .unwrap()["result"]["state"],
        "none"
    );
    assert_eq!(snapshot(&f.0), before);
    f.write("config/cortex/routing-profiles.json",br#"{"format":2,"active":"a","profiles":{"a":{"description":"","sections":["user_defaults"],"settings":{"user_defaults":{"defaults":{},"ceiling":{}}}}}}"#);
    let p = f.write("config/cortex/defaults.json", b"{}");
    fs::set_permissions(p, fs::Permissions::from_mode(0o644)).unwrap();
    let v = f
        .reader(false)
        .profiles_management_observation(None)
        .unwrap();
    assert_eq!(v["result"]["state"], "unavailable");
    assert_eq!(
        v["result"]["current_revisions"]["user_defaults"],
        Value::Null
    );
    assert_eq!(v["result"]["problems"][0]["code"], "unsafe_storage");
}
