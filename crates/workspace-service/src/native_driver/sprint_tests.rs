use crate::{
    native_runtime::NativeRoutes, native_sessions::SessionRoute, Config, LaunchSpec, Service,
};
use cortex_core::CoreResult;
use cortex_engine::environment::{EngineKind, LaunchSpec as EngineLaunch};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap, os::unix::fs::PermissionsExt, path::Path, sync::Arc, time::Duration,
};

/// No-paid ACP peer (a POSIX shell script): a two-task plan only for a read-only planning turn.
/// The deadline journey additionally gates its reply after two clarifications.
fn peer_script() -> String {
    let plan = json!({"tasks": [
        {"key": "t1", "title": "First", "description": "Do the first part", "dependsOn": []},
        {"key": "t2", "title": "Second", "description": "Do the second part", "dependsOn": ["t1"]}]})
    .to_string();
    let plan = json!(plan).to_string();
    let items: Vec<_> = (1..=39)
        .map(|n| {
            let deps = if n > 1 {
                vec![format!("t{}", n - 1)]
            } else {
                vec![]
            };
            json!({"key":format!("t{n}"),"description":format!("Plan part {n}; verify its documented result."),"dependsOn":deps})
        })
        .collect();
    let large_plan = json!(json!({"tasks":items}).to_string());
    let detailed_items: Vec<_> = (1..=50)
        .map(|n| {
            let area = ["navigation", "editor", "activity", "search", "settings"][(n - 1) % 5];
            let description = format!(
                "Implement the {area} behavior for scenario {n}. Record the current \
                 selection before applying the change, and preserve it when the user \
                 returns to this view or refreshes the underlying project. Keep the \
                 action available from the keyboard without changing the order of \
                 unrelated controls. Show the pending state beside the affected \
                 control, then replace it with the result or a readable reason. \
                 Acceptance: exercise a successful change and verify that the \
                 saved selection is restored after reopening the view. Cover a \
                 refusal with the selection intact, the previous content still \
                 visible, and a descriptive message that names the next available \
                 action. Check the same operation in a second project to verify \
                 that it does not reuse state from the first."
            );
            let dependencies: Vec<_> =
                (n > 1).then(|| format!("t{}", n - 1)).into_iter().collect();
            json!({"key":format!("t{n}"), "title":format!("{area}: scenario {n}"),
                "description":description, "dependsOn":dependencies})
        })
        .collect();
    let detailed_plan = json!({"tasks": detailed_items}).to_string();
    assert!(
        detailed_plan.chars().count() > 32_000,
        "fixture must exceed the old limit"
    );
    let detailed_plan = json!(detailed_plan);
    let first =
        json!(json!({"question":"Choose the scope?","options":["Two parts"]}).to_string());
    let second =
        json!(json!({"question":"Choose the order?","options":["Part one first"]}).to_string());
    let fixture = json!("Fixture coordinator.").to_string();
    let other = json!("not read-only").to_string();
    format!(
        r#"
mode=agent
config() {{
printf '[{{"id":"model","type":"select","currentValue":"fixture-model","options":[{{"value":"fixture-model","name":"F"}}]}},{{"id":"mode","type":"select","currentValue":"%s","options":[{{"value":"agent","name":"agent"}},{{"value":"read-only","name":"read-only"}}]}}]' "$mode"
}}
update() {{
printf '{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"s1","update":{{"sessionUpdate":"agent_message_chunk","content":{{"type":"text","text":%s}}}}}}}}\n' "$1"
}}
while IFS= read -r line; do
id=${{line#*\"id\":}}
id=${{id%%,*}}
id=${{id%%\}}*}}
case "$line" in
*'"method":"initialize"'*)
    printf '{{"jsonrpc":"2.0","id":%s,"result":{{"protocolVersion":1,"agentCapabilities":{{}}}}}}\n' "$id" ;;
*'"method":"session/new"'*)
    printf '{{"jsonrpc":"2.0","id":%s,"result":{{"sessionId":"s1","configOptions":%s}}}}\n' "$id" "$(config)" ;;
