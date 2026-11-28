//! Sprint revisions: validated against the live graph, applied only after review
//! approval in Automatic, waiting for the owner in Hybrid, never touching started
//! work and never cancelling anything.
use super::*;
use cortex_core::lifecycle::{self, review, TaskMachine};
use std::os::unix::fs::DirBuilderExt;
use std::{path::PathBuf, sync::Arc};

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
    let project_root = store.key().root().to_owned();
    store
        .write(move |db| {
            db.execute("INSERT INTO acp_host_sessions VALUES ('acp1',?,'rt','{}','{}','[]','now',NULL)",
                [project_root.to_str()])?;
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
        submit(&self.store, &self.state, "rt", "coord", &args, None).unwrap()["structuredContent"].clone()
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
    let reply = submit(&env.store, &env.state, "rt", "coord", &args, None).unwrap();
    assert_eq!(reply["structuredContent"]["code"], "stale_graph_revision");
    assert!(env.statuses().is_empty());
    // Outside a sprint the tool is refused.
    TaskMachine::new(&env.store).create("lone", "x", "owner").unwrap();
    let outside = submit(&env.store, &env.state, "lone", "coord",
        &json!({"request_key":"o","trigger":1,"verdict":"unchanged","reason":"r"}), None);
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

fn running_typed_dependency(env: &Env) -> Value {
    let (b, c) = (&env.ids[1], &env.ids[2]);
    TaskMachine::new(&env.store).create("runtime-b", "Two", "owner").unwrap();
    let artifact = json!({"name":"design","version":"1","kind":"markdown"});
    let input = json!([{"from":b,"name":"design","version":"1","kind":"markdown"}]);
    env.store.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='EXECUTING' WHERE task_id='runtime-b'", [])?;
        db.execute("INSERT INTO acp_host_sessions VALUES ('acp-b',?,'runtime-b','{}','{}','[]','now',NULL)",
            [env.store.key().root().to_str()])?;
        db.execute("INSERT INTO workspace_executions(execution_id,task_id,runtime_task_id,acp_session_id,owner_epoch,bound_at)
                    VALUES ('ex-b',?,'runtime-b','acp-b',1,'now')", [b])?;
        db.execute("UPDATE workspace_tasks SET execution_id='ex-b' WHERE task_id=?", [b])?;
        db.execute("UPDATE workspace_task_options SET options_json=json_set(options_json,'$.sprintDeliverables',json(?))
                    WHERE task_id=?", (json!({"sprintId":"sp","produces":[],"consumes":input}).to_string(), c))?;
        Ok(())
    }).unwrap();
    let mut saved = env.saved();
    saved["tasks"][b]["produces"] = json!([artifact]);
    saved["tasks"][c]["dependencies"] = json!([b]);
    saved["tasks"][c]["consumes"] = input;
    settings::write_json(&product_path(&env.state, "p").unwrap(), &saved).unwrap();
    artifact
}

#[tokio::test]
async fn authenticated_coordinator_corrects_running_member_and_redirects_draft() {
    use cortex_core::{channels::Principal, ownership, store::executor::StoreExecutor};
    let env = env("running-correction", "automatic");
    let (a, b, c) = (&env.ids[0], &env.ids[1], &env.ids[2]);
    let artifact = running_typed_dependency(&env);
    let saved = env.saved();
    let credential = env.store.write(|db| {
        let owner = ownership::OwnerEpoch { owner: "owner".into(), epoch: 1 };
        let credential = channels::issue_tx(db, "rt", &owner, Principal::Coordinator)?;
        channels::bind_session_tx(db, &credential, "fake-coordinator")?;
        Ok(credential)
    }).unwrap();
    let executor = Arc::new(StoreExecutor::open(env.store.key().clone()).unwrap());
    let submit = |args| crate::native_sprint_revision::execute(
        executor.clone(), credential.clone(), Some(env.state.clone()), args,
    );
    let mut args = json!({"request_key":"fix","verdict":"revise","reason":"B uses an obsolete interface",
        "base_graph_revision":env.pending()["graphRevision"],"changes":[
            {"op":"add","key":"fix-b","title":"Correct B","description":"Publish the corrected design",
                "corrects":b,"dependsOn":[b]},
            {"op":"update","member_id":c,"expected_version":env.version(c),"dependsOn":["fix-b"]}]});
    let mut invalid = args.clone();
    invalid["changes"][1]["dependsOn"] = json!(["other"]);
    assert_eq!(submit(invalid).await.unwrap()["structuredContent"]["code"], "dependency_cross_sprint");
    assert!(env.statuses().is_empty());
    assert_eq!(env.store.read(|db| sprint_impact::pending(db, "rt")).unwrap(), None);
    let valid = args.clone();
    args["changes"].as_array_mut().unwrap().pop();
    assert_eq!(submit(args).await.unwrap()["structuredContent"]["code"], "correction_consumer_required");
    let recorded = submit(valid.clone()).await.unwrap()["structuredContent"].clone();
    assert_eq!(recorded["review"], "scheduled");
    let canonical_id = new_task_id("p", "sp", recorded["revision_id"].as_str().unwrap(), "fix-b");
    let (objective, expected_projection) = env.store.read(|db| {
        let objective: String = db.query_row(
            "SELECT objective FROM runtime_nodes WHERE task_id='rt' AND node_id='_sprint_revision_review_1'",
            [], |r| r.get(0),
        )?;
        let projection = sprint_impact::revisions(db, "rt")?[0]["reviewProjection"].clone();
        Ok((objective, projection))
    }).unwrap();
    let (_, typed_section) = objective.split_once("evidence of publication:\n").unwrap();
    let (reviewer_contracts, _) = typed_section.split_once("\n\nProposed changes").unwrap();
    let reviewer_contracts: Value = serde_json::from_str(reviewer_contracts).unwrap();
    assert_eq!(reviewer_contracts, expected_projection);
    assert_eq!(reviewer_contracts[0]["member_id"], canonical_id);
    assert_eq!(reviewer_contracts[0]["produces"], json!([artifact]));
    assert_eq!(reviewer_contracts[1]["member_id"], *c);
    assert_eq!(reviewer_contracts[1]["consumes"][0]["from"], canonical_id);
    let (_, current_section) = objective.split_once("Current saved state of the changed tasks (JSON data):\n").unwrap();
    let (current_contracts, _) = current_section.split_once("\nDownstream tasks").unwrap();
    let current_contracts: Value = serde_json::from_str(current_contracts).unwrap();
    assert_eq!(current_contracts[0]["consumes"][0]["from"], *b);
    assert_eq!(env.description(c), "Do three");
    assert_eq!(env.saved()["tasks"][c]["dependencies"], json!([b]));
    assert_eq!(env.pending()["tasks"][0]["held"], true);
    env.review("APPROVE");
    let note = settle(&env.store, &env.state, "rt", &|_| None).unwrap();
    assert!(note.contains("applied"), "{note}");
    assert_eq!(submit(valid).await.unwrap()["structuredContent"], recorded);
    let final_product = env.saved();
    let added = final_product["sprints"][0]["taskIds"][3].as_str().unwrap();
    assert_eq!(final_product["tasks"][added]["corrects"], *b);
    assert_eq!(final_product["tasks"][added]["dependencies"], json!([b]));
    assert_eq!(final_product["tasks"][c]["dependencies"], json!([added]));
    assert_eq!(canonical_id, added);
    assert_eq!(final_product["tasks"][c]["consumes"], reviewer_contracts[1]["consumes"]);
    assert_eq!(final_product["tasks"][a], saved["tasks"][a]);
    assert_eq!(final_product["tasks"][b], saved["tasks"][b]);
    assert_eq!(env.description(b), "Do two");
    let persisted = env.store.read(|db| {
        let options = |id: &str| -> CoreResult<Value> {
            let raw: String = db.query_row("SELECT options_json FROM workspace_task_options WHERE task_id=?", [id], |r| r.get(0))?;
            Ok(serde_json::from_str(&raw)?)
        };
        assert_eq!(options(added)?["sprintDeliverables"]["produces"], reviewer_contracts[0]["produces"]);
        assert_eq!(options(c)?["sprintDeliverables"]["consumes"], reviewer_contracts[1]["consumes"]);
        let execution: String = db.query_row("SELECT execution_id FROM workspace_tasks WHERE task_id=?", [b], |r| r.get(0))?;
        Ok(execution)
    }).unwrap();
    assert_eq!(persisted, "ex-b");
}

#[test]
fn owner_chat_request_reaches_the_impact_turn_and_resumes_after_review() {
    use cortex_core::{channels::Principal, lifecycle::steering, ownership, workspace::control};
    let env = env("owner-chat", "automatic");
    env.store.write(|db| {
        lifecycle::event_tx(db, "rt", "NATIVE_TASK_ADOPTED", json!({"execution_host":"rust"}))?;
        Ok(())
    }).unwrap();
    coordinator_turn(&env.store, &env.state, "rt", &|_| None).unwrap();
    let request = json!({"action":"prompt","requestId":"owner-replan","executionId":"ex1",
        "text":"Please revise the remaining sprint work to cover the new format."});
    control::admit_input(&env.store, "p", &crate::actor(), &request, None).unwrap();
    control::finish(&env.store, "p", &crate::actor(), &request, false).unwrap();
    let trigger = env.store.write(|db| {
        let messages = steering::deliver_tx(db, "rt")?;
        assert_eq!(messages[0]["text"], request["text"]);
        let owner = ownership::OwnerEpoch { owner: "owner".into(), epoch: 1 };
        let credential = channels::issue_tx(db, "rt", &owner, Principal::Coordinator)?;
        channels::bind_session_tx(db, &credential, "owner-replan-coordinator")?;
        lifecycle::event_tx(db, "rt", "NATIVE_TURN_SETTLED", json!({"channel_id":credential.channel_id}))?;
        assert_eq!(steering::confirm_tx(db, "rt")?, 1);
        Ok(sprint_impact::pending(db, "rt")?.unwrap())
    }).unwrap();
    assert!(coordinator_turn(&env.store, &env.state, "rt", &|_| None).unwrap().contains(&format!("trigger {trigger}")));
    let c = &env.ids[2];
    let verdict = env.submit("owner-turn", trigger, json!([
        {"op":"update","member_id":c,"expected_version":env.version(c),"description":"Verify the new format"}]));
    assert_eq!(verdict["review"], "scheduled");
    env.review("APPROVE");
    assert!(settle(&env.store, &env.state, "rt", &|_| None).unwrap().contains("applied"));
    assert_eq!(env.description(c), "Verify the new format");
    assert_eq!(env.store.read(|db| sprint_impact::pending(db, "rt")).unwrap(), None);
    assert_eq!(env.store.read(|db| Ok(lifecycle::domain::task_tx(db, "rt")?.0)).unwrap(), cortex_core::TaskState::Planning);
}

#[test]
fn stale_draft_in_review_rolls_back_the_entire_revision() {
    let env = env("apply-stale", "automatic");
    let t = env.trigger();
    let (b, c) = (&env.ids[1], &env.ids[2]);
    let reply = env.submit("apply-stale", t, json!([
        {"op":"update","member_id":b,"expected_version":env.version(b),"description":"Revised B"},
        {"op":"update","member_id":c,"expected_version":env.version(c),"description":"Revised C"},
        {"op":"add","key":"extra","title":"Extra","description":"Verify both changes","dependsOn":[b,c]}]));
    assert_eq!(reply["review"], "scheduled");
    env.store.write(|db| {
        db.execute("UPDATE workspace_tasks SET description='Owner edit',version=version+1 WHERE task_id=?", [c])?;
        Ok(())
    }).unwrap();
    let saved = env.saved();
    env.review("APPROVE");
    let note = settle(&env.store, &env.state, "rt", &|_| None).unwrap();
    assert_eq!(note, "");
    assert_eq!(env.statuses(), ["apply_refused"]);
    assert_eq!(env.saved(), saved);
    assert_eq!(env.description(b), "Do two");
    assert_eq!(env.description(c), "Owner edit");
    assert_eq!(env.store.read(|db| sprint_impact::pending(db, "rt")).unwrap(), Some(t));
}

#[test]
fn revision_crash_child() {
    let Some(base) = std::env::var_os("CORTEX_TEST_REVISION_CRASH_DIR") else {
        return;
    };
    let base = PathBuf::from(base);
    let checkpoint = std::env::var("CORTEX_TEST_REVISION_CRASH_POINT").unwrap().parse().unwrap();
    CRASH_AFTER.with(|point| point.set(checkpoint));
    let store = Store::open(context::key(&base.join("repo")).unwrap(), OpenMode::Initialize).unwrap();
    let result = settle(&store, &base.join("state"), "rt", &|_| None);
    panic!("expected termination while applying revision: {result:?}");
}

#[test]
fn revision_recovers_sigkill_on_either_side_of_commit() {
    use std::os::unix::process::ExitStatusExt;
    for (checkpoint, committed) in [(1, false), (2, true)] {
        let env = env("crash", "automatic");
        let artifact = running_typed_dependency(&env);
        let t = env.trigger();
        let (b, c) = (&env.ids[1], &env.ids[2]);
        let before = env.saved();
        let reply = env.submit("cross-card", t, json!([
            {"op":"add","key":"handoff","title":"Handoff","description":"Verify the revised work","corrects":b,"dependsOn":[b]},
            {"op":"update","member_id":c,"expected_version":env.version(c),"dependsOn":["handoff"],
                "description":"Use the verified work"}]));
        let rev = reply["revision_id"].as_str().unwrap();
        let new = new_task_id("p", "sp", rev, "handoff");
        assert_eq!(reply["review"], "scheduled");
        env.review("APPROVE");
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "machine::sprint_revision::tests::revision_crash_child"])
            .env("CORTEX_TEST_REVISION_CRASH_DIR", &env.base)
            .env("CORTEX_TEST_REVISION_CRASH_POINT", checkpoint.to_string())
            .output()
            .unwrap();
        assert_eq!(child.status.signal(), Some(libc::SIGKILL), "{child:?}");
        let raw = settings::read_json(&product_path(&env.state, "p").unwrap(), json!({})).unwrap();
        assert_ne!(raw["sprintMaterializationInFlight"], Value::Null);
        let count = || env.store.read(|db| {
            Ok(db.query_row("SELECT count(*) FROM workspace_tasks WHERE task_id=?", [&new], |r| r.get::<_, i64>(0))?)
        }).unwrap();
        let input_source = || env.store.read(|db| {
            let source = db.query_row(
                "SELECT json_extract(options_json,'$.sprintDeliverables.consumes[0].from')
                 FROM workspace_task_options WHERE task_id=?", [c], |r| r.get::<_, String>(0))?;
            Ok(source)
        }).unwrap();
        if committed {
            assert_eq!(input_source(), new);
            assert_eq!(env.saved()["tasks"][c]["dependencies"], json!([new]));
            assert_eq!(env.statuses(), ["applied"]);
            assert_eq!(count(), 1);
        } else {
            assert_eq!(input_source(), *b);
            assert_eq!(env.description(c), "Do three");
            assert_eq!(env.saved(), before);
            assert_eq!(env.statuses(), ["approved"]);
            assert_eq!(count(), 0);
        }
        settle(&env.store, &env.state, "rt", &|_| None).unwrap();
        settle(&env.store, &env.state, "rt", &|_| None).unwrap();
        let saved = env.saved();
        assert_eq!(saved["tasks"][&new]["produces"], json!([artifact]));
        assert_eq!(saved["tasks"][c]["dependencies"], json!([new]));
        assert_eq!(input_source(), new);
        assert_eq!(env.description(c), "Use the verified work");
        assert_eq!(env.description(b), "Do two");
        assert_eq!(env.statuses(), ["applied"]);
        assert_eq!(count(), 1);
    }
}

#[test]
fn stale_or_legacy_typed_review_requires_a_fresh_proposal() {
    for (legacy, code) in [(true, "sprint_review_projection_required"), (false, "sprint_review_projection_changed")] {
        let env = env("old-review-projection", "automatic");
        running_typed_dependency(&env);
        let t = env.trigger();
        let (b, c) = (&env.ids[1], &env.ids[2]);
        let changes = || json!([
            {"op":"add","key":"fix-b","title":"Correct B","description":"Publish the corrected design",
             "corrects":b,"dependsOn":[b]},
            {"op":"update","member_id":c,"expected_version":env.version(c),"dependsOn":["fix-b"]}]);
        assert_eq!(env.submit("previous-shape", t, changes())["review"], "scheduled");
        if legacy {
            // A revision durably submitted and reviewed under a runtime without a projection.
            env.store.write(|db| {
                db.execute("UPDATE runtime_events SET payload_json=json_remove(payload_json,'$.review_projection')
                    WHERE type=? AND task_id='rt'", [sprint_impact::SUBMITTED])?;
                Ok(())
            }).unwrap();
        } else {
            let mut saved = env.saved();
            saved["tasks"][b]["produces"].as_array_mut().unwrap().push(
                json!({"name":"release-notes","version":"1","kind":"markdown"}));
            settings::write_json(&product_path(&env.state, "p").unwrap(), &saved).unwrap();
        }
        env.review("APPROVE");
        let before = env.saved();
        settle(&env.store, &env.state, "rt", &|_| None).unwrap();
        assert_eq!(env.saved(), before);
        let states = env.store.read(|db| sprint_impact::revisions(db, "rt")).unwrap();
        assert_eq!(states[0]["status"], "apply_refused");
        assert_eq!(states[0]["code"], code);
        assert_eq!(env.store.read(|db| sprint_impact::pending(db, "rt")).unwrap(), Some(t));
        assert_eq!(env.submit("updated-shape", t, changes())["review"], "scheduled");
        env.review("APPROVE");
        assert!(settle(&env.store, &env.state, "rt", &|_| None).unwrap().contains("applied"));
        assert_eq!(env.saved()["sprints"][0]["taskIds"].as_array().unwrap().len(), 4);
    }
}
