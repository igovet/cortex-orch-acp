use super::*;
use std::{
    os::unix::fs::PermissionsExt,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
struct Fixture {
    root: std::path::PathBuf,
    config: crate::Config,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "cw-apply-unit-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        crate::local::private_tree(&root).unwrap();
        let mut launch = crate::LaunchSpec::bare();
        launch.dev_config = Some(root.join("config"));
        Self {
            config: crate::Config {
                runtime_dir: root.join("run"),
                state_dir: root.join("state"),
                allowed_root: root.clone(),
                launch,
                request_timeout: Duration::from_secs(5),
                shutdown_timeout: Duration::from_millis(100),
                event_capacity: 16,
            },
            root,
        }
    }
    fn folder(&self) -> std::path::PathBuf {
        self.root.join("config/cortex")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
#[test]
fn apply_the_recorded_oracle_complete_replies_files_and_modes() {
    let c: Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/config-apply-python.json"
    ))
    .unwrap();
    for case in c["cases"].as_array().unwrap() {
        let f = Fixture::new();
        crate::local::private_tree(&f.folder()).unwrap();
        for (n, b) in case["initial"].as_object().unwrap() {
            let p = f.folder().join(n);
            fs::write(&p, b.as_str().unwrap()).unwrap();
            fs::set_permissions(p, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let actual = super::super::super::native::envelope(apply_with(
            &f.config,
            c["now"].as_str().unwrap(),
            create,
        ));
        let expand = |v: &Value| -> Value {
            serde_json::from_str(
                &v.to_string()
                    .replace("$CONFIG", f.folder().to_str().unwrap()),
            )
            .unwrap()
        };
        assert_eq!(actual, expand(&case["reply"]), "{}", case["name"]);
        let mut files = json!({});
        for e in fs::read_dir(f.folder()).unwrap() {
            let p = e.unwrap().path();
            files[p.file_name().unwrap().to_str().unwrap()] = json!({"content":fs::read_to_string(&p).unwrap(),
                "mode":fs::metadata(&p).unwrap().permissions().mode() & 0o777});
        }
        assert_eq!(files, expand(&case["files"]), "{}", case["name"]);
        assert_eq!(
            fs::metadata(f.folder()).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
}
#[test]
fn publication_never_replaces_a_racing_owner_file_or_replays_uncertainty() {
    let f = Fixture::new();
    crate::local::private_tree(&f.folder()).unwrap();
    let p = f.folder().join("routing.json");
    fs::write(&p, b"owner bytes").unwrap();
    assert!(!create(&p, b"replacement").unwrap());
    assert_eq!(fs::read(&p).unwrap(), b"owner bytes");
    fs::remove_file(p).unwrap();
    let mut calls = 0;
    let error = apply_with(&f.config, "clock", |p, b| {
        calls += 1;
        create(p, b)?;
        Err(refuse(
            "machine_outcome_unknown",
            "injected post-publication",
        ))
    })
    .unwrap_err();
    assert_eq!(calls, 1);
    assert_eq!(error.code, "machine_outcome_unknown");
    assert!(f.folder().join("routing.json").exists());
    assert!(!f.folder().join("defaults.json").exists());
}
#[test]
fn fresh_root_and_partial_publication_are_explicit() {
    let f = Fixture::new();
    assert!(!f.folder().exists());
    let mut calls = 0;
    let error = apply_with(&f.config, "clock", |p, b| {
        calls += 1;
        if calls == 2 {
            return Err(refuse(
                "storage_error",
                "injected before second publication",
            ));
        }
        create(p, b)
    })
    .unwrap_err();
    assert_eq!(error.code, "machine_outcome_unknown");
    assert_eq!(calls, 2);
    assert!(f.folder().join("routing.json").is_file());
    assert!(!f.folder().join("defaults.json").exists());
    assert_eq!(
        fs::metadata(f.folder()).unwrap().permissions().mode() & 0o777,
        0o700
    );
    // Absent-only recovery publishes the remainder without replacing the first.
    let first = fs::read(f.folder().join("routing.json")).unwrap();
    assert!(apply_with(&f.config, "clock", create).unwrap()["changed"] == true);
    assert_eq!(fs::read(f.folder().join("routing.json")).unwrap(), first);
    assert!(f.folder().join("routing-profiles.json").is_file());
}
#[test]
fn unclassifiable_existing_documents_are_refused_naming_the_file_without_effects() {
    for (name, body) in [
        ("routing.json", "{broken"),
        ("defaults.json", "{broken"),
        ("mcp-servers.json", "{broken"),
        ("connections.json", "{broken"),
    ] {
        let f = Fixture::new();
        crate::local::private_tree(&f.folder()).unwrap();
        let p = f.folder().join(name);
        fs::write(&p, body).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
        match apply_with(&f.config, "clock", create) {
            // An existing file is diagnosed, never replaced; the diagnosis names it.
            Ok(reply) => {
                let key = name.trim_end_matches(".json").replace('-', "_");
                let key = if key == "mcp_servers" || key == "routing" || key == "defaults" {
                    key
                } else {
                    unreachable!("{name}")
                };
                let row = &reply["settings"][key];
                assert_eq!(row["changed"], false, "{name}");
                assert!(
                    !["current", "created"].contains(&row["status"].as_str().unwrap()),
                    "{name}"
                );
            }
            Err(error) => {
                assert!(!error.message.is_empty(), "{name}");
                if name != "connections.json" {
                    assert_eq!(error.code, "invalid_settings", "{name}");
                    assert!(error.message.contains(name), "{name}");
                }
            }
        }
        // The offending file is never modified.
        assert_eq!(fs::read_to_string(&p).unwrap(), body);
    }
}
