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
            "/tmp/cw-ccu-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        crate::local::private_tree(&root).unwrap();
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
    fn expand(&self, v: &Value) -> Value {
        serde_json::from_str(
            &v.to_string()
                .replace("$CONFIG", self.folder().to_str().unwrap())
                .replace("$HOME", self.root.join("home").to_str().unwrap())
                .replace("$STATE", self.root.join("state").to_str().unwrap()),
        )
        .unwrap()
    }
    fn prepare(&self, case: &Value) -> Value {
        crate::local::private_tree(&self.folder()).unwrap();
        for (key, v) in self.expand(&case["initial"]).as_object().unwrap() {
            if let Some(name) = key.strip_prefix("config/cortex/") {
                if let Some(content) = v["content"].as_str() {
                    let path = self.folder().join(name);
                    fs::write(&path, content).unwrap();
                    fs::set_permissions(
                        path,
                        fs::Permissions::from_mode(v["mode"].as_u64().unwrap() as u32),
                    )
                    .unwrap();
                }
            }
        }
        let before = reader::read_plain(&self.folder().join("connections.json"), true).unwrap();
        serde_json::from_str(
            &case["params"]
                .to_string()
                .replace("$REVISION", &write::token(before.as_deref())),
        )
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn corpus() -> Value {
    serde_json::from_str(include_str!(
        "../../tests/fixtures/config-connections-python.json"
    ))
    .unwrap()
}

#[test]
fn fixed_clock_removals_compare_complete_replies_bytes_and_modes() {
    let c = corpus();
    for case in c["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["params"]["operation"] == "remove")
    {
        let f = Fixture::new();
        let params = f.prepare(case);
        let before = reader::read_plain(&f.folder().join("connections.json"), true).unwrap();
        let reply = crate::machine::native::envelope(super::super::update(
            &f.config,
            &params,
            c["now"].as_str().unwrap(),
            write::replace,
        ));
        let expected_files = f.expand(&case["files"]);
        let after = expected_files["config/cortex/connections.json"]["content"]
            .as_str()
            .map(str::as_bytes);
        let expected: Value = serde_json::from_str(
            &f.expand(&case["reply"])
                .to_string()
                .replace("$REVISION", &write::token(before.as_deref()))
                .replace("$AFTER_REVISION", &write::token(after)),
        )
        .unwrap();
        assert_eq!(reply, expected, "{}", case["name"]);
        let mut files = json!({});
        for e in fs::read_dir(f.folder()).unwrap() {
            let p = e.unwrap().path();
            files[&format!("config/cortex/{}", p.file_name().unwrap().to_str().unwrap())] = json!({"content":fs::read_to_string(&p).unwrap(),"mode":fs::metadata(&p).unwrap().permissions().mode() & 0o777});
        }
        let mut expected = expected_files.as_object().unwrap().clone();
        expected.retain(|k, _| k.starts_with("config/cortex/"));
        assert_eq!(files, json!(expected), "{}", case["name"]);
    }
}
#[test]
fn removal_precommit_and_uncertain_commit_never_replay_or_touch_credentials() {
    let c = corpus();
    let case = c["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "remove-keyring")
        .unwrap();
    let f = Fixture::new();
    let params = f.prepare(case);
    let path = f.folder().join("connections.json");
    let before = fs::read(&path).unwrap();
    assert!(!keyring_unobservable(&f.config, &params));
    assert_eq!(
        super::update(&f.config, &params, "fixed", |_, _| Err(refuse(
            "storage_error",
            "injected"
        )))
        .unwrap_err()
        .code,
        "storage_error"
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(
        super::update(&f.config, &params, "fixed", |p, b| {
            write::replace(p, b)?;
            Err(refuse("machine_outcome_unknown", "injected"))
        })
        .unwrap_err()
        .code,
        "machine_outcome_unknown"
    );
    assert_ne!(fs::read(&path).unwrap(), before);
    assert_eq!(
        super::update(&f.config, &params, "fixed", write::replace)
            .unwrap_err()
            .code,
        "revision_conflict"
    );
    // Secret removal is native too; keyring validation is still selected before effects.
    let f = Fixture::new();
    let mut params = f.prepare(case);
    let path = f.folder().join("connections.json");
    let mut raw: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    raw["connections"]["cn_0123456789ab"]["credential_ref"] =
        json!({"kind":"secret_file","path":f.folder().join("secrets/cn_0123456789ab")});
    let bytes = serde_json::to_vec(&raw).unwrap();
    fs::write(&path, &bytes).unwrap();
    params["expected_revision"] = json!(write::token(Some(&bytes)));
    assert!(!keyring_unobservable(&f.config, &params));
    assert_eq!(
        super::update(&f.config, &params, "fixed", |_, _| Err(refuse(
            "storage_error",
            "injected"
        )))
        .unwrap_err()
        .code,
        "storage_error"
    );
    assert_eq!(fs::read(&path).unwrap(), bytes);
    params["operation"] = json!("validate");
    raw["connections"]["cn_0123456789ab"]["credential_ref"] =
        json!({"kind":"os_keyring","service":"fixture","account":"fixture"});
    let bytes = serde_json::to_vec(&raw).unwrap();
    fs::write(&path, &bytes).unwrap();
    // A stale revision is refused before any keyring lookup, so it is native either way.
    assert!(!keyring_unobservable(&f.config, &params));
    params["expected_revision"] = json!(write::token(Some(&bytes)));
    assert!(keyring_unobservable(&f.config, &params));
}

struct FakeKeyring {
    behavior: String,
    calls: std::cell::RefCell<Vec<Value>>,
}
impl keyring::Backend for FakeKeyring {
    fn get_password(
        &self,
        service: &str,
        account: &str,
    ) -> std::result::Result<Option<String>, String> {
        self.calls
            .borrow_mut()
            .push(json!(["get_password", service, account]));
        match self.behavior.as_str() {
            "present" => Ok(Some("synthetic-not-a-key".into())),
            "empty" => Ok(Some(String::new())),
            "absent" => Ok(None),
            _ => Err("RuntimeError".into()),
        }
    }
}
fn secrets_corpus() -> Value {
    serde_json::from_str(include_str!(
        "../../tests/fixtures/settings-secrets-python.json"
    ))
    .unwrap()
}
fn folder_files(f: &Fixture) -> Value {
    let mut files = json!({});
    for e in fs::read_dir(f.folder()).unwrap() {
        let p = e.unwrap().path();
        files[&format!("config/cortex/{}", p.file_name().unwrap().to_str().unwrap())] = json!({"content":fs::read_to_string(&p).unwrap(),"mode":fs::metadata(&p).unwrap().permissions().mode() & 0o777});
    }
    files
}

#[test]
fn keyring_validation_matches_recorded_oracle_replies_effects_and_lookups() {
    let c = secrets_corpus();
    assert_eq!(c["real_keyring_calls"], 0);
    let cases: Vec<_> = c["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case["variant"].get("keyring").is_some())
        .collect();
    assert_eq!(cases.len(), 12);
    for case in cases {
        let f = Fixture::new();
        crate::local::private_tree(&f.folder()).unwrap();
        for (key, v) in case["before"].as_object().unwrap() {
            if let (Some(name), Some(content)) =
                (key.strip_prefix("config/cortex/"), v["content"].as_str())
            {
                let path = f.folder().join(name);
                fs::write(&path, content).unwrap();
                fs::set_permissions(
                    &path,
                    fs::Permissions::from_mode(v["mode"].as_u64().unwrap() as u32),
                )
                .unwrap();
            }
        }
        let behavior = case["variant"]["keyring"].as_str().unwrap();
        let fake = FakeKeyring {
            behavior: behavior.into(),
            calls: Default::default(),
        };
        // Only an actual provider import can establish the missing-module branch.
        let backend: Option<&dyn keyring::Backend> = (behavior != "missing").then_some(&fake);
        let params = &case["params"];
        let native_refusal =
            case["variant"]["stale"] == true || case["variant"]["invalid_identifier"] == true;
        assert_eq!(
            keyring_unobservable(&f.config, params),
            !native_refusal,
            "{}",
            case["name"]
        );
        if behavior == "missing" {
            let before = folder_files(&f);
            assert_eq!(
                super::update(&f.config, params, "fixed", |_, _| panic!(
                    "must not persist"
                ))
                .unwrap_err()
                .code,
                "config_action_unavailable"
            );
            assert_eq!(folder_files(&f), before);
            continue;
        }
        let reply = crate::machine::native::envelope(super::update_with(
            &f.config,
            params,
            c["now"].as_str().unwrap(),
            backend,
            write::replace,
        ));
        assert_eq!(reply, case["reply"], "reply {}", case["name"]);
        let mut expected = case["after"].as_object().unwrap().clone();
        expected.retain(|k, v| k.starts_with("config/cortex/") && v.get("content").is_some());
        assert_eq!(
            folder_files(&f),
            json!(expected),
            "effects {}",
            case["name"]
        );
        assert_eq!(
            json!(*fake.calls.borrow()),
            case["keyring_calls"],
            "lookups {}",
            case["name"]
        );
    }
}

#[test]
fn keyring_validation_refuses_unobserved_state_with_or_without_provider() {
    let c = secrets_corpus();
    let case = c["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| {
            case["variant"]["keyring"] == "present"
                && case["variant"].get("stale").is_none()
                && case["variant"]["state"] == "configured"
        })
        .unwrap();
    let f = Fixture::new();
    crate::local::private_tree(&f.folder()).unwrap();
    let content = case["before"]["config/cortex/connections.json"]["content"]
        .as_str()
        .unwrap();
    let path = f.folder().join("connections.json");
    fs::write(&path, content).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(keyring_unobservable(&f.config, &case["params"]));
    let error = super::update(&f.config, &case["params"], "fixed", |_, _| {
        panic!("must not write")
    })
    .unwrap_err();
    assert_eq!(error.code, "config_action_unavailable");
    assert_eq!(fs::read_to_string(&path).unwrap(), content);
}
#[test]
fn removal_waiting_for_shared_lock_rechecks_revision() {
    let c = corpus();
    let case = c["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "remove-keyring")
        .unwrap();
    let f = Fixture::new();
    let params = f.prepare(case);
    let lock = write::owner_lock(&f.folder()).unwrap();
    let config = f.config.clone();
    let worker =
        std::thread::spawn(move || super::update(&config, &params, "fixed", write::replace));
    std::thread::sleep(Duration::from_millis(30));
    let path = f.folder().join("connections.json");
    let mut bytes = fs::read(&path).unwrap();
    bytes.push(b' ');
    fs::write(&path, &bytes).unwrap();
    drop(lock);
    assert_eq!(
        worker.join().unwrap().unwrap_err().code,
        "revision_conflict"
    );
    assert_eq!(fs::read(path).unwrap(), bytes);
}

#[test]
fn secret_removal_never_reads_bytes_and_partial_failure_stays_unknown() {
    let c = corpus();
    let case = c["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "remove-keyring")
        .unwrap();
    for shape in ["regular", "hardlink", "linked-parent"] {
        let f = Fixture::new();
        let mut params = f.prepare(case);
        let path = f.folder().join("connections.json");
        let secret = f.folder().join("secrets/cn_0123456789ab");
        let outside = f.root.join("outside");
        crate::local::private_tree(&outside).unwrap();
        if shape == "linked-parent" {
            std::os::unix::fs::symlink(&outside, secret.parent().unwrap()).unwrap();
        } else {
            crate::local::private_tree(secret.parent().unwrap()).unwrap();
        }
        fs::write(&secret, b"synthetic, not a key").unwrap();
        if shape == "hardlink" {
            fs::hard_link(&secret, outside.join("kept")).unwrap();
        }
        let mut raw: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        raw["connections"]["cn_0123456789ab"]["credential_ref"] =
            json!({"kind":"secret_file","path":secret});
        let before = serde_json::to_vec(&raw).unwrap();
        fs::write(&path, &before).unwrap();
        params["expected_revision"] = json!(write::token(Some(&before)));
        let mut calls = 0;
        let error = super::update(&f.config, &params, "fixed", |_, _| {
            calls += 1;
            Err(refuse("storage_error", "injected registry failure"))
        })
        .unwrap_err();
        assert_eq!(
            error.code,
            if shape == "regular" {
                "machine_outcome_unknown"
            } else if shape == "hardlink" {
                "storage_error"
            } else {
                "unsafe_storage"
            }
        );
        assert_eq!(calls, usize::from(shape != "linked-parent"));
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(secret.exists(), shape != "regular");
        if shape == "hardlink" {
            assert!(outside.join("kept").is_file());
        }
    }
}
#[test]
fn unsafe_lock_and_target_preserve_outside_bytes() {
    let c = corpus();
    let case = c["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "remove-keyring")
        .unwrap();
    for target in ["connections.lock", "connections.json"] {
        let f = Fixture::new();
        let params = f.prepare(case);
        let outside = f.root.join("outside");
        fs::write(&outside, b"untouched").unwrap();
        let path = f.folder().join(target);
        if path.exists() {
            fs::remove_file(&path).unwrap();
        }
        std::os::unix::fs::symlink(&outside, path).unwrap();
        assert_eq!(
            super::update(&f.config, &params, "fixed", |_, _| panic!("must not write"))
                .unwrap_err()
                .code,
            "unsafe_storage"
        );
        assert_eq!(fs::read(outside).unwrap(), b"untouched");
    }
}

#[test]
fn removal_with_unreadable_defaults_is_refused_naming_the_file_without_writing() {
    let c = corpus();
    let case = c["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "remove-keyring")
        .unwrap();
    let f = Fixture::new();
    let params = f.prepare(case);
    let defaults = f.folder().join("defaults.json");
    fs::write(&defaults, b"{broken").unwrap();
    fs::set_permissions(&defaults, fs::Permissions::from_mode(0o600)).unwrap();
    let registry = f.folder().join("connections.json");
    let before = fs::read(&registry).unwrap();
    let error = super::update(&f.config, &params, "fixed", |_, _| {
        panic!("must not write on uncertain input")
    })
    .unwrap_err();
    assert_eq!(error.code, "invalid_settings");
    assert!(error.message.contains("defaults.json"));
    assert_eq!(fs::read(&registry).unwrap(), before);
    assert_eq!(fs::read(&defaults).unwrap(), b"{broken");
}
