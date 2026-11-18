//! Replies AND persisted state compared against the offline Python oracle.
#[path = "support/hermetic.rs"]
mod hermetic;
#[path = "task_actions/merge.rs"]
mod merge_tests;
#[path = "task_actions/resolution.rs"]
mod resolution_tests;
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
        // Unix socket limits must not depend on the caller's TMPDIR.
        let base = Path::new("/tmp").join(format!(
            "cw-act-{}-{}",
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
        let id = service.register(&root, "Actions").await.unwrap()["projectId"]
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
    fn path(&self) -> PathBuf {
        self.state.join("product").join(format!("{}.json", self.id))
    }
    fn replace(&self, v: &Value) -> Value {
        match v {
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
    async fn call(&self, p: Value) -> Value {
        self.service
            .machine(
                serde_json::from_value(json!({"op":"product.task","projectId":self.id,"params":p}))
                    .unwrap(),
            )
            .await
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}
fn seed(f: &Fixture, store: &cortex_core::Store) {
    store.write(|db|{
        db.execute("INSERT INTO workspace_projects VALUES (?,?,?,'now',1)",(&f.id,f.root.to_str().unwrap(),"fixture"))?;
        db.execute("INSERT INTO workspace_tasks(task_id,project_id,description,version,created_at) VALUES ('t1',?,'Original',1,'now')",[&f.id])?;
        Ok(())
    }).unwrap();
}
#[tokio::test]
async fn concurrent_edits_and_unknown_start_keep_the_draft_fence() {
    let f = Fixture::new().await;
    let store = f.store();
    seed(&f, &store);
    let payload = |title: &str| json!({"action":"edit","taskId":"t1","expectedVersion":"1","payload":{"title":title,"description":"Changed"}});
    let (a, b) = tokio::join!(f.call(payload("First")), f.call(payload("Second")));
    assert_ne!(a["ok"], b["ok"]);
    let failure = if a["ok"] == false { a } else { b };
    assert_eq!(failure["code"], "stale_version");
    assert_eq!(
        store
            .read(|db| Ok(db.query_row(
                "SELECT version FROM workspace_tasks WHERE task_id='t1'",
                [],
                |r| r.get::<_, i64>(0)
            )?))
            .unwrap(),
        2
    );
    store
        .write(|db| {
            db.execute(
                "UPDATE workspace_tasks SET execution_id='unknown-start' WHERE task_id='t1'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    let before = fs::read(f.path()).unwrap();
    for params in [
        payload("Late"),
        json!({"action":"settings","taskId":"t1","settings":{"model":"fixture","effort":"","mode":"guarded","language":"en"}}),
    ] {
        let reply = f.call(params).await;
        assert_eq!(reply["ok"], false);
        assert!(["task_started", "busy"].contains(&reply["code"].as_str().unwrap()));
        assert_eq!(fs::read(f.path()).unwrap(), before);
    }
    f.service.stop().await.unwrap();
}
/// Live run: an owner could not rename a started task ("I simply can't change the
/// name"). The title is presentation, not lifecycle: a started task's title saves
/// and reads back, while its saved request stays fenced. A cleared task refuses.
#[tokio::test]
async fn a_started_task_can_still_be_renamed() {
    let f = Fixture::new().await;
    let store = f.store();
    seed(&f, &store);
    // Renamed while a draft, then started: the earlier draft title must not win later.
    assert_eq!(
        f.call(json!({"action":"rename","taskId":"t1","title":"Draft name"}))
            .await["ok"],
        true
    );
    store
        .write(|db| {
            db.execute(
                "UPDATE workspace_tasks SET execution_id='started' WHERE task_id='t1'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    let reply = f
        .call(json!({"action":"rename","taskId":"t1","title":"  Started name  "}))
        .await;
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(reply["result"]["version"], "3", "{reply}");
    let saved: Value = serde_json::from_slice(&fs::read(f.path()).unwrap()).unwrap();
    assert_eq!(saved["tasks"]["t1"]["title"], "Started name");
    assert_eq!(saved["tasks"]["t1"]["titleState"], "user");
    let (description, execution, options) = store
        .read(|db| {
            Ok(db.query_row(
                "SELECT t.description,t.execution_id,o.options_json FROM workspace_tasks t JOIN workspace_task_options o USING(task_id) WHERE task_id='t1'",
                [],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, String>(2)?)),
            )?)
        })
        .unwrap();
    assert_eq!(
        (description.as_str(), execution.as_deref()),
        ("Original", Some("started"))
    );
    assert_eq!(
        serde_json::from_str::<Value>(&options).unwrap(),
        json!({"draftTitle":"Started name"})
    );
    let read = f
        .service
        .machine(
            serde_json::from_value(json!({"op":"product.read","projectId":f.id,"params":{}}))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        read["result"]["tasks"]["t1"]["title"], "Started name",
        "{read}"
    );
    // The saved request itself is still read-only once started.
    let edit = f.call(json!({"action":"edit","taskId":"t1","expectedVersion":"3","payload":{"title":"X","description":"Changed"}})).await;
    assert_eq!(edit["code"], "task_started", "{edit}");
    // A cleared task is not renamed, and nothing is written.
    let mut cleared = saved.clone();
    cleared["tasks"]["t1"]["archive"] = json!({"at":1,"previousStatus":"done"});
    fs::write(f.path(), cleared.to_string()).unwrap();
    let before = fs::read(f.path()).unwrap();
    let refused = f
        .call(json!({"action":"rename","taskId":"t1","title":"Too late"}))
        .await;
    assert_eq!(refused["code"], "not_editable", "{refused}");
    assert_eq!(fs::read(f.path()).unwrap(), before);
    f.service.stop().await.unwrap();
}
/// Live run: every task of a planned sprint read as the first sentence of its
/// description ("Библиотека tally"). The plan item's own title names the task
/// when it joins the sprint, and stays across reads; an owner's title is kept.
#[tokio::test]
async fn joining_a_sprint_records_the_plan_title() {
    let f = Fixture::new().await;
    let store = f.store();
    seed(&f, &store);
    store.write(|db| {
        db.execute("INSERT INTO workspace_tasks(task_id,project_id,description,version,created_at) VALUES ('t2',?,'Библиотека tally. Second.',1,'now')",[&f.id])?;
        Ok(())
    }).unwrap();
    fs::create_dir_all(f.path().parent().unwrap()).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(
        f.path().parent().unwrap(),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    fs::write(
        f.path(),
        json!({"tasks":{"t2":{"title":"Owner title","titleState":"user"}},
        "sprints":[{"id":"sp","taskIds":[],"targetBranch":null,"queueState":"idle"}]})
        .to_string(),
    )
    .unwrap();
    let join = |task: &str, title: Value| json!({"action":"sprint","taskId":task,"sprintId":"sp","dependencies":[],"title":title});
    assert_eq!(
        f.call(join("t1", json!("  Добавить метод decrement  ")))
            .await["ok"],
        true
    );
    assert_eq!(f.call(join("t2", json!("Plan title"))).await["ok"], true);
    let saved: Value = serde_json::from_slice(&fs::read(f.path()).unwrap()).unwrap();
    assert_eq!(saved["tasks"]["t1"]["title"], "Добавить метод decrement");
    assert_eq!(saved["tasks"]["t1"]["titleState"], "generated");
    assert_eq!(saved["tasks"]["t2"]["title"], "Owner title");
    assert_eq!(saved["tasks"]["t2"]["titleState"], "user");
    assert_eq!(saved["sprints"][0]["taskIds"], json!(["t1", "t2"]));
    let read = f
        .service
        .machine(
            serde_json::from_value(json!({"op":"product.read","projectId":f.id,"params":{}}))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        read["result"]["tasks"]["t1"]["title"], "Добавить метод decrement",
        "{read}"
    );
    // A title that is not text is refused before any write.
    let before = fs::read(f.path()).unwrap();
    let refused = f.call(join("t1", json!(["x"]))).await;
    assert_eq!(refused["code"], "invalid_request", "{refused}");
    assert_eq!(fs::read(f.path()).unwrap(), before);
    f.service.stop().await.unwrap();
}
#[tokio::test]
async fn legacy_projection_and_other_actions_never_select_the_provider() {
    let f = Fixture::new().await;
    let store = f.store();
    seed(&f, &store);
    store
        .write(|db| {
            db.execute(
                "INSERT INTO workspace_task_options VALUES ('t1',?)",
                [json!({"checkout":{"kind":"worktree"}}).to_string()],
            )?;
            Ok(())
        })
        .unwrap();
    for params in [
        json!({"action":"rename","taskId":"t1","title":"No effect"}),
        json!({"action":"accept","taskId":"t1"}),
        json!({"action":"auto_ready","taskId":"t1"}),
        json!({"action":"merged","taskId":"t1"}),
        json!({"action":"close","taskId":"t1","choice":"external","confirmed":true}),
        json!({"action":"archive","taskId":"t1"}),
        json!({"action":"resolution_started","taskId":"t1"}),
    ] {
        // Every action is answered natively (done or an explicit refusal); an
        // unmaterialized checkout never selects the retained provider.
        let reply = f
            .service
            .machine(
                serde_json::from_value(
                    json!({"op":"product.task","projectId":f.id,"params":params}),
                )
                .unwrap(),
            )
            .await
            .expect("native reply, not a retained-provider failure");
        assert!(reply["ok"].is_boolean(), "{reply}");
        if reply["ok"] == false {}
    }
    store
        .write(|db| {
            db.execute("DELETE FROM workspace_task_options WHERE task_id='t1'", [])?;
            Ok(())
        })
        .unwrap();
    let before = fs::read(f.path()).ok();
    let error=f.service.machine(serde_json::from_value(json!({"op":"product.task","projectId":f.id,"params":{"action":"rename","taskId":"t1","title":{"structured":"legacy"}}})).unwrap()).await.unwrap();
    assert_eq!(error["ok"], false);
    assert_eq!(error["code"], "invalid_request");
    assert_eq!(fs::read(f.path()).ok(), before);
    f.service.stop().await.unwrap();
}
fn sql(v: &Value) -> rusqlite::types::Value {
    match v {
        Value::Null => rusqlite::types::Value::Null,
        Value::String(s) => s.clone().into(),
        Value::Number(n) => n.as_i64().unwrap().into(),
        _ => panic!("bad SQL fixture"),
    }
}
#[tokio::test]
async fn actions_match_without_an_engine() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/task_actions_python.json")).unwrap();
    check_actions(&fixture).await;
}
#[tokio::test]
async fn owner_lifecycle_matches_without_an_engine() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/task_lifecycle_python.json")).unwrap();
    assert_eq!(fixture["cases"].as_array().unwrap().len(), 87);
    check_actions(&fixture).await;
}
#[tokio::test]
async fn owner_deletion_matches_without_an_engine() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/task-deletion-python.json")).unwrap();
    assert_eq!(fixture["cases"].as_array().unwrap().len(), 117);
    check_actions(&fixture).await;
}
#[tokio::test]
async fn owner_deletion_keeps_live_worker_and_exclusive_owner_fences() {
    for action in ["purge", "clear_old"] {
        for fence in ["running_worker", "exclusive_reader"] {
            let f = Fixture::new().await;
            let store = f.store();
            seed(&f, &store);
            store.write(|db| {
                db.execute("INSERT INTO tasks VALUES ('r1',?,'now')",[f.root.to_str()])?;
                db.execute("INSERT INTO runtime_tasks(task_id,state,goal,created_at,updated_at) VALUES ('r1','COMPLETED','Fixture','now','now')",[])?;
                db.execute("INSERT INTO workspace_executions VALUES ('e1','t1','r1','s1',1,1,'now')",[])?;
                db.execute("UPDATE workspace_tasks SET execution_id='e1' WHERE task_id='t1'",[])?;
                if fence=="running_worker" {
                    db.execute("INSERT INTO runtime_nodes VALUES ('r1',0,'n1','worker-backend-dev','Fixture','[]','[]','execution','running','a1',NULL)",[])?;
                }
                Ok(())
            }).unwrap();
            let record = if action == "purge" {
                json!({"status":"done","work":"merged","doneAt":100,"archive":{"at":123}})
            } else {
                json!({"status":"done","work":"merged","doneAt":100})
            };
            fs::create_dir_all(f.path().parent().unwrap()).unwrap();
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                f.path().parent().unwrap(),
                fs::Permissions::from_mode(0o700),
            )
            .unwrap();
            fs::write(f.path(), json!({"tasks":{"t1":record}}).to_string()).unwrap();
            let before = fs::read(f.path()).unwrap();
            let authority_before = authority(&store);
            let lock = if fence == "exclusive_reader" {
                Some(
                    cortex_core::store::locks::AccessLock::acquire(
                        &store.key().directory().join(".access.lock"),
                        false,
                        false,
                        Duration::ZERO,
                    )
                    .unwrap(),
                )
            } else {
                None
            };
            let reply = f
                .call(json!({"action":action,"taskId":"t1","confirmed":true,"cutoff":200}))
                .await;
            assert_eq!(reply["ok"], false);
            assert_eq!(
                reply["code"],
                if fence == "running_worker" {
                    "active_execution"
                } else {
                    "busy"
                }
            );
            assert_eq!(fs::read(f.path()).unwrap(), before);
            assert_eq!(authority(&store), authority_before);
            assert_eq!(
                store
                    .read(|db| Ok(db.query_row(
                        "SELECT count(*) FROM pending_deletions",
                        [],
                        |r| r.get::<_, i64>(0)
                    )?))
                    .unwrap(),
                0
            );
            drop(lock);
            f.service.stop().await.unwrap();
        }
    }
}
#[tokio::test]
async fn owner_deletion_retains_dirty_or_unowned_worktrees() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/task-deletion-python.json")).unwrap();
    let case = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "purge-owned-worktree")
        .unwrap();
    for dirty in [false, true] {
        let f = Fixture::new().await;
        setup_git(&f, &fixture, case);
        let mut saved = f.replace(&case["saved"]);
        saved["tasks"]["t1"]["checkout"]["createdByWorkspace"] = json!(dirty);
        let path = f.base.join("worktree");
        if dirty {
            fs::write(path.join("a.txt"), "keep dirty contents").unwrap();
        }
        let before = fs::read(path.join("a.txt")).unwrap();
        fs::create_dir_all(f.path().parent().unwrap()).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            f.path().parent().unwrap(),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        fs::write(f.path(), saved.to_string()).unwrap();
        if dirty {
            // Retained consent refers to changed source, but no matching digest:
            // refuse deletion, preserve the worktree and ask the owner again.
            let reply = f.call(f.replace(&case["params"])).await;
            assert_eq!(reply["code"], "unmerged_work");
        } else {
            let reply = f.call(f.replace(&case["params"])).await;
            assert_eq!(reply["ok"], true);
            let after: Value = serde_json::from_slice(&fs::read(f.path()).unwrap()).unwrap();
            assert_eq!(after["tasks"]["t1"]["retainedWorktree"], json!(path));
        }
        assert_eq!(fs::read(path.join("a.txt")).unwrap(), before);
        f.service.stop().await.unwrap();
    }
}
#[tokio::test]
async fn owner_deletion_committed_before_overlay_failure_never_falls_back_or_replays() {
    // Unix permissions are the observable fault boundary; do not claim this for root.
    assert_ne!(
        unsafe { libc::geteuid() },
        0,
        "this check requires a non-root test process"
    );
    let f = Fixture::new().await;
    let store = f.store();
    seed(&f, &store);
    fs::create_dir_all(f.path().parent().unwrap()).unwrap();
    use std::os::unix::fs::PermissionsExt;
    let parent = f.path().parent().unwrap().to_owned();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(
        f.path(),
        json!({"tasks":{"t1":{"status":"done","work":"merged","archive":{"at":123}}}}).to_string(),
    )
    .unwrap();
    let before = fs::read(f.path()).unwrap();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o500)).unwrap();
    let params = json!({"action":"purge","taskId":"t1","confirmed":true});
    let reply = f.call(params.clone()).await;
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(reply["ok"], false);
    assert_eq!(reply["code"], "machine_tool_failed");
    assert_eq!(fs::read(f.path()).unwrap(), before);
    assert_eq!(
        store
            .read(|db| Ok(db.query_row(
                "SELECT count(*) FROM workspace_tasks WHERE task_id='t1'",
                [],
                |r| r.get::<_, i64>(0)
            )?))
            .unwrap(),
        0
    );
    let outbox = authority(&store);
    let retry = f.call(params).await;
    assert_eq!(retry["code"], "not_found");
    assert_eq!(authority(&store), outbox);
    assert_eq!(fs::read(f.path()).unwrap(), before);
    f.service.stop().await.unwrap();
}
fn normalize_measured_bytes(actual: &mut Value, expected: &Value) {
    if expected == "$MEASURED_BYTES" {
        assert!(
            actual.as_u64().is_some(),
            "reclaimed bytes must be measured nonnegative integers"
        );
        *actual = json!("$MEASURED_BYTES");
    } else {
        match expected {
            Value::Object(m) => {
                for (key, v) in m {
                    if let Some(a) = actual.get_mut(key) {
                        normalize_measured_bytes(a, v);
                    }
                }
            }
            Value::Array(a) => {
                for (i, v) in a.iter().enumerate() {
                    if let Some(a) = actual.get_mut(i) {
                        normalize_measured_bytes(a, v);
                    }
                }
            }
            _ => (),
        }
    }
}
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}
fn normalize_clock(actual: &mut Value, expected: &Value, clock: u64, before: u64, after: u64) {
    if expected == &json!(clock) {
        let at = actual.as_u64().expect("timestamp must remain numeric");
        assert!(
            (before..=after).contains(&at),
            "timestamp outside the action window"
        );
        *actual = json!(clock);
    } else {
        match expected {
            Value::Object(m) => {
                for (key, value) in m {
                    if let Some(a) = actual.get_mut(key) {
                        normalize_clock(a, value, clock, before, after);
                    }
                }
            }
            Value::Array(values) => {
                for (i, value) in values.iter().enumerate() {
                    if let Some(a) = actual.get_mut(i) {
                        normalize_clock(a, value, clock, before, after);
                    }
                }
            }
            _ => (),
        }
    }
}
fn setup_git(f: &Fixture, fixture: &Value, case: &Value) {
    if let Some(files) = case["files"].as_array() {
        for file in files {
            fs::write(
                f.root.join(file["path"].as_str().unwrap()),
                file["content"].as_str().unwrap(),
            )
            .unwrap();
        }
    }
    if let Some(commands) = case["gitCommands"].as_array() {
        for argv in commands {
            let mut cmd = std::process::Command::new("git");
            cmd.current_dir(&f.root);
            for arg in argv.as_array().unwrap() {
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
            let done = cmd.output().unwrap();
            assert!(
                done.status.success(),
                "{} fixture setup Git failed",
                case["name"]
            );
        }
    }
    if case["git"] == "dirty" {
        fs::write(f.root.join("a.txt"), "dirty\n").unwrap();
    }
    if case["git"] == "not-contained" {
        fs::write(
            f.root.parent().unwrap().join("worktree/a.txt"),
            "task-only\n",
        )
        .unwrap();
    }
}
fn authority(store: &cortex_core::Store) -> Vec<Vec<String>> {
    store
        .read(|db| {
            let mut snapshot = Vec::new();
            for table in [
                "runtime_tasks",
                "runtime_nodes",
                "runtime_events",
                "workspace_executions",
                "workspace_starts",
                "task_owners",
                "workspace_outbox",
            ] {
                let mut query = db.prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))?;
                let columns = query.column_count();
                snapshot.push(
                    query
                        .query_map([], |row| {
                            let mut values = Vec::new();
                            for i in 0..columns {
                                values.push(format!("{:?}", row.get_ref(i)?));
                            }
                            Ok(values.join("|"))
                        })?
                        .collect::<Result<Vec<_>, _>>()?,
                );
            }
            Ok(snapshot)
        })
        .unwrap()
}
#[tokio::test]
async fn owner_review_matches_without_an_engine() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/task-review-python.json")).unwrap();
    assert_eq!(fixture["cases"].as_array().unwrap().len(), 83);
    let cases = fixture["cases"].as_array().unwrap();
    let unique = cases
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(cases.len(), unique.len());
    check_actions(&fixture).await;
}
#[tokio::test]
async fn review_unsupported_legacy_shapes_refuse_natively_before_effects() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/task-review-python.json")).unwrap();
    let case = &fixture["cases"][0];
    for action in ["accept"] {
        for checkout in [
            json!({"kind":123,"path":"/unrelated","branch":"task"}),
            json!("legacy"),
            json!({"kind":"worktree"}),
        ] {
            let f = Fixture::new().await;
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
            fs::write(f.root.join("a.txt"), "original\n").unwrap();
            fs::create_dir_all(f.path().parent().unwrap()).unwrap();
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                f.path().parent().unwrap(),
                fs::Permissions::from_mode(0o700),
            )
            .unwrap();
            let mut saved = f.replace(&case["saved"]);
            saved["tasks"]["t1"]["checkout"] = checkout;
            fs::write(f.path(), saved.to_string()).unwrap();
            let before = fs::read(f.path()).unwrap();
            let authoritative = authority(&store);
            let mut params = f.replace(&case["params"]);
            params["action"] = json!(action);
            params["discard"] = json!(true);
            let error = f
                .service
                .machine(
                    serde_json::from_value(
                        json!({"op":"product.task","projectId":f.id,"params":params}),
                    )
                    .unwrap(),
                )
                .await
                .expect("native refusal, not a retained-provider failure");
            // An incomplete checkout record has no observable worktree: refused
            // before any effect.
            assert_eq!(error["ok"], false);
            assert_eq!(error["code"], "unknown_checkout");
            assert_eq!(fs::read(f.path()).unwrap(), before);
            assert_eq!(authority(&store), authoritative);
            f.service.stop().await.unwrap();
        }
    }
}
#[tokio::test]
async fn owner_review_store_exclusion_and_failed_save_do_not_delegate() {
    use std::os::unix::fs::PermissionsExt;
    assert_ne!(
        unsafe { libc::geteuid() },
        0,
        "requires a non-root test process"
    );
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/task-review-python.json")).unwrap();
    for action in ["accept"] {
        for fault in ["store_lock", "overlay_save"] {
            let f = Fixture::new().await;
            let store = f.store();
            let case = &fixture["cases"][0];
            setup_git(&f, &fixture, case);
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
            fs::create_dir_all(f.path().parent().unwrap()).unwrap();
            let path = f.path();
            let parent = path.parent().unwrap();
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).unwrap();
            fs::write(f.path(), f.replace(&case["saved"]).to_string()).unwrap();
            let before = fs::read(f.path()).unwrap();
            let authoritative = authority(&store);
            let guard = if fault == "store_lock" {
                Some(
                    cortex_core::store::locks::AccessLock::acquire(
                        &store.key().directory().join(".access.lock"),
                        false,
                        true,
                        Duration::ZERO,
                    )
                    .unwrap(),
                )
            } else {
                fs::set_permissions(parent, fs::Permissions::from_mode(0o500)).unwrap();
                None
            };
            let mut params = f.replace(&case["params"]);
            params["action"] = json!(action);
            params["discard"] = json!(true);
            let reply = f.call(params.clone()).await;
            drop(guard);
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).unwrap();
            assert_eq!(reply["ok"], false, "{action}/{fault}");
            assert_eq!(
                reply["code"],
                if fault == "store_lock" {
                    "busy"
                } else {
                    "machine_tool_failed"
                }
            );
            assert_eq!(fs::read(f.path()).unwrap(), before);
            assert_eq!(authority(&store), authoritative);
            // Clearing the actual fault permits the owner to retry the private overlay.
            assert_eq!(f.call(params).await["ok"], true);
            assert_eq!(authority(&store), authoritative);
            assert_eq!(fs::read(f.root.join("a.txt")).unwrap(), b"original\n");
            f.service.stop().await.unwrap();
        }
    }
}
/// The retained cases name files inline; the desktop now uploads them first and sends
/// references. Each body is stored the way the upload stores it, so the cases keep
/// their meaning (an invalid name is still refused, as an invalid reference).
fn saved_references(state: &std::path::Path, mut params: Value) -> Value {
    use base64::Engine as _;
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let Some(items) = params["payload"]["attachments"].as_array_mut() else {
        return params;
    };
    let folder = state.join("attachments");
    fs::DirBuilder::new()
        .mode(0o700)
        .recursive(true)
        .create(&folder)
        .unwrap();
    for item in items {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(item["data"].as_str().unwrap())
            .unwrap();
        let sha = format!("{:x}", Sha256::digest(&bytes));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(folder.join(&sha))
            .unwrap();
        std::io::Write::write_all(&mut file, &bytes).unwrap();
        *item = json!({"name":item["name"],"mimeType":item["mimeType"],"size":bytes.len(),"sha256":sha});
    }
    params
}
async fn check_actions(fixture: &Value) {
    for case in fixture["cases"].as_array().unwrap() {
        // auto_ready now requires native finish-time mode evidence and uses the
        // acceptance validator; the historical unchecked lifecycle oracle no longer applies.
        // Its replacement is covered by task_review::native_acceptance_tests and the queue test.
        if case["params"]["action"] == "auto_ready" {
            continue;
        }
        let f = Fixture::new().await;
        setup_git(&f, fixture, case);
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
        fs::create_dir_all(f.path().parent().unwrap()).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            f.path().parent().unwrap(),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        fs::write(f.path(), f.replace(&case["saved"]).to_string()).unwrap();
        if let Some(reports) = case["reports"].as_array() {
            for report in reports {
                let path = store
                    .as_ref()
                    .unwrap()
                    .key()
                    .directory()
                    .join(report["path"].as_str().unwrap());
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, report["content"].as_str().unwrap()).unwrap();
            }
        }
        if case["drift"] == true {
            fs::write(f.root.join("a.txt"), "late drift\n").unwrap();
        }
        let authority_before = store.as_ref().map(authority);
        let index_before = fs::read(f.root.join(".git/index")).ok();
        let content_before = fs::read(f.root.join("a.txt")).ok();
        let worktree_before = fs::read(f.base.join("worktree/a.txt")).ok();
        if let Some(days) = case.get("days") {
            fs::write(
                f.state.join("machine.json"),
                json!({"autoArchiveDays":days}).to_string(),
            )
            .unwrap();
        }
        if case["unobservable"] == true {
            fs::set_permissions(f.root.join("a.txt"), fs::Permissions::from_mode(0o000)).unwrap();
        }
        let before_time = now_ms();
        let mut reply = f
            .call(saved_references(&f.state, f.replace(&case["params"])))
            .await;
        let after_time = now_ms();
        if case["unobservable"] == true {
            fs::set_permissions(f.root.join("a.txt"), fs::Permissions::from_mode(0o644)).unwrap();
        }
        if fixture["measuredBytes"] == true {
            normalize_measured_bytes(&mut reply, &case["expected"]);
        }
        assert_eq!(reply, case["expected"], "{} reply", case["name"]);
        if fixture["clock"].is_number() {
            assert_eq!(
                store.as_ref().map(authority),
                authority_before,
                "overlay changed authoritative runtime state"
            );
            assert_eq!(
                fs::read(f.root.join(".git/index")).ok(),
                index_before,
                "overlay altered real index"
            );
            assert_eq!(
                fs::read(f.root.join("a.txt")).ok(),
                content_before,
                "overlay altered user contents"
            );
            assert_eq!(
                fs::read(f.base.join("worktree/a.txt")).ok(),
                worktree_before,
                "overlay altered task contents"
            );
        }
        let mut after: Value = serde_json::from_slice(&fs::read(f.path()).unwrap()).unwrap();
        for (key, default) in [
            ("tasks", json!({})),
            ("sprints", json!([])),
            ("drafts", json!({})),
            ("messages", json!([])),
            ("comments", json!({})),
        ] {
            if after.get(key).is_none() {
                after[key] = default;
            }
        }
        if after.get("ghAvailable").is_some() {
            assert!(after["ghAvailable"].is_boolean());
            after["ghAvailable"] = json!("$GH_AVAILABLE");
        }
        if let Some(clock) = fixture["clock"].as_u64() {
            if let Some(delay) = case["delayMs"].as_f64() {
                let actual = after["tasks"]["t1"]["archiveAskAfter"].as_f64().unwrap() - delay;
                assert!((before_time as f64..=after_time as f64).contains(&actual));
                after["tasks"]["t1"]["archiveAskAfter"] =
                    case["after"]["tasks"]["t1"]["archiveAskAfter"].clone();
            }
            let mut expected = case["after"].clone();
            if case.get("delayMs").is_some() {
                expected["tasks"]["t1"]["archiveAskAfter"] = Value::Null;
            }
            normalize_clock(&mut after, &expected, clock, before_time, after_time);
        }
        if fixture["measuredBytes"] == true {
            normalize_measured_bytes(&mut after, &case["after"]);
            assert_eq!(
                fs::read(f.root.join(".git/index")).ok(),
                index_before,
                "deletion changed the source index"
            );
            assert_eq!(
                fs::read(f.root.join("a.txt")).ok(),
                content_before,
                "deletion changed source files"
            );
            assert_eq!(
                f.base.join("worktree").exists(),
                case["worktreeExists"] == true,
                "{} worktree cleanup",
                case["name"]
            );
        }
        assert_eq!(
            after,
            f.replace(&case["after"]),
            "{} persisted JSON",
            case["name"]
        );
        let store = store.or_else(|| {
            (!case["observed"].is_null()).then(|| {
                assert!(
                    f.root
                        .join(if workspace_protocol::channel::current().is_dev() {
                            ".cortex-dev/cortex.sqlite3"
                        } else {
                            ".cortex/cortex.sqlite3"
                        })
                        .is_file(),
                    "action failed to initialize its store"
                );
                f.store()
            })
        });
        if let Some(store) = &store {
            if let Some(counts) = case["deletionCounts"].as_object() {
                for (table, expected) in counts {
                    assert!([
                        "workspace_tasks",
                        "workspace_task_options",
                        "workspace_starts",
                        "workspace_executions",
                        "runtime_tasks",
                        "tasks",
                        "pending_deletions",
                        "task_owners",
                        "workspace_outbox"
                    ]
                    .contains(&table.as_str()));
                    let count = store
                        .read(|db| {
                            Ok(
                                db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| {
                                    r.get::<_, i64>(0)
                                })?,
                            )
                        })
                        .unwrap();
                    assert_eq!(json!(count), *expected, "{} {table}", case["name"]);
                }
                assert!(store
                    .read(|db| Ok(db
                        .prepare("PRAGMA foreign_key_check")?
                        .query([])?
                        .next()?
                        .is_none()))
                    .unwrap());
            }
            let observed=store.read(|db|{
                let mut q=db.prepare("SELECT task_id,description,version,execution_id FROM workspace_tasks ORDER BY task_id")?;
                let tasks=q.query_map([],|r|Ok(json!([r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?,r.get::<_,Option<String>>(3)?])))?
                    .collect::<Result<Vec<_>,_>>()?;
                let mut q=db.prepare("SELECT task_id,options_json FROM workspace_task_options ORDER BY task_id")?;
                let options=q.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?
                    .map(|row| {let (task,raw)=row?;Ok(json!([task,serde_json::from_str::<Value>(&raw)?]))})
                    .collect::<cortex_core::CoreResult<Vec<_>>>()?;
                let mut q=db.prepare("SELECT project_id,version FROM workspace_projects ORDER BY project_id")?;
                let projects=q.query_map([],|r|Ok(json!([r.get::<_,String>(0)?,r.get::<_,i64>(1)?])))?
                    .collect::<Result<Vec<_>,_>>()?;
                Ok(json!({"tasks":tasks,"options":options,"projects":projects}))
            }).unwrap();
            assert_eq!(
                observed,
                f.replace(&case["observed"]),
                "{} persisted SQLite",
                case["name"]
            );
            if case["store"] == true
                && reply["ok"] == true
                && ["edit", "rename", "settings"]
                    .contains(&case["params"]["action"].as_str().unwrap())
                && case["params"]["taskId"] == "t1"
            {
                let count=store.read(|db|Ok(db.query_row("SELECT count(*) FROM workspace_outbox WHERE project_id=? AND entity_id='t1' AND kind='task'",[&f.id],|r|r.get::<_,i64>(0))?)).unwrap();
                assert!(count > 0, "mutation omitted transactional outbox");
            }
        } else {
            assert!(
                !f.root.join(".cortex/cortex.sqlite3").exists()
                    && !f.root.join(".cortex-dev/cortex.sqlite3").exists()
            );
        }
        f.service.stop().await.unwrap();
    }
}
#[tokio::test]
async fn archived_work_is_deleted_only_after_the_owner_confirms_it() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/task_lifecycle_python.json")).unwrap();
    // A dirty merged task: archiving it keeps everything and asks nothing.
    let case = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "archive-dirty")
        .unwrap();
    let f = Fixture::new().await;
    setup_git(&f, &fixture, case);
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
    fs::create_dir_all(f.path().parent().unwrap()).unwrap();
    // A saved record that still carries the retired question fields.
    let mut saved = f.replace(&case["saved"]);
    saved["tasks"]["t1"]["archiveQuestion"] = json!(true);
    saved["tasks"]["t1"]["archiveAskAfter"] = json!(99);
    fs::write(f.path(), saved.to_string()).unwrap();
    let call = |action: &str| {
        let mut p = f.replace(&case["params"]);
        p["action"] = json!(action);
        p
    };
    // Confirming deletion needs an archived task; nothing changes before that.
    let early = f.call(call("confirm_discard")).await;
    assert_eq!(early["code"], "not_archived", "{early}");
    let dirty = fs::read(f.root.join("a.txt")).ok();
    let archived = f.call(call("archive")).await;
    assert_eq!(archived["ok"], true, "{archived}");
    let after: Value = serde_json::from_slice(&fs::read(f.path()).unwrap()).unwrap();
    let t = &after["tasks"]["t1"];
    assert!(t["archive"]["at"].is_u64());
    assert_eq!(t["archiveQuestion"], false);
    assert!(t.get("archiveAskAfter").is_none());
    assert_eq!(t["work"], "merged", "the work record is untouched");
    assert_eq!(fs::read(f.root.join("a.txt")).ok(), dirty);
    // Restore brings it back as before.
    let restored = f.call(call("restore")).await;
    assert_eq!(restored["ok"], true, "{restored}");
    let after: Value = serde_json::from_slice(&fs::read(f.path()).unwrap()).unwrap();
    assert!(after["tasks"]["t1"]["archive"].is_null());
    assert_eq!(f.call(call("archive")).await["ok"], true);
    let confirmed = f.call(call("confirm_discard")).await;
    assert_eq!(confirmed["ok"], true, "{confirmed}");
    let after: Value = serde_json::from_slice(&fs::read(f.path()).unwrap()).unwrap();
    assert_eq!(after["tasks"]["t1"]["work"], "discard_confirmed");
    assert!(after["tasks"]["t1"]["archive"]["at"].is_u64());
    assert_eq!(fs::read(f.root.join("a.txt")).ok(), dirty);
    f.service.stop().await.unwrap();
}
