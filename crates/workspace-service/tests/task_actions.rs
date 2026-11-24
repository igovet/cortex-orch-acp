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
        Self::with_routes(None).await
    }

    async fn with_routes(routes: Option<std::sync::Arc<dyn workspace_service::native_runtime::NativeRoutes>>) -> Self {
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
        let config = Config {
            runtime_dir: base.join("run"),
            state_dir: state.clone(),
            allowed_root: base.clone(),
            launch: LaunchSpec::bare(),
            request_timeout: Duration::from_secs(10),
            shutdown_timeout: Duration::from_millis(100),
            event_capacity: 16,
        };
        let service = match routes {
            Some(routes) => Service::bind_native(config, routes).await,
            None => Service::bind(config).await,
        }.unwrap();
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
async fn sprint_close_reads_core_start_and_disputed_completion_before_archiving() {
    let f = Fixture::new().await;
    let store = f.store();
    seed(&f, &store);
    let sprint = |params: Value| f.service.machine(serde_json::from_value(json!({
        "op":"product.sprint","projectId":f.id,"params":params
    })).unwrap());
    let created = sprint(json!({"action":"create","description":"Sprint"})).await.unwrap();
    let id = &created["result"]["sprintId"];
    let read = || serde_json::from_slice::<Value>(&fs::read(f.path()).unwrap()).unwrap();
    let mut product = read();
    product["sprints"][0]["taskIds"] = json!(["t1"]);
    product["tasks"]["t1"] = json!({"sprintId":id,"title":"Core member"});
    fs::write(f.path(), serde_json::to_vec(&product).unwrap()).unwrap();
    store.write(|db| {
        db.execute("INSERT INTO tasks VALUES ('r1',?,'now')", [f.root.to_str()])?;
        db.execute("INSERT INTO runtime_tasks(task_id,state,goal,created_at,updated_at)
            VALUES ('r1','EXECUTING','Fixture','now','now')", [])?;
        db.execute("INSERT INTO task_owners VALUES ('r1','owner',1,'now')", [])?;
        db.execute("INSERT INTO acp_host_sessions VALUES ('s1',?,'r1','{}','{}','[]','now',NULL)", [f.root.to_str()])?;
        db.execute("INSERT INTO workspace_executions VALUES ('started','t1','r1','s1',1,1,'now')", [])?;
        db.execute("UPDATE workspace_tasks SET execution_id='started' WHERE task_id='t1'", [])?;
        Ok(())
    }).unwrap();
    let close = json!({"action":"close","sprintId":id,"confirmed":true});
    let reply = sprint(close.clone()).await.unwrap();
    assert_eq!(reply["code"], "sprint_close_blocked", "{reply}");
    assert!(reply["message"].as_str().unwrap().contains("Core member (t1): started but not done"), "{reply}");
    assert_eq!(read(), product);

    // A saved Done overlay cannot hide contradictory authoritative evidence.
    product["tasks"]["t1"]["status"] = json!("done");
    fs::write(f.path(), serde_json::to_vec(&product).unwrap()).unwrap();
    store.write(|db| {
        db.execute("UPDATE workspace_tasks SET execution_id='unknown-start' WHERE task_id='t1'", [])?;
        Ok(())
    }).unwrap();
    let reply = sprint(close).await.unwrap();
    assert_eq!(reply["code"], "sprint_close_blocked", "{reply}");
    assert!(reply["message"].as_str().unwrap().contains("Core member (t1): task records disagree"), "{reply}");
    assert_eq!(read(), product);
    f.service.stop().await.unwrap();
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
        assert_eq!(reply["code"], "contradictory_evidence");
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
            db.execute("INSERT INTO tasks VALUES ('r1',?,'now')", [f.root.to_str()])?;
            db.execute(
                "INSERT INTO runtime_tasks(task_id,state,goal,created_at,updated_at)
                VALUES ('r1','EXECUTING','Fixture','now','now')",
                [],
            )?;
            db.execute("INSERT INTO task_owners VALUES ('r1','owner',1,'now')", [])?;
            db.execute(
                "INSERT INTO acp_host_sessions VALUES ('s1',?,'r1','{}','{}','[]','now',NULL)",
                [f.root.to_str()],
            )?;
            db.execute(
                "INSERT INTO workspace_executions VALUES ('started','t1','r1','s1',1,1,'now')",
                [],
            )?;
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
async fn close_commit_and_task_draft_do_not_copy_french_task_prose() {
    use std::os::unix::fs::PermissionsExt;

    // This path uses the real short-text resolver rather than the task session route.
    // Run it in a fresh process so neither the routing override nor native credentials
    // can leak through the environment. Enable the shared private HOME/XDG initializer.
    const CHILD: &str = "CW_TASK_DRAFT_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "close_commit_and_task_draft_do_not_copy_french_task_prose",
            ])
            .env_clear()
            .env("CARGO_MANIFEST_DIR", env!("CARGO_MANIFEST_DIR"))
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env(CHILD, "1")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("test result: ok. 1 passed;"),
            "{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    assert!(home.as_os_str().as_encoded_bytes().starts_with(b"/tmp/cw-test-home-"));
    for (key, leaf) in [("XDG_CONFIG_HOME", "config"), ("XDG_STATE_HOME", "state")] {
        let directory = PathBuf::from(std::env::var_os(key).unwrap());
        assert_eq!(directory, home.join(leaf));
        assert!(fs::read_dir(directory).unwrap().next().is_none());
    }
    assert!(std::env::var_os("CORTEX_ROUTING").is_none());

    let f = Fixture::new().await;
    let store = f.store();
    seed(&f, &store);
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .current_dir(&f.root)
            .env("HOME", &f.base)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "-c", "commit.gpgsign=false"])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.name", "Fixture"]);
    git(&["config", "user.email", "fixture@example.invalid"]);
    git(&["config", "commit.gpgsign", "false"]);
    fs::write(f.root.join("upload.txt"), "before\n").unwrap();
    git(&["add", "upload.txt"]);
    git(&["commit", "-qm", "Initialize fixture"]);
    fs::write(f.root.join("upload.txt"), "after\n").unwrap();

    store.write(|db| {
        db.execute("UPDATE workspace_tasks SET description='Corriger le chargement' WHERE task_id='t1'", [])?;
        Ok(())
    }).unwrap();
    let path = f.path();
    let parent = path.parent().unwrap();
    fs::create_dir_all(parent).unwrap();
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(&path, json!({
        "tasks":{"t1":{"sprintId":"s1","title":"Réparer le chargement","checkout":{"kind":"main"}}},
        "sprints":[{"id":"s1","runMode":"automatic"}],
        "sprintRunModeVersion":1
    }).to_string()).unwrap();

    let started = f.call(json!({"action":"draft_integration_text","taskId":"t1","method":"commit",
        "targetBranch":"main","paths":["upload.txt"],"summary":"Le chargement est réparé."})).await;
    assert_eq!(started["ok"], true, "{started}");
    let draft = tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let reply = f.call(json!({"action":"short_text_poll","taskId":"t1",
                "jobId":started["result"]["jobId"]})).await;
            assert_eq!(reply["ok"], true, "{reply}");
            if reply["result"]["state"] == "done" {
                break reply["result"].clone();
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    }).await.unwrap();
    assert_eq!(draft["source"], "fallback");
    assert!(draft["route"].is_null());
    assert_eq!(draft["reason"], "no_routing", "{draft}");
    assert_eq!(draft["text"]["message"], "Update 1 file\n\nChanged files:\n- `upload.txt` (+1 -1)");

    store.write(|db| {
        db.execute("INSERT INTO tasks VALUES ('r1',?,'now')", [f.root.to_str()])?;
        db.execute("INSERT INTO runtime_tasks(task_id,state,goal,created_at,updated_at) VALUES ('r1','COMPLETED','Fixture','now','now')", [])?;
        db.execute("INSERT INTO task_owners VALUES ('r1','owner',1,'now')", [])?;
        db.execute("INSERT INTO acp_host_sessions VALUES ('s1',?,'r1','{}','{}','[]','now',NULL)", [f.root.to_str()])?;
        db.execute("INSERT INTO workspace_executions VALUES ('e1','t1','r1','s1',1,1,'now')", [])?;
        db.execute("UPDATE workspace_tasks SET execution_id='e1' WHERE task_id='t1'", [])?;
        db.execute("INSERT INTO runtime_events(task_id,type,payload_json,actor,created_at) VALUES ('r1','NATIVE_PROJECT_WRITTEN',?,'runtime','now')",
            [json!({"path":"upload.txt"}).to_string()])?;
        Ok(())
    }).unwrap();

    let closed = f.call(json!({"action":"close","taskId":"t1","choice":"as_is","confirmed":true})).await;
    assert_eq!(closed["result"]["asIsCommit"]["committed"], true, "{closed}");
    assert_eq!(git(&["log", "-1", "--format=%s"]).trim(), "Close task as is (not verified)");
}

