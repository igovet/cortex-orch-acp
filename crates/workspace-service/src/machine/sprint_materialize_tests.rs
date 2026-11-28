//! Create tasks from a plan: all or nothing, once per plan version.
use super::*;
use cortex_core::Store;
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;

struct Env {
    base: PathBuf,
    root: PathBuf,
    state: PathBuf,
}
impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}
/// A bound project store and a saved sprint whose plan v1 has `n` tasks; the tenth
/// waits for the ninetieth when there are that many, every other one for the one before.
fn env(name: &str, n: usize) -> Env {
    let base = std::env::temp_dir().join(format!("cw-materialize-{name}-{}", settings::unique()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .recursive(true)
        .create(&base)
        .unwrap();
    let base = base.canonicalize().unwrap();
    let (root, state) = (base.join("repo"), base.join("state"));
    for dir in [&root, &state] {
        std::fs::DirBuilder::new().mode(0o700).create(dir).unwrap();
    }
    let store = Store::open(context::key(&root).unwrap(), OpenMode::Initialize).unwrap();
    let bound = store.key().root().to_string_lossy().into_owned();
    store
        .write(move |db| {
            db.execute(
                "INSERT INTO workspace_projects VALUES ('p',?,?,'now',1)",
                (&bound, crate::actor()),
            )?;
            Ok(())
        })
        .unwrap();
    let items: Vec<Value> = (1..=n)
        .map(|i| {
            let deps = if i == 10 && n >= 90 {
                json!(["t90"])
            } else if i > 1 && i != 11 {
                json!([format!("t{}", i - 1)])
            } else {
                json!([])
            };
            json!({"key":format!("t{i}"),"title":format!("Item {i}"),"description":format!("Do item {i}"),"dependsOn":deps})
        })
        .collect();
    let product = json!({"tasks":{},"messages":[],"comments":{},"drafts":{},"sprints":[{
        "id":"sp","projectId":"p","description":"Sprint","title":"Sprint","runMode":"automatic",
        "targetBranch":"release","checkout":{"kind":"worktree"},
        "settings":{"model":"connection:model","effort":"high","mode":"auto","language":"en"},
        "planning":"ready","planningTaskId":"planner","conversation":[],"plan":{"version":1,"items":items},
        "history":[],"materializedVersion":null,"taskIds":[],"queueState":"idle","pauseReason":null}]});
    settings::write_json(&product_path(&state, "p").unwrap(), &product).unwrap();
    Env { base, root, state }
}
impl Env {
    fn run(&self, version: u64) -> Result<Value> {
        materialize(
            &self.root,
            "p",
            &self.state,
            &json!({"action":"materialize","sprintId":"sp","planVersion":version}),
        )
    }
    fn drafts(&self) -> Vec<(String, String, Value)> {
        let store = Store::open(context::key(&self.root).unwrap(), OpenMode::LiveReadOnly).unwrap();
        store
            .read(|db| {
                let mut q = db.prepare("SELECT t.task_id,t.description,o.options_json FROM workspace_tasks t
                    JOIN workspace_task_options o USING(task_id) WHERE t.project_id='p' ORDER BY t.sequence")?;
                let rows = q
                    .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, String>(2)?)))?
                    .map(|r| r.map(|(a, b, c)| (a, b, serde_json::from_str(&c).unwrap())))
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .unwrap()
    }
    fn saved(&self) -> Value {
        product(&self.state, "p").unwrap()
    }
    fn bytes(&self) -> Vec<u8> {
        std::fs::read(product_path(&self.state, "p").unwrap()).unwrap()
    }
}
fn untouched(env: &Env, before: &[u8]) {
    assert!(env.drafts().is_empty(), "no draft remains");
    assert_eq!(env.bytes(), before, "the saved sprint is exactly as it was");
    let sprint = &env.saved()["sprints"][0];
    assert_eq!(
        (
            sprint["planning"].as_str(),
            sprint["taskIds"].as_array().map(Vec::len)
        ),
        (Some("ready"), Some(0))
    );
    assert!(sprint["materializedVersion"].is_null());
}

#[test]
fn a_120_task_plan_becomes_its_tasks_and_the_sprint_created_in_one_transition() {
    let env = env("large", 120);
    let reply = env.run(1).unwrap();
    assert_eq!(reply["created"], 120);
    let ids: Vec<String> = reply["taskIds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(ids.len(), 120);
    let id = |key: &str| task_id("p", "sp", 1, key);
    // Dependency order: t90 is created before t10, which waits for it.
    let at = |key: &str| ids.iter().position(|i| *i == id(key)).unwrap();
    assert!(at("t90") < at("t10"));
    let saved = env.saved();
    let sprint = &saved["sprints"][0];
    assert_eq!(sprint["planning"], "created");
    assert_eq!(sprint["materializedVersion"], 1);
    assert_eq!(sprint["taskIds"], json!(ids));
    let drafts = env.drafts();
    assert_eq!(drafts.len(), 120);
    let route = json!({"model":"connection:model","effort":"high","mode":"auto","language":"en"});
    for (task, _, options) in &drafts {
        assert_eq!(
            options,
            &json!({"checkout":{"kind":"worktree"},"settings":route}),
            "{task}"
        );
        assert!(ids.contains(task));
    }
    let ten = &saved["tasks"][id("t10")];
    assert_eq!(ten["dependencies"], json!([id("t90")]));
    assert_eq!(
        (
            ten["sprintId"].as_str(),
            ten["title"].as_str(),
            ten["titleState"].as_str()
        ),
        (Some("sp"), Some("Item 10"), Some("generated"))
    );
    assert_eq!(
        (
            ten["targetBranch"].as_str(),
            ten["targetInherited"].as_bool()
        ),
        (Some("release"), Some(true))
    );
    assert_eq!(saved["tasks"][id("t1")]["dependencies"], json!([]));
    assert_eq!(
        drafts.iter().find(|d| d.0 == id("t10")).unwrap().1,
        "Do item 10"
    );
}

/// Rewrite the saved sprint's settings and plan items in place.
fn reshape(env: &Env, settings: Value, complexities: &[Option<&str>]) {
    let mut product = env.saved();
    let sprint = &mut product["sprints"][0];
    sprint["settings"] = settings;
    for (item, complexity) in sprint["plan"]["items"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .zip(complexities)
    {
        if let Some(c) = complexity {
            item["complexity"] = json!(c);
        }
    }
    settings::write_json(&product_path(&env.state, "p").unwrap(), &product).unwrap();
}
fn routed(env: &Env) -> Result<Value> {
    materialize_routed(
        &env.root,
        "p",
        &env.state,
        &json!({"action":"materialize","sprintId":"sp","planVersion":1}),
        &|complexity| {
            let (route, tier, effort) = match complexity {
                "high" => ("xr_opus", "flagship", "high"),
                "low" => ("xr_light", "light", "medium"),
                _ => ("xr_sonnet", "standard", "high"),
            };
            Some(json!({"routeId":route,"engine":"claude-agent-acp","model":route,
                "tier":tier,"effort":effort,"complexity":complexity,"usedPercent":null}))
        },
    )
}

#[test]
fn without_a_sprint_model_each_task_gets_the_route_of_its_complexity() {
    let env = env("routed", 3);
    reshape(
        &env,
        json!({"model":"","effort":"","mode":"auto","language":"en"}),
        &[Some("high"), None, Some("low")],
    );
    routed(&env).unwrap();
    let saved = env.saved();
    let drafts = env.drafts();
    let id = |key: &str| task_id("p", "sp", 1, key);
    for (key, route, tier, effort, complexity) in [
        ("t1", "xr_opus", "flagship", "high", "high"),
        ("t2", "xr_sonnet", "standard", "high", "normal"),
        ("t3", "xr_light", "light", "medium", "low"),
    ] {
        let (_, _, options) = drafts.iter().find(|d| d.0 == id(key)).unwrap();
        assert_eq!(
            options["settings"],
            json!({"model":route,"effort":effort,"mode":"auto","language":"en"}),
            "{key}"
        );
        let record = &saved["tasks"][id(key)]["sprintRoute"];
        assert_eq!(
            (record["routeId"].as_str(), record["tier"].as_str(), record["effort"].as_str()),
            (Some(route), Some(tier), Some(effort))
        );
        assert_eq!(record["complexity"], complexity);
    }
}

#[test]
fn an_owner_chosen_sprint_model_wins_over_the_automatic_route() {
    let env = env("explicit", 2);
    reshape(
        &env,
        json!({"model":"connection:model","effort":"high","mode":"auto","language":"en"}),
        &[Some("high"), Some("low")],
    );
    routed(&env).unwrap();
    let saved = env.saved();
    for (task, _, options) in env.drafts() {
        assert_eq!(options["settings"]["model"], "connection:model");
        assert_eq!(options["settings"]["effort"], "high");
        assert!(saved["tasks"][&task].get("sprintRoute").is_none());
    }
}

/// The new-sprint form always saves the planning model; only `taskRoute: "sprint"`
/// (the owner picked it) makes it every task's route.
#[test]
fn the_planning_model_is_not_a_sprint_model_unless_the_owner_picked_it() {
    for (name, task_route, sprint_model) in [
        ("auto", "automatic", false),
        ("picked", "sprint", true),
    ] {
        let env = env(name, 2);
        reshape(
            &env,
            json!({"model":"connection:model","effort":"high","mode":"auto","language":"en"}),
            &[Some("low"), Some("high")],
        );
        let mut product = env.saved();
        product["sprints"][0]["taskRoute"] = json!(task_route);
        settings::write_json(&product_path(&env.state, "p").unwrap(), &product).unwrap();
        routed(&env).unwrap();
        let saved = env.saved();
        let id = |key: &str| task_id("p", "sp", 1, key);
        for (key, route, effort) in [("t1", "xr_light", "medium"), ("t2", "xr_opus", "high")] {
            let (_, _, options) = env.drafts().into_iter().find(|d| d.0 == id(key)).unwrap();
            let record = &saved["tasks"][id(key)];
            if sprint_model {
                assert_eq!(options["settings"]["model"], "connection:model", "{key}");
                assert!(record.get("sprintRoute").is_none(), "{key}");
            } else {
                assert_eq!(
                    options["settings"],
                    json!({"model":route,"effort":effort,"mode":"auto","language":"en"}),
                    "{key}"
                );
                assert_eq!(record["sprintRoute"]["routeId"], route, "{key}");
            }
        }
    }
}

#[test]
fn no_profile_route_leaves_the_task_on_the_default_coordinator() {
    let env = env("unrouted", 2);
    reshape(&env, json!({"model":"","effort":"","mode":"auto","language":"en"}), &[]);
    materialize_routed(
        &env.root,
        "p",
        &env.state,
        &json!({"action":"materialize","sprintId":"sp","planVersion":1}),
        &|_| None,
    )
    .unwrap();
    for (_, _, options) in env.drafts() {
        assert!(options.get("settings").is_none());
    }
}

#[test]
fn an_unknown_complexity_refuses_the_plan() {
    let env = env("badcomplexity", 2);
    reshape(&env, json!({"model":"","mode":"auto","language":"en"}), &[Some("huge")]);
    let before = env.bytes();
    assert_eq!(routed(&env).unwrap_err().code, "invalid_plan");
    untouched(&env, &before);
}

#[test]
fn a_failure_part_way_leaves_no_task_and_the_plan_ready_and_a_retry_completes_it() {
    let env = env("fail", 120);
    let before = env.bytes();
    // A real refusal at the 60th draft: its identity is held by another project.
    let store = Store::open(context::key(&env.root).unwrap(), OpenMode::RuntimeValidate).unwrap();
    let held = task_id("p", "sp", 1, "t60");
    store
        .write(move |db| {
            db.execute("INSERT INTO workspace_projects VALUES ('q','/elsewhere','owner','now',1)", [])?;
            db.execute("INSERT INTO workspace_tasks(task_id,project_id,description,created_at) VALUES (?,'q','other','now')", [&held])?;
            Ok(())
        })
        .unwrap();
    assert_eq!(env.run(1).unwrap_err().code, "payload_conflict");
    untouched(&env, &before);
    store
        .write(|db| {
            db.execute("DELETE FROM workspace_tasks WHERE project_id='q'", [])?;
            Ok(())
        })
        .unwrap();
    // Every draft written, then a failure before the sprint record; and a failure after it.
    for stage in [1, 2] {
        FAIL.with(|f| f.set(stage));
        let error = env.run(1).unwrap_err();
        FAIL.with(|f| f.set(0));
        assert_eq!(error.code, "injected_failure", "stage {stage}");
        untouched(&env, &before);
    }
    let reply = env.run(1).unwrap();
    assert_eq!(
        (
            reply["created"].as_u64(),
            reply["taskIds"].as_array().map(Vec::len)
        ),
        (Some(120), Some(120))
    );
    assert_eq!(env.drafts().len(), 120);
}

#[test]
fn an_existing_same_project_draft_cannot_supply_a_plan_item() {
    let env = env("same-project-conflict", 2);
    let before = env.bytes();
    let id = task_id("p", "sp", 1, "t2");
    let store = Store::open(context::key(&env.root).unwrap(), OpenMode::RuntimeValidate).unwrap();
    store.write(|db| {
        db.execute("INSERT INTO workspace_tasks(task_id,project_id,description,created_at) VALUES (?,'p','Other work','now')", [&id])?;
        Ok(())
    }).unwrap();
    assert_eq!(env.run(1).unwrap_err().code, "payload_conflict");
    assert_eq!(env.bytes(), before);
    let rows = store.read(|db| {
        let mut query = db.prepare("SELECT task_id,description FROM workspace_tasks")?;
        let rows = query.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }).unwrap();
    assert_eq!(rows, [(id, "Other work".into())]);
}

#[test]
fn creating_again_returns_the_same_tasks_and_never_a_second_set() {
    let env = env("again", 12);
    let first = env.run(1).unwrap();
    let again = env.run(1).unwrap();
    assert_eq!(again["created"], 0);
    assert_eq!(again["taskIds"], first["taskIds"]);
    assert_eq!(env.drafts().len(), 12);
    assert_eq!(env.saved()["sprints"][0]["taskIds"], first["taskIds"]);
    let before = env.saved();
    let changed = super::super::product::sprint(
        &env.state,
        "p",
        &json!({"action":"patch","sprintId":"sp","patch":{"materializedVersion":null,"taskIds":[]}}),
        Some(&env.root),
    );
    assert_eq!(changed.unwrap_err().code, "sprint_started");
    assert_eq!(env.saved(), before);
    // Another version of an already created sprint is refused, not created beside it.
    assert_eq!(env.run(2).unwrap_err().code, "sprint_started");
    assert_eq!(env.drafts().len(), 12);
    let mut product = env.saved();
    product["sprints"][0]["taskIds"].as_array_mut().unwrap().push(json!("later-draft"));
    settings::write_json(&product_path(&env.state, "p").unwrap(), &product).unwrap();
    assert_eq!(env.run(1).unwrap()["taskIds"], first["taskIds"]);
}

#[test]
fn a_saved_materialization_with_a_missing_draft_is_not_acknowledged() {
    let env = env("unconfirmed", 2);
    // Exercise the persisted state left if the process exits after publishing
    // the product JSON and SQLite then rolls back the draft transaction.
    let mut product = env.saved();
    let ids: Vec<_> = ["t1", "t2"].map(|key| task_id("p", "sp", 1, key)).into();
    product["sprints"][0]["planning"] = json!("created");
    product["sprints"][0]["materializedVersion"] = json!(1);
    product["sprints"][0]["taskIds"] = json!(ids);
    for id in ids {
        product["tasks"][id] = json!({"sprintId":"sp","planning":false});
    }
    settings::write_json(&product_path(&env.state, "p").unwrap(), &product).unwrap();
    let before = env.bytes();
    assert_eq!(env.run(1).unwrap_err().code, "machine_outcome_unknown");
    assert!(env.drafts().is_empty());
    assert_eq!(env.bytes(), before);
}

#[test]
fn materialization_crash_child() {
    let Some(base) = std::env::var_os("CORTEX_TEST_MATERIALIZE_CRASH_DIR") else {
        return;
    };
    let base = PathBuf::from(base);
    let checkpoint = std::env::var("CORTEX_TEST_MATERIALIZE_CRASH_CHECKPOINT").unwrap().parse().unwrap();
    FAIL.with(|fail| fail.set(checkpoint));
    let result = materialize(
        &base.join("repo"),
        "p",
        &base.join("state"),
        &json!({"action":"materialize","sprintId":"sp","planVersion":1}),
    );
    panic!("expected process exit before materialize returned: {result:?}");
}

#[test]
fn materialization_recovers_process_exit_on_either_side_of_commit() {
    for (checkpoint, committed) in [(3, false), (4, true)] {
        let env = env("crash", 2);
        let before = env.saved();
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "machine::sprint_materialize::tests::materialization_crash_child"])
            .env("CORTEX_TEST_MATERIALIZE_CRASH_DIR", &env.base)
            .env("CORTEX_TEST_MATERIALIZE_CRASH_CHECKPOINT", checkpoint.to_string())
            .output()
            .unwrap();
        assert_eq!(child.status.code(), Some(80 + checkpoint), "{:?}", child);
        let ids: Vec<_> = ["t1", "t2"].map(|key| task_id("p", "sp", 1, key)).into();
        if committed {
            assert_eq!(env.saved()["sprints"][0]["taskIds"], json!(ids));
            assert_eq!(env.drafts().len(), 2);
        } else {
            assert_eq!(env.saved(), before);
            assert!(env.drafts().is_empty());
        }
        let retried = env.run(1).unwrap();
        assert_eq!(retried["created"], if committed { 0 } else { 2 });
        assert_eq!(retried["taskIds"], json!(ids));
        assert_eq!(env.drafts().into_iter().map(|d| d.0).collect::<Vec<_>>(), ids);
    }
}

#[test]
fn a_stale_or_unready_plan_is_refused_before_any_effect() {
    let env = env("stale", 3);
    let before = env.bytes();
    assert_eq!(env.run(2).unwrap_err().code, "stale_plan");
    untouched(&env, &before);
    for (planning, code) in [
        ("question", "plan_not_ready"),
        ("planning", "plan_not_ready"),
        ("cancelled", "plan_not_ready"),
    ] {
        let mut saved = env.saved();
        saved["sprints"][0]["planning"] = json!(planning);
        settings::write_json(&product_path(&env.state, "p").unwrap(), &saved).unwrap();
        assert_eq!(env.run(1).unwrap_err().code, code, "{planning}");
        assert!(env.drafts().is_empty());
    }
    let mut saved = env.saved();
    saved["sprints"][0]["planning"] = json!("ready");
    saved["sprints"][0]["plan"]["items"][2]["dependsOn"] = json!(["t3"]);
    settings::write_json(&product_path(&env.state, "p").unwrap(), &saved).unwrap();
    assert_eq!(env.run(1).unwrap_err().code, "invalid_plan");
    saved["sprints"][0]["plan"]["items"][0]["dependsOn"] = json!(["t3"]);
    saved["sprints"][0]["plan"]["items"][2]["dependsOn"] = json!(["t2"]);
    settings::write_json(&product_path(&env.state, "p").unwrap(), &saved).unwrap();
    assert_eq!(env.run(1).unwrap_err().code, "invalid_plan", "a cycle");
    assert!(env.drafts().is_empty());
}

#[test]
fn an_ambiguous_or_foreign_sprint_binding_cannot_publish_drafts() {
    let env = env("ambiguous", 2);
    let path = product_path(&env.state, "p").unwrap();
    let saved = env.saved();
    let mut foreign = saved.clone();
    foreign["sprints"][0]["projectId"] = json!("another-project");
    settings::write_json(&path, &foreign).unwrap();
    assert_eq!(env.run(1).unwrap_err().code, "sprint_membership_invalid");
    let mut duplicate = saved.clone();
    duplicate["sprints"].as_array_mut().unwrap().push(saved["sprints"][0].clone());
    settings::write_json(&path, &duplicate).unwrap();
    assert_eq!(env.run(1).unwrap_err().code, "sprint_membership_invalid");
    assert!(env.drafts().is_empty());
}

#[test]
fn two_concurrent_creates_make_one_set() {
    let env = std::sync::Arc::new(env("double", 120));
    let replies: Vec<Value> = (0..2)
        .map(|_| {
            let env = env.clone();
            std::thread::spawn(move || env.run(1).unwrap())
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|t| t.join().unwrap())
        .collect();
    assert_eq!(replies[0]["taskIds"], replies[1]["taskIds"]);
    let mut created: Vec<u64> = replies
        .iter()
        .map(|r| r["created"].as_u64().unwrap())
        .collect();
    created.sort();
    assert_eq!(created, [0, 120]);
    assert_eq!(env.drafts().len(), 120);
}

#[test]
fn tasks_an_earlier_partial_create_left_are_reported_never_adopted() {
    let env = env("strays", 2);
    let mut saved = env.saved();
    saved["tasks"]["wt_old"] = json!({"sprintId":"sp","dependencies":[],"planning":false});
    saved["tasks"]["planner"] = json!({"sprintId":"sp","planning":true});
    settings::write_json(&product_path(&env.state, "p").unwrap(), &saved).unwrap();
    let reply = env.run(1).unwrap();
    assert_eq!(reply["strayTaskIds"], json!(["wt_old"]));
    assert!(!reply["taskIds"]
        .as_array()
        .unwrap()
        .contains(&json!("wt_old")));
    assert_eq!(env.run(1).unwrap()["strayTaskIds"], json!(["wt_old"]));
}

#[test]
fn an_unbound_project_store_creates_nothing() {
    let env = env("unbound", 2);
    let before = env.bytes();
    let store = Store::open(context::key(&env.root).unwrap(), OpenMode::RuntimeValidate).unwrap();
    store
        .write(|db| {
            db.execute(
                "UPDATE workspace_projects SET activated_by='someone-else'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    assert_eq!(env.run(1).unwrap_err().code, "project_store_unavailable");
    untouched(&env, &before);
}

/// The owner action reaches this through the machine helper's `product.sprint` route.
#[tokio::test]
async fn the_product_sprint_materialize_action_is_this_one_operation() {
    let env = env("route", 3);
    let config = crate::Config {
        runtime_dir: env.base.join("runtime"),
        state_dir: env.state.clone(),
        allowed_root: env.base.clone(),
        launch: crate::LaunchSpec::bare(),
        request_timeout: std::time::Duration::from_secs(5),
        shutdown_timeout: std::time::Duration::from_millis(100),
        event_capacity: 4,
    };
    let params = json!({"action":"materialize","sprintId":"sp","planVersion":1});
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let reply = super::super::native::run(
        "product.sprint",
        Some(&env.root),
        "p",
        &env.state,
        &params,
        &json!([]),
        &config,
        None,
        deadline,
    )
    .await
    .unwrap();
    assert_eq!(
        (
            reply["created"].as_u64(),
            reply["taskIds"].as_array().map(Vec::len)
        ),
        (Some(3), Some(3))
    );
    assert_eq!(env.saved()["sprints"][0]["planning"], "created");
}

fn live(env: &Env, request: Value) -> Result<Value> {
    super::super::sprint_live::owner_action(&env.root, &env.state, "p", &request, &|_| None)
}

fn graph(env: &Env) -> Value {
    live(env, json!({"action":"live_graph","sprintId":"sp"})).unwrap()
}

fn edge(env: &Env, revision: &Value, task: &str, dep: &str, present: bool) -> Result<Value> {
    live(env, json!({"action":"set_dependency","sprintId":"sp","expectedGraphRevision":revision,
        "dependentTaskId":task,"predecessorTaskId":dep,"present":present}))
}

#[tokio::test]
async fn sprint_live_graph_keeps_the_approved_plan_when_ten_later_drafts_are_linked() {
    let env = env("live-later", 2);
    let initial = env.run(1).unwrap()["taskIds"].clone();
    let approved = env.saved()["sprints"][0].clone();
    let store = Store::open(context::key(&env.root).unwrap(), OpenMode::RuntimeValidate).unwrap();
    let config = crate::Config {
        runtime_dir: env.base.join("runtime"), state_dir: env.state.clone(),
        allowed_root: env.base.clone(), launch: crate::LaunchSpec::bare(),
        request_timeout: std::time::Duration::from_secs(5),
        shutdown_timeout: std::time::Duration::from_millis(100), event_capacity: 4,
    };
    let mut git = super::super::git::Git::for_service(&config);
    let mut added = Vec::new();
    for i in 0..10 {
        let request = json!({"protocol":{"major":1,"minor":8},"requestId":format!("later-{i}"),
            "projectId":"p","actorId":crate::actor(),"operation":"tasks.create","target":"p",
            "expectedVersion":"0","ownerEpoch":null,"payload":{"description":"Later draft","checkout":{"kind":"worktree"}}});
        let draft = core_workspace::create_draft(&store, "p", &crate::actor(), &request).unwrap().receipt;
        let id = draft["target"].as_str().unwrap().to_owned();
        super::super::task_actions::run(&env.root, "p", &env.state, &json!({"action":"sprint",
            "sprintId":"sp","taskId":id,"dependencies":[]}), &mut git).await.unwrap();
        added.push(id);
    }
    let first = graph(&env);
    assert_eq!(first["members"].as_array().unwrap().len(), 12);
    assert_eq!(first["members"][0]["taskId"], initial[0]);
    assert_eq!(first["members"][0]["approvedPlanKey"], "t1");
    assert_eq!(first["members"][11]["taskId"], added[9]);
    assert!(first["members"][11]["approvedPlanKey"].is_null());
    assert_eq!(first["members"][11]["dependencies"], json!([]));
    assert_eq!(first["priority"][11], added[9]);

    let rev = &first["graphRevision"];
    assert_eq!(edge(&env, rev, &added[0], initial[1].as_str().unwrap(), true).unwrap()["changed"], true);
    assert_eq!(edge(&env, rev, &added[1], &added[0], true).unwrap_err().code, "stale_graph_revision");
    let second = graph(&env);
    assert_eq!(edge(&env, &second["graphRevision"], &added[1], &added[0], true).unwrap()["changed"], true);
    let third = graph(&env);
    let rev = &third["graphRevision"];
    let bytes = env.bytes();
    assert_eq!(edge(&env, rev, &added[1], &added[0], true).unwrap()["changed"], false);
    assert_eq!(edge(&env, rev, &added[0], &added[1], true).unwrap_err().code, "dependency_cycle");
    assert_eq!(edge(&env, rev, &added[0], &added[0], true).unwrap_err().code, "dependency_self");
    assert_eq!(edge(&env, rev, &added[0], "absent", true).unwrap_err().code, "dependency_missing");
    assert_eq!(env.bytes(), bytes, "no-op and refusals do not rewrite the product");
    assert_eq!(third["members"][2]["dependencies"], json!([initial[1]]));
    assert_eq!(third["members"][3]["dependencies"], json!([added[0]]));
    let saved = env.saved();
    for key in ["plan", "history", "materializedVersion"] {
        assert_eq!(saved["sprints"][0][key], approved[key]);
    }
    assert_eq!(env.drafts().len(), 12);
}

#[test]
fn sprint_live_graph_reports_missing_and_accepts_only_confirmed_predecessors() {
    let env = env("live-refusals", 3);
    let ids: Vec<String> = serde_json::from_value(env.run(1).unwrap()["taskIds"].clone()).unwrap();
    let mut saved = env.saved();
    let path = product_path(&env.state, "p").unwrap();
    saved["tasks"]["foreign"] = json!({"sprintId":"another"});
    saved["tasks"][&ids[0]]["closedAs"] = json!("cancel");
    saved["tasks"][&ids[0]]["status"] = json!("done");
    settings::write_json(&path, &saved).unwrap();
    let view = graph(&env);
    let rev = &view["graphRevision"];
    assert_eq!(edge(&env, rev, &ids[2], "foreign", true).unwrap_err().code, "dependency_cross_sprint");
    assert_eq!(edge(&env, rev, &ids[2], &ids[0], true).unwrap_err().code, "predecessor_unavailable");
    // A legacy duplicate stays readable; removing that link removes every copy.
    saved["tasks"][&ids[1]]["dependencies"] = json!([ids[0], ids[0]]);
    settings::write_json(&path, &saved).unwrap();
    let view = graph(&env);
    assert_eq!(view["members"][1]["dependencies"], json!([ids[0], ids[0]]));
    assert_eq!(edge(&env, &view["graphRevision"], &ids[2], &ids[1], false).unwrap_err().code, "dependency_duplicate");
    assert_eq!(edge(&env, &view["graphRevision"], &ids[1], &ids[0], false).unwrap()["changed"], true);
    let mut saved = env.saved();
    saved["tasks"][&ids[0]]["closedAs"] = json!("as_is");
    saved["tasks"][&ids[0]]["archive"] = json!({"previousStatus":"done"});
    settings::write_json(&path, &saved).unwrap();
    let view = graph(&env);
    assert_eq!(view["members"][0]["canBePredecessor"], true);
    assert_eq!(edge(&env, &view["graphRevision"], &ids[2], &ids[0], true).unwrap()["changed"], true);

    let mut saved = env.saved();
    saved["sprints"][0]["taskIds"].as_array_mut().unwrap().push(json!("missing"));
    settings::write_json(&path, &saved).unwrap();
    let view = graph(&env);
    assert_eq!(view["complete"], false);
    assert_eq!(view["members"][3]["readStatus"], "missing");
    assert!(view["members"][3]["dependencies"].is_null());
    assert_eq!(view["members"][2]["editable"], false);
    // Even a valid revision cannot edit an archived sprint.
    saved["sprints"][0]["archivedAt"] = json!(1);
    settings::write_json(&path, &saved).unwrap();
    assert_eq!(edge(&env, &graph(&env)["graphRevision"], &ids[2], &ids[1], false).unwrap_err().code, "sprint_read_only");
}

#[test]
fn sprint_live_order_uses_the_claim_sequence_and_labels_unknown_start_order() {
    let env = env("live-claim-order", 4);
    let mut saved = env.saved();
    saved["sprints"][0]["checkout"]["kind"] = json!("main");
    saved["sprints"][0]["runMode"] = json!("manual");
    let path = product_path(&env.state, "p").unwrap();
    settings::write_json(&path, &saved).unwrap();
    let ids: Vec<String> = serde_json::from_value(env.run(1).unwrap()["taskIds"].clone()).unwrap();
    let store = Store::open(context::key(&env.root).unwrap(), OpenMode::RuntimeValidate).unwrap();
    let mut runtimes = Vec::new();
    for i in [2, 1, 0] {
        let request = json!({"protocol":{"major":1,"minor":8},"operation":"tasks.start",
            "requestId":format!("manual-{i}"),"projectId":"p","actorId":crate::actor(),
            "target":ids[i],"expectedVersion":"1","ownerEpoch":null,"payload":{}});
        let result = core_workspace::start_native_input_with_policy(&store, "p", &crate::actor(), &request,
            None, None, None).unwrap();
        assert_eq!(result["fresh"], true);
        runtimes.push(result["task"].as_str().unwrap().to_owned());
    }
    store.write(|db| {
        // Model an older execution without retained native claim-order evidence.
        db.execute("DELETE FROM runtime_events WHERE task_id=? AND type='NATIVE_START_ADMITTED'", [&runtimes[0]])?;
        Ok(())
    }).unwrap();
    let view = graph(&env);
    let working = view["order"]["working"].as_array().unwrap();
    assert_eq!(working.iter().map(|w| w["taskId"].as_str().unwrap()).collect::<Vec<_>>(),
        vec![ids[1].as_str(), ids[0].as_str(), ids[2].as_str()]);
    assert!(working[0]["startOrder"].as_str().unwrap().parse::<i64>().unwrap()
        < working[1]["startOrder"].as_str().unwrap().parse::<i64>().unwrap());
    assert!(working[2]["startOrder"].is_null());
    assert!(view["order"]["notStarted"].as_array().unwrap().iter().all(|r| r["next"] == false));

    let mut saved = env.saved();
    saved["sprints"][0]["runMode"] = json!("hybrid");
    saved["sprints"][0]["queueState"] = json!("paused");
    saved["sprints"][0]["pauseReason"] = json!("Paused by you");
    settings::write_json(&path, &saved).unwrap();
    let paused = graph(&env);
    assert_eq!(paused["order"]["notStarted"][0]["reason"], "queue_paused");
    assert!(paused["order"]["notStarted"][0]["rank"].is_null());
    assert_eq!(edge(&env, &paused["graphRevision"], &ids[3], &ids[2], false).unwrap()["changed"], true);
    let after = graph(&env);
    assert_eq!(after["order"]["queueState"], "paused");
    assert_eq!(after["order"]["notStarted"][0]["next"], false);
}

#[test]
fn sprint_live_graph_failed_atomic_save_leaves_the_previous_graph() {
    let env = env("live-write-limit", 2);
    let ids: Vec<String> = serde_json::from_value(env.run(1).unwrap()["taskIds"].clone()).unwrap();
    let mut saved = env.saved();
    saved["tasks"][&ids[1]]["dependencies"] = json!([]);
    saved["fixturePadding"] = json!("");
    let allowance = FRAME_BUDGET - serde_json::to_vec(&saved).unwrap().len();
    saved["fixturePadding"] = json!("x".repeat(allowance));
    let path = product_path(&env.state, "p").unwrap();
    settings::write_json(&path, &saved).unwrap();
    let view = graph(&env);
    let before = env.bytes();
    assert_eq!(edge(&env, &view["graphRevision"], &ids[1], &ids[0], true).unwrap_err().code, "graph_save_unconfirmed");
    assert_eq!(env.bytes(), before);
    assert_eq!(graph(&env), view);
}

fn b64(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// The description's own file (a document) and a planning-chat message's file (an
/// image) both become attachments of every created task.
fn with_files(env: &Env) -> Vec<Value> {
    let png: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0x0d, b'I', b'H', b'D', b'R', 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1f, 0x15, 0xc4, 0x89];
    let files = vec![
        json!({"name":"Site Fixes v1.md","mimeType":"text/markdown","data":b64(b"# Fixes\n")}),
        json!({"name":"shot.png","mimeType":"image/png","data":b64(png)}),
    ];
    let mut product = env.saved();
    product["sprints"][0]["attachments"] = json!([files[0]]);
    product["sprints"][0]["conversation"] =
        json!([{"kind":"message","text":"see","attachments":[files[1]]}]);
    settings::write_json(&product_path(&env.state, "p").unwrap(), &product).unwrap();
    files
}

#[test]
fn every_created_task_gets_the_sprints_description_and_chat_files() {
    let env = env("files", 2);
    let files = with_files(&env);
    env.run(1).unwrap();
    for (_, _, options) in env.drafts() {
        assert_eq!(options["attachments"], json!(files));
    }
}

#[test]
fn a_draft_created_without_the_files_gets_them_and_a_running_task_is_told_once() {
    let env = env("backfill", 2);
    env.run(1).unwrap();
    let files = with_files(&env);
    let store = Store::open(context::key(&env.root).unwrap(), OpenMode::Initialize).unwrap();
    let ids: Vec<String> = env.saved()["sprints"][0]["taskIds"]
        .as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect();
    // The second task runs: bound to an execution whose runtime task is working.
    let running = ids[1].clone();
    store.write(move |db| {
        db.execute_batch("PRAGMA foreign_keys=OFF")?;
        db.execute("INSERT INTO workspace_executions(execution_id,task_id,runtime_task_id) VALUES ('x',?,'rt')", [&running]).ok();
        Ok(())
    }).ok();
    let mut product = env.saved();
    super::super::sprint_files::backfill(&store, &env.state, "p", &mut product, "sp").unwrap();
    let drafts = env.drafts();
    assert_eq!(drafts.iter().find(|d| d.0 == ids[0]).unwrap().2["attachments"], json!(files));
    // Repeating adds nothing.
    super::super::sprint_files::backfill(&store, &env.state, "p", &mut product, "sp").unwrap();
    assert_eq!(env.drafts().iter().find(|d| d.0 == ids[0]).unwrap().2["attachments"], json!(files));
}

#[test]
fn a_shared_worktree_sprint_marks_every_member_with_its_sprint_and_a_per_task_one_does_not() {
    for (shared, name) in [(true, "shared-wt"), (false, "separate-wt")] {
        let env = env(name, 3);
        let mut saved = env.saved();
        saved["sprints"][0]["checkout"] = json!({"kind":"worktree","shared":shared});
        settings::write_json(&product_path(&env.state, "p").unwrap(), &saved).unwrap();
        assert_eq!(env.run(1).unwrap()["created"], 3);
        let drafts = env.drafts();
        assert_eq!(drafts.len(), 3);
        for (task, _, options) in &drafts {
            assert_eq!(options["checkout"], json!({"kind":"worktree"}), "{task}");
            if shared {
                assert_eq!(options["sprintWorktree"], json!({"sprintId":"sp","title":"Sprint"}), "{task}");
            } else {
                assert!(options.get("sprintWorktree").is_none(), "{task}");
            }
        }
    }
}
