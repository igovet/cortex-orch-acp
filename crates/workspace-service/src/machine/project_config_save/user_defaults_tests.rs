use super::*;
use std::{
    os::unix::fs::{symlink, PermissionsExt},
    path::PathBuf,
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
            "/tmp/cw-mdw-{}-{}",
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
    fn owner(&self) -> PathBuf {
        self.root.join("config/cortex/defaults.json")
    }
    fn workspace(&self) -> PathBuf {
        self.config.state_dir.join("defaults.json")
    }
    fn write(&self, path: &Path, text: &str) {
        crate::local::private_tree(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn params() -> Value {
    json!({"surface":"user_defaults","value":{},"tokens":{"user_defaults":"absent"}})
}
fn bind(v: &Value, f: &Fixture) -> Value {
    match v {
        Value::String(s) => json!(s
            .replace("$CONFIG", f.root.join("config").to_str().unwrap())
            .replace("$WORKSPACE", f.config.state_dir.to_str().unwrap())),
        Value::Array(a) => json!(a.iter().map(|v| bind(v, f)).collect::<Vec<_>>()),
        Value::Object(m) => Value::Object(m.iter().map(|(k, v)| (k.clone(), bind(v, f))).collect()),
        v => v.clone(),
    }
}
fn files(f: &Fixture) -> Value {
    let mut v = json!({});
    for (key, path) in [
        ("owner", f.owner()),
        ("workspace", f.workspace()),
        ("ignore", f.owner().parent().unwrap().join(".gitignore")),
    ] {
        v[key] = match fs::read_to_string(&path) {
            Ok(body) => {
                json!({"body":body,"mode":fs::metadata(path).unwrap().permissions().mode() & 0o777})
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Value::Null,
            Err(e) => panic!("{e}"),
        };
    }
    v
}
#[test]
fn user_defaults_complete_replies_and_both_files_match_the_frozen_oracle() {
    let corpus: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/machine-defaults-python.json"
    ))
    .unwrap();
    assert_eq!(corpus["processNetworkKeyringAttempts"], 0);
    assert_eq!(corpus["cases"].as_array().unwrap().len(), 47);
    for c in corpus["cases"].as_array().unwrap() {
        let f = Fixture::new();
        for (key, path) in [
            ("initial", f.owner()),
            ("workspace", f.workspace()),
            (
                "registry",
                f.owner().parent().unwrap().join("connections.json"),
            ),
        ] {
            if let Some(s) = c[key].as_str() {
                f.write(&path, s);
            }
        }
        for o in c["observations"].as_array().unwrap() {
            let actual = match save(&f.config, &o["params"]) {
                Ok(v) => json!({"ok":true,"result":v}),
                Err(e) => json!({"ok":false,"code":e.code,"message":e.message}),
            };
            assert_eq!(actual, bind(&o["reply"], &f), "{}", c["name"]);
            assert_eq!(files(&f), o["files"], "{}", c["name"]);
        }
        assert!(!f.root.join(".cortex").exists());
        assert!(!f.root.join(".cortex-dev").exists());
        assert!(!f.config.runtime_dir.exists());
        assert!(super::super::super::native::supported("config.save"));
    }
}
#[test]
fn user_defaults_preflights_workspace_and_budget_before_settings_effects() {
    for bad in ["{", "[]"] {
        let f = Fixture::new();
        f.write(&f.workspace(), bad);
        let error = save(&f.config, &params()).unwrap_err();
        assert!(["state_unreadable", "machine_tool_failed"].contains(&error.code));
        assert!(!f.owner().exists());
        assert_eq!(fs::read_to_string(f.workspace()).unwrap(), bad);
    }
    let f = Fixture::new();
    let mut p = params();
    p["value"]["mode"] = json!("a".repeat(FRAME_BUDGET));
    assert_eq!(
        save(&f.config, &p).unwrap_err().code,
        "machine_reply_too_large"
    );
    assert!(!f.owner().exists());
    assert!(!f.workspace().exists());
}
#[test]
fn user_defaults_partial_write_and_durability_failure_stay_unknown_without_replay() {
    let f = Fixture::new();
    let mut calls = 0;
    let error = save_with(&f.config, &params(), |path, body| {
        calls += 1;
        if calls == 1 {
            replace(path, body)
        } else {
            Err(refuse("storage_error", "injected Workspace failure"))
        }
    })
    .unwrap_err();
    assert_eq!(error.code, "machine_outcome_unknown");
    assert_eq!(calls, 2);
    assert!(f.owner().exists());
    assert!(!f.workspace().exists());
    let f = Fixture::new();
    let mut calls = 0;
    let error = save_with(&f.config, &params(), |path, body| {
        calls += 1;
        super::super::replace_sync(path, body, |_| {
            Err(refuse("storage_error", "injected sync failure"))
        })
    })
    .unwrap_err();
    assert_eq!(error.code, "machine_outcome_unknown");
    assert_eq!(calls, 1);
    assert!(f.owner().exists());
    assert!(!f.workspace().exists());
    let f = Fixture::new();
    let mut calls = 0;
    let error = save_with(&f.config, &params(), |_, _| {
        calls += 1;
        Err(refuse("storage_error", "injected pre-effect failure"))
    })
    .unwrap_err();
    assert_eq!(error.code, "storage_error");
    assert_eq!(calls, 1);
    assert!(!f.owner().exists());
    assert!(!f.workspace().exists());
}
#[test]
fn user_defaults_conflict_unsafe_storage_and_exact_number_bounds_never_save() {
    let f = Fixture::new();
    f.write(&f.owner(), r#"{"defaults":{}}"#);
    let error = save(&f.config, &params()).unwrap_err();
    assert_eq!(error.code, "revision_conflict");
    assert!(!f.workspace().exists());
    for target in ["defaults.json", ".gitignore", "connections.lock"] {
        let f = Fixture::new();
        let outside = f.root.join("outside");
        f.write(&outside, "sentinel");
        crate::local::private_tree(f.owner().parent().unwrap()).unwrap();
        symlink(&outside, f.owner().parent().unwrap().join(target)).unwrap();
        assert!(save(&f.config, &params()).is_err());
        assert_eq!(fs::read_to_string(outside).unwrap(), "sentinel");
        assert!(!f.workspace().exists());
    }
    for target in ["defaults.json", "machine.lock"] {
        let f = Fixture::new();
        let outside = f.root.join("outside");
        f.write(&outside, "sentinel");
        crate::local::private_tree(&f.config.state_dir).unwrap();
        fs::hard_link(&outside, f.config.state_dir.join(target)).unwrap();
        assert!(save(&f.config, &params()).is_err());
        assert!(!f.owner().exists());
        assert_eq!(fs::read_to_string(outside).unwrap(), "sentinel");
    }
    for body in [
        r#"{"defaults":{"opaque":18446744073709551616}}"#,
        r#"{"opaque":-9223372036854775809}"#,
    ] {
        let f = Fixture::new();
        f.write(&f.owner(), body);
        let mut p = params();
        p["tokens"]["user_defaults"] = json!(token(Some(body.as_bytes())));
        assert_eq!(
            save(&f.config, &p).unwrap_err().code,
            "native_config_unavailable"
        );
        assert_eq!(fs::read_to_string(f.owner()).unwrap(), body);
        assert!(!f.workspace().exists());
    }
}
#[test]
fn user_defaults_concurrent_writers_use_one_revision_and_preserve_workspace() {
    let f = Fixture::new();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let results = std::thread::scope(|scope| {
        let a = scope.spawn(|| {
            barrier.wait();
            save(&f.config, &params())
        });
        let b = scope.spawn(|| {
            barrier.wait();
            save(&f.config, &params())
        });
        [a.join().unwrap(), b.join().unwrap()]
    });
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter_map(|r| r.as_ref().err())
            .next()
            .unwrap()
            .code,
        "revision_conflict"
    );
    assert_eq!(
        fs::read_to_string(f.workspace()).unwrap(),
        r#"{"checkout": "main", "mode": "guarded"}"#
    );
}