#[tokio::test]
async fn as_is_close_reports_a_commit_outcome_only_for_eligible_automatic_tasks() {
    use std::os::unix::fs::PermissionsExt;

    for mode in ["manual", "hybrid", "automatic"] {
        let f = Fixture::new().await;
        seed(&f, &f.store());
        let path = f.path();
        let parent = path.parent().unwrap();
        fs::create_dir_all(parent).unwrap();
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(
            &path,
            json!({
                "tasks":{"t1":{"sprintId":"s1","checkout":{"kind":"main"}}},
                "sprints":[{"id":"s1","runMode":mode}],
                "sprintRunModeVersion":1
            })
            .to_string(),
        )
        .unwrap();

        let reply = f
            .call(json!({"action":"close","taskId":"t1","choice":"as_is","confirmed":true}))
            .await;
        let mut expected = json!({"ok":true,"result":{"done":true,"reclamation":null}});
        if mode == "automatic" {
            expected["result"]["asIsCommit"] = json!({"committed":false,"reason":"no_git_history"});
        }
        assert_eq!(reply, expected, "{mode}");
        assert!(!f.root.join(".git").exists());
    }
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
                db.execute("INSERT INTO task_owners VALUES ('r1','owner',1,'now')",[])?;
                db.execute("INSERT INTO acp_host_sessions VALUES ('s1',?,'r1','{}','{}','[]','now',NULL)",[f.root.to_str()])?;
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
fn worktree_git(root: &Path, argv: &[&str]) -> String {
    let done = std::process::Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["-c", "core.hooksPath=/dev/null", "-c", "user.name=Test",
               "-c", "user.email=test@example.invalid"])
        .current_dir(root)
        .args(argv)
        .output().unwrap();
    assert!(done.status.success(), "{argv:?}: {}", String::from_utf8_lossy(&done.stderr));
    String::from_utf8(done.stdout).unwrap().trim().into()
}

struct ReworkRoute;
impl workspace_service::native_runtime::NativeRoutes for ReworkRoute {
    fn route(
        &self,
        root: &Path,
        _: &str,
    ) -> cortex_core::CoreResult<workspace_service::native_sessions::SessionRoute> {
        use cortex_engine::environment::{EngineKind, LaunchSpec as EngineLaunch};
        // This fixture checks the service boundary and durable handoff, not model
        // completion. The local process cannot dispatch model calls.
        Ok(workspace_service::native_sessions::SessionRoute {
            launch: EngineLaunch {
                engine: EngineKind::Codex,
                executable: "/usr/bin/false".into(),
                args: Vec::new(),
                cwd: root.into(),
                native_home: root.into(),
                credential_directory: root.into(),
                base_environment: Default::default(),
                codex_launcher: None,
                confinement: None,
            },
            relay: "/usr/bin/false".into(),
            broker_parent: root.into(),
            connection: "fixture".into(),
            connection_limit: 1,
            model: "fixture".into(),
            parameters: Default::default(),
            instructions: "Local rework fixture.".into(),
            deadline: Duration::from_secs(2),
        })
    }
}

