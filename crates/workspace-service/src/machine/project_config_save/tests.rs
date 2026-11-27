use super::*;
use std::{
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "cw-psv-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o700)).unwrap();
        Self(p)
    }
    fn config(&self) -> crate::Config {
        let config = self.0.join("config");
        fs::create_dir_all(config.join("cortex")).unwrap();
        fs::set_permissions(config.join("cortex"), fs::Permissions::from_mode(0o700)).unwrap();
        let mut launch = crate::LaunchSpec::bare();
        launch.dev_config = Some(config);
        crate::Config {
            runtime_dir: self.0.join("run"),
            state_dir: self.0.join("ws"),
            allowed_root: self.0.clone(),
            launch,
            request_timeout: Duration::from_secs(5),
            shutdown_timeout: Duration::from_millis(100),
            event_capacity: 16,
        }
    }
    fn root(&self) -> PathBuf {
        let p = self.0.join("repo");
        fs::create_dir(&p).unwrap();
        p
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn write(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}
fn rebind(v: &Value, root: &Path, config: &Path) -> Value {
    match v {
        Value::String(s) => json!(s
            .replace(
                "$ROOT/.cortex/",
                &format!("{}/", context::key(root).unwrap().directory().display())
            )
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
fn rows(root: &Path) -> Value {
    let Some(s) = context::open(root, cortex_core::OpenMode::LiveReadOnly).unwrap() else {
        return json!([]);
    };
    json!(s.read(|db|{
        let mut q=db.prepare("SELECT project_root,defaults,ceiling,updated_at FROM project_configuration_defaults ORDER BY project_root")?;
        let r=q.query_map([],|r|Ok(vec![r.get::<_,String>(0)?,r.get(1)?,r.get(2)?,r.get(3)?]))?.collect::<std::result::Result<Vec<_>,_>>()?;Ok(r)
    }).unwrap())
}
#[tokio::test]
async fn project_save_matches_the_recorded_oracle_replies_and_disk_without_provider() {
    let fixtures: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/project-config-save-python.json"
    ))
    .unwrap();
    assert_eq!(fixtures["cases"].as_array().unwrap().len(), 94);
    assert_eq!(fixtures["processAttempts"], 0);
    for c in fixtures["cases"].as_array().unwrap() {
        let f = Fixture::new();
        let root = f.root();
        let cfg = f.config();
        let config = cfg.launch_config_root().unwrap();
        let owner = config.join("cortex");
        if c.get("user").is_some() || c.get("userRaw").is_some() {
            write(
                &owner.join("defaults.json"),
                c["userRaw"]
                    .as_str()
                    .map(str::as_bytes)
                    .unwrap_or_else(|| b""),
            );
            if c.get("user").is_some() {
                write(
                    &owner.join("defaults.json"),
                    c["user"].to_string().as_bytes(),
                );
            }
        }
        if let Some(raw) = c["registryRaw"].as_str() {
            write(&owner.join("connections.json"), raw.as_bytes());
        }
        if c.get("userRouting").is_some() {
            write(
                &owner.join("routing.json"),
                &c["initialUserRoutingBytes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|n| n.as_u64().unwrap() as u8)
                    .collect::<Vec<_>>(),
            );
        }
        let service = crate::Service::bind(cfg.clone()).await.unwrap();
        let id = service.register(&root, "Project save").await.unwrap()["projectId"]
            .as_str()
            .unwrap()
            .to_owned();
        let key = context::key(&root).unwrap();
        if c["store"] != false {
            let s =
                cortex_core::Store::open(key.clone(), cortex_core::OpenMode::Initialize).unwrap();
            if let Some(ceiling) = c["ceiling"].as_str() {
                s.write(|db| {
                    // Match the real capture's initial JSON bytes, including spacing.
                    let defaults = if c.get("defaults").is_some() {
                        c["defaults"]
                            .to_string()
                            .replace(":", ": ")
                            .replace(",", ", ")
                    } else {
                        "{}".into()
                    };
                    db.execute(
                        "INSERT INTO project_configuration_defaults VALUES (?1,?2,?3,'before')",
                        rusqlite::params![root.to_str().unwrap(), defaults, ceiling],
                    )?;
                    Ok(())
                })
                .unwrap();
            }
        }
        if c.get("routing").is_some() || c.get("routingRaw").is_some() {
            write(
                &key.directory().join("routing.json"),
                &c["initialRoutingBytes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|n| n.as_u64().unwrap() as u8)
                    .collect::<Vec<_>>(),
            );
        }
        for observation in c["observations"].as_array().unwrap() {
            let mut params = rebind(&observation["params"], &root, &config);
            let before = rows(&root);
            // Only successful preceding ceiling writes have a genuinely different clock.
            // Bind each optimistic token to our actual stored bytes, never invent a pass.
            if params["surface"] == "project_ceiling"
                && params["tokens"]["project_ceiling"] != "absent"
                && params["tokens"]["project_ceiling"] != format!("sha256:{}", "0".repeat(64))
            {
                if let Some(r) = before.as_array().unwrap().first() {
                    params["tokens"]["project_ceiling"] = json!(token(Some(
                        serde_json::to_vec(&json!([r[1], r[2], r[3]]))
                            .unwrap()
                            .as_slice()
                    )));
                }
            }
            let reply = service
                .machine(
                    serde_json::from_value(
                        json!({"op":"project.config.save","projectId":id,"params":params}),
                    )
                    .unwrap(),
                )
                .await
                .unwrap();
            let mut actual = reply.clone();
            let mut expected = rebind(&observation["reply"], &root, &config);
            let mut got_rows = rows(&root);
            let mut wanted_rows = rebind(&observation["rows"], &root, &config);
            if params["surface"] == "project_ceiling" {
                if reply["ok"] == true && reply["result"]["exists"] == true {
                    let r = &got_rows[0];
                    let t = r[3].as_str().unwrap();
                    assert!(
                        t.ends_with("+00:00") && t.contains('T') && t.len() >= 32,
                        "not a real UTC timestamp"
                    );
                    assert_eq!(reply["result"]["updated_at"], r[3]);
                    assert_eq!(
                        reply["result"]["revision"],
                        token(Some(
                            serde_json::to_vec(&json!([r[1], r[2], r[3]]))
                                .unwrap()
                                .as_slice()
                        ))
                    );
                    actual["result"]["updated_at"] = json!("$NOW");
                    expected["result"]["updated_at"] = json!("$NOW");
                    actual["result"]["revision"] = json!("$REV");
                    expected["result"]["revision"] = json!("$REV");
                }
                if reply["code"] == "revision_conflict" {
                    let a = reply["message"].as_str().unwrap();
                    let b = expected["message"].as_str().unwrap();
                    let start = a.find(", current ").unwrap() + 10;
                    let end = a[start..].find(')').unwrap() + start;
                    let bs = b.find(", current ").unwrap() + 10;
                    let be = b[bs..].find(')').unwrap() + bs;
                    actual["message"] = json!(format!("{}$REV{}", &a[..start], &a[end..]));
                    expected["message"] = json!(format!("{}$REV{}", &b[..bs], &b[be..]));
                }
                for r in got_rows.as_array_mut().unwrap() {
                    if r[3] != "before" {
                        r[3] = json!("$NOW");
                    }
                }
                for r in wanted_rows.as_array_mut().unwrap() {
                    if r[3] != "before" {
                        r[3] = json!("$NOW");
                    }
                }
            }
            assert_eq!(actual, expected, "reply: {} params={params}", c["name"]);
            assert_eq!(got_rows, wanted_rows, "rows: {}", c["name"]);
            let path = key.directory().join("routing.json");
            let bytes = fs::read(&path)
                .ok()
                .map(|b| json!(b))
                .unwrap_or(Value::Null);
            assert_eq!(
                bytes, observation["routingBytes"],
                "routing bytes: {}",
                c["name"]
            );
            if path.is_file() && reply["ok"] == true && params["surface"] == "routing" {
                assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o600);
            }
            if reply["ok"] == false {
                assert_eq!(rows(&root), before, "refusal mutated row: {}", c["name"]);
            }
        }
        service.stop().await.unwrap();
    }
}
#[tokio::test]
async fn concurrent_writers_refuse_stale_tokens_and_unsafe_targets() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let root = f.root();
    let cfg = f.config();
    let service = std::sync::Arc::new(crate::Service::bind(cfg.clone()).await.unwrap());
    let id = service.register(&root, "save races").await.unwrap()["projectId"]
        .as_str()
        .unwrap()
        .to_owned();
    cortex_core::Store::open(
        context::key(&root).unwrap(),
        cortex_core::OpenMode::Initialize,
    )
    .unwrap();
    let params = json!({"surface":"routing","tokens":{"routing":"absent"},"value":{"version":2}});
    let a = service.clone();
    let b = service.clone();
    let id2 = id.clone();
    let p = params.clone();
    let (a, b) = tokio::join!(
        async move {
            a.machine(
                serde_json::from_value(
                    json!({"op":"project.config.save","projectId":id2,"params":p}),
                )
                .unwrap(),
            )
            .await
            .unwrap()
        },
        async move {
            b.machine(
                serde_json::from_value(
                    json!({"op":"project.config.save","projectId":id,"params":params}),
                )
                .unwrap(),
            )
            .await
            .unwrap()
        }
    );
    assert_eq!([&a, &b].iter().filter(|r| r["ok"] == true).count(), 1);
    assert_eq!(
        [&a, &b]
            .iter()
            .filter(|r| r["code"] == "revision_conflict")
            .count(),
        1
    );
    let path = context::key(&root)
        .unwrap()
        .directory()
        .join("routing.json");
    let bytes = fs::read(&path).unwrap();
    let before = rows(&root);
    let params = json!({"surface":"routing","tokens":{"routing":token(Some(&bytes))},"value":{"version":2,"deny":["changed"]}});
    let config = cfg.launch_config_root().unwrap().join("cortex");
    let lock = config.join("connections.lock");
    fs::remove_file(&lock).unwrap();
    let foreign = f.0.join("not-lock");
    write(&foreign, b"unchanged");
    symlink(&foreign, &lock).unwrap();
    assert_eq!(
        save(&root, &cfg, &params).unwrap_err().code,
        "unsafe_storage"
    );
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(rows(&root), before);
    assert_eq!(fs::read(&foreign).unwrap(), b"unchanged");
    fs::remove_file(&lock).unwrap();
    fs::hard_link(&foreign, &lock).unwrap();
    assert_eq!(
        save(&root, &cfg, &params).unwrap_err().code,
        "unsafe_storage"
    );
    fs::remove_file(&lock).unwrap();
    fs::hard_link(&path, f.0.join("linked-routing")).unwrap();
    assert_eq!(
        save(&root, &cfg, &params).unwrap_err().code,
        "unsafe_storage"
    );
    assert_eq!(fs::read(&path).unwrap(), bytes);
    service.stop().await.unwrap();
}
#[test]
fn atomic_replace_refuses_unsafe_files_and_cleans_temporary_on_failure() {
    let f = Fixture::new();
    let path = f.0.join("routing.json");
    fs::create_dir(&path).unwrap();
    assert_eq!(replace(&path, b"new").unwrap_err().code, "unsafe_storage");
    assert_eq!(fs::read_dir(&f.0).unwrap().count(), 1);
    fs::remove_dir(&path).unwrap();
    write(&path, b"old");
    assert_eq!(
        replace(&path, &vec![b'x'; FRAME_BUDGET + 1])
            .unwrap_err()
            .code,
        "machine_reply_too_large"
    );
    assert_eq!(fs::read(&path).unwrap(), b"old");
    replace(&path, b"new").unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"new");
    assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(fs::read_dir(&f.0).unwrap().count(), 1);
}
#[tokio::test]
async fn failed_database_write_rolls_back_without_delegation() {
    let f = Fixture::new();
    let root = f.root();
    let cfg = f.config();
    let service = crate::Service::bind(cfg.clone()).await.unwrap();
    let id = service.register(&root, "save failure").await.unwrap()["projectId"]
        .as_str()
        .unwrap()
        .to_owned();
    let store = cortex_core::Store::open(
        context::key(&root).unwrap(),
        cortex_core::OpenMode::Initialize,
    )
    .unwrap();
    store.write(|db|{db.execute_batch("CREATE TRIGGER fail_test_ceiling BEFORE INSERT ON project_configuration_defaults BEGIN SELECT RAISE(ABORT,'fixture'); END;")?;Ok(())}).unwrap();
    let reply=service.machine(serde_json::from_value(json!({"op":"project.config.save","projectId":id,"params":{"surface":"project_ceiling","value":{},"tokens":{"project_ceiling":"absent"}}})).unwrap()).await.unwrap();
    assert_eq!(reply["code"], "machine_outcome_unknown");
    assert_eq!(rows(&root), json!([]));
    service.stop().await.unwrap();
}
#[test]
fn post_rename_failure_is_unknown_and_never_replayed() {
    let f = Fixture::new();
    let path = f.0.join("routing.json");
    write(&path, b"old");
    let err = replace_sync(&path, b"new", |_| {
        Err(refuse("injected_sync_failure", "fixture"))
    })
    .unwrap_err();
    assert_eq!(err.code, "machine_outcome_unknown");
    assert_eq!(fs::read(&path).unwrap(), b"new");
    assert_eq!(fs::read_dir(&f.0).unwrap().count(), 1);
}
#[tokio::test]
async fn exact_metadata_reply_budgets_and_exclusive_store_fence_refuse_without_effects() {
    let f = Fixture::new();
    let root = f.root();
    let cfg = f.config();
    let service = crate::Service::bind(cfg.clone()).await.unwrap();
    service.register(&root, "budget").await.unwrap();
    let key = context::key(&root).unwrap();
    cortex_core::Store::open(key.clone(), cortex_core::OpenMode::Initialize).unwrap();
    let path = key.directory().join("routing.json");
    let bytes = br#"{"model_policy_version":2,"metadata":18446744073709551616}"#;
    write(&path, bytes);
    let params =
        json!({"surface":"routing","value":{"version":2},"tokens":{"routing":token(Some(bytes))}});
    assert_eq!(
        save(&root, &cfg, &params).unwrap_err().code,
        "invalid_routing_overrides"
    );
    assert_eq!(fs::read(&path).unwrap(), bytes);
    fs::remove_file(&path).unwrap();
    let big = "x".repeat(FRAME_BUDGET / 2 + 1);
    let params = json!({"surface":"project_ceiling","value":{"allowedRoutes":[big]},"tokens":{"project_ceiling":"absent"}});
    assert_eq!(
        save(&root, &cfg, &params).unwrap_err().code,
        "machine_reply_too_large"
    );
    assert_eq!(rows(&root), json!([]));
    let lock = cortex_core::store::locks::AccessLock::acquire(
        &key.directory().join(".access.lock"),
        false,
        true,
        Duration::ZERO,
    )
    .unwrap();
    let params = json!({"surface":"routing","value":{"version":2},"tokens":{"routing":"absent"}});
    assert!(save(&root, &cfg, &params).is_err());
    assert!(!path.exists());
    drop(lock);
    assert_eq!(rows(&root), json!([]));
    service.stop().await.unwrap();
}
#[test]
fn observed_catalog_efforts_match_the_frozen_oracle_and_never_write() {
    let fixtures: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/project-config-save-python.json"
    ))
    .unwrap();
    assert_eq!(fixtures["effortCases"].as_array().unwrap().len(), 11);
    for case in fixtures["effortCases"].as_array().unwrap() {
        let f = Fixture::new();
        let state = f.0.join("cortex");
        fs::create_dir(&state).unwrap();
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
        for file in case["files"].as_array().unwrap() {
            let path = state.join(file["path"].as_str().unwrap());
            crate::local::private_tree(path.parent().unwrap()).unwrap();
            write(&path, file["raw"].as_str().unwrap().as_bytes());
            let time = std::time::UNIX_EPOCH + Duration::from_secs(file["mtime"].as_u64().unwrap());
            std::fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(time))
                .unwrap();
        }
        let got = routing::view::efforts(&state, &case["registry"]).unwrap();
        assert_eq!(got, case["expected"], "{}", case["name"]);
        for file in case["files"].as_array().unwrap() {
            assert_eq!(
                fs::read_to_string(state.join(file["path"].as_str().unwrap())).unwrap(),
                file["raw"].as_str().unwrap()
            );
        }
    }
}
#[test]
fn catalog_state_refuses_linked_and_project_nested_roots_without_reading_bodies() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let real = f.0.join("actual");
    fs::create_dir(&real).unwrap();
    let linked = f.0.join("linked");
    symlink(&real, &linked).unwrap();
    let registry = json!({"connections":{"cn_000000000001":{}}});
    assert_eq!(
        routing::view::efforts(&linked.join("cortex"), &registry)
            .unwrap_err()
            .code,
        "unsafe_storage"
    );
    fs::create_dir(real.join(".cortex")).unwrap();
    assert_eq!(
        routing::view::efforts(&real.join("cortex"), &registry)
            .unwrap_err()
            .code,
        "connection_storage_under_project"
    );
    assert_eq!(fs::read_dir(&real).unwrap().count(), 1);
}
