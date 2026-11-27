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
        let root = std::env::temp_dir().join(format!(
            "cw-cat-{}-{}",
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
    fn path(&self) -> PathBuf {
        self.root.join("config/cortex/connections.json")
    }
    fn write(&self, bytes: &[u8]) {
        crate::local::private_tree(self.path().parent().unwrap()).unwrap();
        fs::write(self.path(), bytes).unwrap();
        fs::set_permissions(self.path(), fs::Permissions::from_mode(0o600)).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
#[test]
fn catalog_refresh_admission_locks_revisions_and_never_creates_or_replays() {
    use super::catalog_refresh::{refresh, refresh_with};
    use cortex_core::store::locks::AccessLock;
    let f = Fixture::new();
    f.write(br#"{"schema_version":1,"connections":{}}"#);
    let folder = f.path().parent().unwrap().to_owned();
    // The bare refresh creates no lock/ignore files just to fail.
    assert_eq!(
        refresh(&f.config).unwrap_err().code,
        "config_action_unavailable"
    );
    assert!(!folder.join("connections.lock").exists());
    fs::write(folder.join("connections.lock"), b"").unwrap();
    fs::set_permissions(
        folder.join("connections.lock"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let held = AccessLock::acquire(
        &folder.join("connections.lock"),
        false,
        true,
        Duration::ZERO,
    )
    .unwrap();
    assert_eq!(
        refresh_with(&f.config, Duration::ZERO, || {})
            .unwrap_err()
            .code,
        "machine_busy"
    );
    drop(held);
    let error = refresh_with(&f.config, Duration::ZERO, || {
        f.write(br#"{"schema_version":1,"connections":{}} "#);
    })
    .unwrap_err();
    assert_eq!(error.code, "revision_conflict");
    assert_eq!(
        fs::read(f.path()).unwrap(),
        br#"{"schema_version":1,"connections":{}} "#
    );
    assert_eq!(refresh(&f.config).unwrap()["refresh"], json!([]));
    assert!(!folder.join(".gitignore").exists());
    assert!(!f.config.state_dir.exists());
    // A record no native producer launches is its own failed outcome, never a
    // partially refreshed subset nor a delegated request.
    let mut registry: Value =
        serde_json::from_str(corpus()["cases"][0]["initial"].as_str().unwrap()).unwrap();
    assert!(registry["connections"].as_object().unwrap().len() > 1);
    for r in registry["connections"]
        .as_object_mut()
        .unwrap()
        .values_mut()
    {
        r["launch_command"] = Value::Null;
    }
    f.write(&serde_json::to_vec(&registry).unwrap());
    registry["connections"]["cn_0123456789ab"]["launch_command"] = json!(["/not-executed"]);
    f.write(&serde_json::to_vec(&registry).unwrap());
    let reply = refresh(&f.config).unwrap();
    let outcome = reply["refresh"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["connection_id"] == "cn_0123456789ab")
        .unwrap();
    assert_eq!(outcome["success"], false);
    assert!(outcome["reason"]
        .as_str()
        .unwrap()
        .starts_with("engine_unreachable:"));
    assert_eq!(
        reply["refresh"].as_array().unwrap().len(),
        registry["connections"].as_object().unwrap().len()
    );
    fs::remove_file(folder.join("connections.lock")).unwrap();
    std::os::unix::fs::symlink(f.path(), folder.join("connections.lock")).unwrap();
    assert_eq!(refresh(&f.config).unwrap_err().code, "unsafe_storage");
}

#[test]
fn opencode_install_becoming_launchable_after_admission_refuses_without_replay() {
    use super::catalog_refresh::refresh_with;
    let f = Fixture::new();
    let corpus: Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/config-catalog-producer-python.json"
    ))
    .unwrap();
    let case = corpus["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == "opencode-missing")
        .unwrap();
    let mut registry: Value = serde_json::from_str(
        case["initial"]["config/cortex/connections.json"]["content"]
            .as_str()
            .unwrap()
            .replace("@ROOT@", f.root.to_str().unwrap())
            .as_str(),
    )
    .unwrap();
    let binary = f.root.join("peer");
    registry["connections"]["cn_000000000001"]["launch_command"] = json!([binary, "acp"]);
    f.write(&serde_json::to_vec(&registry).unwrap());
    let folder = f.path().parent().unwrap().to_owned();
    fs::write(folder.join("connections.lock"), b"").unwrap();
    fs::set_permissions(
        folder.join("connections.lock"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let before = fs::read(f.path()).unwrap();
    // The binary appears after the registry was read: it is never launched here
    // (no native OpenCode producer), so its row is an explicit failed attempt.
    let reply = refresh_with(&f.config, Duration::ZERO, || {
        fs::write(&binary, "#!/bin/sh\nexit 91\n").unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
    })
    .unwrap();
    let outcome = reply["refresh"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["connection_id"] == "cn_000000000001")
        .unwrap();
    assert_eq!(outcome["success"], false);
    // The fixture's recorded auth shape is not an OpenCode native account: PortableError.
    assert_eq!(outcome["reason"], "engine_unreachable:PortableError");
    assert!(!fs::read_to_string(&binary).unwrap().is_empty());
    assert_eq!(fs::read(f.path()).unwrap(), before);
    assert!(!folder.join(".gitignore").exists());
    assert!(!f.config.state_dir.exists());
}

#[test]
fn metadata_edge_cases_match_the_oracle_replies_and_all_directory_effects() {
    let c: Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/config-metadata-python.json"
    ))
    .unwrap();
    assert_eq!(c["processNetworkKeyringAttempts"], 0);
    for case in c["cases"].as_array().unwrap() {
        let f = Fixture::new();
        if let Some(initial) = case["initial"].as_str() {
            f.write(initial.as_bytes());
        } else {
            crate::local::private_tree(f.path().parent().unwrap()).unwrap();
        }
        if let Some(ignore) = case["ignore"].as_str() {
            let path = f.path().parent().unwrap().join(".gitignore");
            fs::write(&path, ignore).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let actual = super::super::native::envelope(update(
            &f.config,
            &case["params"],
            c["now"].as_str().unwrap(),
            write::replace,
        ));
        let expected: Value = serde_json::from_str(
            &case["reply"]
                .to_string()
                .replace("$CONFIG", &f.path().parent().unwrap().display().to_string()),
        )
        .unwrap();
        assert_eq!(actual, expected, "{}", case["name"]);
        let mut files = json!({});
        for item in fs::read_dir(f.path().parent().unwrap()).unwrap() {
            let p = item.unwrap().path();
            files[p.file_name().unwrap().to_str().unwrap()] = json!({
                "content":fs::read_to_string(&p).unwrap(), "mode":fs::metadata(&p).unwrap().permissions().mode() & 0o777});
        }
        assert_eq!(files, case["files"], "{}", case["name"]);
        assert!(!f.config.state_dir.exists());
    }
}
fn corpus() -> Value {
    serde_json::from_str(include_str!(
        "../../tests/fixtures/config-action-python.json"
    ))
    .unwrap()
}

#[test]
fn metadata_actions_compare_complete_oracle_reply_bytes_and_mode() {
    let c = corpus();
    assert_eq!(c["processOrNetworkAttempts"], 0);
    assert_eq!(c["cases"].as_array().unwrap().len(), 23);
    for case in c["cases"].as_array().unwrap() {
        let f = Fixture::new();
        f.write(case["initial"].as_str().unwrap().as_bytes());
        let actual = super::super::native::envelope(update(
            &f.config,
            &case["params"],
            c["now"].as_str().unwrap(),
            write::replace,
        ));
        assert_eq!(actual, case["reply"], "{}", case["name"]);
        assert_eq!(
            fs::read(f.path()).unwrap(),
            case["after"].as_str().unwrap().as_bytes(),
            "{}",
            case["name"]
        );
        assert_eq!(
            fs::metadata(f.path()).unwrap().permissions().mode() & 0o777,
            case["mode"].as_u64().unwrap() as u32
        );
        assert!(!f.config.state_dir.exists());
    }
}
#[test]
fn every_request_shape_is_answered_natively_with_an_explicit_refusal_or_result() {
    let c = corpus();
    let f = Fixture::new();
    // Floats and non-ASCII verbs have no exact Python repr: refused natively, naming the verb.
    for verb in [json!(1.5), json!("caf\u{e9}"), json!(-0.0)] {
        let e = action(
            &f.config,
            &json!({"action":"profile","verb":verb}),
            std::time::Instant::now() + super::super::HELPER_DEADLINE,
        )
        .unwrap_err();
        assert_eq!(e.code, "unsupported_action", "{verb}");
        assert!(e.message.contains("not supported"), "{verb}");
        assert!(!f.root.join("config").exists());
    }
    // Every corpus request reaches a native outcome; none is delegated or panics.
    for item in c["explicitlyUnavailable"].as_array().unwrap() {
        let g = Fixture::new();
        let outcome = action(
            &g.config,
            &item["params"],
            std::time::Instant::now() + super::super::HELPER_DEADLINE,
        );
        if let Err(e) = outcome {
            assert!(!e.code.is_empty() && !e.message.is_empty(), "{item}");
        }
    }
    for params in [
        json!({}),
        json!({"action":"unknown"}),
        json!({"action":true}),
        Value::Null,
        json!([]),
    ] {
        let e = action(
            &f.config,
            &params,
            std::time::Instant::now() + super::super::HELPER_DEADLINE,
        )
        .unwrap_err();
        assert_eq!(e.code, "unsupported_action");
        assert_eq!(e.message, "This machine does not support that yet.");
    }
}
#[test]
fn stale_concurrent_admission_unsafe_targets_and_uncertain_commit() {
    let c = corpus();
    let case = &c["cases"][0];
    let now = c["now"].as_str().unwrap();
    let f = Fixture::new();
    f.write(case["initial"].as_str().unwrap().as_bytes());
    let p = &case["params"];
    // Exactly one of two callers admitted with the same revision can commit.
    std::thread::scope(|s| {
        let a = s.spawn(|| update(&f.config, p, now, write::replace));
        let b = s.spawn(|| update(&f.config, p, now, write::replace));
        let replies = [a.join().unwrap(), b.join().unwrap()];
        assert_eq!(replies.iter().filter(|r| r.is_ok()).count(), 1);
        assert_eq!(
            replies.into_iter().find_map(Result::err).unwrap().code,
            "revision_conflict"
        );
    });
    let after = fs::read(f.path()).unwrap();
    assert_eq!(
        update(&f.config, p, now, write::replace).unwrap_err().code,
        "revision_conflict"
    );
    assert_eq!(fs::read(f.path()).unwrap(), after);
    f.write(case["initial"].as_str().unwrap().as_bytes());
    let e = update(&f.config, p, now, |path, body| {
        // Simulate a durability failure after rename: preserve unknown, no replay.
        write::replace(path, body)?;
        Err(refuse(
            "machine_outcome_unknown",
            "Injected post-commit failure",
        ))
    })
    .unwrap_err();
    assert_eq!(e.code, "machine_outcome_unknown");
    assert_eq!(
        fs::read(f.path()).unwrap(),
        case["after"].as_str().unwrap().as_bytes()
    );
    assert_eq!(
        update(&f.config, p, now, write::replace).unwrap_err().code,
        "revision_conflict"
    );
    for hard in [false, true] {
        let f = Fixture::new();
        f.write(case["initial"].as_str().unwrap().as_bytes());
        let outside = f.root.join("untouched");
        fs::rename(f.path(), &outside).unwrap();
        if hard {
            fs::hard_link(&outside, f.path()).unwrap();
        } else {
            std::os::unix::fs::symlink(&outside, f.path()).unwrap();
        }
        assert_eq!(
            update(&f.config, p, now, write::replace).unwrap_err().code,
            "unsafe_storage"
        );
        assert_eq!(
            fs::read(outside).unwrap(),
            case["initial"].as_str().unwrap().as_bytes()
        );
    }
}
#[test]
fn missing_registry_and_unsafe_lock_refuse_without_writing_registry() {
    let c = corpus();
    let case = &c["cases"][0];
    let f = Fixture::new();
    let mut p = case["params"].clone();
    p["expected_revision"] = json!("absent");
    assert_eq!(
        update(&f.config, &p, c["now"].as_str().unwrap(), write::replace)
            .unwrap_err()
            .code,
        "unknown_connection"
    );
    assert!(!f.path().exists());
    let f = Fixture::new();
    f.write(case["initial"].as_str().unwrap().as_bytes());
    let peer = f.root.join("peer");
    fs::write(&peer, b"untouched").unwrap();
    std::os::unix::fs::symlink(&peer, f.path().parent().unwrap().join("connections.lock")).unwrap();
    assert_eq!(
        update(
            &f.config,
            &case["params"],
            c["now"].as_str().unwrap(),
            write::replace
        )
        .unwrap_err()
        .code,
        "unsafe_storage"
    );
    assert_eq!(fs::read(peer).unwrap(), b"untouched");
    assert_eq!(
        fs::read(f.path()).unwrap(),
        case["initial"].as_str().unwrap().as_bytes()
    );
}

#[test]
fn no_op_preserves_bytes_and_malformed_history_never_writes() {
    let c = corpus();
    let case = &c["cases"][0];
    let f = Fixture::new();
    let now = c["now"].as_str().unwrap();
    f.write(case["initial"].as_str().unwrap().as_bytes());
    let result = update(&f.config, &case["params"], now, write::replace).unwrap();
    let bytes = fs::read(f.path()).unwrap();
    let mut p = case["params"].clone();
    p["expected_revision"] = result["revision"].clone();
    update(&f.config, &p, now, |_, _| panic!("no-op must not replace")).unwrap();
    assert_eq!(fs::read(f.path()).unwrap(), bytes);
    f.write(b"{broken");
    p["expected_revision"] = json!(write::token(Some(b"{broken")));
    assert_eq!(
        update(&f.config, &p, now, write::replace).unwrap_err().code,
        "invalid_registry"
    );
    assert_eq!(fs::read(f.path()).unwrap(), b"{broken");
    let mut input: Value = serde_json::from_str(case["initial"].as_str().unwrap()).unwrap();
    input["override_history"] = json!({"cn_0123456789ab":[{"fingerprint":1.5,"previous_fingerprint":null,
        "engine":"codex-acp","provider":"openai","endpoint":null,"launch_command":null,
        "approved_at":"now","approved_by_user":true}]});
    let body = serde_json::to_vec(&input).unwrap();
    f.write(&body);
    p["expected_revision"] = json!(write::token(Some(&body)));
    update(&f.config, &p, now, write::replace).unwrap();
    let after: Value = serde_json::from_slice(&fs::read(f.path()).unwrap()).unwrap();
    assert_eq!(after["override_history"], input["override_history"]);
    assert_eq!(
        fs::read(f.path().parent().unwrap().join(".gitignore")).unwrap(),
        b"*\n"
    );
}

fn boundary() -> Value {
    serde_json::from_str(include_str!(
        "../../tests/fixtures/settings-native-boundary-python.json"
    ))
    .unwrap()
}

#[test]
fn installer_and_onboarding_pre_effect_refusals_match_the_oracle_without_effects() {
    let c = boundary();
    assert_eq!(c["forbiddenAttempts"], json!([]));
    assert_eq!(
        c["engines"],
        json!({"management":["codex-acp","claude-agent-acp","opencode"],"installable":["codex-acp","claude-agent-acp"]})
    );
    let refusals = c["refusals"].as_array().unwrap();
    assert_eq!(refusals.len(), 24);
    for case in refusals {
        let f = Fixture::new();
        let registry = case["initial"]["config/cortex/connections.json"]["content"].as_str();
        if let Some(content) = registry {
            f.write(content.as_bytes());
        }
        assert_eq!(
            crate::machine::native::envelope(action(
                &f.config,
                &case["params"],
                std::time::Instant::now() + super::super::HELPER_DEADLINE
            )),
            case["reply"],
            "{}",
            case["name"]
        );
        // No lock, ignore, state or registry change: the refusal precedes all effects.
        let folder = f.path().parent().unwrap().to_owned();
        if let Some(content) = registry {
            assert_eq!(fs::read_to_string(f.path()).unwrap(), content);
            assert_eq!(
                fs::read_dir(&folder).unwrap().count(),
                1,
                "{}",
                case["name"]
            );
        } else {
            assert!(!f.root.join("config").exists(), "{}", case["name"]);
        }
        assert!(!f.config.state_dir.exists());
    }
    // OpenCode onboarding is an explicit native refusal (no probe sandbox, no
    // OpenCode catalog producer here), never delegated and with no effects.
    let f = Fixture::new();
    let error = action(
        &f.config,
        &json!({"action":"onboard_opencode","explicit_opt_in":true,"expected_revision":"absent"}),
        std::time::Instant::now() + super::super::HELPER_DEADLINE,
    )
    .unwrap_err();
    assert_eq!(error.code, "config_action_unavailable");
    assert!(!f.root.join("config").exists());
}

#[test]
fn keyring_catalog_refresh_reports_unobserved_keyring_without_any_provider() {
    let c = boundary();
    let f = Fixture::new();
    let case = c["refresh"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == "keyring-codex-openai")
        .unwrap();
    let content = case["initial"]["config/cortex/connections.json"]["content"]
        .as_str()
        .unwrap()
        .replace("@ROOT@", f.root.to_str().unwrap());
    f.write(content.as_bytes());
    let folder = f.path().parent().unwrap().to_owned();
    fs::write(folder.join("connections.lock"), b"").unwrap();
    fs::set_permissions(
        folder.join("connections.lock"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    // The unit host's existing (empty) state root, as the socket fixtures provide.
    let state = f.config.state_dir.join("state");
    crate::local::private_tree(&state).unwrap();
    let reply = super::catalog_refresh::refresh(&f.config).unwrap();
    let outcome = &reply["refresh"][0];
    assert_eq!(outcome["success"], false);
    assert_eq!(outcome["reason"], "engine_unreachable:keyring_unobservable");
    // No state, ignore or registry effect.
    assert_eq!(fs::read_dir(&state).unwrap().count(), 0);
    assert!(!folder.join(".gitignore").exists());
    assert_eq!(fs::read_to_string(f.path()).unwrap(), content);
}