#[tokio::test]
async fn worktree_start_requires_git_without_blocking_project_folder_tasks() {
    use workspace_service::{connect, read_frame, write_frame};

    let f = Fixture::with_routes(Some(std::sync::Arc::new(ReworkRoute))).await;
    let directory = f.base.join("trees");
    let settings = f.service.machine(serde_json::from_value(json!({
        "op":"machine.update","projectId":f.id,"params":{"patch":{"worktreeRoot":directory}},
    })).unwrap()).await.unwrap();
    assert_eq!(settings["ok"], true, "{settings}");
    f.service.activate(&f.id).await.unwrap();

    let server = tokio::spawn(f.service.clone().serve());
    let mut client = connect(f.service.socket_path()).await.unwrap();
    let call = |operation: &str, target: &str, id: &str, version: &str, payload: Value| json!({
        "jsonrpc":"2.0","id":id,"method":"workspace","params":{
            "protocol":{"major":1,"minor":8},"requestId":id,"projectId":f.id,
            "actorId":"fixture","operation":operation,"target":target,
            "ownerEpoch":null,"expectedVersion":version,"payload":payload,
        }
    });
    write_frame(&mut client, &call("tasks.create", &f.id, "create-tree", "0",
        json!({"description":"Dedicated checkout","checkout":{"kind":"worktree"}})))
        .await.unwrap();
    let created = read_frame(&mut client).await.unwrap();
    let draft = created["result"]["target"].as_str().unwrap_or_else(|| panic!("{created}"));
    let worktree_draft = draft.to_owned();
    let start = call("tasks.start", &worktree_draft, "start-tree", "1", json!({}));
    for _ in 0..2 {
        write_frame(&mut client, &start).await.unwrap();
        let reply = read_frame(&mut client).await.unwrap();
        assert_eq!(reply, json!({
            "jsonrpc":"2.0","id":"start-tree","error":{
                "code":-32000,
                "message":"A separate worktree needs a Git repository. Initialize Git or choose Project folder.",
                "data":{"kind":"worktree_start","reason":"worktree_git_required"},
            }
        }));
    }
    let store = f.store();
    store.read(|db| {
        for table in ["runtime_tasks", "workspace_executions", "task_workspaces"] {
            let count: i64 = db.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))?;
            assert_eq!(count, 0, "{table}");
        }
        Ok(())
    }).unwrap();
    assert!(!f.root.join(".git").exists());
    assert!(fs::read_dir(&directory).is_ok_and(|mut entries| entries.next().is_none())
        || !directory.exists());

    write_frame(&mut client, &call("tasks.create", &f.id, "create-main", "0",
        json!({"description":"Project folder checkout","checkout":{"kind":"main"}})))
        .await.unwrap();
    let created = read_frame(&mut client).await.unwrap();
    let draft = created["result"]["target"].as_str().unwrap_or_else(|| panic!("{created}"));
    write_frame(&mut client, &call("tasks.start", draft, "start-main", "1", json!({})))
        .await.unwrap();
    let started = read_frame(&mut client).await.unwrap();
    assert_eq!(started["result"]["outcome"], "recorded", "{started}");

    worktree_git(&f.root, &["init", "-b", "main"]);
    let unborn = call("tasks.start", &worktree_draft, "start-unborn", "1", json!({}));
    write_frame(&mut client, &unborn).await.unwrap();
    let reply = read_frame(&mut client).await.unwrap();
    assert_eq!(reply["error"], json!({
        "code":-32000,
        "message":"A separate worktree needs a commit. Make the first commit or choose Project folder.",
        "data":{"kind":"worktree_start","reason":"worktree_head_unborn"},
    }), "{reply}");

    drop(client);
    f.service.stop().await.unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn owner_rework_uses_the_current_target_and_reopens_sprint_membership() {
    use cortex_core::workspace::{self, worktrees};
    for (kind, close) in [
        ("main", "merged"),
        ("worktree", "merged"),
        ("worktree", "already_in_target"),
        ("worktree", "external"),
    ] {
        let f = Fixture::with_routes(Some(std::sync::Arc::new(ReworkRoute))).await;
        let response = |op: &str, params: Value| {
            let service = f.service.clone();
            let request = serde_json::from_value(json!({"op":op,"projectId":f.id,"params":params})).unwrap();
            async move { service.machine(request).await.unwrap() }
        };
        let store = f.store();
        worktree_git(&f.root, &["init", "-b", "main"]);
        fs::write(f.root.join("tracked"), "initial").unwrap();
        worktree_git(&f.root, &["add", "tracked"]);
        worktree_git(&f.root, &["commit", "-m", "initial"]);
        let initial = worktree_git(&f.root, &["rev-parse", "HEAD"]);
        let directory = f.base.join("trees");
        let settings = response("machine.update", json!({"patch":{"worktreeRoot":directory}})).await;
        assert_eq!(settings["ok"], true, "{settings}");
        f.service.activate(&f.id).await.unwrap();
        let actor = format!("uid:{}", unsafe { libc::geteuid() });
        let command = |operation: &str, target: &str, request: &str, expected: &str, payload: Value| json!({
            "protocol":{"major":1,"minor":8},"requestId":request,"projectId":f.id,
            "actorId":actor,"operation":operation,"target":target,
            "ownerEpoch":null,"expectedVersion":expected,"payload":payload,
        });
        let task = |kind: &str, create_key: &str, start_key: &str| {
            let t = workspace::create_draft(&store, &f.id, &actor, &command(
                "tasks.create", &f.id, create_key, "0",
                json!({"description":"Fixture","checkout":{"kind":kind}}),
            )).unwrap().receipt["target"].as_str().unwrap().to_owned();
            let start = command("tasks.start", &t, start_key, "1", json!({}));
            let admitted = if kind == "main" {
                workspace::start_native(&store, &f.id, &actor, &start)
            } else {
                workspace::start_native_worktree(&store, &f.id, &actor, &start, &directory)
            }.unwrap();
            (t, admitted)
        };
        let (task, admission) = task(kind, "create-old", "start-old");
        let (dependent, downstream) = {
            let draft = workspace::create_draft(&store, &f.id, &actor, &command(
                "tasks.create", &f.id, "create-next", "0", json!({"description":"Depends on old result"}),
            )).unwrap().receipt["target"].as_str().unwrap().to_owned();
            let start = command("tasks.start", &draft, "start-next", "1", json!({}));
            (draft, workspace::start_native(&store, &f.id, &actor, &start).unwrap())
        };
        let execution = admission["receipt"]["target"].as_str().unwrap();
        let runtime = admission["task"].as_str().unwrap();
        store.write(|db| {
            for id in [runtime, downstream["task"].as_str().unwrap()] {
                db.execute("UPDATE runtime_tasks SET state='COMPLETED',revision=2,synthesis='Old result'
                    WHERE task_id=?", [id])?;
            }
            Ok(())
        }).unwrap();
        let old_tree = worktrees::owned(&store, &f.id, &task).unwrap();
        if let Some(tree) = &old_tree {
            if close == "merged" {
                worktree_git(&f.root, &["worktree", "remove", tree.path.to_str().unwrap()]);
            } else {
                fs::write(tree.path.join("landed"), "old task change").unwrap();
                worktree_git(&tree.path, &["add", "landed"]);
                worktree_git(&tree.path, &["commit", "-m", "old task change"]);
                let source = worktree_git(&tree.path, &["rev-parse", "HEAD"]);
                worktree_git(&f.root, &["cherry-pick", &source]);
            }
        }
        fs::create_dir_all(f.path().parent().unwrap()).unwrap();
        // Seed the comparison override retained after a prior Update from target.
        let mut closed = json!({
            "tasks":{
                (&task):{"status":"done","work":"merged","doneAt":now_ms(),"targetBranch":"main",
                    "checkout":{"kind":kind},"integration":{"targetBranch":"main","commit":initial},
                    "acceptance":{"version":"1"},"sprintId":"sprint","changesBaseRevision":initial},
                (&dependent):{"status":"done","sprintId":"sprint","dependencies":[task]},
            },
            "sprints":[{"id":"sprint","taskIds":[task,dependent],"runMode":"manual","queueState":"complete"}],
        });
        if close != "merged" {
            let record = &mut closed["tasks"][&task];
            record["status"] = json!("ready_to_merge");
            record["work"] = json!("unmerged");
            record["integration"] = Value::Null;
            record["baseRevision"] = json!(initial);
            fs::write(f.path(), serde_json::to_vec(&closed).unwrap()).unwrap();
            let result = f.call(json!({"action":"close","choice":close,"confirmed":true,"taskId":task})).await;
            assert_eq!(result["ok"], true, "{result}");
            closed = serde_json::from_slice(&fs::read(f.path()).unwrap()).unwrap();
            let saved = &closed["tasks"][&task];
            assert!(saved["acceptance"].is_null() && saved["integration"].is_null());
            assert_eq!(saved["notVerifiedByCortex"], true);
            assert_eq!(
                saved["alreadyInTargetBranch"],
                if close == "already_in_target" { json!("main") } else { Value::Null },
            );
        } else if kind == "worktree" {
            closed["tasks"][&task]["conflict"] = json!("none");
            closed["tasks"][&task]["merge"] = json!({
                "finished":true,"gitState":"completed","targetRevision":initial,
                "freshChecks":{"state":"requested"},
            });
        }
        fs::write(f.root.join("tracked"), "advanced target").unwrap();
        worktree_git(&f.root, &["commit", "-am", "advance target"]);
        let advanced = worktree_git(&f.root, &["rev-parse", "HEAD"]);
        assert_ne!(advanced, initial);
        fs::write(f.path(), serde_json::to_vec(&closed).unwrap()).unwrap();
        let read = response("product.board", json!({})).await;
        let projected = &read["result"]["tasks"][&task];
        assert_eq!(projected["reworkAvailable"], true, "{read}");
        assert_eq!(projected["reworkWarnings"], json!(["dependents_started"]));
        let upload = response("product.attachment", json!({"action":"upload","files":[
            {"name":"notes.txt","mimeType":"text/plain","data":"bm90ZXM="},
        ]})).await;
        assert_eq!(upload["ok"], true, "{upload}");
        let request = json!({
            "action":"rework","taskId":task,"requestId":"explicit-rework","text":"Do it again.",
            "attachments":upload["result"],
            "executionId":execution,"expectedVersion":projected["reworkVersion"],
            "ownerEpoch":projected["reworkOwnerEpoch"],
        });
        assert_eq!(
            f.service.prompt_execution(&f.id, execution, "Late ordinary message", Some("late"), json!([]))
                .await.unwrap_err().to_string(),
            "task_not_resumable",
        );
        assert_eq!(f.call(json!({"action":"reopen","taskId":task})).await["code"], "task_not_resumable");
        for (key, value, refusal) in [
            ("text",json!(" \n "),"invalid_message"),
            ("attachments",json!([{}]),"invalid_attachments"),
            ("expectedVersion",json!("0"),"stale_version"),
            ("ownerEpoch",json!("0"),"stale_epoch"),
            ("somethingElse",json!(true),"invalid_request"),
        ] {
            let mut bad = request.clone();
            bad[key] = value;
            let reply = f.call(bad).await;
            assert_eq!(reply["code"], refusal, "{reply}");
        }
        if close == "merged" && kind == "worktree" {
            for (finished, state) in [(false, "pending"), (true, "unknown")] {
                let mut unresolved = closed.clone();
                unresolved["tasks"][&task]["merge"]["finished"] = json!(finished);
                unresolved["tasks"][&task]["merge"]["gitState"] = json!(state);
                fs::write(f.path(), serde_json::to_vec(&unresolved).unwrap()).unwrap();
                let board = response("product.board", json!({})).await;
                let projected = &board["result"]["tasks"][&task];
                assert_eq!(projected["reworkAvailable"], false, "{board}");
                assert_eq!(projected["reworkUnavailableReason"], "merge_in_progress");
                assert_eq!(f.call(request.clone()).await["code"], "merge_in_progress");
            }
        }
        let mut archived = closed.clone();
        archived["tasks"][&task]["archive"] = json!({"at":2});
        fs::write(f.path(), serde_json::to_vec(&archived).unwrap()).unwrap();
        let paused = f.call(request.clone()).await;
        assert_eq!(paused["code"], "task_archived", "{paused}");
        assert_eq!(paused["message"], "Restore the task first.", "{paused}");
        let archived_board = response("product.board", json!({})).await;
        assert_eq!(archived_board["result"]["tasks"][&task]["reworkAvailable"], false);
        let restored = f.call(json!({"action":"restore","taskId":task})).await;
        assert_eq!(restored["ok"], true, "{restored}");
        let reply = f.call(request.clone()).await;
        assert_eq!(reply["ok"], true, "{reply}");
        assert_eq!(reply["result"]["state"], "PLANNING");
        if close == "external" {
            assert!(reply["result"]["baseRevision"].is_null(), "{reply}");
            let current = worktrees::owned(&store, &f.id, &task).unwrap().unwrap();
            assert_eq!(current.path, old_tree.unwrap().path);
            let overlay: Value = serde_json::from_slice(&fs::read(f.path()).unwrap()).unwrap();
            assert_eq!(overlay["tasks"][&task]["changesBaseRevision"], initial);
            f.service.stop().await.unwrap();
            continue;
        }
        assert_eq!(reply["result"]["baseRevision"], advanced);
        let changes = response("repo.snapshot", json!({})).await;
        assert_eq!(changes["result"]["taskDiffs"][&task]["files"], json!([]), "{changes}");
        assert_eq!(f.call(request.clone()).await, reply);
        let board = response("product.board", json!({})).await;
        let working = &board["result"]["tasks"][&task];
        for key in ["status", "doneAt", "acceptance", "integration"] {
            assert!(working[key].is_null(), "{key}: {board}");
        }
        assert_eq!(working["reworkAvailable"], false, "{board}");
        assert_eq!(board["result"]["sprints"][0]["queueState"], "paused", "{board}");
        // The durable event also recovers a lost overlay replacement.
        fs::write(f.path(), serde_json::to_vec(&closed).unwrap()).unwrap();
        let recovered = response("product.board", json!({})).await;
        assert_eq!(recovered["result"]["tasks"][&task]["baseRevision"], advanced);
        assert!(recovered["result"]["tasks"][&task]["integration"].is_null());
        let changes = response("repo.snapshot", json!({})).await;
        assert_eq!(changes["result"]["taskDiffs"][&task]["files"], json!([]), "{changes}");
        let sprint_close = response("product.sprint", json!({"action":"close","sprintId":"sprint","confirmed":true})).await;
        assert_eq!(sprint_close["code"], "rework_pending", "{sprint_close}");
        let checkout = if let Some(old) = old_tree {
            let current = worktrees::owned(&store, &f.id, &task).unwrap().unwrap();
            assert_ne!(current.path, old.path);
            assert_eq!(old.path.exists(), close == "already_in_target");
            assert_eq!(current.base_sha, advanced);
            assert_eq!(worktree_git(&current.path, &["rev-parse", "HEAD"]), advanced);
            assert_eq!(fs::read_to_string(current.path.join("tracked")).unwrap(), "advanced target");
            current.path
        } else {
            assert_eq!(worktree_git(&f.root, &["rev-parse", "HEAD"]), advanced);
            f.root.clone()
        };
        fs::write(checkout.join("new-edit"), "second cycle only").unwrap();
        let changes = response("repo.snapshot", json!({})).await;
        let files = changes["result"]["taskDiffs"][&task]["files"].as_array().unwrap();
        assert_eq!(files.len(), 1, "{changes}");
        assert_eq!(files[0]["path"], "new-edit", "{changes}");
        store.read(|db| {
            let (origin, integration, base): (String,Option<String>,String) = db.query_row(
                "SELECT json_extract(payload_json,'$.origin'),
                    json_extract(payload_json,'$.closed.integration.commit'),
                    json_extract(payload_json,'$.baseRevision') FROM runtime_events
                 WHERE type='TASK_REOPENED' AND task_id=?", [runtime],
                |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
            )?;
            assert_eq!(origin, "owner_rework");
            assert_eq!(integration.as_deref(), (close == "merged").then_some(initial.as_str()));
            assert_eq!(base, advanced);
            let source: i64 = db.query_row(
                "SELECT count(*) FROM runtime_events WHERE task_id=? AND type='USER_STEERING'
                  AND json_extract(payload_json,'$.text')='Do it again.'
                  AND json_extract(payload_json,'$.attachments[0].name')='notes.txt'", [runtime], |r|r.get(0),
            )?;
            assert_eq!(source, 1);
            Ok(())
        }).unwrap();
        f.service.stop().await.unwrap();
    }
}

#[tokio::test]
async fn owner_rework_after_cleanup_restores_the_verified_unmerged_branch() {
    use cortex_core::workspace::{self, worktrees};
    let f = Fixture::with_routes(Some(std::sync::Arc::new(ReworkRoute))).await;
    let store = f.store();
    worktree_git(&f.root, &["init", "-b", "main"]);
    worktree_git(&f.root, &["commit", "--allow-empty", "-m", "initial"]);
    let base = worktree_git(&f.root, &["rev-parse", "HEAD"]);
    let trees = f.base.join("trees");
    fs::write(
        f.state.join("machine.json"),
        json!({"worktreeRoot":trees}).to_string(),
    )
    .unwrap();
    f.service.activate(&f.id).await.unwrap();
    let actor = format!("uid:{}", unsafe { libc::geteuid() });
    let command = |operation: &str, target: &str, request: &str, expected: &str, payload: Value| json!({
        "protocol":{"major":1,"minor":8},"requestId":request,"projectId":f.id,
        "actorId":actor,"operation":operation,"target":target,
        "ownerEpoch":null,"expectedVersion":expected,"payload":payload,
    });
    let draft = workspace::create_draft(&store, &f.id, &actor, &command(
        "tasks.create", &f.id, "create", "0",
        json!({"description":"Retain the unmerged commit","checkout":{"kind":"worktree"}}),
    )).unwrap().receipt["target"].as_str().unwrap().to_owned();
    let start = command("tasks.start", &draft, "start", "1", json!({}));
    let admission = workspace::start_native_worktree(&store, &f.id, &actor, &start, &trees).unwrap();
    let task = admission["task"].as_str().unwrap();
    let execution = admission["receipt"]["target"].as_str().unwrap();
    let old = worktrees::owned(&store, &f.id, &draft).unwrap().unwrap();
    fs::write(old.path.join("previous.txt"), "unmerged work").unwrap();
    worktree_git(&old.path, &["add", "previous.txt"]);
    worktree_git(&old.path, &["commit", "-m", "keep this change"]);
    let retained = worktree_git(&old.path, &["rev-parse", "HEAD"]);
    worktree_git(&f.root, &["commit", "--allow-empty", "-m", "advance target separately"]);
    let target = worktree_git(&f.root, &["rev-parse", "HEAD"]);
    assert_ne!(retained, target);
    store.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='CANCELLED' WHERE task_id=?", [task])?;
        Ok(())
    }).unwrap();
    let closed = f.call(json!({
        "action":"close_remove_worktree","taskId":draft,"confirmed":true,
    })).await;
    assert_eq!(closed["result"]["worktreeCleanup"]["status"], "removed", "{closed}");
    assert!(closed["result"]["worktreeCleanup"]["retainedHead"].is_null());
    assert!(!old.path.exists());
    let branch = format!("refs/heads/{}", old.branch);
    assert_eq!(worktree_git(&f.root, &["rev-parse", &branch]), retained);
    let board = f.service.machine(serde_json::from_value(json!({
        "op":"product.board","projectId":f.id,"params":{},
    })).unwrap()).await.unwrap();
    let projected = &board["result"]["tasks"][&draft];
    assert_eq!(projected["reworkAvailable"], true, "{board}");
    let request = json!({
        "action":"rework","taskId":draft,"requestId":"rework","text":"Build on the unmerged work.",
        "executionId":execution,"expectedVersion":projected["reworkVersion"],
        "ownerEpoch":projected["reworkOwnerEpoch"],
    });

    // A branch with the right name and ancestry but a different tip is not proof.
    worktree_git(&f.root, &["update-ref", &branch, &base, &retained]);
    let refused = f.call(request.clone()).await;
    assert_eq!(refused["code"], "worktree_unavailable", "{refused}");
    assert_eq!(worktrees::owned(&store, &f.id, &draft).unwrap().unwrap().id, old.id);
    store.read(|db| {
        assert_eq!(db.query_row(
            "SELECT state FROM runtime_tasks WHERE task_id=?", [task], |r| r.get::<_, String>(0),
        )?, "CANCELLED");
        assert_eq!(db.query_row(
            "SELECT count(*) FROM runtime_events WHERE task_id=? AND type='TASK_REOPENED'",
            [task], |r| r.get::<_, i64>(0),
        )?, 0);
        Ok(())
    }).unwrap();
    worktree_git(&f.root, &["update-ref", &branch, &retained, &base]);

    // Git must not override another checkout of the retained branch.
    let elsewhere = f.base.join("elsewhere");
    let elsewhere_path = elsewhere.to_str().unwrap();
    worktree_git(&f.root, &["worktree", "add", elsewhere_path, &old.branch]);
    let occupied = f.call(request.clone()).await;
    assert_eq!(occupied["code"], "worktree_git_failed", "{occupied}");
    assert!(elsewhere.join("previous.txt").is_file());
    worktree_git(&f.root, &["worktree", "remove", elsewhere_path]);

    let reopened = f.call(request.clone()).await;
    assert_eq!(reopened["ok"], true, "{reopened}");
    assert_eq!(reopened["result"]["state"], "PLANNING");
    assert_eq!(reopened["result"]["baseRevision"], base);
    assert_eq!(f.call(request).await, reopened);
    let current = worktrees::owned(&store, &f.id, &draft).unwrap().unwrap();
    assert_ne!(current.path, old.path);
    assert_eq!(current.branch, old.branch);
    assert_eq!(current.base_sha, base);
    assert_eq!(worktree_git(&current.path, &["rev-parse", "HEAD"]), retained);
    assert_eq!(fs::read_to_string(current.path.join("previous.txt")).unwrap(), "unmerged work");
    assert_eq!(worktree_git(&f.root, &["rev-parse", "HEAD"]), target);
    assert!(!f.root.join("previous.txt").exists());
    f.service.stop().await.unwrap();
}

