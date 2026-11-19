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
fn creating_again_returns_the_same_tasks_and_never_a_second_set() {
    let env = env("again", 12);
    let first = env.run(1).unwrap();
    let again = env.run(1).unwrap();
    assert_eq!(again["created"], 0);
    assert_eq!(again["taskIds"], first["taskIds"]);
    assert_eq!(env.drafts().len(), 12);
    assert_eq!(env.saved()["sprints"][0]["taskIds"], first["taskIds"]);
    // Another version of an already created sprint is refused, not created beside it.
    assert_eq!(env.run(2).unwrap_err().code, "sprint_started");
    assert_eq!(env.drafts().len(), 12);
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
