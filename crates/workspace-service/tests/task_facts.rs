//! Stored Python-oracle DTOs exercised without Python or native engines at test time.
#[path = "support/hermetic.rs"]
mod hermetic;
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use workspace_service::{Config, LaunchSpec, Service};
struct Fixture {
    base: PathBuf,
    root: PathBuf,
    state: PathBuf,
    id: String,
    service: std::sync::Arc<Service>,
}
impl Fixture {
    async fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        // Keep this service fixture's socket root short even under a long TMPDIR.
        let base = Path::new("/tmp").join(format!(
            "cortex-facts-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&base).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&base, fs::Permissions::from_mode(0o700)).unwrap();
        let root = base.join("repo");
        fs::create_dir(&root).unwrap();
        let state = base.join("state");
        let service = Service::bind(Config {
            runtime_dir: base.join("runtime"),
            state_dir: state.clone(),
            allowed_root: base.clone(),
            launch: LaunchSpec::bare(),
            request_timeout: Duration::from_secs(10),
            shutdown_timeout: Duration::from_millis(100),
            event_capacity: 16,
        })
        .await
        .unwrap();
        let id = service.register(&root, "Facts").await.unwrap()["projectId"]
            .as_str()
            .unwrap()
            .to_owned();
        Self {
            base,
            root,
            state,
            id,
            service,
        }
    }
    fn git(&self, args: &[&str]) {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(&self.root)
            .env("HOME", &self.base)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(output.status.success(), "fixture Git failed");
    }
    fn store(&self) -> cortex_core::Store {
        let channel = if workspace_protocol::channel::current().is_dev() {
            cortex_core::StoreChannel::Dev
        } else {
            cortex_core::StoreChannel::Release
        };
        cortex_core::Store::open(
            cortex_core::ProjectKey::new(&self.root, channel).unwrap(),
            cortex_core::OpenMode::Initialize,
        )
        .unwrap()
    }
    fn save(&self, value: &Value) {
        let p = self.state.join("product");
        fs::create_dir_all(&p).unwrap();
        fs::write(p.join(format!("{}.json", self.id)), value.to_string()).unwrap();
    }
    async fn call(&self, op: &str, params: Value) -> std::io::Result<Value> {
        self.service
            .machine(
                serde_json::from_value(json!({"op":op,"projectId":self.id,"params":params}))
                    .unwrap(),
            )
            .await
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}
fn replace(value: &Value, root: &Path, project: &str) -> Value {
    match value {
        Value::String(s) => json!(s
            .replace("$ROOT", root.to_str().unwrap())
            .replace("p1", project)),
        Value::Array(a) => Value::Array(a.iter().map(|v| replace(v, root, project)).collect()),
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, v)| (k.clone(), replace(v, root, project)))
                .collect(),
        ),
        v => v.clone(),
    }
}
fn sql(value: &Value) -> rusqlite::types::Value {
    match value {
        Value::Null => rusqlite::types::Value::Null,
        Value::String(s) => s.clone().into(),
        Value::Number(n) => n.as_i64().unwrap().into(),
        _ => panic!("unsupported fixture SQL value"),
    }
}
fn database_snapshot(store: &cortex_core::Store) -> Value {
    store
        .read(|db| {
            let mut result = serde_json::Map::new();
            for table in [
                "runtime_tasks",
                "runtime_nodes",
                "runtime_events",
                "task_owners",
                "workspace_tasks",
                "workspace_executions",
                "workspace_starts",
                "workspace_task_options",
                "workspace_outbox",
                "task_workspaces",
                "attempt_revisions",
                "large_outputs",
                "delegation_provisioning",
            ] {
                let mut query = db.prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))?;
                let columns = query.column_count();
                let rows = query
                    .query_map([], |r| {
                        (0..columns)
                            .map(|i| {
                                Ok(match r.get_ref(i)? {
                                    rusqlite::types::ValueRef::Null => Value::Null,
                                    rusqlite::types::ValueRef::Integer(n) => json!(n),
                                    rusqlite::types::ValueRef::Real(n) => json!(n),
                                    rusqlite::types::ValueRef::Text(s) => {
                                        json!(String::from_utf8_lossy(s))
                                    }
                                    rusqlite::types::ValueRef::Blob(b) => json!(b),
                                })
                            })
                            .collect::<rusqlite::Result<Vec<_>>>()
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                result.insert(table.to_owned(), json!(rows));
            }
            Ok(Value::Object(result))
        })
        .unwrap()
}
#[tokio::test]
async fn stored_oracle_task_facts_match_without_helper_or_activation() {
    let fixtures: Value = serde_json::from_str(include_str!("fixtures/task-facts.json")).unwrap();
    let work: Value = serde_json::from_str(include_str!("fixtures/task-work-facts.json")).unwrap();
    let provision: Value =
        serde_json::from_str(include_str!("fixtures/task-provision-facts.json")).unwrap();
    let attempts: Value =
        serde_json::from_str(include_str!("fixtures/task-attempt-facts.json")).unwrap();
    let source: Value =
        serde_json::from_str(include_str!("fixtures/task-source-nonpass-facts.json")).unwrap();
    let verification: Value =
        serde_json::from_str(include_str!("fixtures/task-source-verification.json")).unwrap();
    let git_source: Value =
        serde_json::from_str(include_str!("fixtures/task-git-source-verification.json")).unwrap();
    let rerun: Value =
        serde_json::from_str(include_str!("fixtures/task-rerun-closures.json")).unwrap();
    assert_eq!(fixtures["sourceSha256"], rerun["sourceSha256"]);
    assert_eq!(fixtures["sourceSha256"], git_source["sourceSha256"]);
    assert_eq!(fixtures["sourceSha256"], source["sourceSha256"]);
    assert_eq!(fixtures["sourceSha256"], attempts["sourceSha256"]);
    assert_eq!(fixtures["sourceSha256"], work["sourceSha256"]);
    assert_eq!(fixtures["sourceSha256"], provision["sourceSha256"]);
    assert_eq!(
        fixtures["sourceSha256"],
        "b8232d24f056283777303170c023dbd2cff760b0dc4d7bc458760ca83b9e6b36"
    );
    for case in fixtures["cases"]
        .as_array()
        .unwrap()
        .iter()
        .chain(work["cases"].as_array().unwrap())
        .chain(provision["cases"].as_array().unwrap())
        .chain(attempts["cases"].as_array().unwrap())
        .chain(source["cases"].as_array().unwrap())
        .chain(verification["cases"].as_array().unwrap())
        .chain(git_source["cases"].as_array().unwrap())
        .chain(rerun["cases"].as_array().unwrap())
    {
        let f = Fixture::new().await;
        if case["git"] == true {
            f.git(&["init", "-b", "main"]);
        }
        let _store = if case["store"] == true {
            let store = f.store();
            store
                .write(|db| {
                    for row in case["rows"].as_array().unwrap() {
                        let row = replace(row, &f.root, &f.id);
                        let params = row[1]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(sql)
                            .collect::<Vec<_>>();
                        db.execute(row[0].as_str().unwrap(), rusqlite::params_from_iter(params))?;
                    }
                    Ok(())
                })
                .unwrap();
            Some(store)
        } else {
            None
        };
        for file in case["files"].as_array().into_iter().flatten() {
            let store = _store.as_ref().unwrap();
            let path = store.key().directory().join(file["path"].as_str().unwrap());
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
            let content = file["content"].as_str().unwrap();
            let task_dir = store.key().directory().join("r1");
            if task_dir.exists() {
                fs::set_permissions(task_dir, fs::Permissions::from_mode(0o700)).unwrap();
            }
            if file["fifo"] == true {
                use std::os::unix::ffi::OsStrExt;
                let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
                assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
            } else if file["symlink"] == true {
                fs::write(path.parent().unwrap().join("target.md"), content).unwrap();
                std::os::unix::fs::symlink("target.md", &path).unwrap();
            } else {
                fs::write(&path, content).unwrap();
                fs::set_permissions(
                    &path,
                    fs::Permissions::from_mode(if file["public"] == true { 0o644 } else { 0o600 }),
                )
                .unwrap();
                if file["hardlink"] == true {
                    fs::hard_link(&path, path.parent().unwrap().join("linked.bin")).unwrap();
                }
            }
            if file["public_directory"] == true {
                fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o755))
                    .unwrap();
            }
        }
        for entry in case["entries"]
            .as_array()
            .into_iter()
            .flatten()
            .chain(case["drift"].as_array().into_iter().flatten())
        {
            let path = f.root.join(entry["path"].as_str().unwrap());
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            match entry["kind"].as_str().unwrap() {
                "d" => fs::create_dir_all(path).unwrap(),
                "f" => fs::write(path, entry["text"].as_str().unwrap()).unwrap(),
                "l" => std::os::unix::fs::symlink(entry["target"].as_str().unwrap(), path).unwrap(),
                _ => panic!("unsupported source fixture"),
            }
        }
        if let Some(path) = case["delete"].as_str() {
            fs::remove_file(f.root.join(path)).unwrap();
        }
        if let Some(target) = case["link"].as_str() {
            fs::remove_file(f.root.join("alias")).unwrap();
            std::os::unix::fs::symlink(target, f.root.join("alias")).unwrap();
        }
        for command in case["git_commands"].as_array().into_iter().flatten() {
            let args = command
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect::<Vec<_>>();
            f.git(&args);
        }
        f.save(case.get("saved").unwrap_or(&json!({})));
        let expected = replace(&case["expected"], &f.root, &f.id);
        let before = case
            .get("entries")
            .map(|_| database_snapshot(_store.as_ref().unwrap()));
        let overlay_path = f.state.join("product").join(format!("{}.json", f.id));
        let overlay = fs::read(&overlay_path).unwrap();
        for op in ["product.read", "product.facts"] {
            let reply = f.call(op, json!({"taskIds":[]})).await.unwrap();
            assert_eq!(reply["ok"], true, "{}: {reply}", case["name"]);
            let mut actual = if op == "product.read" {
                reply["result"].clone()
            } else {
                assert_eq!(reply["result"]["workers"], json!([]));
                reply["result"]["product"].clone()
            };
            assert!(actual["ghAvailable"].is_boolean());
            actual["ghAvailable"] = json!("$GH_AVAILABLE");
            assert_eq!(actual, expected, "{} {op}", case["name"]);
        }
        assert_eq!(fs::read(&overlay_path).unwrap(), overlay);
        if let Some(before) = before {
            assert_eq!(
                database_snapshot(_store.as_ref().unwrap()),
                before,
                "read mutated durable state"
            );
        }
        if case["store"] == false {
            assert!(
                !f.root.join(".cortex/cortex.sqlite3").exists()
                    && !f.root.join(".cortex-dev/cortex.sqlite3").exists(),
                "a read initialized a store"
            );
        }
        if matches!(case["name"].as_str(), Some("valid" | "final-valid")) {
            let store = _store.as_ref().unwrap();
            // A body beyond the former 8 MiB budget is streamed natively, with the
            // same verdict; one beyond the 256 MiB bound is an unverified, never
            // a partial pass.
            let baseline = f.call("product.read", json!({})).await.unwrap();
            let accept = baseline["result"]["tasks"]["t1"]["reviewActions"]["accept"].clone();
            let body = store.key().directory().join(if case["name"] == "valid" {
                "r1/large_outputs/lo_fixture.bin"
            } else {
                "r1/large_outputs/lo_final0.bin"
            });
            let length = 8 * 1024 * 1024 + 1;
            fs::OpenOptions::new()
                .write(true)
                .open(body)
                .unwrap()
                .set_len(length)
                .unwrap();
            store
                .write(|db| {
                    db.execute("UPDATE large_outputs SET byte_count=?", [length as i64])?;
                    Ok(())
                })
                .unwrap();
            let reply = f.call("product.read", json!({})).await.unwrap();
            assert_eq!(reply["ok"], true, "{reply}");
            assert_eq!(
                reply["result"]["tasks"]["t1"]["reviewActions"]["accept"],
                accept
            );
            let length = 256 * 1024 * 1024 + 1;
            let body = store.key().directory().join(if case["name"] == "valid" {
                "r1/large_outputs/lo_fixture.bin"
            } else {
                "r1/large_outputs/lo_final0.bin"
            });
            fs::OpenOptions::new()
                .write(true)
                .open(body)
                .unwrap()
                .set_len(length)
                .unwrap();
            store
                .write(|db| {
                    db.execute("UPDATE large_outputs SET byte_count=?", [length as i64])?;
                    Ok(())
                })
                .unwrap();
            let before = database_snapshot(store);
            let reply = f.call("product.read", json!({})).await.unwrap();
            assert_eq!(reply["ok"], true, "{reply}");
            assert_eq!(
                reply["result"]["tasks"]["t1"]["reviewActions"]["accept"], false,
                "an unverifiable body must not leave the result accepted"
            );
            assert_eq!(database_snapshot(store), before);
            assert_eq!(fs::read(&overlay_path).unwrap(), overlay);
        }
        if case["name"] == "passed-command" {
            let store = _store.as_ref().unwrap();
            store.write(|db| {
                db.execute("UPDATE runtime_events SET payload_json=json_set(payload_json,'$.checks[0].command_ref','verification:0:0:0') WHERE type='VERIFICATION_PASSED'", [])?;
                db.execute("UPDATE runtime_nodes SET result_json=json_set(result_json,'$.checks',json(?)) WHERE stage='execution'",
                    [json!([{"name":"gap","outcome":"not_run","command":["tool"]}]).to_string()])?;
                Ok(())
            }).unwrap();
            let before = database_snapshot(store);
            // A reference without its exact start/output/publication evidence
            // now stays a native non-pass, rather than retaining the interpreter.
            let reply = f.call("product.read", json!({})).await.unwrap();
            assert_eq!(reply["ok"], true);
            assert_eq!(
                reply["result"]["tasks"]["t1"]["reviewActions"]["accept"],
                false
            );
            assert_eq!(database_snapshot(store), before);
            store.write(|db| {
                db.execute("UPDATE runtime_nodes SET result_json=json_set(result_json,'$.checks',json('[]')) WHERE stage='execution'", [])?;
                Ok(())
            }).unwrap();
            f.git(&["init", "-b", "main"]);
            let before = database_snapshot(store);
            let reply = f.call("product.read", json!({})).await.unwrap();
            assert_eq!(reply["ok"], true);
            assert_eq!(database_snapshot(store), before);
        }
        f.service.stop().await.unwrap();
    }
}
#[tokio::test]
async fn stored_oracle_worker_pages_match_without_helper_or_activation() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/worker-facts.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let f = Fixture::new().await;
        if case["store"] == true {
            f.store()
                .write(|db| {
                    for row in case["rows"].as_array().unwrap() {
                        let row = replace(row, &f.root, &f.id);
                        let params = row[1]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(sql)
                            .collect::<Vec<_>>();
                        db.execute(row[0].as_str().unwrap(), rusqlite::params_from_iter(params))?;
                    }
                    Ok(())
                })
                .unwrap();
        }
        use sha2::{Digest, Sha256};
        let refs = case
            .get("cursorRefs")
            .cloned()
            .unwrap_or(json!(["a1", "key1"]));
        let refs = refs
            .as_array()
            .unwrap()
            .iter()
            .map(|s| format!("{:?}", s.as_str().unwrap()))
            .collect::<Vec<_>>()
            .join(", ");
        let binding = |project: &str| {
            format!(
                "{:x}",
                Sha256::digest(format!(r#"["{project}", "t1", "a1", [{refs}]]"#).as_bytes())
            )[..24]
                .to_owned()
        };
        // Only the cursor's registered-project binding varies with this disposable root.
        let normalized: Value =
            serde_json::from_str(&case.to_string().replace(&binding("p1"), &binding(&f.id)))
                .unwrap();
        let case = &normalized;
        let mut params = replace(&case["params"], &f.root, &f.id);
        let mut reply = f.call("workers", params.clone()).await.unwrap();
        if case.get("error").is_some() {
            assert_eq!(reply, case["error"], "{}", case["name"]);
        } else {
            assert_eq!(reply["ok"], true, "{}: {reply}", case["name"]);
            assert_eq!(reply["result"], case["expected"], "{}", case["name"]);
            if case["pages"] == true {
                // An older continuation must not admit later inserts from either namespace.
                f.store().write(|db| {
                    let reference = case.get("cursorRefs").and_then(Value::as_array)
                        .and_then(|a|a.last()).and_then(Value::as_str).unwrap_or("a1");
                    db.execute("INSERT INTO task_events(task_id,event_type,assignment_ref,dedup_key,payload_json,origin,created_at) VALUES ('r1','worker_question',?,'late-fixture','{\"text\":\"Must not enter an older page\"}','fixture','2026-11-13T00:00:00Z')",[reference])?;
                    db.execute("INSERT INTO runtime_events(task_id,type,payload_json,actor,created_at) VALUES ('r1','WORKER_GUIDANCE','{\"channelId\":\"channel1\",\"text\":\"Late message\",\"requestId\":\"late\"}','runtime','2026-11-13T00:00:00Z')",[])?;
                    Ok(())
                }).unwrap();
            }
            for page in case["expectedPages"].as_array().unwrap().iter().skip(1) {
                params["cursor"] = reply["result"][0]["activityCursor"].clone();
                reply = f.call("workers", params.clone()).await.unwrap();
                assert_eq!(reply["ok"], true, "{}: {reply}", case["name"]);
                assert_eq!(&reply["result"], page, "{}", case["name"]);
            }
            let private = reply["result"].to_string();
            for hidden in [
                "PRIVATE READ BODY",
                "PRIVATE REPORT BODY",
                "PRIVATE THOUGHT",
                "FOREIGN ATTEMPT",
                "FOREIGN CHANNEL",
            ] {
                assert!(
                    !private.contains(hidden),
                    "{} exposed {hidden}",
                    case["name"]
                );
            }
            if case["params"]["cursor"].is_null() && case["pages"] != true {
                let bundle = f
                    .call(
                        "product.facts",
                        json!({"taskIds":case["params"]["taskIds"]}),
                    )
                    .await
                    .unwrap();
                assert_eq!(bundle["ok"], true, "{}: {bundle}", case["name"]);
                if case["params"]["workerId"].is_null() {
                    assert_eq!(
                        bundle["result"]["workers"], case["expected"],
                        "{}",
                        case["name"]
                    );
                }
            }
        }
        if case["name"] == "provisioned-nonwithdrawn-result" {
            let store = f.store();
            let reply = f
                .call(
                    "workers",
                    json!({"taskIds":["t1"],"cursor":"wa2.foreign.1.1"}),
                )
                .await
                .unwrap();
            assert_eq!(reply["code"], "invalid_cursor");
            store.write(|db| {
                db.execute("UPDATE runtime_nodes SET result_json=? WHERE task_id='r1'",
                    [json!({"checks":[{"name":"pass","outcome":"passed"},{"name":"fail","outcome":"failed","retracted_by":"probe_withdrawn","probe_withdrawn":"fixture"}]}).to_string()])?;
                Ok(())
            }).unwrap();
            let reply = f.call("workers", json!({"taskIds":["t1"]})).await.unwrap();
            assert_eq!(reply["ok"], true, "{reply}");
            let checks = &reply["result"][0]["result"]["checks"];
            assert_eq!(
                checks
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|c| c["outcome"].as_str().unwrap())
                    .collect::<Vec<_>>(),
                ["passed", "failed"],
                "a withdrawal claim is never trusted: the failed claim stays displayed"
            );
        }
        if case["name"] == "status-running" {
            f.store()
                .write(|db| {
                    db.execute(
                        "UPDATE work_assignments SET created_at='2026-11-12T01:02:03'",
                        [],
                    )?;
                    Ok(())
                })
                .unwrap();
            let naive = f.call("workers", json!({"taskIds":["t1"]})).await.unwrap();
            assert_eq!(naive["ok"], true, "{naive}");
            // Naive timestamps are local time, like the reference parser.
            f.store()
                .write(|db| {
                    db.execute(
                        "UPDATE work_assignments SET created_at='2026-11-12T01:02:03+00:00'",
                        [],
                    )?;
                    Ok(())
                })
                .unwrap();
            let utc = f.call("workers", json!({"taskIds":["t1"]})).await.unwrap();
            let local = naive["result"][0]["runningSince"].as_i64().unwrap();
            let universal = utc["result"][0]["runningSince"].as_i64().unwrap();
            assert_eq!((universal - local) % 60_000, 0);
            assert!((universal - local).abs() <= 14 * 3_600_000);
        }
        if case["store"] == false {
            assert!(!f.root.join(".cortex/cortex.sqlite3").exists());
            assert!(!f.root.join(".cortex-dev/cortex.sqlite3").exists());
        }
        f.service.stop().await.unwrap();
    }
}
#[tokio::test]
async fn stored_oracle_coordinator_pages_match_without_helper_or_activation() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/coordinator-facts.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let f = Fixture::new().await;
        let store = f.store();
        store
            .write(|db| {
                for row in case["rows"].as_array().unwrap() {
                    let row = replace(row, &f.root, &f.id);
                    let args = row[1]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(sql)
                        .collect::<Vec<_>>();
                    db.execute(row[0].as_str().unwrap(), rusqlite::params_from_iter(args))?;
                }
                Ok(())
            })
            .unwrap();
        use sha2::{Digest, Sha256};
        let binding = |project: &str| {
            format!(
                "{:x}",
                Sha256::digest(json!([project, "t1", "s1", 100]).to_string().as_bytes())
            )[..24]
                .to_owned()
        };
        let normalized: Value =
            serde_json::from_str(&case.to_string().replace(&binding("p1"), &binding(&f.id)))
                .unwrap();
        let case = &normalized;
        let mut params = case["params"].clone();
        let mut reply = f.call("workers", params.clone()).await.unwrap();
        if case.get("error").is_some() {
            assert_eq!(reply, case["error"], "{}", case["name"]);
        } else {
            assert_eq!(reply["ok"], true, "{}: {reply}", case["name"]);
            assert_eq!(reply["result"], case["expected"], "{}", case["name"]);
            if case["pages"] == true {
                store.write(|db| {
                    db.execute("INSERT INTO host_transcript(session_id,seq,kind,text,created_at) VALUES ('s1',9999,'agent_message','LATE INSERT','2026-11-13T00:00:00Z')",[])?;
                    Ok(())
                }).unwrap();
            }
            for page in case["expectedPages"].as_array().unwrap().iter().skip(1) {
                params["cursor"] = reply["result"][0]["activityCursor"].clone();
                reply = f.call("workers", params.clone()).await.unwrap();
                assert_eq!(reply["result"], *page, "{}: {reply}", case["name"]);
            }
            if case["name"] == "paged-hidden-and-visible" {
                let first_cursor = case["expected"][0]["activityCursor"].clone();
                store.write(|db| {
                    db.execute("INSERT INTO acp_host_sessions VALUES ('replacement',?,'r1','{}','{}','[]','now',NULL)", [f.root.to_str().unwrap()])?;
                    db.execute("UPDATE workspace_executions SET acp_session_id='replacement' WHERE execution_id='e1'", [])?;
                    Ok(())
                }).unwrap();
                let rejected = f
                    .call(
                        "workers",
                        json!({"taskIds":["t1"],"workerId":"coordinator:t1","cursor":first_cursor}),
                    )
                    .await
                    .unwrap();
                assert_eq!(
                    rejected,
                    json!({"ok":false,"code":"invalid_cursor","message":"This coordinator history is unavailable. Reopen the task."})
                );
                let fresh = f
                    .call(
                        "workers",
                        json!({"taskIds":["t1"],"workerId":"coordinator:t1"}),
                    )
                    .await
                    .unwrap();
                assert_eq!(fresh["result"][0]["activity"], json!([]));
            }
            for private in [
                "PRIVATE READ BODY",
                "PRIVATE REPORT BODY",
                "PRIVATE THOUGHT",
                "FOREIGN SESSION",
                "HIDDEN WAIT",
                "LATE INSERT",
            ] {
                assert!(
                    !reply.to_string().contains(private),
                    "{} exposed {private}",
                    case["name"]
                );
            }
        }
        f.service.stop().await.unwrap();
    }
}
#[tokio::test]
async fn local_containment_checks_content_without_altering_real_git_state() {
    let f = Fixture::new().await;
    f.git(&["init", "-b", "main"]);
    f.git(&["config", "user.name", "Fixture"]);
    f.git(&["config", "user.email", "fixture@example.invalid"]);
    f.git(&["config", "commit.gpgsign", "false"]);
    f.git(&["config", "core.hooksPath", "/dev/null"]);
    fs::write(f.root.join("a.txt"), "base\n").unwrap();
    f.git(&["add", "a.txt"]);
    f.git(&["commit", "-m", "base"]);
    let base = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&f.root)
        .output()
        .unwrap();
    let base = String::from_utf8(base.stdout).unwrap().trim().to_owned();
    let tree = f.base.join("tree");
    f.git(&["worktree", "add", "-b", "cw/task", tree.to_str().unwrap()]);
    fs::write(tree.join("a.txt"), "task\n").unwrap();
    let saved = json!({"tasks":{"t1":{"status":"ready_to_merge","checkout":{"kind":"worktree","path":tree,"branch":"cw/task"},"baseRevision":base}}});
    f.save(&saved);
    let read = || f.call("product.read", json!({}));
    let before = fs::read(f.root.join(".git/worktrees/tree/index")).unwrap();
    assert_eq!(
        read().await.unwrap()["result"]["tasks"]["t1"]["integrationFacts"]["alreadyInTarget"],
        false
    );
    fs::write(f.root.join("a.txt"), "task\n").unwrap();
    f.git(&["add", "a.txt"]);
    f.git(&["commit", "-m", "owner squash"]);
    let facts = read().await.unwrap();
    assert_eq!(facts["ok"], true, "{facts}");
    assert_eq!(
        facts["result"]["tasks"]["t1"]["integrationFacts"],
        json!({"targetBranch":"main","alreadyInTarget":true,"pushStatus":null})
    );
    assert_eq!(
        fs::read(f.root.join(".git/worktrees/tree/index")).unwrap(),
        before
    );
    use std::os::unix::fs::symlink;
    for folder in [&tree, &f.root] {
        fs::write(folder.join("binary"), [0, 255, 128, 1]).unwrap();
        symlink("../../absent-target", folder.join("dangling-link")).unwrap();
    }
    f.git(&["add", "binary", "dangling-link"]);
    f.git(&["commit", "-m", "owner copied binary and link"]);
    assert_eq!(
        read().await.unwrap()["result"]["tasks"]["t1"]["integrationFacts"]["alreadyInTarget"],
        true
    );
    fs::write(tree.join("a.txt"), "changed again\n").unwrap();
    assert_eq!(
        read().await.unwrap()["result"]["tasks"]["t1"]["integrationFacts"]["alreadyInTarget"],
        false
    );
    let saved = json!({"tasks":{"t1":{"status":"ready_to_merge","checkout":{"kind":"main"}}}});
    f.save(&saved);
    let facts = read().await.unwrap();
    assert_eq!(
        facts["result"]["tasks"]["t1"]["integrationFacts"]["sameBranch"],
        true
    );
    assert_eq!(
        facts["result"]["tasks"]["t1"]["integrationFacts"]["alreadyInTarget"],
        false
    );
    assert_eq!(
        facts["result"]["tasks"]["t1"]["integrationFacts"]["commitAvailable"],
        false
    );
    f.service.stop().await.unwrap();
}
#[tokio::test]
async fn shared_branch_uses_recorded_bytes_and_keeps_other_staged_work() {
    use sha2::{Digest, Sha256};
    let f = Fixture::new().await;
    f.git(&["init", "-b", "main"]);
    f.git(&["config", "user.name", "Fixture"]);
    f.git(&["config", "user.email", "fixture@example.invalid"]);
    f.git(&["config", "commit.gpgsign", "false"]);
    f.git(&["config", "core.hooksPath", "/dev/null"]);
    fs::write(f.root.join("base"), "base").unwrap();
    f.git(&["add", "base"]);
    f.git(&["commit", "-m", "base"]);
    let store = f.store();
    let wanted = format!("{:x}", Sha256::digest([0u8, 255, 128, 1]));
    store.write(|db|{
        db.execute("INSERT INTO workspace_projects VALUES (?,?,?,?,1)",(&f.id,f.root.to_str().unwrap(),"fixture","now"))?;
        db.execute("INSERT INTO workspace_tasks(task_id,project_id,description,version,execution_id,created_at) VALUES ('t1',?,'Task',1,'e1','now')",[&f.id])?;
        db.execute("INSERT INTO tasks VALUES ('r1',?,'now')",[f.root.to_str().unwrap()])?;
        db.execute("INSERT INTO runtime_tasks(task_id,state,goal,created_at,updated_at) VALUES ('r1','PAUSED','Task','now','now')",[])?;
        db.execute("INSERT INTO workspace_executions VALUES ('e1','t1','r1','s1',1,1,'now')",[])?;
        db.execute("INSERT INTO trusted_channels(channel_id,secret_hash,task_id,principal_thread_id,role,owner_epoch,issued_at)
            VALUES ('c1','fixture-not-a-credential','r1','fixture','worker',1,'now')",[])?;
        let payload=json!({"operation":"project_write","path":"mine","result":{"full_content_sha256":wanted}});
        db.execute("INSERT INTO runtime_events(task_id,type,payload_json,actor,created_at) VALUES ('r1','TOOL_COMPLETED',?,'c1','2000-01-01T00:00:00+00:00')",[payload.to_string()])?;
        Ok(())
    }).unwrap();
    fs::write(f.root.join("mine"), [0, 255, 128, 1]).unwrap();
    fs::write(f.root.join("other"), "unrelated").unwrap();
    f.git(&["add", "other"]);
    f.save(&json!({"tasks":{"t1":{"status":"ready_to_merge","checkout":{"kind":"main"},"targetBranch":"main"}}}));
    let facts = f.call("product.read", json!({})).await.unwrap();
    let v = &facts["result"]["tasks"]["t1"]["integrationFacts"];
    assert_eq!(v["pendingPaths"], json!(["mine"]));
    assert_eq!(v["commitAvailable"], true);
    assert_eq!(v["otherUncommitted"], true);
    assert_eq!(v["alreadyInTarget"], false);
    f.git(&["add", "mine"]);
    f.git(&["commit", "--only", "-m", "own binary", "--", "mine"]);
    fs::write(f.root.join("mine"), "somebody changed it later").unwrap();
    let facts = f.call("product.read", json!({})).await.unwrap();
    let v = &facts["result"]["tasks"]["t1"]["integrationFacts"];
    assert_eq!(v["alreadyInTarget"], true, "{facts}");
    assert_eq!(v["otherUncommitted"], true);
    f.service.stop().await.unwrap();
}
/// Live run: a project-folder task whose engine edited files with its own tools
/// read "Task-owned file evidence is unavailable". It now says what is missing
/// and where, and tells that apart from receipts that could not be confirmed.
#[tokio::test]
async fn a_project_folder_task_without_file_receipts_says_what_is_missing() {
    let f = Fixture::new().await;
    f.git(&["init", "-b", "main"]);
    f.git(&["config", "user.name", "Fixture"]);
    f.git(&["config", "user.email", "fixture@example.invalid"]);
    f.git(&["config", "commit.gpgsign", "false"]);
    f.git(&["config", "core.hooksPath", "/dev/null"]);
    fs::write(f.root.join("base"), "base").unwrap();
    f.git(&["add", "base"]);
    f.git(&["commit", "-m", "base"]);
    fs::write(f.root.join("README.md"), "edited by the engine itself").unwrap();
    f.save(&json!({"tasks":{"t1":{"status":"ready_to_merge","checkout":{"kind":"main"},"targetBranch":"main"}}}));
    let reason = || async {
        let facts = f.call("product.read", json!({})).await.unwrap();
        let v = facts["result"]["tasks"]["t1"]["integrationFacts"].clone();
        assert_eq!(v["sameBranch"], true, "{facts}");
        assert_eq!(v["commitAvailable"], false);
        v["reason"].as_str().unwrap_or_default().to_owned()
    };
    let unrecorded = "There is no record of which files this task changed in the project folder (only changes made through the app's own file tools are recorded), so they cannot be committed from here. Close as is, or commit in Git and use Done outside the app.";
    // No Cortex store at all, then a run with no file-write receipt (native engine edits).
    assert_eq!(reason().await, unrecorded);
    let store = f.store();
    store.write(|db|{
        db.execute("INSERT INTO workspace_projects VALUES (?,?,?,?,1)",(&f.id,f.root.to_str().unwrap(),"fixture","now"))?;
        db.execute("INSERT INTO workspace_tasks(task_id,project_id,description,version,execution_id,created_at) VALUES ('t1',?,'Task',1,'e1','now')",[&f.id])?;
        db.execute("INSERT INTO tasks VALUES ('r1',?,'now')",[f.root.to_str().unwrap()])?;
        db.execute("INSERT INTO runtime_tasks(task_id,state,goal,created_at,updated_at) VALUES ('r1','PAUSED','Task','now','now')",[])?;
        db.execute("INSERT INTO workspace_executions VALUES ('e1','t1','r1','s1',1,1,'now')",[])?;
        db.execute("INSERT INTO trusted_channels(channel_id,secret_hash,task_id,principal_thread_id,role,owner_epoch,issued_at)
            VALUES ('c1','fixture-not-a-credential','r1','fixture','worker',1,'now')",[])?;
        Ok(())
    }).unwrap();
    assert_eq!(reason().await, unrecorded);
    // A receipt without its content digest cannot be confirmed: a different, accurate reason.
    store.write(|db|{
        let payload=json!({"operation":"project_write","path":"README.md","result":{}});
        db.execute("INSERT INTO runtime_events(task_id,type,payload_json,actor,created_at) VALUES ('r1','TOOL_COMPLETED',?,'c1','2000-01-01T00:00:00+00:00')",[payload.to_string()])?;
        Ok(())
    }).unwrap();
    assert_eq!(
        reason().await,
        "Which files this task changed in the project folder could not be confirmed, so they cannot be committed from here. Close as is, or commit in Git and use Done outside the app."
    );
    f.service.stop().await.unwrap();
}
/// Live run 2026-11-17: an answer-only task (no plan, no workers, no write) read
/// "There is no record of which files this task changed…", as if unrecorded changes
/// existed. It now says there is nothing to commit; any write, command, worker or
/// engine edit request keeps the unrecorded wording.
#[tokio::test]
async fn an_answer_that_changed_no_files_has_nothing_to_commit() {
    let f = Fixture::new().await;
    f.git(&["init", "-b", "main"]);
    f.git(&["config", "user.name", "Fixture"]);
    f.git(&["config", "user.email", "fixture@example.invalid"]);
    f.git(&["config", "commit.gpgsign", "false"]);
    f.git(&["config", "core.hooksPath", "/dev/null"]);
    fs::write(f.root.join("base"), "base").unwrap();
    f.git(&["add", "base"]);
    f.git(&["commit", "-m", "base"]);
    f.save(&json!({"tasks":{"t1":{"status":"ready_to_merge","checkout":{"kind":"main"},"targetBranch":"main"}}}));
    let store = f.store();
    store.write(|db|{
        db.execute("INSERT INTO workspace_projects VALUES (?,?,?,?,1)",(&f.id,f.root.to_str().unwrap(),"fixture","now"))?;
        db.execute("INSERT INTO workspace_tasks(task_id,project_id,description,version,execution_id,created_at) VALUES ('t1',?,'Task',1,'e1','now')",[&f.id])?;
        db.execute("INSERT INTO tasks VALUES ('r1',?,'now')",[f.root.to_str().unwrap()])?;
        db.execute("INSERT INTO runtime_tasks(task_id,state,goal,created_at,updated_at) VALUES ('r1','COMPLETED','Task','now','now')",[])?;
        db.execute("INSERT INTO workspace_executions VALUES ('e1','t1','r1','s1',1,1,'now')",[])?;
        db.execute("INSERT INTO runtime_events(task_id,type,payload_json,actor,created_at) VALUES ('r1','ANSWER_SUBMITTED','{\"text\":\"Seven stripes.\"}','runtime','now')",[])?;
        Ok(())
    }).unwrap();
    let facts = f.call("product.read", json!({})).await.unwrap();
    let v = facts["result"]["tasks"]["t1"]["integrationFacts"].clone();
    assert_eq!(v["sameBranch"], true, "{facts}");
    assert_eq!(v["commitAvailable"], false);
    assert_eq!(v["noChanges"], true, "{facts}");
    // Nothing to land: Automatic finishes the no-op instead of holding the sprint.
    assert_eq!(v["alreadyInTarget"], true, "{facts}");
    assert_eq!(v["reason"], "This task answered without changing any files, so there is nothing to commit. Close the task when you are done with it.");
    // Owner store 2026-11-23: an investigation-only plan (read-only workers that ran
    // read-only commands, and their review) changed nothing either.
    store.write(|db| {
        db.execute("DELETE FROM runtime_events WHERE type='ANSWER_SUBMITTED'", [])?;
        db.execute("INSERT INTO runtime_events(task_id,type,payload_json,actor,created_at) VALUES ('r1','TASK_CREATED','{\"execution_host\":\"rust\"}','runtime','now')", [])?;
        db.execute("INSERT INTO runtime_nodes VALUES ('r1',1,'inspect','worker-explorer','Read','[]','[]','execution','completed','a',NULL)", [])?;
        db.execute("INSERT INTO runtime_requests VALUES ('r1','k','explorer','command_run','d','{}')", [])?;
        Ok(())
    }).unwrap();
    let facts = f.call("product.read", json!({})).await.unwrap();
    let v = &facts["result"]["tasks"]["t1"]["integrationFacts"];
    assert_eq!(v["noChanges"], true, "{facts}");
    assert_eq!(v["alreadyInTarget"], true, "{facts}");
    store.write(|db| {
        db.execute_batch("DELETE FROM runtime_requests; DELETE FROM runtime_nodes; DELETE FROM runtime_events;")?;
        db.execute("INSERT INTO runtime_events(task_id,type,payload_json,actor,created_at) VALUES ('r1','ANSWER_SUBMITTED','{\"text\":\"Seven stripes.\"}','runtime','now')", [])?;
        Ok(())
    }).unwrap();
    let unrecorded = "There is no record of which files this task changed in the project folder (only changes made through the app's own file tools are recorded), so they cannot be committed from here. Close as is, or commit in Git and use Done outside the app.";
    for (n, change) in [
        "INSERT INTO runtime_requests VALUES ('r1','k','coordinator','outside_command','d','{}')",
        "INSERT INTO runtime_events(task_id,type,payload_json,actor,created_at) VALUES ('r1','NATIVE_PERMISSION_REQUESTED','{\"kind\":\"edit\"}','runtime','now')",
        "INSERT INTO runtime_events(task_id,type,payload_json,actor,created_at) VALUES ('r1','NATIVE_WRITER_TURN_STARTED','{\"channel_id\":\"c\"}','runtime','now')",
    ].into_iter().enumerate() {
        store.write(|db| {
            db.execute_batch("DELETE FROM runtime_requests; DELETE FROM runtime_nodes; DELETE FROM runtime_events WHERE type!='ANSWER_SUBMITTED';")?;
            db.execute(change, [])?;
            Ok(())
        }).unwrap();
        let facts = f.call("product.read", json!({})).await.unwrap();
        let v = &facts["result"]["tasks"]["t1"]["integrationFacts"];
        assert_eq!(v["noChanges"], Value::Null, "{n}: {facts}");
        assert_eq!(v["reason"], unrecorded, "{n}");
    }
    f.service.stop().await.unwrap();
}
/// Live run: a project-folder task whose worker wrote through Cortex's own
/// `project_write`/`project_edit` read "could not be confirmed" on Finish. The
/// native runtime records each write as NATIVE_PROJECT_WRITTEN with its completed
/// request receipt; those exact paths and digests make the files committable.
#[tokio::test]
async fn native_project_write_receipts_confirm_project_folder_files() {
    use sha2::{Digest, Sha256};
    let f = Fixture::new().await;
    f.git(&["init", "-b", "main"]);
    f.git(&["config", "user.name", "Fixture"]);
    f.git(&["config", "user.email", "fixture@example.invalid"]);
    f.git(&["config", "commit.gpgsign", "false"]);
    f.git(&["config", "core.hooksPath", "/dev/null"]);
    fs::write(f.root.join("README.md"), "base\n").unwrap();
    f.git(&["add", "README.md"]);
    f.git(&["commit", "-m", "base"]);
    let digest = |s: &str| format!("{:x}", Sha256::digest(s.as_bytes()));
    let (first, last) = ("first draft\n", "final text\n");
    let store = f.store();
    store.write(|db|{
        db.execute("INSERT INTO workspace_projects VALUES (?,?,?,?,1)",(&f.id,f.root.to_str().unwrap(),"fixture","now"))?;
        db.execute("INSERT INTO workspace_tasks(task_id,project_id,description,version,execution_id,created_at) VALUES ('t1',?,'Task',1,'e1','now')",[&f.id])?;
        db.execute("INSERT INTO tasks VALUES ('r1',?,'now')",[f.root.to_str().unwrap()])?;
        db.execute("INSERT INTO runtime_tasks(task_id,state,goal,created_at,updated_at) VALUES ('r1','PAUSED','Task','now','now')",[])?;
        db.execute("INSERT INTO workspace_executions VALUES ('e1','t1','r1','s1',1,1,'now')",[])?;
        db.execute("INSERT INTO trusted_channels(channel_id,secret_hash,task_id,principal_thread_id,role,owner_epoch,issued_at)
            VALUES ('ch_w','fixture-not-a-credential','r1','fixture','worker',1,'now')",[])?;
        // Two writes of the same file; the later one is the task's content.
        for (key, op, text) in [("k1","project_write",first),("k2","project_edit",last)] {
            let reply=json!({"complete":true,"reply":{"content":[],"isError":false,"structuredContent":{"path":"README.md","sha256":digest(text),"outcome":"written"}}});
            db.execute("INSERT INTO runtime_requests VALUES ('r1',?,'ch_w',?,'d',?)",(key,op,reply.to_string()))?;
            let event=json!({"channel_id":"ch_w","path":"README.md","sha256":digest(text),"add":1,"del":1});
            db.execute("INSERT INTO runtime_events(task_id,type,payload_json,actor,created_at) VALUES ('r1','NATIVE_PROJECT_WRITTEN',?,'runtime','2000-01-01T00:00:00+00:00')",[event.to_string()])?;
        }
        Ok(())
    }).unwrap();
    fs::write(f.root.join("README.md"), last).unwrap();
    f.save(&json!({"tasks":{"t1":{"status":"ready_to_merge","checkout":{"kind":"main"},"targetBranch":"main"}}}));
    let facts = f.call("product.read", json!({})).await.unwrap();
    let v = &facts["result"]["tasks"]["t1"]["integrationFacts"];
    assert_eq!(v["sameBranch"], true, "{facts}");
    assert_eq!(v["pendingPaths"], json!(["README.md"]), "{facts}");
    assert_eq!(v["commitAvailable"], true, "{facts}");
    assert_eq!(v["reason"], Value::Null);
    assert_eq!(v["otherUncommitted"], false);
    // The bytes on disk are no longer what Cortex wrote: not committable from here.
    fs::write(f.root.join("README.md"), "owner edit\n").unwrap();
    let facts = f.call("product.read", json!({})).await.unwrap();
    let v = &facts["result"]["tasks"]["t1"]["integrationFacts"];
    assert_eq!(v["commitAvailable"], false, "{facts}");
    assert_eq!(v["pendingPaths"], json!([]));
    fs::write(f.root.join("README.md"), last).unwrap();
    // A write event without its completed request receipt (another channel's, or an
    // unfinished attempt) is never taken as the task's content.
    store.write(|db|{
        db.execute("INSERT INTO trusted_channels(channel_id,secret_hash,task_id,principal_thread_id,role,owner_epoch,issued_at)
            VALUES ('ch_x','fixture-not-a-credential','r1','fixture','worker',1,'now')",[])?;
        let event=json!({"channel_id":"ch_x","path":"README.md","sha256":digest(last),"add":1,"del":1});
        db.execute("INSERT INTO runtime_events(task_id,type,payload_json,actor,created_at) VALUES ('r1','NATIVE_PROJECT_WRITTEN',?,'runtime','2000-01-01T00:00:00+00:00')",[event.to_string()])?;
        Ok(())
    }).unwrap();
    let facts = f.call("product.read", json!({})).await.unwrap();
    let v = &facts["result"]["tasks"]["t1"]["integrationFacts"];
    assert_eq!(v["commitAvailable"], false, "{facts}");
    assert_eq!(
        v["reason"],
        "Which files this task changed in the project folder could not be confirmed, so they cannot be committed from here. Close as is, or commit in Git and use Done outside the app."
    );
    f.service.stop().await.unwrap();
}
#[tokio::test]
async fn unfinished_source_evidence_degrades_explicitly_without_the_provider() {
    let f = Fixture::new().await;
    let store = f.store();
    let reply = f
        .call(
            "workers",
            json!({"taskIds":["t1"],"workerId":"coordinator:t1"}),
        )
        .await
        .unwrap();
    assert_eq!(reply, json!({"ok":true,"result":[]}));
    store.write(|db|{
        db.execute("INSERT INTO workspace_projects VALUES (?,?,?,?,1)",(&f.id,f.root.to_str().unwrap(),"fixture","now"))?;
        db.execute("INSERT INTO workspace_tasks(task_id,project_id,description,version,execution_id,created_at) VALUES ('t1',?,'Task',1,'e1','now')",[&f.id])?;
        db.execute("INSERT INTO tasks VALUES ('r1',?,'now')",[f.root.to_str().unwrap()])?;
        db.execute("INSERT INTO runtime_tasks(task_id,state,goal,synthesis,created_at,updated_at) VALUES ('r1','COMPLETED','Task','Result','now','now')",[])?;
        db.execute("INSERT INTO task_owners VALUES ('r1','owner',1,'now')",[])?;
        db.execute("INSERT INTO acp_host_sessions VALUES ('s1',?,'r1','{}','{}','[]','now',NULL)",[f.root.to_str().unwrap()])?;
        db.execute("INSERT INTO workspace_executions VALUES ('e1','t1','r1','s1',1,1,'now')",[])?;
        db.execute("INSERT INTO runtime_events(task_id,type,payload_json,actor,created_at) VALUES ('r1','VERIFICATION_PASSED','{}','runtime','now')",[])?;
        Ok(())
    }).unwrap();
    // A digest-less verification no longer needs the helper, and is not a pass.
    let reply = f.call("product.read", json!({})).await.unwrap();
    assert_eq!(
        reply["result"]["tasks"]["t1"]["reviewActions"]["accept"],
        false
    );
    store
        .write(|db| {
            db.execute(
                "UPDATE runtime_events SET payload_json=? WHERE task_id='r1'",
                [json!({"revision":0,"revision_digest":"a".repeat(64),"checks":[]}).to_string()],
            )?;
            Ok(())
        })
        .unwrap();
    // Plain-folder source drift now has a native, Python-compatible refusal.
    let reply = f.call("product.read", json!({})).await.unwrap();
    assert_eq!(
        reply["result"]["tasks"]["t1"]["reviewActions"]["accept"],
        false
    );
    assert_eq!(
        reply["result"]["tasks"]["t1"]["reviewActions"]["unblockAction"],
        "request_changes"
    );
    // Git roots now observe source without a Python helper; drift is still not a pass.
    f.git(&["init", "-b", "main"]);
    let reply = f.call("product.read", json!({})).await.unwrap();
    assert_eq!(
        reply["result"]["tasks"]["t1"]["reviewActions"]["accept"],
        false
    );
    // Provisioning alone no longer requires Python. A withdrawal claim is never
    // trusted: it is read natively and remains an open failed check. A cited
    // authorized receipt is read natively too; neither is ever a pass.
    store.write(|db| {
        db.execute("DELETE FROM runtime_events WHERE task_id='r1'", [])?;
        db.execute("INSERT INTO runtime_nodes VALUES ('r1',0,'n0','worker-backend-dev','Task','[]','[]','execution','completed','a0',?)",
            [json!({"checks":[{"name":"check","outcome":"failed","retracted_by":"probe_withdrawn"}]}).to_string()])?;
        db.execute("INSERT INTO delegation_provisioning(idempotency_key,task_id,attempt_id,role_id,step,status,owner_epoch,created_at,updated_at) VALUES ('a0','r1','attempt0','worker-backend-dev','ready','running',1,'now','now')", [])?;
        Ok(())
    }).unwrap();
    let reply = f.call("product.read", json!({})).await.unwrap();
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(
        reply["result"]["tasks"]["t1"]["reviewActions"]["accept"],
        false
    );
    store.write(|db| {
        db.execute("UPDATE runtime_nodes SET result_json=?", [json!({"checks":[{"name":"check","outcome":"passed","tool_call_ref":"gateway:cited"}]}).to_string()])?;
        db.execute("INSERT INTO runtime_events(task_id,type,payload_json,actor,created_at) VALUES ('r1','TOOL_AUTHORIZED',?,'channel','now')",
            [json!({"tool_call_ref":"gateway:cited","operation":"command_run"}).to_string()])?;
        Ok(())
    }).unwrap();
    let reply = f.call("product.read", json!({})).await.unwrap();
    assert_eq!(reply["ok"], true, "{reply}");
    let actions = &reply["result"]["tasks"]["t1"]["reviewActions"];
    assert_eq!(actions["accept"], false);
    assert_eq!(
        actions["acknowledgeOpenChecks"], false,
        "a plain cited pass is not open"
    );
    // A policy blocker on the claim makes the authorized pass not_run, as the
    // legacy `_policy_blocker(argv, check)` did, even with an unpaid receipt.
    store.write(|db| {
        db.execute("UPDATE runtime_nodes SET result_json=?", [json!({"checks":[{"name":"check","outcome":"passed","tool_call_ref":"gateway:cited","blocker":"Blocked by policy"}]}).to_string()])?;
        Ok(())
    }).unwrap();
    let reply = f.call("product.read", json!({})).await.unwrap();
    assert_eq!(reply["ok"], true, "{reply}");
    let actions = &reply["result"]["tasks"]["t1"]["reviewActions"];
    assert_eq!(actions["accept"], false);
    assert_eq!(actions["acknowledgeOpenChecks"], true, "{actions}");
    store.write(|db| {
        db.execute("UPDATE runtime_nodes SET result_json=?", [json!({"checks":[{"name":"check","outcome":"passed","tool_call_ref":"gateway:cited"}]}).to_string()])?;
        Ok(())
    }).unwrap();
    // The cited receipt's own command is a paid probe: the claimed pass is a
    // policy-blocked, open (not_run) check, never a pass.
    store.write(|db| {
        db.execute("INSERT INTO runtime_requests VALUES ('r1','k1','channel','command_run','d',?)",
            [json!({"receipt":{"tool_call_ref":"gateway:cited","argv":["python3","scripts/cortex_codex_probe.py"]}}).to_string()])?;
        Ok(())
    }).unwrap();
    let reply = f.call("product.read", json!({})).await.unwrap();
    assert_eq!(reply["ok"], true, "{reply}");
    let actions = &reply["result"]["tasks"]["t1"]["reviewActions"];
    assert_eq!(actions["accept"], false);
    assert_eq!(actions["acknowledgeOpenChecks"], true, "{actions}");
    store
        .write(|db| {
            db.execute("DELETE FROM runtime_nodes WHERE task_id='r1'", [])?;
            Ok(())
        })
        .unwrap();
    // Malformed reruns make the result's evidence explicitly unavailable; they
    // never select the interpreter and never accept. Non-closing current attempts
    // are compared with Python in the stored DTO fixtures.
    for payload in [
        json!({}),
        json!({"binding":{"revision":0}}),
        json!({"binding":{"revision":0,"owner_epoch":1,"target":{"assignment":"a0","index":0}},"result":null}),
        json!({"binding":{"revision":0,"owner_epoch":1,"target":{"assignment":"a0","index":0}},"result":{"checks":null}}),
        json!({"binding":{"revision":0,"owner_epoch":true,"target":{"assignment":"a0","index":0}}}),
        json!({"binding":{"revision":1},"padding":"x".repeat(16000)}),
    ] {
        store.write(|db| {
            db.execute("UPDATE runtime_events SET type='WORKER_CHECK_FINISHED',actor='runtime',payload_json=? WHERE task_id='r1'", [payload.to_string()])?;
            Ok(())
        }).unwrap();
        let reply = f.call("product.read", json!({})).await.unwrap();
        assert_eq!(reply["ok"], true, "{reply}");
        assert_eq!(
            reply["result"]["tasks"]["t1"]["reviewActions"]["accept"], false,
            "{payload}"
        );
    }
    // Source drift now rejects a nominal pass natively. Malformed review
    // bindings still retain the interpreter; no recorded claim proves closure.
    for review in [
        Some(json!({"verdict":"APPROVE","project_digest":"a".repeat(64)})),
        Some(Value::Null),
        Some(json!([])),
        None,
    ] {
        store.write(|db| {
            db.execute("DELETE FROM runtime_events WHERE task_id='r1'", [])?;
            db.execute("DELETE FROM runtime_nodes WHERE task_id='r1'", [])?;
            if let Some(value) = &review {
                db.execute("INSERT INTO runtime_nodes VALUES ('r1',0,'review','worker-reviewer','Review','[]','[]','review','completed','review0',?)",
                    [value.to_string()])?;
            } else {
                db.execute("DELETE FROM runtime_nodes WHERE task_id='r1'", [])?;
                db.execute("INSERT INTO runtime_events(task_id,type,payload_json,actor,created_at) VALUES ('r1','REVIEW_NOT_REQUIRED',?,'runtime','now')",
                    [json!({"revision":0,"revision_digest":"a".repeat(64)}).to_string()])?;
            }
            db.execute("INSERT INTO runtime_events(task_id,type,payload_json,actor,created_at) VALUES ('r1','WORKER_CHECK_FINISHED',?,'runtime','now')",
                [json!({"result":{"digest":"a".repeat(64),"checks":[{"complete":true,"exit_code":0,"revision_digest":"a".repeat(64)}]},
                    "binding":{"revision":0,"owner_epoch":1,"digest":"a".repeat(64),"target":{"assignment":"a0","index":0}}}).to_string()])?;
            Ok(())
        }).unwrap();
        let reply = f.call("product.read", json!({})).await.unwrap();
        assert_eq!(reply["ok"], true, "{reply}");
        assert_eq!(
            reply["result"]["tasks"]["t1"]["reviewActions"]["accept"],
            false
        );
    }
    store.write(|db| {
        db.execute("DELETE FROM runtime_events WHERE task_id='r1'", [])?;
        for _ in 0..1001 {
            db.execute("INSERT INTO runtime_events(task_id,type,payload_json,actor,created_at) VALUES ('r1','WORKER_CHECK_REQUESTED',?,'runtime','now')",
                [json!({"binding":{"revision":1}}).to_string()])?;
        }
        Ok(())
    }).unwrap();
    // Past 1,000 attempt rows is still read natively (other revisions are ignored).
    let reply = f.call("product.read", json!({})).await.unwrap();
    assert_eq!(reply["ok"], true, "{reply}");
    // Beyond the 10,000-row bound the evidence is explicitly unavailable.
    store.write(|db| {
        for _ in 0..9000 {
            db.execute("INSERT INTO runtime_events(task_id,type,payload_json,actor,created_at) VALUES ('r1','WORKER_CHECK_REQUESTED',?,'runtime','now')",
                [json!({"binding":{"revision":1}}).to_string()])?;
        }
        db.execute("DELETE FROM runtime_nodes WHERE task_id='r1'", [])?;
        db.execute("INSERT INTO runtime_nodes VALUES ('r1',0,'n0','worker-backend-dev','Task','[]','[]','execution','completed','a0',?)",
            [json!({"checks":[]}).to_string()])?;
        Ok(())
    }).unwrap();
    let reply = f.call("product.read", json!({})).await.unwrap();
    assert_eq!(reply["ok"], true, "{reply}");
    let actions = &reply["result"]["tasks"]["t1"]["reviewActions"];
    assert_eq!(actions["accept"], false);
    assert_eq!(
        actions["reason"],
        "Result evidence is unavailable or the files changed. Request changes and run fresh checks."
    );
    f.service.stop().await.unwrap();
}
fn seed_coordinator(f: &Fixture, synthesis: Option<&str>, state: &str) {
    f.store().write(|db| {
        db.execute("INSERT INTO workspace_projects VALUES (?,?,?,?,1)",(&f.id,f.root.to_str().unwrap(),"fixture","now"))?;
        db.execute("INSERT INTO workspace_tasks(task_id,project_id,description,version,execution_id,created_at) VALUES ('t1',?,'Task',1,'e1','now')",[&f.id])?;
        db.execute("INSERT INTO tasks VALUES ('r1',?,'now')",[f.root.to_str().unwrap()])?;
        db.execute("INSERT INTO runtime_tasks(task_id,state,goal,synthesis,created_at,updated_at) VALUES ('r1',?,'Goal',?,'now','now')",(state,synthesis))?;
        db.execute("INSERT INTO task_owners VALUES ('r1','owner',1,'now')",[])?;
        db.execute("INSERT INTO acp_host_sessions VALUES ('s1',?,'r1','{}','{}','[]','now',NULL)",[f.root.to_str().unwrap()])?;
        db.execute("INSERT INTO workspace_executions VALUES ('e1','t1','r1','s1',1,1,'now')",[])?;
        Ok(())
    }).unwrap();
}
#[tokio::test]
async fn malformed_display_shapes_degrade_visibly_without_the_provider() {
    let f = Fixture::new().await;
    seed_coordinator(&f, None, "EXECUTING");
    f.store().write(|db| {
        for (seq, update) in [
            (0, json!({"sessionUpdate":"plan","entries":null})),
            (1, json!({"sessionUpdate":"tool_call","toolCallId":"a","title":"Run","status":5,"rawInput":{"command":"echo"}})),
            (2, json!({"sessionUpdate":"plan","entries":"abc"})),
            (3, json!({"sessionUpdate":"tool_call","toolCallId":"b","title":42,"rawInput":{"path":"x"}})),
            (4, json!({"sessionUpdate":"plan","entries":[{"content":7,"status":["done"]}]})),
        ] {
            db.execute("INSERT INTO host_updates VALUES ('s1',?,?,'2026-11-12 01:02:03')",(seq,update.to_string()))?;
        }
        Ok(())
    }).unwrap();
    let reply = f
        .call(
            "workers",
            json!({"taskIds":["t1"],"workerId":"coordinator:t1"}),
        )
        .await
        .expect("native reply");
    assert_eq!(reply["ok"], true, "{reply}");
    let activity = reply["result"][0]["activity"].as_array().unwrap();
    // A plan whose entries are null is an unavailable-content row, never invented text.
    assert_eq!(
        activity[0]["text"],
        "Content unavailable in the retained display history."
    );
    // A non-string status is not shown as a status; the step is still listed.
    assert_eq!(activity[1]["kind"], "command");
    assert_eq!(activity[1]["error"], false);
    assert_eq!(activity[2]["text"], "Plan\n");
    assert_eq!(activity[3]["text"], "42");
    assert_eq!(activity[4]["text"], "Plan\n7 · ['done']");
    // Timestamps in the legacy `YYYY-MM-DD HH:MM:SS` form are parsed (local time).
    assert!(activity[0]["at"].is_i64());
    // A string `taskIds` is an unsupported request shape: explicit refusal.
    let refused = f
        .call("workers", json!({"taskIds":"t1"}))
        .await
        .expect("native refusal");
    assert_eq!(refused["ok"], false);
    assert_eq!(refused["code"], "invalid_request");
    // A non-text task identity matches no task rather than selecting the provider.
    let none = f
        .call("workers", json!({"taskIds":[5, null]}))
        .await
        .unwrap();
    assert_eq!(none, json!({"ok":true,"result":[]}));
    f.service.stop().await.unwrap();
}
#[tokio::test]
async fn oversized_result_text_is_unavailable_and_never_acceptable() {
    for (size, accept) in [(31_000, true), (32_001, false)] {
        let f = Fixture::new().await;
        seed_coordinator(&f, Some(&"x".repeat(size)), "COMPLETED");
        f.store().write(|db| {
            db.execute("INSERT INTO runtime_events(task_id,type,payload_json,actor,created_at) VALUES ('r1','ANSWER_SUBMITTED','{}','runtime','now')", [])?;
            Ok(())
        }).unwrap();
        let reply = f
            .call("product.read", json!({}))
            .await
            .expect("native read");
        assert_eq!(reply["ok"], true, "{reply}");
        let actions = &reply["result"]["tasks"]["t1"]["reviewActions"];
        assert_eq!(actions["accept"], accept, "{size}: {actions}");
        if !accept {
            assert_eq!(actions["unblockAction"], "request_changes");
            assert_eq!(
                actions["reason"],
                "Result evidence is unavailable or the files changed. Request changes and run fresh checks."
            );
        }
        f.service.stop().await.unwrap();
    }
}
#[tokio::test]
async fn unmaterialized_legacy_checkout_is_read_natively_with_a_prospective_path() {
    let f = Fixture::new().await;
    seed_coordinator(&f, None, "CREATED");
    f.git(&["init", "-b", "main"]);
    f.store()
        .write(|db| {
            db.execute(
                "INSERT INTO workspace_task_options VALUES ('t1',?)",
                [json!({"checkout":{"kind":"worktree"}}).to_string()],
            )?;
            Ok(())
        })
        .unwrap();
    for op in ["product.read", "product.facts"] {
        let params = if op == "product.read" {
            json!({})
        } else {
            json!({"taskIds":[]})
        };
        let reply = f
            .call(op, params)
            .await
            .expect("native read, not a provider failure");
        assert_eq!(reply["ok"], true, "{op}: {reply}");
        let product = if op == "product.read" {
            &reply["result"]
        } else {
            &reply["result"]["product"]
        };
        let checkout = &product["tasks"]["t1"]["checkout"];
        assert_eq!(checkout["kind"], "worktree");
        assert!(
            checkout["path"]
                .as_str()
                .is_some_and(|p| p.ends_with("/t1")),
            "{checkout}"
        );
        assert_eq!(checkout["branch"], "cw/t1");
    }
    f.service.stop().await.unwrap();
}
fn withdrawal_check(retracted_by: &str) -> Value {
    json!({"name":"fail","outcome":"failed","tool_call_ref":"gateway:ref","probe_withdrawn":"Diagnostic","retracted_by":retracted_by})
}
fn withdrawal_receipts(db: &rusqlite::Connection) -> rusqlite::Result<()> {
    db.execute("INSERT INTO trusted_channels(channel_id,secret_hash,task_id,principal_thread_id,role,assignment_id,attempt_id,owner_epoch,issued_at) VALUES ('channel1','h','r1','p','worker','a1','engine1',1,'now')", [])?;
    db.execute("INSERT INTO large_outputs(id,task_id,owner_scope,source_kind,source_ref,filename,byte_count,truncated,exit_code,created_at) VALUES ('lo1','r1','dispatch1','terminal','gateway:ref','lo1.bin',4,0,1,'now')", [])?;
    db.execute("INSERT INTO runtime_requests VALUES ('r1','k1','channel1','command_run','d',?)",
        [json!({"tool_call_ref":"gateway:ref","argv":["curl","--fail","https://invalid.test"],"cwd":"/fx","profile":"restricted",
            "complete":true,"exit_code":1,"failure":null,"large_output_id":"lo1","byte_count":4,"output":"404\n"}).to_string()])?;
    Ok(())
}
#[tokio::test]
async fn withdrawn_failed_checks_are_omitted_only_when_the_native_assessor_agrees() {
    // Display: matching withdrawal omits the failed claim; absent or mismatched
    // assessment leaves it displayed as failed.
    for (variant, receipts, retracted_by, shown) in [
        ("matching", true, "probe_withdrawn", vec!["passed"]),
        (
            "no-receipts",
            false,
            "probe_withdrawn",
            vec!["passed", "failed"],
        ),
        ("mismatched-claim", true, "other", vec!["passed", "failed"]),
        ("unclaimed", true, "", vec!["passed", "failed"]),
    ] {
        let f = Fixture::new().await;
        seed_coordinator(&f, None, "EXECUTING");
        let mut check = withdrawal_check(retracted_by);
        if retracted_by.is_empty() {
            check.as_object_mut().unwrap().remove("retracted_by");
        }
        let checks = json!([{"name":"pass","outcome":"passed"}, check]);
        f.store().write(|db| {
            db.execute("INSERT INTO work_assignments(assignment_id,task_id,node_key,requirement_bindings_json,status,idempotency_key,created_at) VALUES ('a1','r1','worker-backend-dev','[]','running','key1','2026-11-12T01:02:03+00:00')", [])?;
            db.execute("INSERT INTO work_assignment_attempts(attempt_id,task_id,assignment_id,attempt_no,requirement_bindings_json,status,idempotency_key,created_at) VALUES ('w1','r1','a1',1,'[]','running','attempt-key1','2026-11-12T01:02:03+00:00')", [])?;
            db.execute("INSERT INTO task_events(task_id,event_type,assignment_ref,dedup_key,payload_json,origin,created_at) VALUES ('r1','delegation_requested','dispatch1','te9','{\"work_attempt_ref\":\"w1\"}','fixture','2026-11-12T01:02:03+00:00')", [])?;
            db.execute("INSERT INTO delegation_provisioning(idempotency_key,task_id,attempt_id,role_id,step,status,owner_epoch,channel_id,created_at,updated_at) VALUES ('dispatch1','r1','engine1','worker-backend-dev','ready','running',1,'channel1','now','now')", [])?;
            db.execute("INSERT INTO task_events(task_id,event_type,assignment_ref,dedup_key,payload_json,origin,created_at) VALUES ('r1','worker_result','engine1','te11',?,'fixture','2026-11-12T01:02:03+00:00')",
                [json!({"summary":"s","report_ref":"r","checks":checks}).to_string()])?;
            db.execute("INSERT INTO runtime_nodes VALUES ('r1',0,'n1','worker-backend-dev','Fixture','[]','[]','execution','completed','dispatch1',?)", [json!({"checks":checks}).to_string()])?;
            if receipts {
                withdrawal_receipts(db)?;
            }
            Ok(())
        }).unwrap();
        let reply = f.call("workers", json!({"taskIds":["t1"]})).await.unwrap();
        assert_eq!(reply["ok"], true, "{variant}: {reply}");
        let outcomes = reply["result"][0]["result"]["checks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["outcome"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(outcomes, shown, "{variant}");
        f.service.stop().await.unwrap();
    }
}
#[tokio::test]
async fn withdrawn_failed_check_no_longer_blocks_acceptance_but_unassessed_ones_do() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/task-work-facts.json")).unwrap();
    let case = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "work-current-waiver")
        .unwrap();
    for (variant, check, receipts, open) in [
        ("none", None, false, false),
        (
            "matching",
            Some(withdrawal_check("probe_withdrawn")),
            true,
            false,
        ),
        (
            "no-receipts",
            Some(withdrawal_check("probe_withdrawn")),
            false,
            true,
        ),
        (
            "mismatched-claim",
            Some(withdrawal_check("other")),
            true,
            true,
        ),
    ] {
        let f = Fixture::new().await;
        f.git(&["init", "-b", "main"]);
        let store = f.store();
        store.write(|db| {
            for row in case["rows"].as_array().unwrap() {
                let row = replace(row, &f.root, &f.id);
                let mut params = row[1].as_array().unwrap().iter().map(sql).collect::<Vec<_>>();
                if row[0].as_str().unwrap().contains("INSERT INTO runtime_nodes") {
                    let mut result: Value = serde_json::from_str(row[1][4].as_str().unwrap()).unwrap();
                    if let Some(check) = &check {
                        result["checks"] = json!([check]);
                    }
                    params[4] = result.to_string().into();
                }
                db.execute(row[0].as_str().unwrap(), rusqlite::params_from_iter(params))?;
            }
            if check.is_some() {
                db.execute("INSERT INTO delegation_provisioning(idempotency_key,task_id,attempt_id,role_id,step,status,owner_epoch,channel_id,created_at,updated_at) VALUES ('assignment0','r1','engine1','worker-backend-dev','ready','running',1,'channel1','now','now')", [])?;
            }
            if receipts {
                db.execute("INSERT INTO trusted_channels(channel_id,secret_hash,task_id,principal_thread_id,role,assignment_id,attempt_id,owner_epoch,issued_at) VALUES ('channel1','h','r1','p','worker','a1','engine1',1,'now')", [])?;
                db.execute("INSERT INTO large_outputs(id,task_id,owner_scope,source_kind,source_ref,filename,byte_count,truncated,exit_code,created_at) VALUES ('lo1','r1','assignment0','terminal','gateway:ref','lo1.bin',4,0,1,'now')", [])?;
                db.execute("INSERT INTO runtime_requests VALUES ('r1','k1','channel1','command_run','d',?)",
                    [json!({"tool_call_ref":"gateway:ref","argv":["curl","--fail","https://invalid.test"],"cwd":"/fx","profile":"restricted",
                        "complete":true,"exit_code":1,"failure":null,"large_output_id":"lo1","byte_count":4,"output":"404\n"}).to_string()])?;
            }
            Ok(())
        }).unwrap();
        let path = store.key().directory().join("r1/result.md");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(&path, case["files"][0]["content"].as_str().unwrap()).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let reply = f.call("product.read", json!({})).await.unwrap();
        assert_eq!(reply["ok"], true, "{variant}: {reply}");
        let actions = &reply["result"]["tasks"]["t1"]["reviewActions"];
        // Review is waived and the report matches, so the only remaining blocker of
        // the baseline is the (unrelated) missing runtime verification: a withdrawn
        // check must leave exactly the baseline, an unassessed one must add itself.
        assert_eq!(
            actions["reason"], "Run the current runtime verification checks before accepting work.",
            "{variant}: {actions}"
        );
        assert_eq!(
            actions["acknowledgeOpenChecks"], open,
            "{variant}: {actions}"
        );
        f.service.stop().await.unwrap();
    }
}
/// A standalone task in the project folder (no sprint) stays acceptable while other
/// tasks or the owner change unrelated files, and goes stale only when a file it wrote
/// changes. Its verified folder digest is deliberately not the current folder digest.
#[tokio::test]
async fn standalone_project_folder_result_is_scoped_to_its_own_written_files() {
    use sha2::{Digest, Sha256};
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/task-work-facts.json")).unwrap();
    let case = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "work-current-waiver")
        .unwrap();
    let f = Fixture::new().await;
    f.git(&["init", "-b", "main"]);
    f.git(&["config", "user.name", "Fixture"]);
    f.git(&["config", "user.email", "fixture@example.invalid"]);
    f.git(&["config", "commit.gpgsign", "false"]);
    f.git(&["config", "core.hooksPath", "/dev/null"]);
    fs::write(f.root.join("other.txt"), "base\n").unwrap();
    f.git(&["add", "other.txt"]);
    f.git(&["commit", "-m", "base"]);
    let own = "own text\n";
    fs::write(f.root.join("own.md"), own).unwrap();
    let store = f.store();
    store.write(|db| {
        for row in case["rows"].as_array().unwrap() {
            let row = replace(row, &f.root, &f.id);
            let params = row[1].as_array().unwrap().iter().map(sql).collect::<Vec<_>>();
            db.execute(row[0].as_str().unwrap(), rusqlite::params_from_iter(params))?;
        }
        let events = [
            ("TASK_CREATED", json!({"execution_host":"rust"})),
            ("NATIVE_PROJECT_WRITTEN", json!({"channel_id":"ch","path":"own.md","sha256":format!("{:x}",Sha256::digest(own.as_bytes())),"add":1,"del":0})),
            ("VERIFICATION_PASSED", json!({"revision":0,"checks":[],"revision_digest":"1".repeat(64),"checkout_digest":"2".repeat(64)})),
        ];
        for (kind, payload) in events {
            db.execute("INSERT INTO runtime_events(task_id,type,payload_json,actor,created_at) VALUES ('r1',?,?,'runtime','now')", (kind, payload.to_string()))?;
        }
        Ok(())
    }).unwrap();
    let path = store.key().directory().join("r1/result.md");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(&path, case["files"][0]["content"].as_str().unwrap()).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    f.save(&json!({"tasks":{"t1":{"status":"ready_to_merge","checkout":{"kind":"main"},"targetBranch":"main"}}}));
    let stale = "Result evidence is unavailable or the files changed. Request changes and run fresh checks.";
    // Another task or the owner changes unrelated files in the same folder.
    fs::write(f.root.join("other.txt"), "someone else\n").unwrap();
    fs::write(f.root.join("untracked.txt"), "new\n").unwrap();
    let reply = f.call("product.read", json!({})).await.unwrap();
    let actions = &reply["result"]["tasks"]["t1"]["reviewActions"];
    assert_ne!(actions["reason"], stale, "{actions}");
    assert_eq!(actions["accept"], true, "{actions}");
    // A file the task wrote changed after its result: acceptance is refused.
    fs::write(f.root.join("own.md"), "changed later\n").unwrap();
    let reply = f.call("product.read", json!({})).await.unwrap();
    let actions = &reply["result"]["tasks"]["t1"]["reviewActions"];
    assert_eq!(actions["accept"], false, "{actions}");
    assert_eq!(actions["reason"], stale, "{actions}");
    f.service.stop().await.unwrap();
}