#[tokio::test]
async fn owner_rework_can_start_a_task_that_was_closed_before_its_first_run() {
    for kind in ["main", "worktree"] {
        let f = Fixture::with_routes(Some(std::sync::Arc::new(ReworkRoute))).await;
        let store = f.store();
        let git = kind == "worktree";
        if git {
            worktree_git(&f.root, &["init", "-b", "main"]);
            worktree_git(&f.root, &["commit", "--allow-empty", "-m", "fixture"]);
        }
        fs::write(f.state.join("machine.json"), json!({"worktreeRoot":f.base.join("trees")}).to_string()).unwrap();
        f.service.activate(&f.id).await.unwrap();
        let create = f.service.workspace(json!({"protocol":{"major":1,"minor":8},
            "requestId":"draft","projectId":f.id,"operation":"tasks.create","target":f.id,
            "expectedVersion":"0","ownerEpoch":null,
            "payload":{"description":"Original task","checkout":{"kind":kind}},
        })).await.unwrap();
        let task = create["target"].as_str().unwrap();
        let close = f.call(json!({"action":"close","taskId":task,"choice":"as_is","confirmed":true})).await;
        assert_eq!(close["ok"], true, "{close}");
        let board = f.service.machine(serde_json::from_value(json!({"op":"product.board",
            "projectId":f.id,"params":{}})).unwrap()).await.unwrap();
        let projected = &board["result"]["tasks"][task];
        assert_eq!(projected["reworkAvailable"], true, "{board}");
        assert!(projected["reworkOwnerEpoch"].is_null());
        assert!(projected["reworkExecutionId"].is_null());
        let plain = f.service.workspace(json!({"protocol":{"major":1,"minor":8},
            "requestId":"unexpected-start","projectId":f.id,"operation":"tasks.start","target":task,
            "expectedVersion":projected["reworkVersion"],"ownerEpoch":null,"payload":{},
        })).await.unwrap_err();
        assert_eq!(plain.to_string(), "task_not_resumable");
        let request = json!({"action":"rework","taskId":task,"requestId":"first-rework","text":"Begin rework.",
            "expectedVersion":projected["reworkVersion"],"ownerEpoch":null,"executionId":null});
        let result = f.call(request.clone()).await;
        assert_eq!(result["ok"], true, "{result}");
        assert_eq!(result["result"]["state"], "PLANNING");
        assert_eq!(f.call(request).await, result);
        store.read(|db| {
            let (execution, event, steering): (String, i64, i64) = db.query_row(
                "SELECT t.execution_id,
                    (SELECT count(*) FROM runtime_events v WHERE v.task_id=e.runtime_task_id AND v.type='TASK_REOPENED'
                     AND json_extract(v.payload_json,'$.previous')='NOT_STARTED'),
                    (SELECT count(*) FROM runtime_events v WHERE v.task_id=e.runtime_task_id AND v.type='USER_STEERING'
                     AND json_extract(v.payload_json,'$.text')='Begin rework.')
                 FROM workspace_tasks t JOIN workspace_executions e ON e.execution_id=t.execution_id
                 WHERE t.task_id=?", [task], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
            )?;
            assert_eq!(execution, result["result"]["target"]);
            assert_eq!((event, steering), (1, 1));
            Ok(())
        }).unwrap();
        let binding = cortex_core::workspace::worktrees::owned(&store, &f.id, task).unwrap();
        assert_eq!(binding.is_some(), git);
        f.service.stop().await.unwrap();
    }
}