*'"method":"session/set_config_option"'*)
    case "$line" in
    *'"configId":"mode"'*)
        value=${{line#*\"value\":\"}}
        mode=${{value%%\"*}} ;;
    esac
    printf '{{"jsonrpc":"2.0","id":%s,"result":{{"configOptions":%s}}}}\n' "$id" "$(config)" ;;
*'"method":"session/prompt"'*)
    case "$line" in
    *'Sprint owner load hold'*)
        printf 'held\n' >> "$0/load-entered"
        while [ ! -f "$0/load-release" ]; do /usr/bin/sleep .02; done
        update '{fixture}' ;;
    *'Sprint task t1'*|*'Acceptance: exercise the error path.'*)
        printf '%s\n' "$line" > "$0/.git/sprint-child-prompt"
        update '{fixture}' ;;
    *'Merge manifest:'*)
        printf '%s\n' "$line" >> "$0/.git/merge-handoff-prompts"
        while [ ! -f "$0/.git/release-merge-handoff" ]; do /usr/bin/sleep .02; done
        update '{fixture}' ;;
    *'Run a fresh independent review and runtime verification for the resolved task merge'*)
        printf '%s\n' "$line" >> "$0/.git/merge-fresh-check-prompts"
        update '{fixture}' ;;
    *'Automatic recovery. Inspect the current task'*)
        printf '%s\n' "$line" > "$0/.git/automatic-repair-prompt"
        printf 'repair\n' >> "$0/.git/automatic-repair-input"
        update '{fixture}' ;;
    *'You only plan user tasks'*)
        if [ "$mode" != read-only ]; then
            update '{other}'
        else
            case "$line" in
            *'Sprint deadline fixture'*)
                case "$line" in
                *'First retained answer'*'Second retained answer'*)
                    : > "$0/entered"
                    while [ ! -f "$0/release" ]; do /usr/bin/sleep .02; done
                    update '{large_plan}' ;;
                *'First retained answer'*) update '{second}' ;;
                *) update '{first}' ;;
                esac ;;
            *'Sprint large plan fixture'*)
                printf 'pass\n' >> "$0/passes"
                update '{detailed_plan}' ;;
            *'Sprint compact retry fixture'*|*'Sprint oversized twice fixture'*)
                printf 'pass\n' >> "$0/passes"
                update '{detailed_plan}'
                case "$line" in
                *'Sprint compact retry fixture'*'planning reply exceeded'*) ;;
                *)
                    padding=$(printf '%32768s' '')
                    for chunk in 1 2 3 4; do update "\"$padding\""; done ;;
                esac ;;
            *) update '{plan}' ;;
            esac
        fi ;;
    *) update '{fixture}' ;;
    esac
    printf '{{"jsonrpc":"2.0","id":%s,"result":{{"stopReason":"end_turn"}}}}\n' "$id" ;;
