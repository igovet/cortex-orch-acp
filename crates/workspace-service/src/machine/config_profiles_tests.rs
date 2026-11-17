use super::*;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
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
            "/tmp/cw-profa-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let mut launch = crate::LaunchSpec::bare();
        launch.dev_config = Some(root.join("config"));
        let config = crate::Config {
            runtime_dir: root.join("run"),
            state_dir: root.join("state"),
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
        for (n, b) in initial.as_object().unwrap() {
            let p = self.folder().join(n);
            fs::write(&p, b.as_str().unwrap()).unwrap();
            fs::set_permissions(p, fs::Permissions::from_mode(0o600)).unwrap();
        }
    }
    fn files(&self) -> Value {
        fn walk(root: &Path, at: &Path, out: &mut Value) {
            for e in fs::read_dir(at).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    walk(root, &p, out);
                } else {
                    out[p.strip_prefix(root).unwrap().to_str().unwrap()] = json!({"content":fs::read_to_string(&p).unwrap(),
                        "mode":fs::metadata(&p).unwrap().permissions().mode() & 0o777});
                }
            }
        }
        let mut out = json!({});
        walk(&self.folder(), &self.folder(), &mut out);
        out
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
#[test]
fn full_activation_the_recorded_oracle() {
    for source in [
        include_str!("../../tests/fixtures/config-activate-full-python.json"),
        include_str!("../../tests/fixtures/config-activate-materialize-python.json"),
        include_str!("../../tests/fixtures/config-profiles-wide-python.json"),
    ] {
        let c: Value = serde_json::from_str(source).unwrap();
        for case in c["cases"].as_array().unwrap() {
            if case["native"] == false
                && source
                    == include_str!("../../tests/fixtures/config-activate-materialize-python.json")
            {
                continue;
            }
            let f = Fixture::new();
            f.install(&case["initial"]);
            let reply = crate::machine::native::envelope(action_with(
                &f.config,
                &case["params"],
                c["now"].as_str().unwrap(),
                write::replace,
            ));
            let expected: Value = serde_json::from_str(
                &case["reply"]
                    .to_string()
                    .replace("$CONFIG", f.folder().to_str().unwrap()),
            )
            .unwrap();
            assert_eq!(reply, expected, "reply {}", case["name"]);
            assert_eq!(f.files(), case["files"], "files {}", case["name"]);
        }
    }
}
#[test]
fn full_activation_failure_rollbacks_and_interrupted_journals() {
    let c: Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/config-activate-materialize-python.json"
    ))
    .unwrap();
    let case = &c["cases"][0];
    // Fail at each backing replacement after all candidates were validated.
    for index in 0..4 {
        for unknown in [false, true] {
            let f = Fixture::new();
            f.install(&case["initial"]);
            let fail = f.folder().join(FILES[index]);
            let mut calls = vec![];
            let mut failed = false;
            let error = action_with(
                &f.config,
                &case["params"],
                c["now"].as_str().unwrap(),
                |p, b| {
                    calls.push(p.file_name().unwrap().to_str().unwrap().to_owned());
                    if p == fail && !failed {
                        failed = true;
                        if unknown {
                            write::replace(p, b)?;
                        }
                        return Err(refuse(
                            if unknown {
                                "machine_outcome_unknown"
                            } else {
                                "storage_error"
                            },
                            "injected replacement failure",
                        ));
                    }
                    write::replace(p, b)
                },
            )
            .unwrap_err();
            let pending: Value = serde_json::from_slice(
                &fs::read(f.folder().join("routing-profiles.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(
                error.code,
                if unknown {
                    "machine_outcome_unknown"
                } else {
                    "storage_error"
                }
            );
            assert_eq!(!pending["pending"].is_null(), unknown);
            for (i, file) in FILES.iter().enumerate() {
                if !unknown || i > index {
                    assert_eq!(
                        fs::read_to_string(f.folder().join(file)).unwrap(),
                        case["initial"][*file].as_str().unwrap()
                    );
                }
            }
            assert_eq!(calls.len(), if unknown { index + 2 } else { index * 2 + 3 });
        }
    }
    // Restoring an originally absent backing file unlinks only our completed write.
    let f = Fixture::new();
    let mut initial = case["initial"].clone();
    initial.as_object_mut().unwrap().remove("routing.json");
    f.install(&initial);
    let mut p = case["params"].clone();
    p["expected_revisions"]["routing"] = json!("absent");
    let e = action_with(&f.config, &p, c["now"].as_str().unwrap(), |path, body| {
        if path.ends_with("defaults.json") {
            return Err(refuse("storage_error", "injected pre-replace failure"));
        }
        write::replace(path, body)
    })
    .unwrap_err();
    assert_eq!(e.code, "storage_error");
    assert!(!f.folder().join("routing.json").exists());
    let doc: Value =
        serde_json::from_slice(&fs::read(f.folder().join("routing-profiles.json")).unwrap())
            .unwrap();
    assert!(doc["pending"].is_null());
    // A rollback failure cannot clear pending or manufacture an unchanged outcome.
    let f = Fixture::new();
    f.install(&case["initial"]);
    let mut calls = 0;
    let e = action_with(
        &f.config,
        &case["params"],
        c["now"].as_str().unwrap(),
        |p, b| {
            calls += 1;
            if calls == 3 || calls == 4 {
                return Err(refuse("storage_error", "injected failure"));
            }
            write::replace(p, b)
        },
    )
    .unwrap_err();
    assert_eq!(e.code, "machine_outcome_unknown");
    let pending: Value =
        serde_json::from_slice(&fs::read(f.folder().join("routing-profiles.json")).unwrap())
            .unwrap();
    assert!(!pending["pending"].is_null());
    // After a confirmed section rollback, an unconfirmed journal clear is never reported
    // as the plain section failure. Calls: journal, section 0, failing section 1,
    // restore section 0, clear journal.
    for landed in [false, true] {
        let f = Fixture::new();
        f.install(&case["initial"]);
        let mut calls = 0;
        let e = action_with(
            &f.config,
            &case["params"],
            c["now"].as_str().unwrap(),
            |p, b| {
                calls += 1;
                if calls == 3 {
                    return Err(refuse("storage_error", "injected section failure"));
                }
                if calls == 5 {
                    if landed {
                        write::replace(p, b)?;
                        return Err(refuse(
                            "machine_outcome_unknown",
                            "injected uncertain clear",
                        ));
                    }
                    return Err(refuse("storage_error", "injected clear failure"));
                }
                write::replace(p, b)
            },
        )
        .unwrap_err();
        assert_eq!(e.code, "machine_outcome_unknown");
        assert_eq!(calls, 5);
        for file in FILES {
            assert_eq!(
                fs::read_to_string(f.folder().join(file)).unwrap(),
                case["initial"][file].as_str().unwrap()
            );
        }
        let doc: Value =
            serde_json::from_slice(&fs::read(f.folder().join("routing-profiles.json")).unwrap())
                .unwrap();
        assert_eq!(doc["pending"].is_null(), landed);
    }
    // The first legacy journal is migrated/verified before any backing effect.
    let legacy = &c["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "format1-change")
        .unwrap();
    let f = Fixture::new();
    f.install(&legacy["initial"]);
    let mut calls = 0;
    let e = action_with(
        &f.config,
        &legacy["params"],
        c["now"].as_str().unwrap(),
        |p, b| {
            calls += 1;
            write::replace(p, b)?;
            if calls == 2 {
                fs::write(p, b"{}").unwrap();
            }
            Ok(())
        },
    )
    .unwrap_err();
    assert_eq!(e.code, "profile_migration_failed");
    assert_eq!(calls, 3);
    assert_eq!(
        fs::read_to_string(f.folder().join("routing-profiles.json")).unwrap(),
        legacy["initial"]["routing-profiles.json"].as_str().unwrap()
    );
}
#[test]
fn routing_activation_the_recorded_oracle_and_journal_failure_boundaries() {
    let c: Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/config-activate-python.json"
    ))
    .unwrap();
    for case in c["cases"].as_array().unwrap() {
        let f = Fixture::new();
        f.install(&case["initial"]);
        let reply = crate::machine::native::envelope(action_with(
            &f.config,
            &case["params"],
            c["now"].as_str().unwrap(),
            write::replace,
        ));
        let expected: Value = serde_json::from_str(
            &case["reply"]
                .to_string()
                .replace("$CONFIG", f.folder().to_str().unwrap()),
        )
        .unwrap();
        assert_eq!(reply, expected, "{}", case["name"]);
        assert_eq!(f.files(), case["files"], "{}", case["name"]);
    }
    let case = &c["cases"][0];
    for outcome in [
        "journal-before",
        "routing-before",
        "routing-unknown",
        "final-before",
        "clear-fails",
    ] {
        let f = Fixture::new();
        f.install(&case["initial"]);
        let mut calls = 0;
        let error = action_with(
            &f.config,
            &case["params"],
            c["now"].as_str().unwrap(),
            |p, b| {
                calls += 1;
                if outcome == "journal-before" && calls == 1
                    || matches!(outcome, "routing-before" | "clear-fails") && calls == 2
                    || outcome == "final-before" && calls == 3
                    || outcome == "clear-fails" && calls == 3
                {
                    return Err(refuse("storage_error", "fixture pre-replacement failure"));
                }
                write::replace(p, b)?;
                if outcome == "routing-unknown" && calls == 2 {
                    return Err(refuse(
                        "machine_outcome_unknown",
                        "fixture post-replacement failure",
                    ));
                }
                Ok(())
            },
        )
        .unwrap_err();
        let doc: Value =
            serde_json::from_slice(&fs::read(f.folder().join("routing-profiles.json")).unwrap())
                .unwrap();
        assert_eq!(
            error.code,
            if matches!(outcome, "routing-unknown" | "final-before" | "clear-fails") {
                "machine_outcome_unknown"
            } else {
                "storage_error"
            }
        );
        assert_eq!(
            f.folder().join("routing.json").exists(),
            matches!(outcome, "routing-unknown" | "final-before")
        );
        assert_eq!(
            !doc["pending"].is_null(),
            matches!(outcome, "routing-unknown" | "final-before" | "clear-fails")
        );
        assert_eq!(
            calls,
            if outcome == "journal-before" {
                1
            } else if outcome == "routing-unknown" {
                2
            } else {
                3
            }
        );
    }
}
#[test]
fn config_profiles_uncertainty_and_verification_rollback() {
    let c = corpus();
    let case = c["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "format1-update")
        .unwrap();
    for outcome in ["unknown", "mismatch", "rollback-failure", "before-replace"] {
        let f = Fixture::new();
        f.install(&case["initial"]);
        let path = f.folder().join("routing-profiles.json");
        let original = fs::read(&path).unwrap();
        let mut calls = 0;
        let result = action_with(
            &f.config,
            &case["params"],
            c["now"].as_str().unwrap(),
            |p, b| {
                calls += 1;
                if calls == 2 {
                    if outcome == "before-replace" {
                        return Err(refuse("storage_error", "fixture pre-replacement failure"));
                    }
                    write::replace(p, b)?;
                    if outcome == "unknown" {
                        return Err(refuse(
                            "machine_outcome_unknown",
                            "fixture uncertain completed replacement",
                        ));
                    }
                    // Simulate readback corruption, not an ordinary replace error.
                    fs::write(p, b"{}").unwrap();
                    return Ok(());
                }
                if calls == 3 && outcome == "rollback-failure" {
                    return Err(refuse("storage_error", "fixture restoration failure"));
                }
                write::replace(p, b)
            },
        )
        .unwrap_err();
        assert_eq!(
            result.code,
            match outcome {
                "unknown" => "machine_outcome_unknown",
                "rollback-failure" => "profile_rollback_failed",
                "before-replace" => "storage_error",
                _ => "profile_migration_failed",
            }
        );
        if outcome == "unknown" {
            assert_ne!(fs::read(&path).unwrap(), original);
            assert_eq!(calls, 2);
        } else if outcome == "rollback-failure" {
            assert_eq!(fs::read(&path).unwrap(), b"{}");
        } else {
            assert_eq!(fs::read(&path).unwrap(), original);
        }
        let backups = f.folder().join(".migration-backups");
        let saved = fs::read_dir(&backups)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(fs::read(saved).unwrap(), original);
        assert_eq!(
            fs::metadata(backups).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
}
#[test]
fn config_profiles_shared_lock_rechecks_revision_and_never_replays() {
    let c = corpus();
    let case = &c["cases"][3];
    let f = Fixture::new();
    f.install(&case["initial"]);
    let folder = f.folder();
    let lock = write::owner_lock(&folder).unwrap();
    std::thread::scope(|scope| {
        let task = scope.spawn(|| {
            action_with(
                &f.config,
                &case["params"],
                c["now"].as_str().unwrap(),
                write::replace,
            )
        });
        std::thread::sleep(Duration::from_millis(80));
        write::replace(
            &folder.join("routing-profiles.json"),
            br#"{"format":2,"profiles":{},"active":null}"#,
        )
        .unwrap();
        drop(lock);
        assert_eq!(task.join().unwrap().unwrap_err().code, "revision_conflict");
    });
    assert_eq!(
        fs::read(folder.join("routing-profiles.json")).unwrap(),
        br#"{"format":2,"profiles":{},"active":null}"#
    );
}
#[test]
fn config_profiles_unsafe_targets_backup_collision_and_exact_number_fence() {
    let c = corpus();
    let case = &c["cases"][3];
    for target in ["routing-profiles.json", "connections.lock"] {
        let f = Fixture::new();
        f.install(&case["initial"]);
        let outside = f.root.join("outside");
        fs::write(&outside, b"not touched").unwrap();
        let path = f.folder().join(target);
        if path.exists() {
            fs::remove_file(&path).unwrap();
        }
        std::os::unix::fs::symlink(&outside, &path).unwrap();
        assert_eq!(
            action_with(
                &f.config,
                &case["params"],
                c["now"].as_str().unwrap(),
                write::replace
            )
            .unwrap_err()
            .code,
            "unsafe_storage"
        );
        assert_eq!(fs::read(outside).unwrap(), b"not touched");
    }
    let f = Fixture::new();
    let legacy = c["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "format1-update")
        .unwrap();
    f.install(&legacy["initial"]);
    let backups = f.folder().join(".migration-backups");
    crate::local::private_tree(&backups).unwrap();
    let backup = backups.join("routing-profiles.format-1.20270115T080000123456Z.json");
    fs::write(&backup, b"earlier backup").unwrap();
    assert_eq!(
        action_with(
            &f.config,
            &legacy["params"],
            c["now"].as_str().unwrap(),
            write::replace
        )
        .unwrap_err()
        .code,
        "unsafe_storage"
    );
    assert_eq!(fs::read(backup).unwrap(), b"earlier backup");
    let original = fs::read(f.folder().join("routing-profiles.json")).unwrap();
    assert_eq!(
        original,
        legacy["initial"]["routing-profiles.json"]
            .as_str()
            .unwrap()
            .as_bytes()
    );
    assert_eq!(
        parse(Some(
            br#"{"format":2,"profiles":{},"applied":{"metadata":123456789012345678901234567890}}"#
        ))
        .err()
        .unwrap()
        .code,
        "native_config_unavailable"
    );
}
#[test]
fn config_profiles_wide_numeric_selection_is_passive_before_admission() {
    let f = Fixture::new();
    assert!(unrepresentable_file(&f.config).is_none());
    assert!(!f.folder().exists());
    for raw in [
        r#"{"format":2,"profiles":{},"metadata":123456789012345678901234567890}"#,
        r#"{"format":2,"profiles":{},"metadata":NaN}"#,
        r#"{"format":2,"profiles":{},"metadata":-Infinity}"#,
        r#"{"format":2,"profiles":{},"metadata":1e999}"#,
    ] {
        f.install(&json!({"routing-profiles.json":raw}));
        assert!(unrepresentable_file(&f.config).is_none());
        // A profile read is native whatever the ignored profile metadata holds.
        assert!(action(&f.config, &json!({"action":"profile","verb":"read"})).is_ok());
        assert_eq!(fs::read_dir(f.folder()).unwrap().count(), 1);
    }
    for file in FILES {
        let f = Fixture::new();
        f.install(&json!({file:r#"{"metadata":123456789012345678901234567890}"#}));
        assert_eq!(unrepresentable_file(&f.config), Some(file), "{file}");
        let error = action(&f.config, &json!({"action":"profile","verb":"read"})).unwrap_err();
        assert_eq!(error.code, "invalid_settings", "{file}");
        assert!(error.message.contains(file), "{file}");
    }
    for raw in [
        r#"{"format":2,"profiles":{},"applied":{"metadata":NaN}}"#,
        r#"{"format":2,"profiles":{},"pending":{"metadata":1e999}}"#,
        r#"{"format":2,"profiles":{"a":{"metadata":123456789012345678901234567890}}}"#,
    ] {
        let f = Fixture::new();
        f.install(&json!({"routing-profiles.json":raw}));
        assert!(unrepresentable_file(&f.config).is_some());
        assert!(parse(Some(raw.as_bytes())).is_err());
    }
    for raw in [
        r#"{"metadata":01}"#,
        r#"{"metadata":NaNx}"#,
        r#"{123456789012345678901234567890:0}"#,
    ] {
        assert!(profile_json::parse(raw.as_bytes()).is_err());
    }
    f.install(&json!({"routing-profiles.json":r#"{"format":2,"profiles":{},"metadata":"NaN Infinity 1e999 123456789012345678901234567890"}"#}));
    assert!(unrepresentable_file(&f.config).is_none());
}
fn corpus() -> Value {
    serde_json::from_str(include_str!(
        "../../tests/fixtures/config-profiles-python.json"
    ))
    .unwrap()
}
#[test]
fn config_profiles_match_the_recorded_oracle() {
    for c in [
        corpus(),
        serde_json::from_str::<Value>(include_str!(
            "../../tests/fixtures/config-profiles-edges-python.json"
        ))
        .unwrap(),
    ] {
        for case in c["cases"].as_array().unwrap() {
            let f = Fixture::new();
            f.install(&case["initial"]);
            let actual = super::super::super::native::envelope(action_with(
                &f.config,
                &case["params"],
                c["now"].as_str().unwrap(),
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
        }
    }
}
