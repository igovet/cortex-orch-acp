//! Sprint revisions: validated against the live graph, applied only after review
//! approval in Automatic, waiting for the owner in Hybrid, never touching started
//! work and never cancelling anything.
use super::*;
use cortex_core::lifecycle::{self, review, TaskMachine};
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;

struct Env {
    base: PathBuf,
    root: PathBuf,
    state: PathBuf,
    ids: Vec<String>,
    store: Store,
}
impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

/// A created sprint t1 -> t2, t3 (independent), t1 started as runtime task `rt`
/// with its coordinator bound, and a second sprint holding `other`.
fn env(name: &str, mode: &str) -> Env {
    let base = std::env::temp_dir().join(format!("cw-revision-{name}-{}", settings::unique()));
    std::fs::DirBuilder::new().mode(0o700).recursive(true).create(&base).unwrap();
    let base = base.canonicalize().unwrap();
    let (root, state) = (base.join("repo"), base.join("state"));
    for dir in [&root, &state] {
        std::fs::DirBuilder::new().mode(0o700).create(dir).unwrap();
    }
    let store = Store::open(context::key(&root).unwrap(), OpenMode::Initialize).unwrap();
    let bound = store.key().root().to_string_lossy().into_owned();
    store
        .write(move |db| {
            db.execute("INSERT INTO workspace_projects VALUES ('p',?,?,'now',1)", (&bound, crate::actor()))?;
            Ok(())
        })
        .unwrap();
    let items = json!([
        {"key":"t1","title":"One","description":"Do one","dependsOn":[]},
        {"key":"t2","title":"Two","description":"Do two","dependsOn":["t1"]},
        {"key":"t3","title":"Three","description":"Do three","dependsOn":[]}]);
    let product = json!({"tasks":{"other":{"sprintId":"sp2","dependencies":[]}},"messages":[],"comments":{},"drafts":{},
        "sprintRunModeVersion":1,"sprints":[{
        "id":"sp","projectId":"p","description":"Sprint","title":"Sprint","runMode":mode,
        "targetBranch":"main","planning":"ready","planningTaskId":"planner","conversation":[],
        "plan":{"version":1,"items":items},"history":[],"materializedVersion":null,"taskIds":[],
        "queueState":"running","pauseReason":null},
        {"id":"sp2","projectId":"p","description":"Other","title":"Other","runMode":"manual","planning":"created",
         "materializedVersion":1,"taskIds":["other"],"queueState":"idle"}]});
    settings::write_json(&product_path(&state, "p").unwrap(), &product).unwrap();
    let reply = super::super::sprint_materialize::materialize(&root, "p", &state,
        &json!({"action":"materialize","sprintId":"sp","planVersion":1})).unwrap();
    let ids: Vec<String> = ["t1", "t2", "t3"]
        .iter()
        .map(|k| super::super::sprint_materialize::task_id("p", "sp", 1, k))
        .collect();
    assert_eq!(reply["created"], 3);
    TaskMachine::new(&store).create("rt", "One", "owner").unwrap();
    let first = ids[0].clone();
    store
        .write(move |db| {
            db.execute("INSERT INTO workspace_executions(execution_id,task_id,runtime_task_id,acp_session_id,owner_epoch,bound_at)
                VALUES ('ex1',?,'rt','acp1',1,'now')", [&first])?;
            db.execute("UPDATE workspace_tasks SET execution_id='ex1' WHERE task_id=?", [&first])?;
            db.execute("UPDATE runtime_tasks SET state='RECONCILING',synthesis='Done' WHERE task_id='rt'", [])?;
            Ok(())
        })
        .unwrap();
    Env { base, root, state, ids, store }
}

impl Env {
    /// The coordinator turn binds the sprint, then the owner's message arrives.
    fn trigger(&self) -> i64 {
        coordinator_turn(&self.store, &self.state, "rt", &|_| None).unwrap();
        self.store
            .write(|db| lifecycle::event_tx(db, "rt", "STEERING_DELIVERED", json!({"request":"m"})))
            .unwrap()
    }
    fn pending(&self) -> Value {
        pending_descriptions(&self.store, &self.state, "rt", &json!({})).unwrap()
    }
    fn version(&self, id: &str) -> String {
        self.pending()["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["member_id"] == id)
            .unwrap()["version"]
            .as_str()
            .unwrap()
            .to_owned()
    }
    fn submit(&self, key: &str, trigger: i64, changes: Value) -> Value {
        let base = self.pending()["graphRevision"].clone();
        let args = json!({"request_key":key,"trigger":trigger,"verdict":"revise","reason":"Owner changed scope",
            "base_graph_revision":base,"changes":changes});
        submit(&self.store, &self.state, "rt", "coord", &args).unwrap()["structuredContent"].clone()
    }
    fn review(&self, verdict: &str) {
        self.store
            .write(|db| {
                db.execute("UPDATE runtime_nodes SET status='completed',result_json=? WHERE task_id='rt' AND status='pending'",
                    [json!({"verdict":verdict,"findings":[],"report_ref":"r"}).to_string()])?;
                review::advance_tx(db, "rt")
            })
            .unwrap();
    }
    fn description(&self, id: &str) -> String {
        let id = id.to_owned();
        self.store
            .read(move |db| Ok(db.query_row("SELECT description FROM workspace_tasks WHERE task_id=?", [id], |r| r.get(0))?))
            .unwrap()
    }
    fn saved(&self) -> Value {
        product(&self.state, "p").unwrap()
    }
    fn statuses(&self) -> Vec<String> {
        self.store
            .read(|db| sprint_impact::revisions(db, "rt"))
            .unwrap()
            .iter()
            .map(|r| r["status"].as_str().unwrap().to_owned())
            .collect()
    }
}

#[test]
fn refusals_name_the_rule_and_record_nothing() {
    let env = env("refusals", "automatic");
    let t = env.trigger();
    let (one, two, three) = (&env.ids[0], &env.ids[1], &env.ids[2]);
    let v2 = env.version(two);
    let cases = [
        (json!([{"op":"update","member_id":one,"expected_version":"1","description":"x"}]), "sprint_member_read_only"),
        (json!([{"op":"update","member_id":two,"expected_version":"99","description":"x"}]), "stale_version"),
        (json!([{"op":"update","member_id":two,"expected_version":v2,"dependsOn":[two]}]), "dependency_self"),
        (json!([{"op":"update","member_id":two,"expected_version":v2,"dependsOn":["other"]}]), "dependency_cross_sprint"),
        (json!([{"op":"update","member_id":two,"expected_version":v2,"dependsOn":[three]},
                {"op":"update","member_id":three,"expected_version":env.version(three),"dependsOn":[two]}]), "dependency_cycle"),
        (json!([{"op":"add","key":"n1","title":"N","description":"New","dependsOn":["n2"]},
                {"op":"add","key":"n2","title":"M","description":"New","dependsOn":["n1"]}]), "dependency_cycle"),
        (json!([{"op":"update","member_id":two,"expected_version":v2}]), "invalid_change"),
    ];
    for (changes, code) in cases {
        let reply = env.submit("k", t, changes);
        assert_eq!((reply["recorded"].as_bool(), reply["code"].as_str()), (Some(false), Some(code)));
    }
    // A stale base graph revision is refused too.
    let args = json!({"request_key":"s","trigger":t,"verdict":"revise","reason":"r","base_graph_revision":"stale",
        "changes":[{"op":"update","member_id":two,"expected_version":v2,"description":"x"}]});
    let reply = submit(&env.store, &env.state, "rt", "coord", &args).unwrap();
    assert_eq!(reply["structuredContent"]["code"], "stale_graph_revision");
    assert!(env.statuses().is_empty());
    // Outside a sprint the tool is refused.
    TaskMachine::new(&env.store).create("lone", "x", "owner").unwrap();
    let outside = submit(&env.store, &env.state, "lone", "coord",
        &json!({"request_key":"o","trigger":1,"verdict":"unchanged","reason":"r"}));
    assert_eq!(outside.unwrap_err().code(), "sprint_membership_required");
}

#[test]
fn the_tool_is_a_coordinator_tool_only() {
    let coordinator = cortex_engine::schema::catalogue("coordinator", None, None).unwrap();
    let tool = coordinator.iter().find(|t| t["name"] == "sprint_revision_submit").expect("advertised");
    assert_eq!(tool["inputSchema"]["properties"]["changes"]["maxItems"], 50);
    let graph = coordinator.iter().find(|t| t["name"] == "sprint_graph").unwrap();
    assert_eq!(graph["inputSchema"]["properties"]["include"]["enum"], json!(["pending_descriptions"]));
    for profile in ["worker-backend-dev", "worker-code-reviewer"] {
        for stage in ["execution", "review"] {
            let worker = cortex_engine::schema::catalogue("worker", Some(profile), Some(stage)).unwrap();
            assert!(!worker.iter().any(|t| t["name"] == "sprint_revision_submit"));
        }
    }
}

#[test]
fn automatic_applies_only_after_review_approval() {
    let env = env("automatic", "automatic");
    let t = env.trigger();
    let two = env.ids[1].clone();
    let v2 = env.version(&two);
    let reply = env.submit("a", t, json!([
        {"op":"update","member_id":two,"expected_version":v2,"description":"Do two the new way","dependsOn":[]},
        {"op":"add","key":"n1","title":"Migrate","description":"Migrate the data","dependsOn":[two]}]));
    assert_eq!(reply["review"], "scheduled", "{reply}");
    // Under review: nothing is applied, and the changed draft is held back.
    assert_eq!(settle(&env.store, &env.state, "rt", &|_| None).unwrap(), "");
    assert_eq!(env.description(&two), "Do two");
    assert!(env.pending()["tasks"].as_array().unwrap().iter().any(|t| t["member_id"] == two && t["held"] == true));
    env.review("APPROVE");
    let before = env.saved()["sprints"][0].clone();
    let note = settle(&env.store, &env.state, "rt", &|_| None).unwrap();
    assert!(note.contains("applied"), "{note}");
    assert_eq!(env.description(&two), "Do two the new way");
    let saved = env.saved();
    let sprint = &saved["sprints"][0];
    assert_eq!(sprint["taskIds"].as_array().unwrap().len(), 4);
    let added = sprint["taskIds"][3].as_str().unwrap();
    assert_eq!(saved["tasks"][added]["dependencies"], json!([two]));
    assert_eq!(saved["tasks"][&two]["dependencies"], json!([]));
    // The approved plan, its history and materialized version are untouched; the queue state too.
    for key in ["plan", "history", "materializedVersion", "queueState"] {
        assert_eq!(sprint[key], before[key], "{key}");
    }
    assert_eq!(env.description(added), "Migrate the data");
    assert_eq!(env.statuses(), ["applied"]);
    let applied = &env.store.read(|db| sprint_impact::revisions(db, "rt")).unwrap()[0];
    assert_eq!(applied["previous"][0]["description"], "Do two");
    assert_ne!(applied["before_digest"], applied["after_digest"]);
    assert_eq!(env.store.read(|db| sprint_impact::pending(db, "rt")).unwrap(), None);
}

#[test]
fn hybrid_waits_for_the_owner_and_propose_cancel_is_never_applied() {
    let env = env("hybrid", "hybrid");
    let t = env.trigger();
    let (two, three) = (env.ids[1].clone(), env.ids[2].clone());
    let v2 = env.version(&two);
    env.submit("h", t, json!([
        {"op":"update","member_id":two,"expected_version":v2,"title":"Two again"},
        {"op":"propose_cancel","member_id":three,"reason":"No longer needed"}]));
    env.review("APPROVE");
    settle(&env.store, &env.state, "rt", &|_| None).unwrap();
    assert_eq!(env.statuses(), ["awaiting_owner"]);
    assert_eq!(env.saved()["tasks"][&two]["title"], "Two");
    // The impact check is resolved; the owner decides on the card.
    assert_eq!(env.store.read(|db| sprint_impact::pending(db, "rt")).unwrap(), None);
    let live = super::super::sprint_live::owner_action(&env.root, &env.state, "p",
        &json!({"action":"live_graph","sprintId":"sp"}), &|_| None).unwrap();
    let card = &live["revisions"][0];
    assert_eq!((card["status"].as_str(), card["taskId"].as_str()), (Some("awaiting_owner"), Some(env.ids[0].as_str())));
    let applied = super::super::sprint_live::owner_action(&env.root, &env.state, "p",
        &json!({"action":"apply_revision","sprintId":"sp","revisionId":card["revisionId"],
            "expectedGraphRevision":live["graphRevision"]}), &|_| None).unwrap();
    assert_eq!(applied["changed"], true);
    let saved = env.saved();
    assert_eq!(saved["tasks"][&two]["title"], "Two again");
    // The proposed cancellation is still only a proposal: the task is an untouched draft.
    assert!(saved["tasks"][&three]["closedAs"].is_null());
    let all = env.store.read(|db| sprint_impact::revisions(db, "rt")).unwrap();
    assert_eq!((all[0]["status"].as_str(), all[0]["held"].clone()), (Some("awaiting_owner"), json!([three])));
    super::super::sprint_live::owner_action(&env.root, &env.state, "p",
        &json!({"action":"reject_revision","sprintId":"sp","revisionId":card["revisionId"]}), &|_| None).unwrap();
    assert_eq!(env.statuses(), ["rejected"]);
}

#[test]
fn the_start_fence_holds_only_the_affected_draft() {
    let env = env("fence", "automatic");
    let t = env.trigger();
    let (two, three) = (env.ids[1].clone(), env.ids[2].clone());
    env.submit("f", t, json!([{"op":"update","member_id":three,"expected_version":env.version(&three),"description":"Three, revised"}]));
    let snapshot = sprint_live::read_snapshot(&env.store, &env.state, "p", "sp").unwrap();
    assert_eq!(snapshot.records[&three]["revisionPending"], true);
    assert!(snapshot.records[&two]["revisionPending"].is_null());
    let order = crate::native_driver::sprints::live_execution_order(&snapshot);
    let row = |id: &str| order["notStarted"].as_array().unwrap().iter().find(|r| r["taskId"] == id).unwrap().clone();
    assert_eq!(row(&three)["reason"], "sprint_revision_pending");
    assert!(row(&two)["reason"] != "sprint_revision_pending");
    assert_ne!(crate::native_driver::sprints::selected_now(&snapshot), Some(three.as_str()));
}

#[test]
fn added_tasks_get_the_route_of_their_own_complexity() {
    let env = env("routes", "automatic");
    let t = env.trigger();
    let two = env.ids[1].clone();
    let reply = env.submit("r", t, json!([
        {"op":"add","key":"n1","title":"Quick","description":"Rename a flag","dependsOn":[two],"complexity":"low"},
        {"op":"add","key":"n2","title":"Hard","description":"Rework locking","dependsOn":[two],"complexity":"high"},
        {"op":"add","key":"n3","title":"Plain","description":"Add a test","dependsOn":[two]}]));
    assert_eq!(reply["review"], "scheduled", "{reply}");
    env.review("APPROVE");
    let routes = |c: &str| match c {
        "low" => Some(json!({"routeId":"light-route","engine":"e","model":"light","tier":"light","effort":"low","usedPercent":null})),
        "high" => Some(json!({"routeId":"flagship-route","engine":"e","model":"big","tier":"flagship","effort":"high","usedPercent":null})),
        _ => None,
    };
    let note = settle(&env.store, &env.state, "rt", &routes).unwrap();
    assert!(note.contains("applied"), "{note}");
    let saved = env.saved();
    let roster = saved["sprints"][0]["taskIds"].as_array().unwrap();
    let by_title = |title: &str| roster.iter().map(|i| &saved["tasks"][i.as_str().unwrap()]).find(|t| t["title"] == title).unwrap().clone();
    assert_eq!((by_title("Quick")["sprintRoute"]["routeId"].as_str(), by_title("Quick")["sprintRoute"]["complexity"].as_str()),
        (Some("light-route"), Some("low")));
    assert_eq!((by_title("Hard")["sprintRoute"]["routeId"].as_str(), by_title("Hard")["sprintRoute"]["effort"].as_str()),
        (Some("flagship-route"), Some("high")));
    // No route for a complexity: the owner's default coordinator, as before.
    assert!(by_title("Plain")["sprintRoute"].is_null());
}

#[test]
fn an_unknown_complexity_is_refused_with_the_change_named() {
    let env = env("bad-complexity", "automatic");
    let t = env.trigger();
    let two = env.ids[1].clone();
    let reply = env.submit("c", t, json!([
        {"op":"add","key":"n1","title":"X","description":"Do x","dependsOn":[two],"complexity":"huge"}]));
    assert_ne!(reply["review"], "scheduled", "{reply}");
}
