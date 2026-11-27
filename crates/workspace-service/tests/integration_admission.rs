//! Integration refuses in Rust before effects, against the offline Python oracle.
#[path = "support/hermetic.rs"]
mod hermetic;
use serde_json::{json, Value};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
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
        // Socket roots are deliberately independent of TMPDIR.
        let base = std::env::temp_dir().join(format!(
            "cw-int-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&base).unwrap();
        fs::set_permissions(&base, fs::Permissions::from_mode(0o700)).unwrap();
        let root = base.join("repo");
        let state = base.join("state");
        fs::create_dir(&root).unwrap();
        let service = Service::bind(Config {
            runtime_dir: base.join("run"),
            state_dir: state.clone(),
            allowed_root: base.clone(),
            launch: LaunchSpec::bare(),
            request_timeout: Duration::from_secs(10),
            shutdown_timeout: Duration::from_millis(100),
            event_capacity: 16,
        })
        .await
        .unwrap();
        let id = service.register(&root, "Integration").await.unwrap()["projectId"]
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
    fn store(&self) -> cortex_core::Store {
        cortex_core::Store::open(
            cortex_core::ProjectKey::new(
                &self.root,
                if workspace_protocol::channel::current().is_dev() {
                    cortex_core::StoreChannel::Dev
                } else {
                    cortex_core::StoreChannel::Release
                },
            )
            .unwrap(),
            cortex_core::OpenMode::Initialize,
        )
        .unwrap()
    }
    fn overlay(&self) -> PathBuf {
        self.state.join("product").join(format!("{}.json", self.id))
    }
    fn replace(&self, value: &Value) -> Value {
        match value {
            Value::String(s) => json!(s
                .replace("$ROOT", self.root.to_str().unwrap())
                .replace("$BASE", self.base.to_str().unwrap())
                .replace("p1", &self.id)),
            Value::Array(a) => Value::Array(a.iter().map(|v| self.replace(v)).collect()),
            Value::Object(m) => Value::Object(
                m.iter()
                    .map(|(k, v)| (k.clone(), self.replace(v)))
                    .collect(),
            ),
            v => v.clone(),
        }
    }
    async fn call(&self, params: Value) -> std::io::Result<Value> {
        self.service
            .machine(
                serde_json::from_value(json!({
                    "op":"product.integrate","projectId":self.id,"params":params
                }))
                .unwrap(),
            )
            .await
    }
    fn save(&self, value: &Value) {
        fs::create_dir_all(self.overlay().parent().unwrap()).unwrap();
        fs::set_permissions(
            self.overlay().parent().unwrap(),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        fs::write(self.overlay(), value.to_string()).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}
fn sql(value: &Value) -> rusqlite::types::Value {
    match value {
        Value::Null => rusqlite::types::Value::Null,
        Value::String(s) => rusqlite::types::Value::Text(s.clone()),
        Value::Number(n) => rusqlite::types::Value::Integer(n.as_i64().unwrap()),
        v => panic!("unexpected SQL fixture {v}"),
    }
}
fn authority(store: &cortex_core::Store) -> Vec<Vec<String>> {
    store
        .read(|db| {
            let mut result = Vec::new();
            for table in [
                "workspace_projects",
                "workspace_tasks",
                "workspace_task_options",
                "workspace_executions",
                "workspace_starts",
                "runtime_tasks",
                "runtime_nodes",
                "runtime_events",
                "task_owners",
                "workspace_outbox",
            ] {
                let mut q = db.prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))?;
                let count = q.column_count();
                result.push(
                    q.query_map([], |row| {
                        let mut values = Vec::new();
                        for i in 0..count {
                            values.push(format!("{:?}", row.get_ref(i)?));
                        }
                        Ok(values.join("|"))
                    })?
                    .collect::<Result<Vec<_>, _>>()?,
                );
            }
            Ok(result)
        })
        .unwrap()
}
fn setup(f: &Fixture, fixture: &Value, case: &Value) {
    for file in case["files"].as_array().unwrap() {
        fs::write(
            f.root.join(file["path"].as_str().unwrap()),
            file["content"].as_str().unwrap(),
        )
        .unwrap();
    }
    for args in case["gitCommands"].as_array().unwrap() {
        let mut cmd = std::process::Command::new("git");
        cmd.current_dir(&f.root);
        for arg in args.as_array().unwrap() {
            cmd.arg(f.replace(arg).as_str().unwrap());
        }
        for (key, value) in fixture["gitEnv"].as_object().unwrap() {
            cmd.env(key, value.as_str().unwrap());
        }
        for key in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_OBJECT_DIRECTORY",
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        ] {
            cmd.env_remove(key);
        }
        assert!(
            cmd.output().unwrap().status.success(),
            "{} fixture Git",
            case["name"]
        );
    }
    if case["git"] == "pending" {
        fs::write(f.base.join("worktree/a.txt"), "task-only\n").unwrap();
    }
}
#[tokio::test]
async fn integration_refusals_match_oracle_without_an_engine() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/integration_python.json")).unwrap();
    assert_eq!(fixture["cases"].as_array().unwrap().len(), 57);
    for case in fixture["cases"].as_array().unwrap() {
        let f = Fixture::new().await;
        setup(&f, &fixture, case);
        let store = if case["store"] == true {
            let store = f.store();
            store
                .write(|db| {
                    for row in case["rows"].as_array().unwrap() {
                        let row = f.replace(row);
                        db.execute(
                            row[0].as_str().unwrap(),
                            rusqlite::params_from_iter(row[1].as_array().unwrap().iter().map(sql)),
                        )?;
                    }
                    Ok(())
                })
                .unwrap();
            Some(store)
        } else {
            None
        };
        f.save(&f.replace(&case["saved"]));
        let before = fs::read(f.overlay()).unwrap();
        let authority_before = store.as_ref().map(authority);
        let index = fs::read(f.root.join(".git/index")).ok();
        let head = fs::read(f.root.join(".git/refs/heads/main")).ok();
        let content = fs::read(f.root.join("a.txt")).ok();
        let worktree = fs::read(f.base.join("worktree/a.txt")).ok();
        let reply = f.call(f.replace(&case["params"])).await.unwrap();
        assert_eq!(reply, case["expected"], "{} reply", case["name"]);
        assert_eq!(fs::read(f.overlay()).unwrap(), before, "overlay changed");
        assert_eq!(
            store.as_ref().map(authority),
            authority_before,
            "authority changed"
        );
        assert_eq!(
            fs::read(f.root.join(".git/index")).ok(),
            index,
            "real index changed"
        );
        assert_eq!(
            fs::read(f.root.join(".git/refs/heads/main")).ok(),
            head,
            "HEAD changed"
        );
        assert_eq!(
            fs::read(f.root.join("a.txt")).ok(),
            content,
            "user bytes changed"
        );
        assert_eq!(
            fs::read(f.base.join("worktree/a.txt")).ok(),
            worktree,
            "worktree bytes changed"
        );
        if store.is_none() {
            assert!(!f.root.join(".cortex/cortex.sqlite3").exists());
            assert!(!f.root.join(".cortex-dev/cortex.sqlite3").exists());
        }
        f.service.stop().await.unwrap();
    }
}
fn local_git(path: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(path)
        .output()
        .unwrap();
    assert!(out.status.success(), "fixture Git observation failed");
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}
#[tokio::test]
async fn committed_local_squash_matches_the_frozen_oracle_reply_state_and_git() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/integration_squash_python.json")).unwrap();
    assert_eq!(fixture["cases"].as_array().unwrap().len(), 17);
    for case in fixture["cases"].as_array().unwrap() {
        let f = Fixture::new().await;
        for step in case["steps"].as_array().unwrap() {
            let step = f.replace(step);
            if let Some(args) = step["argv"].as_array() {
                let mut command = std::process::Command::new("git");
                command.current_dir(&f.root);
                for arg in args {
                    command.arg(arg.as_str().unwrap());
                }
                for (key, value) in fixture["gitEnv"].as_object().unwrap() {
                    command.env(key, value.as_str().unwrap());
                }
                for key in [
                    "GIT_DIR",
                    "GIT_WORK_TREE",
                    "GIT_INDEX_FILE",
                    "GIT_OBJECT_DIRECTORY",
                    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
                ] {
                    command.env_remove(key);
                }
                assert!(
                    command.output().unwrap().status.success(),
                    "{} setup",
                    case["name"]
                );
            } else {
                let path = Path::new(step["path"].as_str().unwrap());
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, step["content"].as_str().unwrap()).unwrap();
                if step["executable"] == true {
                    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
                }
            }
        }
        let store = f.store();
        store
            .write(|db| {
                for row in case["rows"].as_array().unwrap() {
                    let row = f.replace(row);
                    db.execute(
                        row[0].as_str().unwrap(),
                        rusqlite::params_from_iter(row[1].as_array().unwrap().iter().map(sql)),
                    )?;
                }
                Ok(())
            })
            .unwrap();
        for file in case["reports"].as_array().unwrap() {
            let path = store.key().directory().join(file["path"].as_str().unwrap());
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, file["content"].as_str().unwrap()).unwrap();
        }
        f.save(&f.replace(&case["saved"]));
        let before = authority(&store);
        let target = f.base.join("target");
        let index_path = local_git(
            &target,
            &["rev-parse", "--path-format=absolute", "--git-path", "index"],
        );
        let index = fs::read(&index_path).unwrap();
        let reply = f.call(f.replace(&case["params"])).await.unwrap();
        assert_eq!(reply, case["expected"], "{} reply", case["name"]);
        assert_eq!(authority(&store), before, "{} authority", case["name"]);
        let mut after: Value = serde_json::from_slice(&fs::read(f.overlay()).unwrap()).unwrap();
        if reply["ok"] == true {
            assert!(after["tasks"]["t1"]["doneAt"]
                .as_u64()
                .is_some_and(|n| n > 0));
            after["tasks"]["t1"]["doneAt"] = json!("$NOW");
        }
        // The private read API adds defaults without writing them on refusal.
        if reply["ok"] != true {
            assert_eq!(
                after,
                f.replace(&case["saved"]),
                "{} unchanged",
                case["name"]
            );
        } else {
            assert_eq!(after, f.replace(&case["after"]), "{} overlay", case["name"]);
        }
        let observed = json!({
            "sourceHead":local_git(&f.root, &["rev-parse","HEAD"]),
            "targetSubject":local_git(&target, &["log","-1","--format=%s"]),
            "targetContent":fs::read_to_string(target.join("a.txt")).unwrap(),
            "targetStatus":local_git(&target, &["status","--porcelain"])
        });
        assert_eq!(observed, case["observation"], "{} Git", case["name"]);
        if case["name"] == "commit-failed" {
            assert_eq!(fs::read(index_path).unwrap(), index);
        }
        if case["name"] == "head-changed" {
            let intent: Value = serde_json::from_slice(
                &fs::read(
                    fs::read_dir(f.state.join("operations"))
                        .unwrap()
                        .next()
                        .unwrap()
                        .unwrap()
                        .path(),
                )
                .unwrap(),
            )
            .unwrap();
            assert_eq!(intent["state"], "unknown");
            let head = local_git(&target, &["rev-parse", "HEAD"]);
            let retried = f.call(f.replace(&case["params"])).await.unwrap();
            assert_eq!(retried["code"], "dirty_target");
            assert_eq!(local_git(&target, &["rev-parse", "HEAD"]), head);
            assert_eq!(authority(&store), before);
        }
        if case["name"] == "success" {
            // A real post-commit hook makes only this disposable overlay unwritable.
            // The Git commit survives; failure must be unknown, never a helper retry.
            let fault_target = f.base.join("fault-target");
            let base = case["saved"]["tasks"]["t1"]["baseRevision"]
                .as_str()
                .unwrap();
            local_git(
                &f.root,
                &[
                    "worktree",
                    "add",
                    "-b",
                    "fault-target",
                    fault_target.to_str().unwrap(),
                    base,
                ],
            );
            let mut saved = f.replace(&case["saved"]);
            saved["tasks"]["t1"]["targetBranch"] = json!("fault-target");
            f.save(&saved);
            let hook = f.base.join("hooks/post-commit");
            fs::create_dir_all(hook.parent().unwrap()).unwrap();
            fs::write(
                &hook,
                format!(
                    "#!/bin/sh\nmv '{}' '{}.before'\nmkdir '{}'\n",
                    f.overlay().display(),
                    f.overlay().display(),
                    f.overlay().display()
                ),
            )
            .unwrap();
            fs::set_permissions(&hook, fs::Permissions::from_mode(0o700)).unwrap();
            let mut params = f.replace(&case["params"]);
            params["targetBranch"] = json!("fault-target");
            params["requestId"] = json!("fault-once");
            let reply = f.call(params).await.unwrap();
            assert_eq!(reply["code"], "integration_unknown");
            assert!(f.overlay().is_dir());
            assert_eq!(
                local_git(&fault_target, &["log", "-1", "--format=%s"]),
                "Integrate completed task"
            );
            let intents = fs::read_dir(f.state.join("operations"))
                .unwrap()
                .map(|entry| {
                    serde_json::from_slice::<Value>(&fs::read(entry.unwrap().path()).unwrap())
                        .unwrap()
                })
                .collect::<Vec<_>>();
            assert!(intents.iter().any(|intent| intent["state"] == "unknown"));
            assert_eq!(authority(&store), before);
        }
        f.service.stop().await.unwrap();
    }
}
fn completed(f: &Fixture) -> cortex_core::Store {
    let store = f.store();
    store.write(|db| {
        db.execute("INSERT INTO workspace_projects VALUES (?,?,?,'now',1)",(&f.id,f.root.to_str().unwrap(),"fixture"))?;
        db.execute("INSERT INTO workspace_tasks(task_id,project_id,description,version,execution_id,created_at) VALUES ('t1',?,'Task',1,'e1','now')",[&f.id])?;
        db.execute("INSERT INTO tasks VALUES ('r1',?,'now')",[f.root.to_str().unwrap()])?;
        db.execute("INSERT INTO runtime_tasks(task_id,state,goal,created_at,updated_at) VALUES ('r1','COMPLETED','Goal','now','now')",[])?;
        db.execute("INSERT INTO task_owners VALUES ('r1','owner',1,'now')",[])?;
        db.execute("INSERT INTO acp_host_sessions VALUES ('s1',?,'r1','{}','{}','[]','now',NULL)",[f.root.to_str().unwrap()])?;
        db.execute("INSERT INTO workspace_executions VALUES ('e1','t1','r1','s1',1,1,'now')",[])?;
        Ok(())
    }).unwrap();
    store
}
#[tokio::test]
async fn unverifiable_result_refuses_natively_before_any_effect_for_every_method() {
    for params in [
        json!({"confirmed":true,"taskId":"t1","digest":"reviewed","method":"pr",
            "message":"Original message","tokens":{"routing":"retained-token"},"deleteWorktree":true}),
        json!({"confirmed":true,"taskId":"t1","digest":"reviewed"}),
        json!({"confirmed":true,"taskId":"t1","digest":"reviewed","method":"commit","message":"m"}),
    ] {
        let f = Fixture::new().await;
        let store = completed(&f);
        f.save(&json!({"tasks":{"t1":{"status":"ready_to_merge","acceptance":{"digest":"reviewed"},"keep":true}}}));
        let before = fs::read(f.overlay()).unwrap();
        let authority_before = authority(&store);
        let reply = f.call(params).await.unwrap();
        assert_eq!(reply["ok"], false);
        assert_eq!(reply["code"], "result_unavailable");
        assert_eq!(fs::read(f.overlay()).unwrap(), before);
        assert_eq!(authority(&store), authority_before);
        f.service.stop().await.unwrap();
    }
}
#[tokio::test]
async fn malformed_legacy_shapes_refuse_explicitly_without_a_provider() {
    for (saved, params) in [
        (
            json!({"tasks":{"t1":"scalar"}}),
            json!({"confirmed":true,"taskId":"t1"}),
        ),
        (
            json!({"tasks":{"t1":{"status":"ready_to_merge","acceptance":{"digest":"d"}}}}),
            json!({"confirmed":true,"taskId":"t1","digest":"d","targetBranch":7}),
        ),
        (
            json!({"tasks":{"t1":{"status":"ready_to_merge","acceptance":{"digest":5}}}}),
            json!({"confirmed":true,"taskId":"t1","digest":5}),
        ),
        (
            json!({"tasks":{}}),
            json!({"confirmed":true,"taskId":["t1"]}),
        ),
    ] {
        let f = Fixture::new().await;
        f.save(&saved);
        let reply = f.call(params).await.unwrap();
        assert_eq!(reply["ok"], false, "{reply}");
        f.service.stop().await.unwrap();
    }
    let f = Fixture::new().await;
    let reply = f.call(json!(["not", "an", "object"])).await.unwrap();
    assert_eq!(reply["code"], "invalid_request");
    f.service.stop().await.unwrap();
}
#[tokio::test]
async fn rename_with_a_non_text_title_refuses_natively_instead_of_delegating() {
    for title in [json!(1.5), json!([1]), json!({"a":1})] {
        let f = Fixture::new().await;
        completed(&f);
        f.save(&json!({"tasks":{"t1":{}}}));
        let reply = f
            .service
            .machine(
                serde_json::from_value(json!({
                    "op":"product.task","projectId":f.id,
                    "params":{"taskId":"t1","action":"rename","title":title}
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(reply["ok"], false, "{reply}");
        assert_eq!(reply["code"], "invalid_request", "{reply}");
        assert_eq!(reply["message"], "A task title must be text.");
        f.service.stop().await.unwrap();
    }
}