esac
done
"#
    )
}
pub(super) struct Peer(pub(super) Option<std::path::PathBuf>);
impl NativeRoutes for Peer {
    fn route(&self, root: &Path, _: &str) -> CoreResult<SessionRoute> {
        if root.join(".git/refuse-merge-route").is_file() {
            return Err(cortex_core::CoreError::Refused("fixture_merge_route_refused"));
        }
        let load_route = root.join("load-route-enabled").is_file();
        Ok(SessionRoute {
            launch: EngineLaunch {
                engine: EngineKind::Codex,
                executable: "/bin/sh".into(),
                args: vec![
                    "-c".into(),
                    peer_script(),
                    self.0.as_deref().unwrap_or(root).display().to_string(),
                ],
                cwd: root.into(),
                native_home: root.into(),
                credential_directory: root.into(),
                base_environment: BTreeMap::new(),
                codex_launcher: None,
                confinement: None,
            },
            relay: "/bin/false".into(),
            broker_parent: root.join(if workspace_protocol::channel::current().is_dev() {
                ".cortex-dev"
            } else {
                ".cortex"
            }),
            connection: "fixture".into(),
            connection_limit: 4,
            model: if load_route {
                "load-model".into()
            } else {
                "fixture-model".into()
            },
            parameters: if load_route {
                BTreeMap::from([("thought_level".into(), "high".into())])
            } else {
                BTreeMap::new()
            },
            instructions: "No-paid fixture.".into(),
            deadline: Duration::from_secs(3),
        })
    }
}
pub(super) async fn call(service: &Service, op: &str, project: &str, params: Value) -> Value {
    let reply = service
        .machine(
            serde_json::from_value(json!({"op":op,"projectId":project,"params":params}))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reply["ok"], true, "{op}: {reply}");
    reply["result"].clone()
}
pub(super) async fn sprint(service: &Service, project: &str, id: &Value) -> Value {
    call(service, "product.board", project, json!({})).await["sprints"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == *id)
        .unwrap()
        .clone()
}
async fn executions(service: &Service, project: &str) -> BTreeMap<String, bool> {
    let runtime = service
        .state
        .lock()
        .await
        .native
        .get(project)
        .unwrap()
        .clone();
    runtime
        .host
        .executor
        .read(|db| {
            let rows = db
                .prepare("SELECT task_id,execution_id IS NOT NULL FROM workspace_tasks")?
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)))?
                .collect::<Result<BTreeMap<_, _>, _>>()?;
            Ok(rows)
        })
        .unwrap()
        .await
        .unwrap()
        .unwrap()
}
async fn wait<F: std::future::Future<Output = bool>>(what: &str, mut check: impl FnMut() -> F) {
    for _ in 0..240 {
        if check().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("timed out waiting for {what}");
}
fn create(project: &str, request: &str) -> Value {
    json!({"protocol":{"major":1,"minor":8},"projectId":project,"actorId":"forged","requestId":request,
        "operation":"tasks.create","target":project,"expectedVersion":"0","ownerEpoch":null,
        "payload":{"description":format!("Sprint task {request}"),"checkout":{"kind":"main"}}})
}

fn rule(tasks: Value) -> (&'static str, String) {
    let e = super::sprints::validate_tasks(&tasks).unwrap_err();
    (e.code, e.detail)
}

#[test]
fn sprint_plan_rules_keep_only_content_fields_and_are_lenient_where_unambiguous() {
    use super::sprints::validate_tasks;
    let kept =
        validate_tasks(&json!([{"key":"a","description":"Do A","dependsOn":[],"extra":1}]))
            .unwrap();
    assert_eq!(
        kept,
        vec![json!({"key":"a","description":"Do A","dependsOn":[]})]
    );
    let missing = validate_tasks(&json!([{"key":"a","description":"A"}])).unwrap();
    assert_eq!(missing[0]["dependsOn"], json!([]));
    assert!(missing[0].get("complexity").is_none());
    let complex =
        validate_tasks(&json!([{"key":"a","description":"A","complexity":"high"}])).unwrap();
    assert_eq!(complex[0]["complexity"], "high");
    assert_eq!(
        rule(json!([{"key":"a","description":"A","complexity":"huge"}])).1,
        "task 1 (a): complexity must be low, normal or high"
    );
    let single = validate_tasks(
        &json!([{"key":"a","description":"A"},{"key":"b","description":"B","dependsOn":"a"}]),
    )
    .unwrap();
    assert_eq!(single[1]["dependsOn"], json!(["a"]));
    let title = validate_tasks(&json!([{"key":"a","title":"ж".repeat(300),"description":"A"}]))
        .unwrap()[0]["title"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(title.chars().count(), 200);
    assert!(title.ends_with('…'));
    // A plan has no item count limit; only the whole plan's bytes are bounded.
    let items = |count: usize, size: usize| -> Value {
        (1..=count).map(|n| json!({"key":format!("t{n}"),"title":format!("Part {n}"),
            "description":"d".repeat(size),"dependsOn":if n > 1 { json!([format!("t{}", n - 1)]) } else { json!([]) }}))
            .collect::<Vec<_>>().into()
    };
    assert_eq!(validate_tasks(&items(400, 700)).unwrap().len(), 400);
    assert_eq!(rule(items(120, 5000)).0, "plan_too_large");
    // An incomplete plan keeps item rules and unique keys but leaves references for later parts.
    let open = json!([{"key":"a","description":"A","dependsOn":["later"]}]);
    assert!(super::sprints::check_tasks(&open, false).is_ok());
    assert_eq!(rule(open).0, "invalid_dependencies");
}

#[test]
fn sprint_plan_rules_name_the_failing_task_and_rule() {
    let ok = |k: &str| json!({"key":k,"description":"D"});
    assert_eq!(
        rule(json!([ok("t1"), ok("t2"), {"key":"t3","description":"  "}])),
        ("invalid_task", "task 3 (t3): description is empty".into())
    );
    assert_eq!(
        rule(json!([ok("t1"), {"key":"t2"}])).1,
        "task 2 (t2): description is missing or not text"
    );
    assert_eq!(
        rule(json!([{"description":"D"}])).1,
        "task 1: key must be non-blank text of 1 to 100 characters"
    );
    assert_eq!(rule(json!([ok("   ")])).0, "invalid_task");
    assert_eq!(rule(json!([7])).1, "task 1: not an object");
    assert_eq!(
        rule(json!([ok("t1"), ok("t1")])).1,
        "task 2 (t1): duplicate key"
    );
    assert_eq!(
        rule(json!([{"key":"t1","description":"D","dependsOn":[1]}])).1,
        "task 1 (t1): dependsOn must be a list of task keys"
    );
    assert_eq!(
        rule(json!([{"key":"t1","description":"D","title":5}])).1,
        "task 1 (t1): title must be text"
    );
    assert_eq!(
        rule(json!([ok("t1"), {"key":"t2","description":"D","dependsOn":["t12"]}])),
        (
            "invalid_dependencies",
            "task 2 (t2) depends on unknown key t12".into()
        )
    );
    assert_eq!(
        rule(json!([{"key":"t1","description":"D","dependsOn":["t1"]}])).0,
        "invalid_dependencies"
    );
    assert_eq!(
        rule(json!([ok("t1"), {"key":"t2","description":"D","dependsOn":["t1","t1"]}])),
        ("invalid_dependencies", "task 2 (t2) repeats the same dependency".into())
    );
    assert_eq!(
        rule(
            json!([{"key":"a","description":"A","dependsOn":["b"]},{"key":"b","description":"B","dependsOn":["a"]}])
        ),
        (
            "invalid_dependencies",
            "task 1 (a) is part of or blocked by a dependency cycle".into()
        )
    );
    assert_eq!(
        rule(json!([])),
        ("tasks_required", "tasks: the list is empty".into())
    );
    assert_eq!(rule(json!({"x":1})).1, "tasks: missing or not a list");
}

/// The advertised schema and the runtime rules agree: the tool is offered to the
/// coordinator only, and its schema accepts what the rules accept.
#[test]
fn sprint_plan_submit_is_a_coordinator_tool_with_the_plan_contract() {
    let coordinator = cortex_engine::schema::catalogue("coordinator", None, None).unwrap();
    let tool = coordinator
        .iter()
        .find(|t| t["name"] == "sprint_plan_submit")
        .expect("advertised");
    let args = json!({"request_key":"k","tasks":[{"key":"t1","title":"T","description":"D","dependsOn":[]}]});
    assert!(cortex_engine::schema::validate(&tool["inputSchema"], &args).is_ok());
    assert!(cortex_engine::schema::validate(
        &tool["inputSchema"],
        &json!({"request_key":"k","tasks":[]})
    )
    .is_err());
    assert!(super::sprints::validate_tasks(&args["tasks"]).is_ok());
    for profile in ["worker-explorer", "worker-backend-dev"] {
        let worker =
            cortex_engine::schema::catalogue("worker", Some(profile), Some("execution"))
                .unwrap();
        assert!(
            !worker.iter().any(|t| t["name"] == "sprint_plan_submit"),
            "{profile}"
        );
    }
}

/// Cross-task reads are coordinator-only and expose bounded cursors. Workers
/// must continue to use their assignment-local report tools.
#[test]
fn sprint_cross_task_reads_are_coordinator_only() {
    let coordinator = cortex_engine::schema::catalogue("coordinator", None, None).unwrap();
    let graph = coordinator
        .iter()
        .find(|t| t["name"] == "sprint_graph")
        .expect("sprint_graph advertised");
    assert!(
        cortex_engine::schema::validate(&graph["inputSchema"], &json!({"limit": 20})).is_ok()
    );
    let result = coordinator
        .iter()
        .find(|t| t["name"] == "sprint_task_result")
        .expect("sprint_task_result advertised");
    assert!(cortex_engine::schema::validate(
        &result["inputSchema"],
        &json!({"member_id": "task-a"})
    )
    .is_ok());
    assert!(cortex_engine::schema::validate(&result["inputSchema"], &json!({})).is_err());
    for profile in ["worker-explorer", "worker-backend-dev"] {
        let worker =
            cortex_engine::schema::catalogue("worker", Some(profile), Some("execution"))
                .unwrap();
        assert!(!worker.iter().any(|t| t["name"] == "sprint_graph"));
        assert!(!worker.iter().any(|t| t["name"] == "sprint_task_result"));
    }
}
#[tokio::test(flavor = "multi_thread")]
async fn native_sprint_plans_read_only_and_runs_its_queue_in_dependency_order() {
    let root = std::env::temp_dir().join(format!("ns-{}", crate::identity().unwrap()));
    std::fs::create_dir(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let folder = root.join("project");
    std::fs::create_dir(&folder).unwrap();
    assert!(std::process::Command::new("/usr/bin/git")
        .args(["init", "-q"])
        .current_dir(&folder)
        .status()
        .unwrap()
        .success());
    let service = Service::bind_native(
        Config {
            runtime_dir: root.join("runtime"),
            state_dir: root.join("state"),
            allowed_root: root.clone(),
            launch: LaunchSpec::bare(),
            request_timeout: Duration::from_secs(5),
            shutdown_timeout: Duration::from_secs(2),
            event_capacity: 64,
        },
        Arc::new(Peer(None)),
    )
    .await
    .unwrap();
    let registered = service.register(&folder, "Sprint").await.unwrap();
    let id = registered["projectId"].as_str().unwrap().to_owned();
    service.activate(&id).await.unwrap();
    let sprint_params = |text: &str| json!({"action":"create","description":text,"runMode":"hybrid","checkout":{"kind":"main"}});
    // Planning claimed before the service (re)started is never silently rerun.
    let stale = call(
        &service,
        "product.sprint",
        &id,
        sprint_params("Planned before restart"),
    )
    .await["sprintId"]
        .clone();
    let runner = tokio::spawn(service.clone().serve());
    wait("the interrupted planning to be cancelled", || async {
        sprint(&service, &id, &stale).await["planning"] == "cancelled"
    })
    .await;
    assert!(sprint(&service, &id, &stale).await["conversation"]
        .to_string()
        .contains("Planning was interrupted"));
    // A sprint created while the service runs is planned by a read-only planning task.
    let fresh = call(
        &service,
        "product.sprint",
        &id,
        sprint_params("Build the two parts"),
    )
    .await["sprintId"]
        .clone();
    wait("the planning task to start", || async {
        sprint(&service, &id, &fresh).await["planningTaskId"].is_string()
    })
    .await;
    let planning = sprint(&service, &id, &fresh).await["planningTaskId"]
        .as_str()
        .unwrap()
        .to_owned();
    // No-stop policy: a turn that ends without a plan is a continuation (RECOVERING).
    wait("the planning coordinator's turn to settle", || async {
        let runtime = service.state.lock().await.native.get(&id).unwrap().clone();
        let t = planning.clone();
        runtime.host.executor.read(move |db| Ok(db.query_row(
            "SELECT EXISTS(SELECT 1 FROM workspace_tasks w JOIN workspace_executions e ON e.execution_id=w.execution_id
             JOIN runtime_tasks r ON r.task_id=e.runtime_task_id WHERE w.task_id=?
             AND r.state IN ('WAITING_USER','FAILED','CANCELLED','BLOCKED','RECOVERING')
             AND NOT EXISTS(SELECT 1 FROM capacity_reservations c WHERE c.task_id=r.task_id AND c.state!='released'))",
            [&t], |r| r.get::<_, bool>(0))?)).unwrap().await.unwrap().unwrap()
    })
    .await;
    // The coordinator's reviewed plan (sprint_plan_submit) and the task's completion.
    let runtime = service.state.lock().await.native.get(&id).unwrap().clone();
    let t = planning.clone();
    runtime.host.executor.write(move |db| {
        let task: String = db.query_row(
            "SELECT e.runtime_task_id FROM workspace_tasks w JOIN workspace_executions e ON e.execution_id=w.execution_id WHERE w.task_id=?",
            [&t], |r| r.get(0))?;
        assert!(cortex_core::lifecycle::sprint_planning::enabled(db, &task)?);
        let plan_event = cortex_core::lifecycle::event_tx(db, &task, "SPRINT_PLAN_SUBMITTED", json!({"revision":0,"tasks":[
            {"key":"t1","title":"First","description":"Goal: build the first part.\nAcceptance: validate the output format.","dependsOn":[]},
            {"key":"t2","title":"Second","description":"Goal: use the first part.\nAcceptance: exercise the error path.","dependsOn":["t1"]}]}))?;
        cortex_core::lifecycle::event_tx(db, &task, "VERIFICATION_PASSED",
            json!({"revision":0,"runtime":"sprint_plan","plan_event":plan_event,"checks":[]}))?;
        db.execute("UPDATE pending_questions SET status='cancelled' WHERE task_id=? AND status='open'", [&task])?;
        db.execute("UPDATE runtime_tasks SET state='COMPLETED',blocker=NULL,synthesis='Planned.' WHERE task_id=?", [&task])?;
        cortex_core::lifecycle::event_tx(db, &task, "TASK_STATE_CHANGED", json!({"state":"COMPLETED"}))?;
        Ok(())
    }).unwrap().await.unwrap().unwrap();
    drop(runtime);
    wait("the planning task's plan to be proposed", || async {
        sprint(&service, &id, &fresh).await["planning"] == "ready"
    })
    .await;
    let planned = sprint(&service, &id, &fresh).await;
    assert_eq!(planned["plan"]["version"], 1);
    let items = planned["plan"]["items"].as_array().unwrap().clone();
    assert_eq!(items.len(), 2);
    assert_eq!(items[1]["dependsOn"], json!(["t1"]));
    let materialized = call(
        &service,
        "product.sprint",
        &id,
        json!({"action":"materialize","sprintId":fresh,"planVersion":1}),
    )
    .await;
    assert_eq!(materialized["created"], 2);
    let created = materialized["taskIds"].as_array().unwrap();
    let board = call(&service, "product.board", &id, json!({})).await;
    assert_eq!(board["tasks"][created[0].as_str().unwrap()]["title"], "First");
    assert_eq!(board["tasks"][created[1].as_str().unwrap()]["title"], "Second");
    assert_eq!(board["tasks"][created[1].as_str().unwrap()]["dependencies"], json!([created[0]]));
    let runtime = service.state.lock().await.native.get(&id).unwrap().clone();
    let expected: Vec<_> = created.iter().zip(&items).map(|(id, item)| {
        (id.as_str().unwrap().to_owned(), item["description"].as_str().unwrap().to_owned())
    }).collect();
    runtime.host.executor.read(move |db| {
        for (id, description) in expected {
            let saved: String = db.query_row("SELECT description FROM workspace_tasks WHERE task_id=?", [id], |r| r.get(0))?;
            assert_eq!(saved, description);
        }
        Ok(())
    }).unwrap().await.unwrap().unwrap();
    call(
        &service,
        "product.sprint",
        &id,
        json!({"action":"patch","sprintId":fresh,
        "patch":{"queueState":"running","pauseReason":null}}),
    )
    .await;
    let (first, second) = (
        created[0].as_str().unwrap().to_owned(),
        created[1].as_str().unwrap().to_owned(),
    );
    wait("the queue to start the first task", || async {
        executions(&service, &id).await.get(&first) == Some(&true)
    })
    .await;
    assert_eq!(
        executions(&service, &id).await.get(&second),
        Some(&false),
        "dependency started early"
    );
    // Settle the first task as a completed answer; acceptance advances the queue.
    wait("the first task's coordinator turns to settle", || async {
        let runtime = service.state.lock().await.native.get(&id).unwrap().clone();
        runtime.host.executor.read(|db| Ok(db.query_row(
            "SELECT NOT EXISTS(SELECT 1 FROM capacity_reservations WHERE state!='released')
             AND EXISTS(SELECT 1 FROM runtime_tasks WHERE state IN ('WAITING_USER','FAILED','CANCELLED','BLOCKED','RECOVERING'))",
            [], |r| r.get::<_, bool>(0))?)).unwrap().await.unwrap().unwrap()
    })
    .await;
    let runtime = service.state.lock().await.native.get(&id).unwrap().clone();
    let t = first.clone();
    let digest = crate::native_effects::observe_digest(folder.clone()).await.unwrap();
    runtime.host.executor.write(move |db| {
        let runtime_task: String = db.query_row(
            "SELECT e.runtime_task_id FROM workspace_tasks w JOIN workspace_executions e ON e.execution_id=w.execution_id WHERE w.task_id=?",
            [&t], |r| r.get(0))?;
        db.execute("UPDATE runtime_tasks SET state='COMPLETED',blocker=NULL,synthesis='Queue fixture answer.' WHERE task_id=?", [&runtime_task])?;
        cortex_core::lifecycle::event_tx(db, &runtime_task, "ANSWER_SUBMITTED", json!({"text":"Queue fixture answer."}))?;
        let revision: i64 = db.query_row(
            "SELECT revision FROM runtime_tasks WHERE task_id=?",
            [&runtime_task], |r| r.get(0),
        )?;
        cortex_core::lifecycle::event_tx(db, &runtime_task, "VERIFICATION_PASSED",
            json!({"revision":revision,"revision_digest":digest,"checks":[]}))?;
        Ok(())
    }).unwrap().await.unwrap().unwrap();
    wait("the queue to start the dependent task", || async {
        executions(&service, &id).await.get(&second) == Some(&true)
    })
    .await;
    wait("the dependent prompt to be recorded", || async {
        folder.join(".git/sprint-child-prompt").is_file()
    })
    .await;
    let child_prompt =
        std::fs::read_to_string(folder.join(".git/sprint-child-prompt")).unwrap();
    assert!(
        child_prompt.contains(&first),
        "dependent prompt omitted predecessor {first}"
    );
    assert!(child_prompt.contains("Acceptance: exercise the error path."));
    let board = call(&service, "product.board", &id, json!({})).await;
    assert_eq!(board["tasks"][first.as_str()]["status"], "ready_to_merge");
    assert_eq!(
        board["tasks"][first.as_str()]["acceptance"]["automatic"],
        true
    );
    service.stop().await.unwrap();
    let _ = runner.await;
    drop(runtime);
    drop(service);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn service_runtime_binds_cross_task_graph_to_the_authenticated_member() {
    let root = std::env::temp_dir().join(format!("ns-graph-{}", crate::identity().unwrap()));
    std::fs::create_dir(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let folder = root.join("project");
    std::fs::create_dir(&folder).unwrap();
    assert!(std::process::Command::new("/usr/bin/git")
        .args(["init", "-q"])
        .current_dir(&folder)
        .status()
        .unwrap()
        .success());
    let service = Service::bind_native(
        Config {
            runtime_dir: root.join("runtime"),
            state_dir: root.join("state"),
            allowed_root: root.clone(),
            launch: LaunchSpec::bare(),
            request_timeout: Duration::from_secs(5),
            shutdown_timeout: Duration::from_secs(2),
            event_capacity: 64,
        },
        Arc::new(Peer(None)),
    )
    .await
    .unwrap();
    let id = service.register(&folder, "Graph").await.unwrap()["projectId"]
        .as_str()
        .unwrap()
        .to_owned();
    service.activate(&id).await.unwrap();
    let sprint_id = call(&service, "product.sprint", &id, json!({"action":"create","description":"Graph fixture","runMode":"manual","checkout":{"kind":"main"}})).await["sprintId"].clone();
    let a = service.workspace(create(&id, "graph-a")).await.unwrap()["target"].clone();
    let b = service.workspace(create(&id, "graph-b")).await.unwrap()["target"].clone();
    call(
        &service,
        "product.task",
        &id,
        json!({"action":"sprint","taskId":a,"sprintId":sprint_id,"dependencies":[]}),
    )
    .await;
    call(
        &service,
        "product.task",
        &id,
        json!({"action":"sprint","taskId":b,"sprintId":sprint_id,"dependencies":[a]}),
    )
    .await;
    call(&service, "product.sprint", &id, json!({"action":"patch","sprintId":sprint_id,"patch":{"planning":"created","materializedVersion":1,"taskIds":[a,b]}})).await;
    let runtime = service.state.lock().await.native.get(&id).unwrap().clone();
    let request = json!({"protocol":{"major":1,"minor":8},"requestId":"graph-start","projectId":id,"actorId":"forged","operation":"tasks.start","target":a,"expectedVersion":"1","ownerEpoch":null,"payload":{}});
    let start = service.workspace(request).await.unwrap();
    assert_eq!(start["outcome"], "recorded", "start refused: {start}");
    let runtime_task = runtime
        .host
        .executor
        .read({
            let a = a.as_str().unwrap().to_owned();
            move |db| {
                Ok(db.query_row(
                    "SELECT runtime_task_id FROM workspace_executions WHERE task_id=?",
                    [&a],
                    |r| r.get::<_, String>(0),
                )?)
            }
        })
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    let state = root.join("state");
    let graph_result = runtime
        .host
        .executor
        .read({
            let state = state.clone();
            let runtime_task = runtime_task.clone();
            let folder = folder.clone();
            move |db| {
                let scope =
                    crate::machine::sprint_graph::Scope::load(db, &state, &folder, &runtime_task)?
                        .expect("sprint scope");
                let graph = scope.graph(&json!({"limit":10}))?;
                let foreign = scope.member("outside").unwrap_err().code();
                Ok((graph, foreign.to_owned()))
            }
        })
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    let graph = graph_result.0;
    assert_eq!(graph["tasks"].as_array().unwrap().len(), 2);
    assert_eq!(graph_result.1, "task_outside_sprint");
    service.stop().await.unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

// Keep Automatic repair classification scenarios in their own file so the
// scheduler tests and the classifier can evolve independently.
#[cfg(test)]
pub(crate) mod automatic_repair_tests {
    include!("../automatic_repair_tests.rs");
}