#[tokio::test]
async fn worktree_cleanup_requires_a_settled_owned_tree_and_preserves_the_branch() {
    use cortex_core::workspace::{self, worktrees::{self, Cleanup}};
    for (kind, expected, reason) in [
        ("clean-close", "removed", Value::Null),
        ("dirty-close", "retained", json!("worktree_dirty")),
        ("locked-close", "retained", json!("worktree_occupied")),
        ("missing-close", "retained", json!("worktree_missing")),
        ("uncertain-merge-cleanup", "retained", json!("worktree_cleanup_unconfirmed")),
        ("clean-purge", "removed", Value::Null),
        ("dirty-purge", "retained", json!("worktree_dirty")),
        ("main", "refused", Value::Null),
    ] {
        let f = Fixture::new().await;
        let store = f.store();
        seed(&f, &store);
        worktree_git(&f.root, &["init"]);
        fs::write(f.root.join("tracked"), "committed").unwrap();
        fs::write(f.root.join(".gitignore"), "local-ignore\n").unwrap();
        worktree_git(&f.root, &["add", "tracked", ".gitignore"]);
        worktree_git(&f.root, &["commit", "-m", "base"]);
        let command = |operation, target, request, expected, payload| json!({
            "protocol":{"major":1,"minor":8},"requestId":request,"projectId":f.id,
            "actorId":"fixture","operation":operation,"target":target,
            "ownerEpoch":null,"expectedVersion":expected,"payload":payload,
        });
        let checkout_kind = if kind == "main" { "main" } else { "worktree" };
        let draft = workspace::create_draft(&store, &f.id, "fixture", &command(
            "tasks.create", &f.id, "draft", "0",
            json!({"description":"Task to keep or delete","checkout":{"kind":checkout_kind}}),
        )).unwrap().receipt["target"].as_str().unwrap().to_owned();
        let start = command("tasks.start", &draft, "start", "1", json!({}));
        let admitted = if kind == "main" {
            workspace::start_native(&store, &f.id, "fixture", &start)
        } else {
            workspace::start_native_worktree(&store, &f.id, "fixture", &start, &f.base.join("trees"))
        }.unwrap();
        let binding = worktrees::owned(&store, &f.id, &draft).unwrap();
        let task = admitted["task"].as_str().unwrap();
        let close = json!({"action":"close_remove_worktree","taskId":draft,"confirmed":true});
        if binding.is_some() {
            let active = f.call(close.clone()).await;
            assert_eq!(active["code"], "stop_required", "{kind}: {active}");
        }
        store.write(|db| {
            db.execute("UPDATE runtime_tasks SET state='CANCELLED' WHERE task_id=?", [task])?;
            Ok(())
        }).unwrap();
        if let Some(binding) = &binding {
            let close_as_is = f.call(json!({"action":"close","choice":"as_is","taskId":draft,"confirmed":true})).await;
            assert_eq!(close_as_is["ok"], true, "{kind}: {close_as_is}");
            assert!(binding.path.is_dir(), "ordinary close keeps the folder");
            if kind == "dirty-close" {
                // Even ignored artifacts are retained.
                fs::write(binding.path.join("local-ignore"), "keep").unwrap();
            } else if kind == "locked-close" {
                worktree_git(&f.root, &["worktree", "lock", binding.path.to_str().unwrap()]);
            } else if kind == "missing-close" {
                // An older pending intent is not evidence that Cortex removed the folder.
                store.write(|db| worktrees::queue_close_tx(db, &f.id, &draft)).unwrap();
                worktree_git(&f.root, &["worktree", "remove", binding.path.to_str().unwrap()]);
            } else if kind == "uncertain-merge-cleanup" {
                // Model an unconfirmed prior removal with the checkout still present.
                let previous = json!({
                    "status":"retained","path":binding.path,
                    "reason":"worktree_cleanup_unconfirmed","afterMerge":true,
                });
                store.write(|db| {
                    db.execute(
                        "UPDATE native_worktree_starts SET close_cleanup=? WHERE runtime_task_id=?",
                        (previous.to_string(), task),
                    )?;
                    Ok(())
                }).unwrap();
            }
        }
        let purge = kind.ends_with("-purge");
        let reply = if purge {
            let path = &binding.as_ref().unwrap().path;
            if kind == "clean-purge" {
                let merge = f.call(json!({"action":"merged","taskId":draft,"integration":null})).await;
                assert_eq!(merge["ok"], true, "{merge}");
            } else {
                fs::write(path.join("tracked"), "keep dirty work").unwrap();
            }
            let archived = f.call(json!({"action":"archive","taskId":draft})).await;
            assert_eq!(archived["ok"], true, "{archived}");
            assert!(path.is_dir(), "archive keeps the folder");
            if kind == "dirty-purge" {
                let confirmed = f.call(json!({"action":"confirm_discard","taskId":draft})).await;
                assert_eq!(confirmed["ok"], true, "{confirmed}");
            }
            f.call(json!({"action":"purge","taskId":draft,"confirmed":true})).await
        } else {
            f.call(close.clone()).await
        };
        if let Some(binding) = binding {
            assert_eq!(reply["ok"], true, "{kind}: {reply}");
            let cleanup = &reply["result"]["worktreeCleanup"];
            assert_eq!(cleanup["status"], expected, "{kind}: {reply}");
            assert_eq!(cleanup["reason"], reason, "{kind}: {reply}");
            let disposition = if purge { Cleanup::Purge } else { Cleanup::Close };
            if kind == "dirty-purge" {
                assert_eq!(fs::read_to_string(binding.path.join("tracked")).unwrap(), "keep dirty work");
                let count: i64 = store.read(|db| Ok(db.query_row(
                    "SELECT count(*) FROM workspace_tasks WHERE task_id=?", [&draft], |r| r.get(0),
                )?)).unwrap();
                assert_eq!(count, 0, "cleanup retention never undoes committed purge");
            }
            assert_eq!(worktrees::cleanup_outcome(&store, &f.id, &draft, disposition).unwrap().as_ref(), Some(cleanup));
            assert_eq!(binding.path.exists(), expected == "retained" && kind != "missing-close");
            let branch = format!("refs/heads/{}", binding.branch);
            let branch_head = worktree_git(&f.root, &["rev-parse", "--verify", &branch]);
            match kind {
                "dirty-close" => {
                    let ignored = binding.path.join("local-ignore");
                    assert_eq!(fs::read_to_string(&ignored).unwrap(), "keep");
                    fs::remove_file(ignored).unwrap();
                }
                "locked-close" => {
                    worktree_git(&f.root, &["worktree", "unlock", binding.path.to_str().unwrap()]);
                }
                _ => {}
            }
            if matches!(kind, "dirty-close" | "locked-close") {
                let unconfirmed = f.call(json!({
                    "action":"close_remove_worktree","taskId":draft,"confirmed":false,
                })).await;
                assert_eq!(unconfirmed["code"], "confirmation_required", "{kind}: {unconfirmed}");
                assert!(binding.path.is_dir());
                assert_eq!(
                    worktrees::cleanup_outcome(&store, &f.id, &draft, Cleanup::Close).unwrap().as_ref(),
                    Some(cleanup),
                );

                let retried = f.call(close.clone()).await;
                let removed = &retried["result"]["worktreeCleanup"];
                assert_eq!(retried["ok"], true, "{kind}: {retried}");
                assert_eq!(removed["status"], "removed", "{kind}: {retried}");
                assert_eq!(
                    worktrees::cleanup_outcome(&store, &f.id, &draft, Cleanup::Close).unwrap().as_ref(),
                    Some(removed),
                );
                assert!(!binding.path.exists());
                assert_eq!(f.call(close.clone()).await, retried, "{kind}: removal stays recorded");
            }
            assert_eq!(worktree_git(&f.root, &["rev-parse", "--verify", &branch]), branch_head);
        } else {
            assert_eq!(reply["code"], "worktree_not_owned", "{reply}");
            assert_eq!(worktrees::cleanup_outcome(&store, &f.id, &draft, Cleanup::Close).unwrap(), None);
        }
        assert_eq!(fs::read_to_string(f.root.join("tracked")).unwrap(), "committed");
        f.service.stop().await.unwrap();
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
        // Deliberately contradictory bindings now fail at shared task admission,
        // before the action-specific fences exercised by the historical oracle.
        // Keep every case's persisted-state and effect assertions below.
        // Historical fixtures called an overlay flag ownership; it cannot
        // authorize removal. Actual native provision is exercised separately.
        let legacy_unowned = matches!(case["name"].as_str(),
            Some("purge-owned-worktree" | "clear_old-owned-worktree"));
        let retained_path = f.base.join("worktree");
        let unbound_handoff = matches!(case["name"].as_str(), Some("resolution-plain" | "resolution-repeat"));
        let mut expected = match case["name"].as_str().unwrap() {
            "resolution-plain" | "resolution-repeat" => json!({
                "ok":false,"code":"handoff_required",
                "message":"Use the owning execution's handoff_merge action."
            }),
            "external-foreign-owner"
            | "external-missing-session"
            | "external-foreign-session"
            | "external-negative-revision"
            | "external-completed-blocker"
            | "external-start-unknown"
            | "external-launch-prepare-False"
            | "external-launch-prepare-True"
            | "external-launch-unknown-False"
            | "external-launch-unknown-True"
            | "purge-start-unknown"
            | "clear_old-start-unknown"
            | "wrong-owner"
            | "wrong-session"
            | "missing-session"
            | "unknown-start"
            | "negative-revision"
            | "completed-blocker"
            | "missing-current-execution" => json!({
                "ok": false,
                "code": "contradictory_evidence",
                "message": "The task's records disagree. Only the explicit close without verification is available."
            }),
            _ => case["expected"].clone(),
        };
        if legacy_unowned {
            expected["result"]["worktreeCleanup"] =
                json!({"status":"retained","path":retained_path,"reason":"worktree_not_owned"});
        }
        assert_eq!(reply, expected, "{} reply", case["name"]);
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
            let mut expected = if unbound_handoff { case["saved"].clone() } else { case["after"].clone() };
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
                legacy_unowned || case["worktreeExists"] == true,
                "{} worktree cleanup",
                case["name"]
            );
        }
        let mut expected_state = f.replace(&case["after"]);
        if unbound_handoff {
            assert_eq!(fs::read(f.path()).unwrap(), f.replace(&case["saved"]).to_string().as_bytes());
            expected_state = f.replace(&case["saved"]);
            for (key, default) in [
                ("tasks", json!({})), ("sprints", json!([])), ("drafts", json!({})),
                ("messages", json!([])), ("comments", json!({})),
            ] {
                if expected_state.get(key).is_none() { expected_state[key] = default; }
            }
        }
        if legacy_unowned {
            expected_state["tasks"]["t1"]["retainedWorktree"] = json!(retained_path);
            expected_state["tasks"]["t1"]["worktreeCleanup"] = expected["result"]["worktreeCleanup"].clone();
            assert_eq!(fs::read(retained_path.join("a.txt")).ok(), worktree_before);
        }
        assert_eq!(after, expected_state, "{} persisted JSON", case["name"]);
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
async fn read(f: &Fixture) -> Value {
    f.service
        .machine(
            serde_json::from_value(json!({"op":"product.read","projectId":f.id,"params":{}}))
                .unwrap(),
        )
        .await
        .unwrap()
}
#[tokio::test]
async fn a_task_the_owner_finished_through_the_runtime_reads_closed_and_archives_without_a_question(
) {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/task_lifecycle_python.json")).unwrap();
    // A bound, consistent CANCELLED runtime task with no saved disposition.
    let case = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "close-external-CANCELLED")
        .unwrap();
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
            db.execute(
                "UPDATE runtime_tasks SET updated_at='2026-01-02T03:04:05.000000+00:00' WHERE task_id='r1'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    fs::create_dir_all(f.path().parent().unwrap()).unwrap();
    fs::write(
        f.path(),
        json!({"tasks":{"t1":{"title":"Kept"}}}).to_string(),
    )
    .unwrap();
    // A run the runtime cancelled by itself is not the owner's close: Archive still refuses.
    let plain = read(&f).await;
    assert!(
        plain["result"]["tasks"]["t1"]["status"].is_null(),
        "{plain}"
    );
    let refused = f.call(json!({"action":"archive","taskId":"t1"})).await;
    assert_eq!(refused["code"], "not_done", "{refused}");
    // Finish chosen on the runtime's stop question.
    store
        .write(|db| {
            db.execute(
                "UPDATE runtime_tasks SET blocker='user_force_finished' WHERE task_id='r1'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    let closed = read(&f).await;
    let t = &closed["result"]["tasks"]["t1"];
    assert_eq!(t["status"], "done", "{closed}");
    assert_eq!(t["closedAs"], "as_is");
    assert_eq!(t["notVerifiedByCortex"], true);
    assert_eq!(t["doneAt"], 1_767_323_045_000u64);
    assert_eq!(t["title"], "Kept");
    let archived = f.call(json!({"action":"archive","taskId":"t1"})).await;
    assert_eq!(archived["ok"], true, "{archived}");
    let saved: Value = serde_json::from_slice(&fs::read(f.path()).unwrap()).unwrap();
    let t = &saved["tasks"]["t1"];
    assert!(t["archive"]["at"].is_u64(), "{saved}");
    assert_eq!(t["archive"]["previousStatus"], "done");
    assert_eq!(t["status"], "done");
    assert_eq!(t["closedAs"], "as_is");
    assert_eq!(t["archiveQuestion"], false);
    // A saved disposition always wins over the runtime's.
    fs::write(
        f.path(),
        json!({"tasks":{"t1":{"status":"done","closedAs":"cancel","doneAt":7}}}).to_string(),
    )
    .unwrap();
    let kept = read(&f).await;
    assert_eq!(
        kept["result"]["tasks"]["t1"]["closedAs"], "cancel",
        "{kept}"
    );
    assert_eq!(kept["result"]["tasks"]["t1"]["doneAt"], 7);
    f.service.stop().await.unwrap();
}

#[tokio::test]
async fn owner_close_with_conflicting_records_survives_service_restart_without_replay() {
    let mut f = Fixture::new().await;
    f.service.activate(&f.id).await.unwrap();
    let store = f.store();
    store.write(|db| {
        db.execute("INSERT INTO workspace_tasks(task_id,project_id,description,execution_id,created_at)
            VALUES ('t1',?,'Build task','e1','now')", [&f.id])?;
        let root = f.root.to_str().unwrap();
        db.execute("INSERT INTO tasks VALUES ('r1',?,'now')", [root])?;
        db.execute("INSERT INTO runtime_tasks(task_id,state,goal,created_at,updated_at)
            VALUES ('r1','CANCELLED','Goal','now','now')", [])?;
        db.execute("INSERT INTO task_owners VALUES ('r1','owner',1,'now')", [])?;
        db.execute("INSERT INTO acp_host_sessions VALUES ('s1',?,'r1','{}','{}','[]','now',NULL)", [root])?;
        db.execute("INSERT INTO workspace_executions VALUES ('e1','t1','r1','s1',2,1,'now')", [])?;
        cortex_core::lifecycle::event_tx(db, "r1", "TASK_CREATED", json!({"execution_host":"rust"}))?;
        Ok(())
    }).unwrap();
    let query = json!({"protocol":{"major":2,"mapping":2},"requestId":"board","projectId":f.id,
        "operation":"tasks.page","payload":{"target":f.id,"cursor":null,"limit":50}});
    let before = f.service.native_query(query.clone()).await.unwrap();
    let before_task = &before["data"]["value"]["items"][0];
    assert_eq!(before_task["phase"], "unknown");
    assert_eq!(before_task["warnings"], json!(["contradictory_evidence"]));
    let saved_before = fs::read(f.path()).ok();
    for params in [
        json!({"action":"close","taskId":"t1","choice":"as_is","confirmed":true}),
        json!({"action":"close","taskId":"t1","choice":"as_is","confirmed":true,"preflight":true}),
        json!({"action":"close","taskId":"t1","choice":"external","confirmed":true}),
        json!({"action":"target","taskId":"t1","branch":"other"}),
        json!({"action":"rename","taskId":"t1","title":"Other task"}),
        json!({"action":"generate_title","taskId":"t1"}),
        json!({"action":"draft_integration_text","taskId":"t1","method":"commit","targetBranch":"main"}),
        json!({"action":"archive","taskId":"t1"}),
    ] {
        let refusal = f.call(params.clone()).await;
        assert_eq!(
            refusal["code"], "contradictory_evidence",
            "{params}: {refusal}"
        );
    }
    assert_eq!(fs::read(f.path()).ok(), saved_before);
    let retry = f
        .service
        .control_execution(json!({"projectId":f.id,"executionId":"e1",
        "requestId":"retry-before-close","action":"retry"}))
        .await
        .unwrap_err();
    assert_eq!(retry.to_string(), "native_execution_binding_invalid");
    let still_open = f.service.native_query(query.clone()).await.unwrap();
    assert_eq!(still_open["data"]["value"]["items"][0], *before_task);
    let close = |confirmed, version, execution| {
        json!({"action":"close_unconfirmed","taskId":"t1",
        "confirmed":confirmed,"expectedVersion":version,"executionId":execution})
    };
    for (params, code) in [
        (close(false, "1", "e1"), "confirmation_required"),
        (close(true, "0", "e1"), "stale_version"),
        (close(true, "1", "e-other"), "stale_version"),
    ] {
        assert_eq!(f.call(params).await["code"], code);
    }
    let version = before_task["version"].as_str().unwrap();
    let expected_next = (version.parse::<u64>().unwrap() + 1).to_string();
    let request = close(true, version, "e1");
    let mut preflight = request.clone();
    preflight["preflight"] = json!(true);
    assert_eq!(f.call(preflight).await["code"], "invalid_request");
    let denied = cortex_core::workspace::control::close_unconfirmed(
        &store,
        &f.id,
        "not-the-owner",
        "t1",
        "e1",
        workspace_protocol::core::Version::new(version.parse().unwrap()).unwrap(),
    )
    .unwrap_err();
    assert_eq!(denied.code(), "unauthorized");
    let first = f.call(request.clone()).await;
    assert_eq!(first["ok"], true, "{first}");
    assert_eq!(f.call(request).await["ok"], true);
    let closed = f.service.native_query(query.clone()).await.unwrap();
    let closed_task = &closed["data"]["value"]["items"][0];
    assert_eq!(closed_task["version"], expected_next);
    assert_eq!(closed_task["executionState"], "unknown");
    assert_eq!(
        closed_task["warnings"],
        json!(["contradictory_evidence", "owner_closed_unconfirmed"])
    );
    store.read(|db| {
        let fact: (i64, i64, String) = db.query_row(
            "SELECT e.owner_epoch,o.epoch,r.state FROM workspace_executions e
             JOIN task_owners o ON o.task_id=e.runtime_task_id JOIN runtime_tasks r ON r.task_id=o.task_id",
            [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
        assert_eq!(fact, (2, 1, "CANCELLED".into()));
        let counts: (i64, i64, i64) = db.query_row(
            "SELECT (SELECT count(*) FROM runtime_events), (SELECT count(*) FROM runtime_requests),
             (SELECT count(*) FROM workspace_requests WHERE operation='tasks.close_unconfirmed')",
            [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
        assert_eq!(counts, (1, 0, 1));
        Ok(())
    }).unwrap();
    drop(store);
    f.service.stop().await.unwrap();

    let restarted = Service::bind(Config {
        runtime_dir: f.base.join("restart"),
        state_dir: f.state.clone(),
        allowed_root: f.base.clone(),
        launch: LaunchSpec::bare(),
        request_timeout: Duration::from_secs(10),
        shutdown_timeout: Duration::from_millis(100),
        event_capacity: 16,
    })
    .await
    .unwrap();
    drop(std::mem::replace(&mut f.service, restarted.clone()));
    restarted.activate(&f.id).await.unwrap();
    let after = restarted.native_query(query).await.unwrap();
    assert_eq!(after["data"]["value"]["items"][0], *closed_task);
    let rejected = restarted
        .control_execution(json!({"projectId":f.id,"executionId":"e1",
        "requestId":"retry","action":"retry"}))
        .await
        .unwrap_err();
    assert_eq!(rejected.to_string(), "native_execution_binding_invalid");
    let archive = restarted
        .machine(
            serde_json::from_value(json!({"op":"product.task",
        "projectId":f.id,"params":{"action":"archive","taskId":"t1"}}))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(archive["code"], "contradictory_evidence");
    restarted.stop().await.unwrap();
}
