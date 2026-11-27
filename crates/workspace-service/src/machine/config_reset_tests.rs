use super::*;
use std::{
    fs,
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
        let root = std::env::temp_dir().join(format!(
            "cw-reset-{}-{}",
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
    fn folder(&self) -> PathBuf {
        self.root.join("config/cortex")
    }
    fn install(&self, initial: &Value) {
        crate::local::private_tree(&self.folder()).unwrap();
        for (file, body) in initial.as_object().unwrap() {
            let path = self.folder().join(file);
            fs::write(&path, body.as_str().unwrap()).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        }
    }
    fn files(&self) -> Value {
        let mut files = json!({});
        for e in fs::read_dir(self.folder()).unwrap() {
            let p = e.unwrap().path();
            files[p.file_name().unwrap().to_str().unwrap()] = json!({"content":fs::read_to_string(&p).unwrap(),
                "mode":fs::metadata(&p).unwrap().permissions().mode() & 0o777});
        }
        files
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn corpus() -> Value {
    serde_json::from_str(include_str!(
        "../../tests/fixtures/config-reset-python.json"
    ))
    .unwrap()
}

#[test]
fn reset_matches_the_recorded_oracle_replies_all_bytes_and_modes() {
    let c = corpus();
    assert_eq!(c["processNetworkAttempts"], 0);
    assert_eq!(c["cases"].as_array().unwrap().len(), 14);
    for case in c["cases"].as_array().unwrap() {
        let f = Fixture::new();
        f.install(&case["initial"]);
        let actual = super::super::super::native::envelope(reset_with(
            &f.config,
            &case["params"],
            || Ok(c["stamp"].as_str().unwrap().into()),
            write::replace,
        ));
        let expected: Value = serde_json::from_str(
            &case["reply"]
                .to_string()
                .replace("$CONFIG", f.folder().to_str().unwrap()),
        )
        .unwrap();
        assert_eq!(actual, expected, "{}", case["name"]);
        assert_eq!(f.files(), case["files"], "{}", case["name"]);
        assert!(!f.config.state_dir.exists());
    }
}
#[test]
fn reset_rolls_back_second_replace_failure_and_retains_backups() {
    for existing in [false, true] {
        let f = Fixture::new();
        f.install(&if existing {
            json!({"routing.json":"old","defaults.json":"old defaults"})
        } else {
            json!({})
        });
        let error = reset_with(
            &f.config,
            &json!({"confirm":true}),
            || Ok("42".into()),
            |p, b| {
                if p.file_name().unwrap() == "defaults.json" {
                    return Err(refuse("storage_error", "Injected before replacement"));
                }
                write::replace(p, b)
            },
        )
        .unwrap_err();
        assert_eq!(error.code, "storage_error");
        if existing {
            assert_eq!(fs::read(f.folder().join("routing.json")).unwrap(), b"old");
            assert_eq!(
                fs::read(f.folder().join("defaults.json")).unwrap(),
                b"old defaults"
            );
            assert_eq!(
                fs::read(f.folder().join("routing.json.cortex-backup-42")).unwrap(),
                b"old"
            );
            assert_eq!(
                fs::read(f.folder().join("defaults.json.cortex-backup-42")).unwrap(),
                b"old defaults"
            );
        } else {
            assert!(!f.folder().join("routing.json").exists());
            assert!(!f.folder().join("defaults.json").exists());
        }
    }
}
#[test]
fn reset_uncertainty_and_failed_rollback_never_replay() {
    let f = Fixture::new();
    f.install(&json!({"routing.json":"old","defaults.json":"old defaults"}));
    let mut replacements = 0;
    let error = reset_with(
        &f.config,
        &json!({"confirm":true}),
        || Ok("43".into()),
        |p, b| {
            write::replace(p, b)?;
            if p.file_name().unwrap() == "routing.json" {
                replacements += 1;
                return Err(refuse(
                    "machine_outcome_unknown",
                    "Injected after replacement",
                ));
            }
            Ok(())
        },
    )
    .unwrap_err();
    assert_eq!(error.code, "machine_outcome_unknown");
    assert_eq!(replacements, 1);
    assert_eq!(
        fs::read(f.folder().join("routing.json")).unwrap(),
        payloads().unwrap()[0]
    );
    assert_eq!(
        fs::read(f.folder().join("defaults.json")).unwrap(),
        b"old defaults"
    );
    let mut replacements = 0;
    let error = reset_with(
        &f.config,
        &json!({"confirm":true}),
        || Ok("44".into()),
        |p, b| {
            if p.file_name().unwrap() == "routing.json" {
                replacements += 1;
                if replacements == 2 {
                    return Err(refuse("storage_error", "Injected rollback failure"));
                }
            }
            if p.file_name().unwrap() == "defaults.json" {
                return Err(refuse("storage_error", "Injected before replacement"));
            }
            write::replace(p, b)
        },
    )
    .unwrap_err();
    assert_eq!(error.code, "preset_rollback_failed");
    assert_eq!(replacements, 2);
}
#[test]
fn reset_rechecks_shared_revision_after_waiting_for_owner_lock() {
    let f = Fixture::new();
    f.install(&json!({"routing.json":"old"}));
    let lock = write::owner_lock(&f.folder()).unwrap();
    std::thread::scope(|s| {
        let reset = s.spawn(|| super::reset(&f.config, &json!({"confirm":true})));
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !f.folder().join(".gitignore").exists() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        fs::write(f.folder().join("routing.json"), b"concurrent").unwrap();
        drop(lock);
        assert_eq!(reset.join().unwrap().unwrap_err().code, "revision_conflict");
    });
    assert_eq!(
        fs::read(f.folder().join("routing.json")).unwrap(),
        b"concurrent"
    );
    assert!(!f.folder().join("defaults.json").exists());
}
#[test]
fn reset_refuses_unsafe_targets_and_backup_collision() {
    for hard in [false, true] {
        let f = Fixture::new();
        f.install(&json!({}));
        let outside = f.root.join("outside");
        fs::write(&outside, b"untouched").unwrap();
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o600)).unwrap();
        let target = f.folder().join("routing.json");
        if hard {
            fs::hard_link(&outside, &target).unwrap();
        } else {
            symlink(&outside, &target).unwrap();
        }
        assert_eq!(
            reset(&f.config, &json!({"confirm":true})).unwrap_err().code,
            "unsafe_storage"
        );
        assert_eq!(fs::read(outside).unwrap(), b"untouched");
        assert!(!f.folder().join("connections.lock").exists());
    }
    let f = Fixture::new();
    f.install(&json!({"routing.json":"old","routing.json.cortex-backup-42":"older backup"}));
    assert_eq!(
        reset_with(
            &f.config,
            &json!({"confirm":true}),
            || Ok("42".into()),
            write::replace
        )
        .unwrap_err()
        .code,
        "unsafe_storage"
    );
    assert_eq!(fs::read(f.folder().join("routing.json")).unwrap(), b"old");
    assert_eq!(
        fs::read(f.folder().join("routing.json.cortex-backup-42")).unwrap(),
        b"older backup"
    );
}
