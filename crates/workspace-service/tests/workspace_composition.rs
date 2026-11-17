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
        let p = PathBuf::from(format!(
            "/tmp/cw-wc-{}-{}",
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
    fn reader(&self) -> ConfigurationReader {
        ConfigurationReader::new(RouteEnvironment {
            home: self.0.join("home"),
            config: self.0.join("config"),
            state: self.0.join("state"),
            routing_override: None,
        })
        .unwrap()
    }
    fn expand(&self, v: &Value) -> Value {
        serde_json::from_str(&v.to_string().replace("@ROOT@", self.0.to_str().unwrap())).unwrap()
    }
    fn composed(
        &self,
        state: &Path,
    ) -> cortex_core::CoreResult<workspace_service::config_read_model::SettingsRead> {
        // None of these forged observations may replace the file producers.
        self.reader().settings_with_workspace_readers(
            &json!({"workspace":{"mode":"forged","checkout":"forged"},"surfaces":{},
                "entries":[{"connection_id":"forged"}],"routes":{"routes":[{"engine":"forged"}]},"discovery":null}),
            1800000000.0,None,state)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn corpus() -> Value {
    serde_json::from_str(include_str!("fixtures/workspace-composition-python.json")).unwrap()
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
fn workspace_and_full_settings_match_the_oracle() {
    let c = corpus();
    assert_eq!(c["cases"].as_array().unwrap().len(), 34);
    let mut complete = 0;
    let mut refused = 0;
    for case in c["cases"].as_array().unwrap() {
        let f = Fixture::new();
        for (name, raw) in case["files"].as_object().unwrap() {
            f.write(name, raw.as_str().unwrap().as_bytes());
        }
        let before = snapshot(&f.0);
        let state = f.0.join("workspace");
        let expected = &case["expected"];
        if expected.get("error").is_some() {
            refused += 1;
            assert!(
                f.reader().workspace_defaults(&state).is_err(),
                "{}",
                case["name"]
            );
            assert!(f.composed(&state).is_err(), "{}", case["name"]);
        } else {
            complete += 1;
            assert_eq!(
                f.reader().workspace_defaults(&state).unwrap(),
                expected["workspace"],
                "{}",
                case["name"]
            );
            let actual = f.composed(&state).unwrap();
            let diagnostics = if case["name"].as_str().unwrap().ends_with("-profile") {
                vec![
                    json!({"connection_id":"cn_000000000001","source":"codex_rollout","code":"native_directory_unavailable"}),
                ]
            } else {
                vec![]
            };
            assert_eq!(actual.diagnostics, diagnostics, "{}", case["name"]);
            assert_eq!(actual.view, f.expand(&expected["view"]), "{}", case["name"]);
        }
        assert_eq!(before, snapshot(&f.0), "nonmutation {}", case["name"]);
    }
    assert_eq!((complete, refused), (28, 6));
}
#[test]
fn capture_provenance_and_no_engine_effects_are_retained() {
    let c = corpus();
    assert_eq!(c["evidence"]["process_attempts"], 0);
    assert_eq!(c["evidence"]["real_engine_calls"], 0);
    assert_eq!(c["evidence"]["disposable_home"], true);
    assert_eq!(c["evidence"]["fake_executables"], true);
}
#[test]
fn unsafe_workspace_refuses_whole_composition_without_repair() {
    for variant in [
        "symlink",
        "hardlink",
        "directory",
        "fifo",
        "public",
        "oversize",
        "bad-utf8",
        "parent-link",
        "public-root",
        "file-root",
        "relative",
        "parent-traversal",
    ] {
        let f = Fixture::new();
        let path = f.write(
            "workspace/defaults.json",
            br#"{"mode":"auto","checkout":"worktree"}"#,
        );
        let mut state = f.0.join("workspace");
        match variant {
            "symlink" => {
                fs::rename(&path, f.0.join("target")).unwrap();
                symlink(f.0.join("target"), &path).unwrap();
            }
            "hardlink" => fs::hard_link(&path, f.0.join("target")).unwrap(),
            "directory" => {
                fs::remove_file(&path).unwrap();
                fs::create_dir(&path).unwrap();
            }
            "fifo" => {
                fs::remove_file(&path).unwrap();
                let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
                assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
            }
            "public" => fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap(),
            "oversize" => fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .unwrap()
                .set_len(4 * 1024 * 1024 + 1)
                .unwrap(),
            "bad-utf8" => fs::write(&path, [0xff]).unwrap(),
            "parent-link" => {
                fs::rename(&state, f.0.join("target")).unwrap();
                symlink(f.0.join("target"), &state).unwrap();
            }
            "public-root" => {
                fs::set_permissions(&state, fs::Permissions::from_mode(0o755)).unwrap()
            }
            "file-root" => state = path,
            "relative" => state = PathBuf::from("workspace"),
            "parent-traversal" => state = state.join("..").join("workspace"),
            _ => unreachable!(),
        }
        let before = snapshot(&f.0);
        assert!(f.composed(&state).is_err(), "{variant}");
        assert_eq!(before, snapshot(&f.0), "{variant}");
    }
}
#[test]
fn explicit_channel_roots_missing_state_and_failure_boundaries() {
    let f = Fixture::new();
    f.write(
        "release/defaults.json",
        br#"{"mode":"guarded","checkout":"main"}"#,
    );
    f.write(
        "dev/defaults.json",
        br#"{"mode":"auto","checkout":"worktree","extra":"ignored"}"#,
    );
    // XDG engine state is deliberately not the Workspace state.
    f.write("state/defaults.json", br#"{"mode":"wrong"}"#);
    let before = snapshot(&f.0);
    for (dir, mode, checkout) in [
        ("release", "guarded", "main"),
        ("dev", "auto", "worktree"),
        ("missing/nested", "guarded", "main"),
    ] {
        let actual = f.composed(&f.0.join(dir)).unwrap().view;
        assert_eq!(actual["config"]["surfaces"]["user_defaults"]["mode"], mode);
        assert_eq!(
            actual["config"]["surfaces"]["user_defaults"]["checkout"],
            checkout
        );
    }
    assert_eq!(before, snapshot(&f.0));
    assert!(f
        .reader()
        .settings_with_workspace_readers(&Value::Null, 1800000000.0, None, &f.0.join("release"))
        .is_err());
    // A valid Workspace observation cannot paper over another producer's refusal.
    f.write("config/cortex/connections.json", b"{");
    assert!(f.composed(&f.0.join("dev")).is_err());
}
