//! Service boundary oracle comparisons use no Python helper or engine.
use super::*;
use std::ffi::CString;
use std::{
    collections::BTreeMap,
    os::unix::fs::{symlink, PermissionsExt},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
struct Fixture {
    base: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        // A long caller TMPDIR must never lengthen the Unix socket.
        let base = PathBuf::from(format!(
            "/tmp/cw-pcfg-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&base).unwrap();
        fs::set_permissions(&base, fs::Permissions::from_mode(0o700)).unwrap();
        Self { base }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}
fn rebind(v: &Value, root: &Path, config: &Path) -> Value {
    match v {
        Value::String(s) => json!(s
            .replace("$ROOT", root.to_str().unwrap())
            .replace("$CONFIG", config.to_str().unwrap())),
        Value::Array(a) => json!(a
            .iter()
            .map(|v| rebind(v, root, config))
            .collect::<Vec<_>>()),
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, v)| (k.clone(), rebind(v, root, config)))
                .collect(),
        ),
        v => v.clone(),
    }
}
fn write(path: &Path, bytes: &[u8], mode: u32) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut result = BTreeMap::new();
    for entry in fs::read_dir(root).unwrap() {
        let p = entry.unwrap().path();
        // SQLite may update shared-memory reader slots without writing authority.
        if p.file_name()
            .is_some_and(|n| n.to_string_lossy().ends_with("-shm"))
        {
            continue;
        }
        if p.is_symlink() {
            result.insert(
                p.clone(),
                fs::read_link(&p)
                    .unwrap()
                    .as_os_str()
                    .as_encoded_bytes()
                    .to_vec(),
            );
        } else if p.is_dir() {
            result.extend(snapshot(&p));
        } else {
            result.insert(p.clone(), fs::read(p).unwrap());
        }
    }
    result
}
#[tokio::test]
async fn project_config_matches_the_oracle_without_provider_and_never_mutates_authority() {
    let fixtures: Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/project-config-python.json"
    ))
    .unwrap();
    assert_eq!(fixtures["cases"].as_array().unwrap().len(), 125);
    for case in fixtures["cases"].as_array().unwrap() {
        let f = Fixture::new();
        let root = f.base.join("repo");
        fs::create_dir(&root).unwrap();
        if case["linked"] == true {
            let main = f.base.join("main");
            fs::create_dir(&main).unwrap();
            let git = |args: &[&str]| {
                let status = std::process::Command::new("/usr/bin/git")
                    .args(["-c", "core.hooksPath=/dev/null"])
                    .args(args)
                    .current_dir(&main)
                    .env_clear()
                    .env("PATH", "/usr/bin:/bin")
                    .env("GIT_CONFIG_GLOBAL", "/dev/null")
                    .env("GIT_CONFIG_NOSYSTEM", "1")
                    .env("GIT_AUTHOR_NAME", "Fixture")
                    .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
                    .env("GIT_COMMITTER_NAME", "Fixture")
                    .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
                    .output()
                    .unwrap();
                assert!(status.status.success(), "disposable Git fixture failed");
            };
            git(&["init", "-b", "main"]);
            git(&["commit", "--allow-empty", "-m", "Fixture"]);
            git(&["worktree", "add", "-b", "linked", root.to_str().unwrap()]);
            let peer = context::key(&main).unwrap();
            cortex_core::Store::open(peer.clone(), OpenMode::Initialize).unwrap();
            write(
                &peer.directory().join("routing.json"),
                b"{\"model_policy_version\":2}",
                0o600,
            );
        }
        let config = f.base.join("config");
        fs::create_dir(&config).unwrap();
        let owner = config.join("cortex");
        fs::create_dir(&owner).unwrap();
        fs::set_permissions(&owner, fs::Permissions::from_mode(0o700)).unwrap();
        if case["userFolderLink"] == true {
            fs::remove_dir(&owner).unwrap();
            let other = f.base.join("other");
            fs::create_dir(&other).unwrap();
            symlink(other, &owner).unwrap();
        }
        if case.get("user").is_some() || case.get("userRaw").is_some() {
            let s = case["userRaw"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| case["user"].to_string());
            write(
                &owner.join("defaults.json"),
                s.as_bytes(),
                case["userMode"].as_u64().unwrap_or(0o600) as u32,
            );
        }
        if case["userLink"] == true {
            let other = f.base.join("elsewhere");
            write(&other, b"{}", 0o600);
            symlink(other, owner.join("defaults.json")).unwrap();
        }
        if let Some(v) = case.get("userRouting") {
            write(&owner.join("routing.json"), v.to_string().as_bytes(), 0o600);
        }
        if case.get("registry").is_some() || case.get("registryRaw").is_some() {
            let s = case["registryRaw"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| case["registry"].to_string());
            write(&owner.join("connections.json"), s.as_bytes(), 0o600);
        }
        let mut launch = crate::LaunchSpec::bare();
        launch.dev_config = Some(config.clone());
        let service = crate::Service::bind(crate::Config {
            runtime_dir: f.base.join("run"),
            state_dir: f.base.join("ws"),
            allowed_root: f.base.clone(),
            launch,
            request_timeout: Duration::from_secs(5),
            shutdown_timeout: Duration::from_millis(100),
            event_capacity: 16,
        })
        .await
        .unwrap();
        let id = service.register(&root, "Project config").await.unwrap()["projectId"]
            .as_str()
            .unwrap()
            .to_owned();
        let key = context::key(&root).unwrap();
        let store = if case["store"] != false {
            Some(cortex_core::Store::open(key.clone(), OpenMode::Initialize).unwrap())
        } else {
            None
        };
        if let Some(store) = &store {
            let rows = rebind(&case["sql"], &root, &config);
            store
                .write(|db| {
                    for row in rows.as_array().unwrap() {
                        let params = row[1]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|v| v.as_str().unwrap().to_owned())
                            .collect::<Vec<_>>();
                        db.execute(row[0].as_str().unwrap(), rusqlite::params_from_iter(params))?;
                    }
                    Ok(())
                })
                .unwrap();
        }
        let path = key.directory().join("routing.json");
        if [
            "routing",
            "routingRaw",
            "routingBytes",
            "routingLink",
            "routingDirectory",
        ]
        .iter()
        .any(|k| case.get(*k).is_some())
        {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            if case["routingLink"] == true {
                let other = f.base.join("routing");
                write(&other, b"{\"model_policy_version\":2}", 0o600);
                symlink(other, &path).unwrap();
            } else if case["routingDirectory"] == true {
                fs::create_dir(&path).unwrap();
            } else {
                let bytes = if let Some(a) = case["routingEncoded"].as_array() {
                    a.iter().map(|v| v.as_u64().unwrap() as u8).collect()
                } else {
                    case["routingRaw"]
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| case["routing"].to_string())
                        .into_bytes()
                };
                write(&path, &bytes, 0o600);
            }
        }
        let before = snapshot(&root);
        let peer_before = (case["linked"] == true).then(|| snapshot(&f.base.join("main")));
        let config_before = snapshot(&config);
        let reply = service
            .machine(
                serde_json::from_value(
                    json!({"op":"project.config.read","projectId":id,"params":{}}),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let expected = rebind(&case["expected"], &root, &config);
        // The project's own MCP registry is additive to the frozen oracle: none exists in
        // these cases, so it reads as empty with an absent revision, then leaves the compared shape.
        let mut reply = reply;
        if reply["ok"] == true {
            assert_eq!(
                reply["result"]["value"]["mcp"],
                json!({"servers":[],"overrides":{}}),
                "{}",
                case["name"]
            );
            assert_eq!(reply["result"]["tokens"]["mcp_servers"], "absent");
            assert_eq!(reply["result"]["value"]["revisions"]["mcp_servers"], 0);
            reply["result"]["value"].as_object_mut().unwrap().remove("mcp");
            reply["result"]["value"]["revisions"].as_object_mut().unwrap().remove("mcp_servers");
            reply["result"]["tokens"].as_object_mut().unwrap().remove("mcp_servers");
        }
        assert_eq!(reply, expected, "{}", case["name"]);
        assert_eq!(snapshot(&root), before, "project mutated: {}", case["name"]);
        assert_eq!(
            snapshot(&config),
            config_before,
            "config mutated: {}",
            case["name"]
        );
        if let Some(before) = peer_before {
            assert_eq!(
                snapshot(&f.base.join("main")),
                before,
                "shared peer store changed"
            );
        }
        service.stop().await.unwrap();
    }
}
#[test]
fn project_config_bounds_and_unsafe_files_never_follow_or_block() {
    let f = Fixture::new();
    let path = f.base.join("settings");
    let c = CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    let start = std::time::Instant::now();
    assert_eq!(read_plain(&path, true).unwrap_err().code, "unsafe_storage");
    assert!(start.elapsed() < Duration::from_secs(1));
    fs::remove_file(&path).unwrap();
    write(&path, b"unchanged", 0o600);
    let alias = f.base.join("alias");
    fs::hard_link(&path, &alias).unwrap();
    assert_eq!(read_plain(&path, true).unwrap_err().code, "unsafe_storage");
    fs::remove_file(&alias).unwrap();
    fs::remove_file(&path).unwrap();
    symlink(&alias, &path).unwrap();
    assert_eq!(read_plain(&path, true).unwrap_err().code, "unsafe_storage");
    assert!(!alias.exists());
    fs::remove_file(&path).unwrap();
    let file = fs::File::create(&path).unwrap();
    file.set_len(FRAME_BUDGET as u64).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        read_plain(&path, true).unwrap().unwrap().len(),
        FRAME_BUDGET
    );
    file.set_len(FRAME_BUDGET as u64 + 1).unwrap();
    assert_eq!(
        read_plain(&path, true).unwrap_err().code,
        "machine_reply_too_large"
    );
}
#[test]
fn project_config_oversized_rows_and_project_nested_owner_storage_refuse() {
    let f = Fixture::new();
    let root = f.base.join("repo");
    fs::create_dir(&root).unwrap();
    let config_root = f.base.join("config");
    fs::create_dir(&config_root).unwrap();
    let mut launch = crate::LaunchSpec::bare();
    launch.dev_config = Some(config_root);
    let mut config = crate::Config {
        runtime_dir: f.base.join("run"),
        state_dir: f.base.join("ws"),
        allowed_root: f.base.clone(),
        launch,
        request_timeout: Duration::from_secs(5),
        shutdown_timeout: Duration::from_millis(100),
        event_capacity: 16,
    };
    let store =
        cortex_core::Store::open(context::key(&root).unwrap(), OpenMode::Initialize).unwrap();
    store
        .write(|db| {
            db.execute(
                "INSERT INTO project_configuration_defaults VALUES (?,?,?,'now')",
                rusqlite::params![root.to_str().unwrap(), "x".repeat(FRAME_BUDGET + 1), "{}"],
            )?;
            Ok(())
        })
        .unwrap();
    let before = snapshot(&root);
    assert_eq!(
        read(&root, &config).unwrap_err().code,
        "machine_reply_too_large"
    );
    assert_eq!(snapshot(&root), before);
    config.launch.dev_config = Some(root.join("config"));
    assert_eq!(
        owner_folder(&config).unwrap_err().code,
        "connection_storage_under_project"
    );
    let link = f.base.join("linked-config");
    symlink(f.base.join("config"), &link).unwrap();
    config.launch.dev_config = Some(link);
    assert_eq!(owner_folder(&config).unwrap_err().code, "unsafe_storage");
}
