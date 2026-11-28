use super::*;
use crate::{
    native_driver::sprint_tests::{call, sprint, Peer},
    Config, LaunchSpec,
};
use std::{
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::Path,
};

/// A disposable fixture directory.
struct Scratch(std::path::PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn planning_fixture(name: &str) -> (Scratch, Arc<Service>, Arc<Runtime>, String) {
    let root = std::env::temp_dir().join(format!("{name}-{}", crate::identity().unwrap()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .unwrap();
    let scratch = Scratch(root.clone());
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
    let registered = service.register(&folder, name).await.unwrap();
    let project = registered["projectId"].as_str().unwrap().to_owned();
    service.activate(&project).await.unwrap();
    let runtime = service.state.lock().await.native[&project].clone();
    (scratch, service, runtime, project)
}

/// A sprint created exactly as the desktop creates one.
async fn task_sprint(service: &Service, project: &str, params: Value) -> Value {
    call(service, "product.sprint", project, params).await["sprintId"].clone()
}

/// The planning task's current runtime task, once the fixture coordinator's turn settled.
async fn settled(runtime: &Runtime, draft: &str) -> String {
    let draft = draft.to_owned();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(12);
    loop {
        let t = draft.clone();
        let task: Option<String> = runtime.host.executor.read(move |db| {
            use rusqlite::OptionalExtension;
            Ok(db.query_row("SELECT e.runtime_task_id FROM workspace_tasks w JOIN workspace_executions e ON e.execution_id=w.execution_id
                JOIN runtime_tasks r ON r.task_id=e.runtime_task_id WHERE w.task_id=?
                AND r.state IN ('WAITING_USER','FAILED','CANCELLED','BLOCKED','RECOVERING','COMPLETED')
                AND NOT EXISTS(SELECT 1 FROM capacity_reservations c WHERE c.task_id=r.task_id AND c.state!='released')",
                [&t], |r| r.get(0)).optional()?)
        }).unwrap().await.unwrap().unwrap();
        if let Some(task) = task {
            return task;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the fixture coordinator did not settle"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The fixture's stand-in for a coordinator that submitted `plan` (if any) through
/// sprint_plan_submit and then finished through the ordinary completion gate.
async fn complete(runtime: &Runtime, task: &str, plan: Option<Value>) {
    let task = task.to_owned();
    runtime.host.executor.write(move |db| {
        if let Some(tasks) = plan {
            cortex_core::lifecycle::event_tx(db, &task, "SPRINT_PLAN_SUBMITTED", json!({"revision":0,"tasks":tasks}))?;
        }
        // This fixture advances the projection from a completed planning turn.
        // Core's sprint_planning tests exercise the preceding review/verification gate.
        if let Some((plan_event, _)) = cortex_core::lifecycle::sprint_planning::latest(db, &task)? {
            cortex_core::lifecycle::event_tx(db, &task, "VERIFICATION_PASSED",
                json!({"runtime":"sprint_plan","plan_event":plan_event,"checks":[]}))?;
        }
        db.execute("UPDATE pending_questions SET status='cancelled' WHERE task_id=? AND status='open'", [&task])?;
        db.execute("UPDATE runtime_tasks SET state='COMPLETED',blocker=NULL,synthesis='Planned.' WHERE task_id=?", [&task])?;
        cortex_core::lifecycle::event_tx(db, &task, "TASK_STATE_CHANGED", json!({"previous":"PLANNING","state":"COMPLETED","blocker":null}))?;
        Ok(())
    }).unwrap().await.unwrap().unwrap();
}

async fn draft_text(runtime: &Runtime, draft: &str) -> String {
    let draft = draft.to_owned();
    runtime
        .host
        .executor
        .read(move |db| {
            Ok(db.query_row(
                "SELECT description FROM workspace_tasks WHERE task_id=?",
                [&draft],
                |r| r.get(0),
            )?)
        })
        .unwrap()
        .await
        .unwrap()
        .unwrap()
}

fn two_parts() -> Value {
    json!([{"key":"t1","title":"First","description":"Do the first part","dependsOn":[]},
        {"key":"t2","title":"Second","description":"Do the second part","dependsOn":["t1"]}])
}

async fn native_board(service: &Service, project: &str) -> Value {
    let reply = service
        .native_query(json!({"protocol":{"major":2,"mapping":2},
            "requestId":"read-planning-board","projectId":project,
            "operation":"tasks.board","payload":{"id":project}}))
        .await
        .unwrap();
    assert_eq!(reply["data"]["type"], "board");
    reply["data"]["value"]["items"].clone()
}

async fn native_board_task(service: &Service, project: &str, task: &str) -> Value {
    native_board(service, project)
        .await
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["taskId"] == task)
        .expect("task on the native board")
        .clone()
}

#[tokio::test(flavor = "multi_thread")]
async fn sprint_planner_identity_is_public_with_the_draft_before_the_sprint_link() {
    let (_scratch, service, runtime, project) = planning_fixture("planner-identity").await;
    let sprint_id = task_sprint(
        &service,
        &project,
        json!({"action":"create","description":"Plan a change"}),
    )
    .await;
    let id = sprint_id.as_str().unwrap();
    assert_eq!(native_board(&service, &project).await, json!([]));

    let create = json!({"protocol":{"major":1,"minor":8},"requestId":"planner-draft",
        "projectId":project,"actorId":actor(),"operation":"tasks.create","target":project,
        "expectedVersion":"0","ownerEpoch":null,
        "payload":{"description":"Plan a change","checkout":{"kind":"main"}}});
    // This is the driver's create step, deliberately without the later CAS or Start.
    let first = runtime
        .create_draft(project.clone(), create.clone(), Some(id.to_owned()))
        .await
        .unwrap();
    assert_eq!(first["outcome"], "recorded");
    assert!(sprint(&service, &project, &sprint_id).await["planningTaskId"].is_null());
    let draft = first["target"].as_str().unwrap();
    let row = native_board_task(&service, &project, draft).await;
    assert_eq!(row["planningSprintId"], id);
    assert_eq!(row["version"], "1");
    assert!(row["executionId"].is_null());

    let replay = runtime
        .create_draft(project.clone(), create.clone(), Some(id.to_owned()))
        .await
        .unwrap();
    assert_eq!(replay, first);
    let conflict = runtime
        .command(project.clone(), create.clone())
        .await
        .unwrap();
    assert_eq!(conflict["outcome"], "payload_conflict");

    let mut ordinary = create;
    ordinary["requestId"] = json!("ordinary-draft");
    ordinary["payload"]["planningSprintId"] = json!(id);
    let forged = runtime
        .command(project.clone(), ordinary.clone())
        .await
        .unwrap_err();
    assert_eq!(forged.to_string(), "invalid_workspace_request");
    ordinary["payload"]
        .as_object_mut()
        .unwrap()
        .remove("planningSprintId");
    let ordinary = runtime.command(project.clone(), ordinary).await.unwrap();
    assert_eq!(ordinary["outcome"], "recorded");
    let task = ordinary["target"].as_str().unwrap();
    assert!(native_board_task(&service, &project, task)
        .await
        .get("planningSprintId")
        .is_none());
    service.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sprint_plans_in_a_read_only_task_revises_on_request_and_approval_closes_it() {
    let (_scratch, service, runtime, project) = planning_fixture("sprint-task").await;
    let mut jobs = Jobs::default();
    let id = task_sprint(&service, &project,
        json!({"action":"create","description":"Build the two parts","runMode":"hybrid","checkout":{"kind":"main"}})).await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let started = sprint(&service, &project, &id).await;
    let draft = started["planningTaskId"]
        .as_str()
        .expect("a planning task")
        .to_owned();
    assert_eq!(started["planning"], "planning");
    assert_eq!(
        native_board_task(&service, &project, &draft).await["planningSprintId"],
        id
    );
    // A task in the project folder, asked to study and plan read-only.
    let text = draft_text(&runtime, &draft).await;
    assert!(text.starts_with("Sprint planning (read-only)"), "{text}");
    assert!(text.contains("Build the two parts"), "{text}");
    assert!(text.contains("sprint_plan_submit"), "{text}");
    let task = settled(&runtime, &draft).await;
    let t = task.clone();
    let bound = runtime
        .host
        .executor
        .read(move |db| Ok(cortex_core::lifecycle::sprint_planning::enabled(db, &t)?))
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert!(bound, "the planning task is bound read-only at Start");
    // It is the sprint's planning task, never one of its members.
    let board = call(&service, "product.board", &project, json!({})).await;
    assert!(board["tasks"][&draft]["sprintId"].is_null());
    assert_eq!(sprint(&service, &project, &id).await["taskIds"], json!([]));
    // A second pass while the task works changes nothing and creates nothing.
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    assert_eq!(
        sprint(&service, &project, &id).await["planningTaskId"],
        draft.as_str()
    );

    // The completed task's plan becomes the sprint's proposal.
    complete(&runtime, &task, Some(two_parts())).await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let ready = sprint(&service, &project, &id).await;
    assert_eq!(ready["planning"], "ready", "{ready}");
    assert_eq!(ready["plan"]["version"], 1);
    assert_eq!(ready["plan"]["items"].as_array().unwrap().len(), 2);
    assert_eq!(ready["plan"]["items"][1]["dependsOn"], json!(["t1"]));
    assert_eq!(ready["history"][0], ready["plan"]);
    // Nothing is created or started by planning.
    assert_eq!(ready["taskIds"], json!([]));
    assert_eq!(ready["queueState"], "idle");
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    assert_eq!(sprint(&service, &project, &id).await["plan"]["version"], 1);

    // The owner asks for changes in the planning conversation: the same task revises the plan.
    call(
        &service,
        "product.sprint",
        &project,
        json!({"action":"patch","sprintId":id,"patch":{"planning":"planning"}}),
    )
    .await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    assert_eq!(
        sprint(&service, &project, &id).await["planning"],
        "planning"
    );
    let mut revised = two_parts();
    revised.as_array_mut().unwrap().push(
        json!({"key":"t3","title":"Third","description":"Document both parts","dependsOn":["t2"]}));
    complete(&runtime, &task, Some(revised)).await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let revised = sprint(&service, &project, &id).await;
    assert_eq!(revised["planning"], "ready", "{revised}");
    assert_eq!(revised["plan"]["version"], 2);
    assert_eq!(revised["plan"]["items"].as_array().unwrap().len(), 3);
    assert_eq!(revised["history"].as_array().unwrap().len(), 2);
    // A question answered without a new plan keeps the current proposal.
    call(
        &service,
        "product.sprint",
        &project,
        json!({"action":"patch","sprintId":id,"patch":{"planning":"planning"}}),
    )
    .await;
    complete(&runtime, &task, None).await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let kept = sprint(&service, &project, &id).await;
    assert_eq!(kept["planning"], "ready", "{kept}");
    assert_eq!(kept["plan"], revised["plan"]);

    // Approval materializes the plan; the planning task closes as done.
    call(&service, "product.sprint", &project,
        json!({"action":"materialize","sprintId":id,"planVersion":2})).await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let board = call(&service, "product.board", &project, json!({})).await;
    assert_eq!(
        board["tasks"][&draft]["status"], "done",
        "{}",
        board["tasks"][&draft]
    );
    service.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_plan_is_asked_for_once_then_planning_is_cancelled_and_resumes() {
    let (_scratch, service, runtime, project) = planning_fixture("sprint-correct").await;
    let mut jobs = Jobs::default();
    let id = task_sprint(
        &service,
        &project,
        json!({"action":"create","description":"Plan the cleanup"}),
    )
    .await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let draft = sprint(&service, &project, &id).await["planningTaskId"]
        .as_str()
        .unwrap()
        .to_owned();
    let task = settled(&runtime, &draft).await;
    complete(&runtime, &task, None).await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let corrected = sprint(&service, &project, &id).await;
    assert_eq!(corrected["planning"], "planning", "{corrected}");
    assert_eq!(
        corrected["planningRun"]["outcome"], "corrected",
        "{corrected}"
    );
    assert_eq!(corrected["planningRun"]["corrections"], 1);
    // The correction went to the same task, as an ordinary message.
    let t = task.clone();
    let delivered: bool = runtime.host.executor.read(move |db| {
        Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND payload_json LIKE '%sprint plan is still missing%')",
            [&t], |r| r.get(0))?)
    }).unwrap().await.unwrap().unwrap();
    assert!(delivered, "the correction reaches the planning task");
    let again = settled(&runtime, &draft).await;
    assert_eq!(again, task, "the same planning task continues");
    complete(&runtime, &task, None).await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let cancelled = sprint(&service, &project, &id).await;
    assert_eq!(cancelled["planning"], "cancelled", "{cancelled}");
    assert!(cancelled["plan"].is_null());
    let note = cancelled["conversation"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()["text"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        note.contains("finished twice without a sprint plan"),
        "{note}"
    );
    assert!(note.contains("Resume planning"), "{note}");
    let event = cancelled["conversation"]
        .as_array()
        .unwrap()
        .last()
        .unwrap();
    assert_eq!(event["kind"], "event");
    assert!(event["at"].as_u64().is_some_and(|at| at > 0));
    assert_eq!(
        cancelled["planningFailure"],
        json!({"text":note,"step":"run",
        "code":"plan_missing","at":event["at"]})
    );
    // Another pass leaves the cancelled sprint as it is.
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    assert_eq!(
        sprint(&service, &project, &id).await["planning"],
        "cancelled"
    );
    // Resume planning asks the same task to continue.
    call(
        &service,
        "product.sprint",
        &project,
        json!({"action":"patch","sprintId":id,"patch":{"planning":"planning"}}),
    )
    .await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let resumed = sprint(&service, &project, &id).await;
    assert_eq!(resumed["planning"], "planning", "{resumed}");
    assert!(resumed.get("planningFailure").is_none());
    assert_eq!(resumed["planningRun"]["outcome"], "continued");
    assert_eq!(resumed["planningTaskId"], draft.as_str());
    settled(&runtime, &draft).await;
    complete(&runtime, &task, Some(two_parts())).await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    assert_eq!(sprint(&service, &project, &id).await["planning"], "ready");
    service.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn sprint_planning_diagnostics_preserve_saved_work_and_owner_cancel_is_neutral() {
    let (_scratch, service, runtime, project) =
        planning_fixture("sprint-diagnostics").await;
    let mut jobs = Jobs::default();
    let id = task_sprint(
        &service,
        &project,
        json!({"action":"create","description":"Keep the saved request"}),
    )
    .await;
    let sprint_id = id.as_str().unwrap();
    let plan =
        json!({"version":1,"items":[{"key":"saved","description":"Previously proposed"}]});
    service
        .sprint_patch(&project, sprint_id, json!({"plan":plan,"history":[plan]}))
        .await
        .unwrap();
    service
        .sprint_note(&project, sprint_id, "answer", "Keep this answer")
        .await
        .unwrap();
    let before = sprint(&service, &project, &id).await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, true)
        .await
        .unwrap();
    let failed = sprint(&service, &project, &id).await;
    assert_eq!(failed["planning"], "cancelled");
    let conversation = failed["conversation"].as_array().unwrap();
    assert_eq!(conversation.len(), 2);
    assert_eq!(conversation[0], before["conversation"][0]);
    assert_eq!(conversation[1]["text"], failed["planningFailure"]["text"]);
    for key in ["description", "attachments", "plan", "history"] {
        assert_eq!(failed[key], before[key], "{key}");
    }

    service
        .sprint_patch(&project, sprint_id, json!({"planning":"planning"}))
        .await
        .unwrap();
    assert!(sprint(&service, &project, &id)
        .await
        .get("planningFailure")
        .is_none());
    for field in ["planningFailure", "planningWarning"] {
        let patch = json!({(field):{"text":"Not a service diagnostic"}});
        let request = serde_json::from_value(json!({
            "op":"product.sprint","projectId":project,
            "params":{"action":"patch","sprintId":sprint_id,"patch":patch}
        }))
        .unwrap();
        let reply = service.machine(request).await.unwrap();
        assert_eq!(reply["ok"], false);
        assert_eq!(reply["code"], "invalid_sprint");
    }
    service
        .sprint_patch(&project, sprint_id, json!({"planning":"cancelled"}))
        .await
        .unwrap();
    // A failure from an in-flight pass cannot overwrite the owner's stop.
    service
        .cancel_planning(
            &project,
            sprint_id,
            "start",
            "planning_start_failed",
            "Planning did not start.",
            None,
        )
        .await
        .unwrap();
    let stopped = sprint(&service, &project, &id).await;
    assert_eq!(stopped["planning"], "cancelled");
    assert!(stopped.get("planningFailure").is_none());
    assert_eq!(stopped["conversation"], failed["conversation"]);
    service.stop().await.unwrap();
}

#[tokio::test]
async fn late_or_duplicate_plan_publication_cannot_replace_owner_state() {
    let (_scratch, service, _runtime, project) = planning_fixture("sprint-plan-publication-fence").await;
    let id = task_sprint(&service, &project, json!({"action":"create","description":"Keep the full request"})).await;
    let sprint_id = id.as_str().unwrap();
    let expected = sprint(&service, &project, &id).await;
    let items = two_parts().as_array().unwrap().clone();
    let run = json!({"runtimeTask":"fixture","handled":9,"outcome":"ready","planEvent":5});
    service.publish_planning_state(&project, sprint_id, expected.clone(), "ready", Some(items.clone()), run.clone()).await.unwrap();
    service.publish_planning_state(&project, sprint_id, expected, "ready", Some(items.clone()), run.clone()).await.unwrap();
    let ready = sprint(&service, &project, &id).await;
    assert_eq!(ready["plan"], json!({"version":1,"items":items}));
    assert_eq!(ready["history"], json!([ready["plan"]]));
    service.sprint_patch(&project, sprint_id, json!({"planning":"cancelled"})).await.unwrap();
    service.publish_planning_state(&project, sprint_id, ready.clone(), "ready", Some(items), run).await.unwrap();
    let stopped = sprint(&service, &project, &id).await;
    assert_eq!(stopped["planning"], "cancelled");
    assert_eq!(stopped["history"], ready["history"]);
    service.stop().await.unwrap();
}

#[tokio::test]
async fn a_stopped_planner_never_admits_a_stale_continuation_prompt() {
    let (_scratch, service, runtime, project) = planning_fixture("sprint-prompt-fence").await;
    let id = task_sprint(&service, &project, json!({"action":"create","description":"Produce a plan"})).await;
    let mut jobs = Jobs::default();
    service.sprint_pass(&project, &runtime, &mut jobs, false).await.unwrap();
    let observed = sprint(&service, &project, &id).await;
    let draft = observed["planningTaskId"].as_str().unwrap().to_owned();
    let task = settled(&runtime, &draft).await;
    complete(&runtime, &task, None).await;
    let fact = planning_fact(&runtime, &draft).await.unwrap();
    assert_eq!(fact.state.as_deref(), Some("COMPLETED"));
    let run = json!({"runtimeTask":task,"handled":fact.completion.unwrap()});
    service.sprint_patch(&project, id.as_str().unwrap(), json!({"planning":"cancelled"})).await.unwrap();
    let request = json!({"projectId":project,"executionId":fact.execution.unwrap(),
        "requestId":"stale-sprint-prompt","action":"prompt","text":"Do not dispatch","attachments":[]});
    let error = runtime.control_planning_prompt(
        project.clone(), request, crate::machine::task_rework::SprintPlanningPrompt { observed, run },
    ).await.unwrap_err();
    assert_eq!(error.to_string(), "planning_observation_stale");
    let completed = planning_fact(&runtime, &draft).await.unwrap();
    assert_eq!(completed.state.as_deref(), Some("COMPLETED"));
    assert_eq!(sprint(&service, &project, &id).await["planning"], "cancelled");
    service.stop().await.unwrap();
}

async fn linked_unstarted(
    service: &Service,
    runtime: &Arc<Runtime>,
    project: &str,
    id: &Value,
) -> String {
    let created = runtime.command(
        project.to_owned(),
        json!({"protocol":{"major":1,"minor":8},"requestId":"fixture-create",
            "projectId":project,"actorId":actor(),"operation":"tasks.create","target":project,
            "expectedVersion":"0","ownerEpoch":null,"payload":{"description":"Old draft"}}),
    ).await.unwrap();
    assert_eq!(created["outcome"], "recorded", "{created}");
    let draft = created["target"].as_str().unwrap().to_owned();
    let linked = draft.clone();
    service.write_planning_state(project, id.as_str().unwrap(), move |sprint| {
        sprint["planningTaskId"] = json!(linked);
        sprint["planningRun"] = json!({"outcome":"fixture"});
        Ok(true)
    }).await.unwrap();
    draft
}

#[tokio::test(flavor = "multi_thread")]
async fn planning_draft_accepts_product_close_as_cancelled() {
    let (_scratch, service, runtime, project) = planning_fixture("sprint-close").await;
    let id = task_sprint(
        &service,
        &project,
        json!({"action":"create","description":"Check closure"}),
    )
    .await;
    let task = linked_unstarted(&service, &runtime, &project, &id).await;

    call(
        &service,
        "product.task",
        &project,
        json!({"action":"close","taskId":task,"choice":"cancel","confirmed":true}),
    )
    .await;

    let board = call(&service, "product.board", &project, json!({})).await;
    assert_eq!(board["tasks"][&task]["status"], "done");
    assert_eq!(board["tasks"][&task]["closedAs"], "cancel");
    assert_eq!(draft_text(&runtime, &task).await, "Old draft");
    service.stop().await.unwrap();
}

async fn assert_retired_planner(
    service: &Service,
    project: &str,
    task: &str,
    expected_phase: &str,
) {
    let board = call(service, "product.board", project, json!({})).await;
    let record = &board["tasks"][task];
    assert_eq!(record["retiredPlanner"], true, "{record}");
    assert_eq!(record["closedAs"], "cancel");
    assert_eq!(record["status"], "done");
    assert_ne!(record["purged"], true);
    let native = native_board_task(service, project, task).await;
    assert_eq!(native["phase"], expected_phase);

    // Even a pending decision from a stale read is not an ordinary task
    // notification. This exercises the projection that feeds the bus.
    let decisions = HashMap::from([(
        task.to_owned(),
        vec![json!({"status":"pending","decisionId":"pending-fixture"})],
    )]);
    assert!(crate::notifier::items(project, &[native], &board, &decisions, &[])
        .iter()
        .all(|item| item.place.sprint || item.place.id != task));
}

async fn recover_sprint(service: &Service, project: &str, params: Value) -> Value {
    let request = serde_json::from_value(json!({
        "op":"product.sprint","projectId":project,"params":params
    }))
    .unwrap();
    service.machine(request).await.unwrap()
}

async fn started_draft(
    service: &Service,
    runtime: &Runtime,
    project: &str,
    id: &Value,
) -> String {
    let saved = sprint(service, project, id).await;
    let task = saved["planningTaskId"].as_str().expect("linked draft");
    assert!(planning_fact(runtime, task)
        .await
        .unwrap()
        .execution
        .is_some());
    task.to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn refused_planning_start_unlinks_the_draft_and_owner_retry_starts_a_new_one() {
    let (_scratch, service, runtime, project) = planning_fixture("sprint-retry").await;
    let mut jobs = Jobs::default();
    let id = task_sprint(
        &service,
        &project,
        json!({"action":"create","description":"Current owner description"}),
    )
    .await;
    let old = linked_unstarted(&service, &runtime, &project, &id).await;

    // Start with an outdated version: the runtime refuses without committing execution.
    let refused = runtime
        .start_sprint_planning(
            project.clone(),
            json!({"protocol":{"major":1,"minor":8},"requestId":"fixture-start-stale",
            "projectId":project,"actorId":actor(),"operation":"tasks.start","target":old,
            "expectedVersion":"2","ownerEpoch":null,"payload":{}}),
        )
        .await;
    assert_eq!(refused.as_ref().unwrap_err().to_string(), "stale_version");
    service
        .finish_planning_start(&project, &runtime, id.as_str().unwrap(), &old, refused)
        .await
        .unwrap();

    let failed = sprint(&service, &project, &id).await;
    assert_eq!(failed["planning"], "cancelled");
    assert!(failed["planningTaskId"].is_null());
    assert!(failed["planningRun"].is_null());
    assert_eq!(failed["planningFailure"]["code"], "planning_start_failed");
    assert_retired_planner(&service, &project, &old, "not_started").await;
    assert_eq!(
        failed["conversation"][0]["text"],
        failed["planningFailure"]["text"]
    );
    assert_eq!(draft_text(&runtime, &old).await, "Old draft");
    assert!(planning_fact(&runtime, &old)
        .await
        .unwrap()
        .execution
        .is_none());
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    assert_eq!(
        sprint(&service, &project, &id).await,
        failed,
        "no automatic retry"
    );

    call(
        &service,
        "product.sprint",
        &project,
        json!({"action":"note","sprintId":id,
        "kind":"answer","text":"Use option A"}),
    )
    .await;
    call(
        &service,
        "product.sprint",
        &project,
        json!({"action":"patch","sprintId":id,
        "patch":{"planning":"planning"}}),
    )
    .await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let new = started_draft(&service, &runtime, &project, &id).await;
    assert_ne!(new, old);
    let text = draft_text(&runtime, &new).await;
    assert!(
        text.contains("Current owner description")
            && text.contains("Owner answer: Use option A")
    );
    assert!(sprint(&service, &project, &id).await["planningFailure"].is_null());
    service.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn retry_recovers_an_old_failed_sprint_with_a_linked_unstarted_draft() {
    let (_scratch, service, runtime, project) = planning_fixture("sprint-stuck").await;
    let mut jobs = Jobs::default();
    let id = task_sprint(
        &service,
        &project,
        json!({"action":"create","description":"Legacy sprint"}),
    )
    .await;
    let old = linked_unstarted(&service, &runtime, &project, &id).await;
    service
        .cancel_planning(
            &project,
            id.as_str().unwrap(),
            "start",
            "planning_start_refused",
            "Legacy failure.",
            None,
        )
        .await
        .unwrap();
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    assert_eq!(
        sprint(&service, &project, &id).await["planningTaskId"],
        old.as_str()
    );

    for _ in 0..2 {
        call(
            &service,
            "product.sprint",
            &project,
            json!({"action":"patch","sprintId":id,
            "patch":{"planning":"planning"}}),
        )
        .await;
    }
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let fresh = started_draft(&service, &runtime, &project, &id).await;
    assert_ne!(fresh, old);
    assert_retired_planner(&service, &project, &old, "not_started").await;
    assert_eq!(draft_text(&runtime, &old).await, "Old draft");
    assert!(planning_fact(&runtime, &old)
        .await
        .unwrap()
        .execution
        .is_none());
    assert!(sprint(&service, &project, &id).await["planningRetry"].is_null());
    service.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn planning_message_recovers_a_failed_sprint_and_passes_the_saved_note() {
    let (_scratch, service, runtime, project) = planning_fixture("sprint-change").await;
    let mut jobs = Jobs::default();
    let id = task_sprint(
        &service,
        &project,
        json!({"action":"create","description":"Keep the original"}),
    )
    .await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let old = started_draft(&service, &runtime, &project, &id).await;
    let runtime_task = settled(&runtime, &old).await;
    runtime.host.executor.write(move |db| {
        db.execute("UPDATE pending_questions SET status='cancelled' WHERE task_id=? AND status='open'", [&runtime_task])?;
        db.execute("UPDATE runtime_tasks SET state='FAILED',blocker='fixture_failure' WHERE task_id=?", [&runtime_task])?;
        cortex_core::lifecycle::event_tx(db, &runtime_task, "TASK_STATE_CHANGED", json!({"state":"FAILED","blocker":"fixture_failure"}))?;
        Ok(())
    }).unwrap().await.unwrap().unwrap();
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let failed = sprint(&service, &project, &id).await;
    assert_eq!(failed["planningTaskId"], old.as_str());
    assert_eq!(failed["planning"], "cancelled");
    let params = json!({"action":"recover_planning_message","sprintId":id,
        "text":"Use the smaller version", "attachments":[]});
    let reply = recover_sprint(&service, &project, params.clone()).await;
    assert_eq!(reply["result"], json!({"done":true}), "{reply}");
    let accepted = sprint(&service, &project, &id).await;
    assert_eq!(accepted["planning"], "planning");
    assert!(accepted["planningTaskId"].is_null());
    assert!(accepted["planningFailure"].is_null());
    assert_retired_planner(&service, &project, &old, "failed").await;
    let repeated = recover_sprint(&service, &project, params).await;
    assert_eq!(repeated["code"], "planning_recovery_unavailable");
    assert_eq!(sprint(&service, &project, &id).await, accepted);

    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let fresh = started_draft(&service, &runtime, &project, &id).await;
    assert_ne!(fresh, old);
    let text = draft_text(&runtime, &fresh).await;
    assert!(
        text.contains("Keep the original")
            && text.contains("Owner message: Use the smaller version")
    );
    service.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn recovery_message_refuses_to_replace_a_live_planner() {
    let (_scratch, service, runtime, project) = planning_fixture("sprint-live-guard").await;
    let mut jobs = Jobs::default();
    let id = task_sprint(
        &service,
        &project,
        json!({"action":"create","description":"Keep the live task"}),
    )
    .await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let task = started_draft(&service, &runtime, &project, &id).await;
    let runtime_task = settled(&runtime, &task).await;
    runtime
        .host
        .executor
        .write(move |db| {
            db.execute(
                "UPDATE runtime_tasks SET state='PLANNING' WHERE task_id=?",
                [&runtime_task],
            )?;
            Ok(())
        })
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    service
        .cancel_planning(
            &project,
            id.as_str().unwrap(),
            "continue",
            "planning_continue_failed",
            "Fixture failure.",
            None,
        )
        .await
        .unwrap();
    let before = sprint(&service, &project, &id).await;
    let reply = recover_sprint(
        &service,
        &project,
        json!({"action":"recover_planning_message","sprintId":id,"text":"Do not detach"}),
    )
    .await;
    assert_eq!(reply["code"], "planning_execution_live", "{reply}");
    assert_eq!(sprint(&service, &project, &id).await, before);
    service.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn replacing_an_overlong_request_preserves_saved_work_and_starts_planning() {
    let (_scratch, service, runtime, project) =
        planning_fixture("sprint-edit-request").await;
    let mut jobs = Jobs::default();
    let files = call(&service, "product.attachment", &project, json!({
        "action":"upload","files":[{"name":"owner.txt","mimeType":"text/plain","data":"YQ=="}]
    })).await;
    let id = task_sprint(
        &service,
        &project,
        json!({"action":"create",
        "description":format!("old request {}", "x".repeat(31_900)), "attachments":files}),
    )
    .await;
    call(
        &service,
        "product.sprint",
        &project,
        json!({"action":"note","sprintId":id,
        "kind":"answer","text":"Preserved answer"}),
    )
    .await;
    let plan = json!({"version":1,"items":two_parts()});
    call(
        &service,
        "product.sprint",
        &project,
        json!({"action":"patch","sprintId":id,
        "patch":{"plan":plan,"history":[plan]}}),
    )
    .await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let failed = sprint(&service, &project, &id).await;
    assert_eq!(
        failed["planningFailure"]["code"],
        "planning_request_invalid"
    );
    assert!(failed["planningFailure"]["text"]
        .as_str()
        .unwrap()
        .contains("limit 32000"));

    // An older failure may still link a draft without a native planner marker.
    let old = linked_unstarted(&service, &runtime, &project, &id).await;
    let failed = sprint(&service, &project, &id).await;
    let reply = recover_sprint(
        &service,
        &project,
        json!({
            "action":"replace_planning_request","sprintId":id,"description":" "
        }),
    )
    .await;
    assert_eq!(reply["code"], "invalid_sprint");
    assert_eq!(sprint(&service, &project, &id).await, failed);

    let reply = recover_sprint(&service, &project, json!({
        "action":"replace_planning_request","sprintId":id,"description":"Short new request"
    })).await;
    assert_eq!(reply["result"], json!({"done":true}), "{reply}");
    let edited = sprint(&service, &project, &id).await;
    assert_eq!(edited["description"], "Short new request");
    assert!(edited["planningFailure"].is_null());
    assert_retired_planner(&service, &project, &old, "not_started").await;
    for key in ["attachments", "conversation", "plan", "history"] {
        assert_eq!(edited[key], failed[key], "{key}");
    }

    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let task = started_draft(&service, &runtime, &project, &id).await;
    let text = draft_text(&runtime, &task).await;
    assert!(
        text.contains("Short new request")
            && text.contains("Owner answer: Preserved answer")
    );
    assert!(!text.contains("old request"));
    assert_eq!(
        runtime.draft_attachments(project, task).await.unwrap()[0]["name"],
        "owner.txt"
    );
    service.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn ambiguous_planning_start_keeps_a_committed_execution_link() {
    let (_scratch, service, runtime, project) = planning_fixture("sprint-lost-reply").await;
    let mut jobs = Jobs::default();
    let id = task_sprint(
        &service,
        &project,
        json!({"action":"create","description":"Started planning"}),
    )
    .await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let draft = started_draft(&service, &runtime, &project, &id).await;

    // Drive the same readback used on a lost Start reply after a real commit.
    service
        .finish_planning_start(
            &project,
            &runtime,
            id.as_str().unwrap(),
            &draft,
            Err(crate::failure("native_host_closed")),
        )
        .await
        .unwrap();
    let saved = sprint(&service, &project, &id).await;
    assert_eq!(saved["planningTaskId"], draft.as_str());
    assert_eq!(saved["planning"], "planning");
    assert!(saved["planningFailure"].is_null());
    let board = call(&service, "product.board", &project, json!({})).await;
    assert!(board["tasks"][&draft]["retiredPlanner"].is_null());

    let runtime_task = settled(&runtime, &draft).await;
    complete(&runtime, &runtime_task, Some(two_parts())).await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    assert_eq!(sprint(&service, &project, &id).await["planning"], "ready");
    assert_eq!(
        sprint(&service, &project, &id).await["planningTaskId"],
        draft.as_str()
    );
    service.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn sprint_planning_warning_is_once_per_streak_and_clears_on_success() {
    let (_scratch, service, runtime, project) = planning_fixture("sprint-warning").await;
    let mut jobs = Jobs::default();
    let id = task_sprint(
        &service,
        &project,
        json!({"action":"create","description":"Check a stalled sprint"}),
    )
    .await;
    let sprint_id = id.as_str().unwrap();
    service
        .sprint_patch(&project, sprint_id, json!({"planning":"question"}))
        .await
        .unwrap();
    // Inject pass results, while exercising the real warning writer and board read.
    for attempt in 1..=5 {
        let snapshot = sprint(&service, &project, &id).await;
        service
            .observe_planning_pass(
                &project,
                &snapshot,
                jobs.1.entry(project.clone()).or_default(),
                Err(crate::failure("planning_unavailable")),
            )
            .await;
        let saved = sprint(&service, &project, &id).await;
        assert_eq!(saved["planning"], "question");
        assert_eq!(
            saved["conversation"].as_array().unwrap().len(),
            usize::from(attempt >= 3)
        );
        if attempt >= 3 {
            let note = &saved["conversation"][0];
            assert_eq!(note["kind"], "event");
            assert!(note["at"].as_u64().is_some_and(|at| at > 0));
            assert_eq!(
                saved["planningWarning"],
                json!({"text":note["text"],"step":"internal",
                "code":"planning_pass_failed","at":note["at"],"count":3})
            );
        } else {
            assert!(saved.get("planningWarning").is_none());
        }
    }
    // This older saved sprint has no planning task; a real pass succeeds without starting one.
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let recovered = sprint(&service, &project, &id).await;
    assert!(recovered.get("planningWarning").is_none());
    assert_eq!(recovered["planning"], "question");
    assert_eq!(recovered["conversation"].as_array().unwrap().len(), 1);
    assert!(!jobs.1[&project].contains_key(sprint_id));
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    assert_eq!(sprint(&service, &project, &id).await, recovered);
    service.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_planning_warning_write_is_retried_while_the_streak_continues() {
    let (scratch, service, runtime, project) =
        planning_fixture("sprint-warning-retry").await;
    let mut jobs = Jobs::default();
    let id = task_sprint(
        &service,
        &project,
        json!({"action":"create","description":"Check a stalled sprint"}),
    )
    .await;
    let sprint_id = id.as_str().unwrap();
    service
        .sprint_patch(&project, sprint_id, json!({"planning":"question"}))
        .await
        .unwrap();
    let product_path = scratch
        .0
        .join("state/product")
        .join(format!("{project}.json"));
    for attempt in 1..=6 {
        let snapshot = sprint(&service, &project, &id).await;
        // The third pass's warning write meets an unreadable store, which is
        // restored before the next pass while planning passes keep failing.
        let saved_bytes = (attempt == 3).then(|| {
            let bytes = std::fs::read(&product_path).unwrap();
            std::fs::write(&product_path, b"{").unwrap();
            bytes
        });
        service
            .observe_planning_pass(
                &project,
                &snapshot,
                jobs.1.entry(project.clone()).or_default(),
                Err(crate::failure("planning_unavailable")),
            )
            .await;
        if let Some(bytes) = saved_bytes {
            std::fs::write(&product_path, bytes).unwrap();
        }
        let saved = sprint(&service, &project, &id).await;
        assert_eq!(saved["planning"], "question");
        let events = saved["conversation"].as_array().unwrap();
        if attempt <= 3 {
            assert!(events.is_empty(), "pass {attempt}");
            assert!(saved.get("planningWarning").is_none(), "pass {attempt}");
        } else {
            // Written on the next failed pass, once for the whole streak.
            assert_eq!(events.len(), 1, "pass {attempt}");
            assert_eq!(events[0]["kind"], "event");
            assert_eq!(
                saved["planningWarning"],
                json!({"text":events[0]["text"],"step":"internal",
                "code":"planning_pass_failed","at":events[0]["at"],"count":3})
            );
        }
    }
    // A successful pass clears the active warning; the event stays.
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let recovered = sprint(&service, &project, &id).await;
    assert!(recovered.get("planningWarning").is_none());
    assert_eq!(recovered["conversation"].as_array().unwrap().len(), 1);
    assert!(!jobs.1[&project].contains_key(sprint_id));
    service.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_old_saved_sprint_resumes_into_a_planning_task_and_close_stops_it() {
    let (_scratch, service, runtime, project) = planning_fixture("sprint-old").await;
    let mut jobs = Jobs::default();
    // Saved by the old version: a description-only planning run without a task,
    // interrupted by the restart. It is never silently rerun.
    let id = task_sprint(
        &service,
        &project,
        json!({"action":"create","description":"Old sprint"}),
    )
    .await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, true)
        .await
        .unwrap();
    let old = sprint(&service, &project, &id).await;
    assert_eq!(old["planning"], "cancelled");
    assert_eq!(old["planningFailure"]["step"], "restart");
    assert_eq!(old["planningFailure"]["code"], "planning_interrupted");
    assert!(old["planningTaskId"].is_null());
    assert!(old["conversation"]
        .to_string()
        .contains("Planning was interrupted"));
    // An old open planning question waits for the owner; nothing runs for it.
    call(&service, "product.sprint", &project,
        json!({"action":"note","sprintId":id,"kind":"question","text":"Which part first?","options":["A","B"]})).await;
    call(
        &service,
        "product.sprint",
        &project,
        json!({"action":"patch","sprintId":id,"patch":{"planning":"question"}}),
    )
    .await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    assert!(sprint(&service, &project, &id).await["planningTaskId"].is_null());
    // The owner answers as before: the answer is kept and planning resumes in a task.
    call(
        &service,
        "product.sprint",
        &project,
        json!({"action":"note","sprintId":id,"kind":"answer","text":"Part A first"}),
    )
    .await;
    call(
        &service,
        "product.sprint",
        &project,
        json!({"action":"patch","sprintId":id,"patch":{"planning":"planning"}}),
    )
    .await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let resumed = sprint(&service, &project, &id).await;
    let draft = resumed["planningTaskId"]
        .as_str()
        .expect("a planning task")
        .to_owned();
    assert_eq!(
        native_board_task(&service, &project, &draft).await["planningSprintId"],
        id
    );
    let text = draft_text(&runtime, &draft).await;
    assert!(
        text.contains("Old sprint") && text.contains("Owner answer: Part A first"),
        "{text}"
    );
    assert!(text.contains("Question: Which part first?"), "{text}");
    // Closing the sprint stops its planning task through the execution control.
    let task = settled(&runtime, &draft).await;
    runtime
        .host
        .executor
        .write({
            let task = task.clone();
            move |db| {
                db.execute(
                    "UPDATE runtime_tasks SET state='PLANNING' WHERE task_id=?",
                    [&task],
                )?;
                Ok(())
            }
        })
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    call(
        &service,
        "product.sprint",
        &project,
        json!({"action":"close","sprintId":id,"confirmed":true}),
    )
    .await;
    assert!(!sprint(&service, &project, &id).await["archivedAt"].is_null());
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let t = task.clone();
    let stopped: bool = runtime.host.executor.read(move |db| {
        Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type LIKE '%PAUSE%')
            OR EXISTS(SELECT 1 FROM task_stopped WHERE task_id=?)", [&t, &t], |r| r.get(0))?)
    }).unwrap().await.unwrap().unwrap();
    assert!(stopped, "closing the sprint stops its planning task");
    service.stop().await.unwrap();
}

/// One `sprint_plan_submit` call of the planning coordinator, after channel checks.
async fn submit(runtime: &Runtime, task: &str, args: Value) -> Value {
    let task = task.to_owned();
    runtime
        .host
        .executor
        .write(move |db| {
            crate::native_sprint_plan::submit_in_tx(db, &task, "coordinator-channel", &args)
        })
        .unwrap()
        .await
        .unwrap()
        .unwrap()["structuredContent"]
        .clone()
}


#[test]
fn sprint_plan_large_request_prompt_directs_ordered_parts_without_prose() {
    let request = planning_request(
        &json!({"description":"Plan at least fifty-five items",
            "conversation":[{"kind":"answer","text":"Keep the shared dependency"}]}),
        None,
    )
    .unwrap();
    for fragment in [
        "submit small ordered parts as you work",
        "part 1, 2, and so on",
        "final:false on each intermediate part and final:true on the last",
        "advertised limits",
        "scope, dependencies, owner decisions and acceptance criteria",
        "Do not echo the full plan in prose.",
        "Owner answer: Keep the shared dependency",
    ] {
        assert!(request.contains(fragment), "missing {fragment}");
    }
    assert!(!request.contains("Retry strategy:"));
}

async fn sprint_plan_set_state(
    runtime: &Runtime,
    task: &str,
    state: &'static str,
    blocker: Option<&'static str>,
) {
    let task = task.to_owned();
    runtime.host.executor.write(move |db| {
        let previous: String = db.query_row(
            "SELECT state FROM runtime_tasks WHERE task_id=?", [&task], |r| r.get(0),
        )?;
        db.execute(
            "UPDATE pending_questions SET status='cancelled' WHERE task_id=? AND status='open'",
            [&task],
        )?;
        db.execute("UPDATE runtime_tasks SET state=?,blocker=? WHERE task_id=?",
            (state, blocker, &task))?;
        cortex_core::lifecycle::event_tx(db, &task, "TASK_STATE_CHANGED",
            json!({"state":state,"previous":previous,"blocker":blocker}))?;
        Ok(())
    }).unwrap().await.unwrap().unwrap();
}

async fn sprint_plan_reject_large_part(runtime: &Runtime, task: &str) {
    sprint_plan_set_state(runtime, task, "PLANNING", None).await;
    let tasks: Vec<_> = (0..32).map(|n| json!({
        "key":format!("t{n}"),"title":format!("Item {n}"),"description":"x".repeat(4096),
    })).collect();
    let refusal = submit(runtime, task,
        json!({"request_key":"oversize","part":1,"final":false,"tasks":tasks})).await;
    assert_eq!(refusal["code"], "plan_part_too_large");
}

#[tokio::test(flavor = "multi_thread")]
async fn sprint_plan_unknown_failure_stays_unknown() {
    let (_scratch, service, runtime, project) = planning_fixture("sprint-unknown").await;
    let mut jobs = Jobs::default();
    let id = task_sprint(&service, &project,
        json!({"action":"create","description":"Plan a sprint"})).await;
    service.sprint_pass(&project, &runtime, &mut jobs, false).await.unwrap();
    let draft = started_draft(&service, &runtime, &project, &id).await;
    let task = settled(&runtime, &draft).await;
    // A later accepted part supersedes an earlier size refusal. A raw
    // blocker containing timeout-like text is never projected verbatim.
    sprint_plan_reject_large_part(&runtime, &task).await;
    let accepted = submit(&runtime, &task,
        json!({"request_key":"corrected","part":1,"final":false,"tasks":two_parts()})).await;
    assert_eq!(accepted["recorded"], true);
    sprint_plan_set_state(&runtime, &task, "FAILED", None).await;
    assert_eq!(planning_fact(&runtime, &draft).await.unwrap().failure, PlanningFailure::Unknown);
    sprint_plan_set_state(&runtime, &task, "FAILED", Some("timeout: raw engine detail")).await;
    service.sprint_pass(&project, &runtime, &mut jobs, false).await.unwrap();
    let saved = sprint(&service, &project, &id).await;
    assert_eq!(saved["planningFailure"]["kind"], "unknown");
    assert_eq!(saved["planningFailure"]["recommendedAction"], "retry");
    assert_eq!(saved["planningFailure"]["consecutiveLimitReplacementFailures"], 0);
    let text = saved["planningFailure"]["text"].as_str().unwrap();
    assert!(text.contains("without a known cause") && !text.contains("timeout"), "{text}");
    service.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn sprint_plan_terminal_failure_reason_is_classified_from_exact_runtime_blockers() {
    let (_scratch, service, runtime, project) = planning_fixture("sprint-blockers").await;
    let mut jobs = Jobs::default();
    let id = task_sprint(&service, &project,
        json!({"action":"create","description":"Plan a sprint"})).await;
    service.sprint_pass(&project, &runtime, &mut jobs, false).await.unwrap();
    let draft = started_draft(&service, &runtime, &project, &id).await;
    let task = settled(&runtime, &draft).await;
    sprint_plan_set_state(&runtime, &task, "FAILED", Some("scheduler_capacity_timeout")).await;
    assert_eq!(planning_fact(&runtime, &draft).await.unwrap().failure, PlanningFailure::CapacityTimeout);
    sprint_plan_reject_large_part(&runtime, &task).await;
    // Explicit owner action outranks the plan refusal.
    let t = task.clone();
    runtime.host.executor.write(move |db| {
        cortex_core::lifecycle::domain::cancel_task_tx(db, &t, "user_force_finished")
    }).unwrap().await.unwrap().unwrap();
    service.sprint_pass(&project, &runtime, &mut jobs, false).await.unwrap();
    let saved = sprint(&service, &project, &id).await;
    assert_eq!(saved["planningFailure"]["kind"], "owner_stopped");
    assert_eq!(saved["planningFailure"]["text"], "You stopped the planning task.");
    service.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn sprint_plan_retry_changes_strategy_and_repeated_limit_recommends_change_request() {
    let (_scratch, service, runtime, project) = planning_fixture("sprint-limit-retry").await;
    let mut jobs = Jobs::default();
    let id = task_sprint(&service, &project,
        json!({"action":"create","description":"Keep the owner's large request"})).await;
    service.sprint_pass(&project, &runtime, &mut jobs, false).await.unwrap();
    let draft = started_draft(&service, &runtime, &project, &id).await;
    let task = settled(&runtime, &draft).await;
    service.sprint_note(&project, id.as_str().unwrap(), "answer", "Keep option A").await.unwrap();
    sprint_plan_reject_large_part(&runtime, &task).await;
    sprint_plan_set_state(&runtime, &task, "FAILED", None).await;
    service.sprint_pass(&project, &runtime, &mut jobs, false).await.unwrap();
    let failed = sprint(&service, &project, &id).await;
    assert_eq!(failed["planning"], "cancelled");
    assert_eq!(failed["planningFailure"]["kind"], "limit");
    assert_eq!(failed["planningFailure"]["code"], "plan_part_too_large");
    assert_eq!(failed["planningFailure"]["recommendedAction"], "retry");
    assert_eq!(failed["planningFailure"]["consecutiveLimitReplacementFailures"], 0);

    let retry = json!({"action":"patch","sprintId":id,"patch":{"planning":"planning"}});
    assert_eq!(recover_sprint(&service, &project, retry.clone()).await["ok"], true);
    service.sprint_pass(&project, &runtime, &mut jobs, false).await.unwrap();
    let replacement = started_draft(&service, &runtime, &project, &id).await;
    assert_ne!(draft, replacement);
    let new_text = draft_text(&runtime, &replacement).await;
    assert!(new_text.contains(PlanningFailure::PartSize.reason()));
    assert!(new_text.contains("from the outset, even if the whole plan might fit in one call"));
    assert!(new_text.contains("Owner answer: Keep option A"));
    let new_task = settled(&runtime, &replacement).await;
    sprint_plan_reject_large_part(&runtime, &new_task).await;
    sprint_plan_set_state(&runtime, &new_task, "FAILED", None).await;
    service.sprint_pass(&project, &runtime, &mut jobs, false).await.unwrap();
    let repeated = sprint(&service, &project, &id).await;
    assert_eq!(repeated["planning"], "cancelled");
    assert_eq!(repeated["planningFailure"]["recommendedAction"], "change_request");
    assert_eq!(repeated["planningFailure"]["consecutiveLimitReplacementFailures"], 1);
    let refusal = recover_sprint(&service, &project, retry).await;
    assert_eq!(refusal["code"], "planning_change_request_required");
    service.sprint_pass(&project, &runtime, &mut jobs, false).await.unwrap();
    assert_eq!(sprint(&service, &project, &id).await, repeated);

    let unchanged = recover_sprint(&service, &project,
        json!({"action":"replace_planning_request","sprintId":id,"description":"Keep the owner's large request"})).await;
    assert_eq!(unchanged["code"], "planning_change_request_required");
    let change = recover_sprint(&service, &project,
        json!({"action":"replace_planning_request","sprintId":id,"description":"Plan only the first milestone"})).await;
    assert_eq!(change["ok"], true, "{change}");
    service.sprint_pass(&project, &runtime, &mut jobs, false).await.unwrap();
    let changed = started_draft(&service, &runtime, &project, &id).await;
    assert_ne!(changed, replacement);
    let text = draft_text(&runtime, &changed).await;
    assert!(text.contains("Plan only the first milestone"));
    assert!(!text.contains("Retry strategy:"));
    service.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_120_task_plan_in_three_parts_is_assembled_validated_reviewed_and_approved() {
    let (_scratch, service, runtime, project) = planning_fixture("sprint-parts").await;
    let mut jobs = Jobs::default();
    let id = task_sprint(&service, &project,
        json!({"action":"create","description":"A large sprint","runMode":"manual","checkout":{"kind":"main"}})).await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let draft = sprint(&service, &project, &id).await["planningTaskId"]
        .as_str()
        .unwrap()
        .to_owned();
    let task = settled(&runtime, &draft).await;
    // The coordinator's turn: planning again, no worker running.
    let t = task.clone();
    runtime.host.executor.write(move |db| {
        db.execute("UPDATE pending_questions SET status='cancelled' WHERE task_id=? AND status='open'", [&t])?;
        db.execute("UPDATE runtime_tasks SET state='PLANNING',blocker=NULL WHERE task_id=?", [&t])?;
        Ok(())
    }).unwrap().await.unwrap().unwrap();
    // Items t1..t120; t10 depends on t90, which only the third part defines.
    let part = |range: std::ops::RangeInclusive<usize>, cycle: bool| -> Value {
        range.map(|n| {
            let deps = match n {
                10 => json!(["t90"]),
                90 if cycle => json!(["t10"]),
                n if n > 1 && n != 11 => json!([format!("t{}", n - 1)]),
                _ => json!([]),
            };
            json!({"key":format!("t{n}"),"title":format!("Item {n}"),"description":format!("Do item {n}"),"dependsOn":deps})
        }).collect::<Vec<_>>().into()
    };
    let first = submit(
        &runtime,
        &task,
        json!({"request_key":"p1","part":1,"final":false,"tasks":part(1..=40, false)}),
    )
    .await;
    assert_eq!(
        first,
        json!({"recorded":true,"part":1,"final":false,"tasks":40})
    );
    let early = submit(&runtime, &task, json!({"request_key":"p3-early","part":3,"final":true,"tasks":part(81..=120, false)})).await;
    assert_eq!(early["code"], "sprint_plan_part_out_of_order", "{early}");
    let second = submit(
        &runtime,
        &task,
        json!({"request_key":"p2","part":2,"final":false,"tasks":part(41..=80, false)}),
    )
    .await;
    assert_eq!(second["tasks"], 80);
    // A key repeated across parts and a cycle across parts are refused, naming the task.
    let mut repeated = part(81..=120, false);
    repeated[0]["key"] = json!("t5");
    let duplicate = submit(
        &runtime,
        &task,
        json!({"request_key":"p3-dup","part":3,"final":true,"tasks":repeated}),
    )
    .await;
    assert_eq!(
        (duplicate["code"].as_str(), duplicate["detail"].as_str()),
        (Some("invalid_task"), Some("task 81 (t5): duplicate key"))
    );
    let cycle = submit(&runtime, &task, json!({"request_key":"p3-cycle","part":3,"final":true,"tasks":part(81..=120, true)})).await;
    assert_eq!(cycle["code"], "invalid_dependencies", "{cycle}");
    assert_eq!(
        cycle["detail"],
        "task 10 (t10) is part of or blocked by a dependency cycle"
    );
    // The corrected final part completes the whole plan and schedules its review.
    let done = submit(
        &runtime,
        &task,
        json!({"request_key":"p3","part":3,"final":true,"tasks":part(81..=120, false)}),
    )
    .await;
    assert_eq!(
        done,
        json!({"recorded":true,"tasks":120,"review":"scheduled"})
    );
    let t = task.clone();
    let (state, objective): (String, String) = runtime.host.executor.read(move |db| {
        Ok(db.query_row("SELECT t.state,n.objective FROM runtime_tasks t JOIN runtime_nodes n USING(task_id)
            WHERE t.task_id=? AND n.node_id='_plan_review_1'", [&t], |r| Ok((r.get(0)?, r.get(1)?)))?)
    }).unwrap().await.unwrap().unwrap();
    assert_eq!(state, "REVIEWING");
    assert!(
        objective.contains("\"t1\"") && objective.contains("\"t120\""),
        "the review covers the whole plan"
    );
    // A later part without a first part belongs to no plan.
    let orphan = submit(
        &runtime,
        &task,
        json!({"request_key":"orphan","part":2,"final":true,"tasks":part(1..=1, false)}),
    )
    .await;
    assert_eq!(orphan["code"], "sprint_plan_part_out_of_order");
    // Approved, verified and completed: the sprint proposes all 120 tasks.
    complete(&runtime, &task, None).await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let ready = sprint(&service, &project, &id).await;
    assert_eq!(ready["planning"], "ready", "{}", ready["conversation"]);
    let items = ready["plan"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 120);
    assert_eq!(items[9]["dependsOn"], json!(["t90"]));
    assert_eq!(items[119]["key"], "t120");
    call(&service, "product.sprint", &project,
        json!({"action":"materialize","sprintId":id,"planVersion":1})).await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let board = call(&service, "product.board", &project, json!({})).await;
    assert_eq!(board["tasks"][&draft]["status"], "done");
    service.stop().await.unwrap();
}

#[test]
fn planning_steps_follow_the_planning_task() {
    let fact = |state: &str, completion: i64, plan: Option<(i64, Value)>| PlanningFact {
        execution: Some("e".into()),
        runtime_task: Some("rt".into()),
        state: Some(state.into()),
        failure: PlanningFailure::Unknown,
        completion: Some(completion),
        plan,
    };
    let base = json!({"description":"Two parts","conversation":[],"planning":"planning","plan":null});
    // Still working: nothing to do.
    assert_eq!(
        planning_step(&base, &fact("EXECUTING", 0, None)),
        PlanningStep::Wait
    );
    // Finished with a valid plan.
    let PlanningStep::Ready { items, run } =
        planning_step(&base, &fact("COMPLETED", 9, Some((5, two_parts()))))
    else {
        panic!()
    };
    assert_eq!(items.len(), 2);
    assert_eq!(
        (run["handled"].clone(), run["planEvent"].clone()),
        (json!(9), json!(5))
    );
    // An invalid plan (only reachable from older data) is never offered: precise reason.
    let cycle = json!([{"key":"a","description":"A","dependsOn":["b"]},{"key":"b","description":"B","dependsOn":["a"]}]);
    let PlanningStep::Cancel { note, .. } =
        planning_step(&base, &fact("COMPLETED", 9, Some((5, cycle))))
    else {
        panic!()
    };
    assert_eq!(note, "The planning task submitted an invalid plan (invalid_dependencies): task 1 (a) is part of or blocked by a dependency cycle.");
    // Finished without a plan: one correction, then cancellation.
    let PlanningStep::Correct { run, .. } =
        planning_step(&base, &fact("COMPLETED", 9, None))
    else {
        panic!()
    };
    let mut corrected = base.clone();
    corrected["planningRun"] = run;
    assert_eq!(
        planning_step(&corrected, &fact("COMPLETED", 9, None)),
        PlanningStep::Wait
    );
    let PlanningStep::Cancel { run, .. } =
        planning_step(&corrected, &fact("COMPLETED", 12, None))
    else {
        panic!()
    };
    let mut cancelled = corrected.clone();
    cancelled["planningRun"] = run;
    cancelled["planning"] = json!("cancelled");
    assert_eq!(
        planning_step(&cancelled, &fact("COMPLETED", 12, None)),
        PlanningStep::Wait
    );
    cancelled["planning"] = json!("planning");
    assert!(matches!(
        planning_step(&cancelled, &fact("COMPLETED", 12, None)),
        PlanningStep::Continue { .. }
    ));
    // A consumed plan is not offered twice; a turn without a new plan keeps the proposal.
    let mut ready = base.clone();
    ready["plan"] = json!({"version":1,"items":two_parts()});
    ready["planningRun"] = json!({"runtimeTask":"rt","handled":9,"outcome":"ready","corrections":0,"planEvent":5});
    assert_eq!(
        planning_step(&ready, &fact("COMPLETED", 9, Some((5, two_parts())))),
        PlanningStep::Wait
    );
    assert!(matches!(
        planning_step(&ready, &fact("COMPLETED", 14, Some((5, two_parts())))),
        PlanningStep::Keep { .. }
    ));
    assert!(matches!(
        planning_step(&ready, &fact("COMPLETED", 14, Some((13, two_parts())))),
        PlanningStep::Ready { .. }
    ));
    // A stopped task cancels planning once; Resume planning then starts a new task.
    let PlanningStep::Terminal { run, .. } =
        planning_step(&base, &fact("FAILED", 0, None))
    else {
        panic!()
    };
    let mut stopped = base.clone();
    stopped["planningRun"] = run;
    stopped["planning"] = json!("cancelled");
    assert_eq!(
        planning_step(&stopped, &fact("FAILED", 0, None)),
        PlanningStep::Wait
    );
    stopped["planning"] = json!("planning");
    assert!(matches!(
        planning_step(&stopped, &fact("FAILED", 0, None)),
        PlanningStep::Replace { .. }
    ));
    // A new execution of the task is observed afresh.
    let mut other = ready.clone();
    other["planningRun"]["runtimeTask"] = json!("old");
    assert!(matches!(
        planning_step(&other, &fact("COMPLETED", 9, Some((5, two_parts())))),
        PlanningStep::Ready { .. }
    ));
}

#[test]
fn a_sole_planned_task_keeps_the_owner_requirements() {
    let sprint = json!({"description":"Ask me before choosing the database.",
        "conversation":[{"kind":"answer","text":"Use SQLite"},{"kind":"event","text":"x"}]});
    let items = finalize_items(
        &sprint,
        vec![json!({"key":"t1","description":"Build it","dependsOn":[]})],
    )
    .unwrap();
    let text = items[0]["description"].as_str().unwrap();
    assert!(text.starts_with("Build it\n\nOriginal owner requirements"));
    assert!(text.contains("Ask me before choosing the database."));
    assert!(text.ends_with("Owner follow-up:\nUse SQLite"));
    let long = json!({"description":"x".repeat(32_000),"conversation":[]});
    assert!(finalize_items(
        &long,
        vec![json!({"key":"t1","description":"D","dependsOn":[]})]
    )
    .is_err());
    let two = finalize_items(&sprint, two_parts().as_array().unwrap().clone()).unwrap();
    assert_eq!(two[0]["description"], "Do the first part");
}

#[tokio::test]
async fn an_automatic_queue_start_binds_the_runtime_policy() {
    let root =
        std::env::temp_dir().join(format!("auto-queue-{}", crate::identity().unwrap()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .unwrap();
    let _scratch = Scratch(root.clone());
    let folder = root.join("project");
    std::fs::create_dir(&folder).unwrap();
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
    let registered = service.register(&folder, "Automatic queue").await.unwrap();
    let project = registered["projectId"].as_str().unwrap().to_owned();
    service.activate(&project).await.unwrap();
    let runtime = service.state.lock().await.native[&project].clone();
    let id = call(&service, "product.sprint", &project,
        json!({"action":"create","description":"An automatic sprint","runMode":"automatic"}))
        .await["sprintId"].clone();
    let draft = service
        .workspace(json!({"protocol":{"major":1,"minor":8},
        "requestId":"draft","projectId":project,"actorId":actor(),
        "operation":"tasks.create","target":project,"expectedVersion":"0","ownerEpoch":null,
        "payload":{"description":"Implement the plan","checkout":{"kind":"main"}}}))
        .await
        .unwrap()["target"]
        .clone();
    call(
        &service,
        "product.task",
        &project,
        json!({"action":"sprint","taskId":draft,"sprintId":id,"dependencies":[]}),
    )
    .await;
    call(
        &service,
        "product.sprint",
        &project,
        json!({"action":"patch","sprintId":id,
        "patch":{"planning":"created","queueState":"running","taskIds":[draft]}}),
    )
    .await;
    service
        .sprint_pass(&project, &runtime, &mut Jobs::default(), false)
        .await
        .unwrap();
    let tasks = service.native_tasks(&project).await.unwrap();
    assert_eq!(
        tasks[draft.as_str().unwrap()]["automaticContinuation"]["policy"],
        cortex_core::lifecycle::automatic::POLICY
    );
    service.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sprint_owner_writes_survive_sustained_contention() {
    let (_scratch, folder, service, runtime, project, sprint_id, _) =
        automatic_fixture("dev", "main", "main").await;
    let create = |request: String, description: &str| {
        json!({"protocol":{"major":1,"minor":8},"requestId":request,
            "projectId":project,"actorId":actor(),"operation":"tasks.create",
            "target":project,"expectedVersion":"0","ownerEpoch":null,
            "payload":{"description":description,"checkout":{"kind":"main"}}})
    };
    let mut live = Vec::new();
    for index in 0..4 {
        let draft = service
            .workspace(create(format!("load-live-{index}"), "Sprint owner load hold"))
            .await
            .unwrap();
        let task = draft["target"].as_str().unwrap().to_owned();
        call(&service, "product.task", &project, json!({
            "action":"sprint","taskId":task,"sprintId":sprint_id,"dependencies":[]
        })).await;
        assert_eq!(service.workspace(json!({"protocol":{"major":1,"minor":8},
            "requestId":format!("load-start-{index}"),"projectId":project,"actorId":actor(),
            "operation":"tasks.start","target":task,"expectedVersion":draft["entityVersion"],
            "ownerEpoch":null,"payload":{}})).await.unwrap()["outcome"], "recorded");
        let bound = task.clone();
        // An exact link retry after Start confirms the committed membership
        // without resetting the task or touching its active execution.
        call(&service, "product.task", &project, json!({
            "action":"sprint","taskId":task,"sprintId":sprint_id,"dependencies":[]
        })).await;
        live.push(runtime.host.executor.read(move |db| {
            Ok(db.query_row("SELECT execution_id FROM workspace_tasks WHERE task_id=?",
                [bound], |row| row.get::<_, String>(0))?)
        }).unwrap().await.unwrap().unwrap());
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if std::fs::read_to_string(folder.join("load-entered"))
                .is_ok_and(|s| s.lines().count() == live.len()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("four real sprint turns held");

    // Offer a different model/effort for later turns while each live peer
    // is still on the original fixture route.
    std::fs::write(folder.join("load-route-enabled"), "").unwrap();
    let settings = json!({"model":"load-model","effort":"high","mode":"guarded","language":"auto"});
    assert_eq!(runtime.routes.task_route(&folder, &settings).unwrap().model, "load-model");

    // Hold the lane *before* a transaction starts. Fill its backlog with
    // read-only jobs so a refusal here cannot originate in SQLite locking.
    let writer = &runtime.host.executor;
    let controls = runtime.controls.lock().await;
    let generic_actions = service.helper.actions.clone().try_acquire_many_owned(4).unwrap();
    let (entered, observed) = tokio::sync::oneshot::channel();
    let (release, gate) = std::sync::mpsc::channel();
    let blocker = writer.publication(move |_| {
        entered.send(()).unwrap();
        gate.recv_timeout(Duration::from_secs(12)).unwrap();
        Ok(())
    }).unwrap();
    observed.await.unwrap();
    let mut queued = Vec::new();
    for _ in 0..129 {
        match writer.publication(|s| s.read(|db| Ok(db.query_row("SELECT 1", [], |r| r.get::<_, i64>(0))?))) {
            Ok(job) => queued.push(job),
            Err(error) => {
                assert_eq!(error.code(), "busy");
                break;
            }
        }
    }
    assert_eq!(queued.len(), 128, "the physical writer queue is full");
    let key = crate::native_runtime::key(&folder).unwrap();
    let unlocked = tokio::task::spawn_blocking(move || {
        let store = cortex_core::Store::open(key, cortex_core::OpenMode::RuntimeValidate).unwrap();
        store.write(|_| Ok(())).unwrap();
    });
    tokio::time::timeout(Duration::from_millis(500), unlocked)
        .await.expect("SQLite permits BEGIN IMMEDIATE while executor is full").unwrap();

    let start = tokio::time::Instant::now();
    let last_background_ticket = writer.submitted();
    let mut requests = tokio::task::JoinSet::new();
    for (index, execution) in live.iter().enumerate() {
        let mut update = json!({"projectId":project,"executionId":execution,
            "requestId":format!("load-settings-{index}"),"action":"task_settings","settings":settings});
        // First change the model, then change only effort on this same active task.
        update["settings"]["effort"] = json!("");
        let draft = create(format!("load-new-{index}"), "Created during the held sprint");
        let svc = service.clone();
        let (project_id, sprint) = (project.clone(), sprint_id.clone());
        requests.spawn(async move {
            let preflight = call(&svc, "product.read", &project_id, json!({})).await;
            let ready = preflight["sprints"].as_array().unwrap().iter()
                .any(|row| row["id"] == sprint && row["planning"] == "created");
            assert!(ready, "desktop sprint preflight must succeed while saturated");
            let result = svc.workspace(draft.clone()).await;
            if let Ok(receipt) = &result {
                if receipt["outcome"] == "recorded" {
                    call(&svc, "product.task", &project_id, json!({
                        "action":"sprint","taskId":receipt["target"],"sprintId":sprint,"dependencies":[]
                    })).await;
                }
            }
            vec![(false, draft, result, start.elapsed())]
        });
        let svc = service.clone();
        requests.spawn(async move {
            let result = svc.control_execution(update.clone()).await;
            let mut effort = update.clone();
            effort["requestId"] = json!(format!("load-effort-{index}"));
            effort["settings"]["effort"] = json!("high");
            let model = (true, update, result, start.elapsed());
            let result = svc.control_execution(effort.clone()).await;
            vec![model, (true, effort, result, start.elapsed())]
        });
    }
    tokio::time::timeout(Duration::from_secs(1), async {
        while writer.submitted() < last_background_ticket + 5 {
            tokio::task::yield_now().await;
        }
    }).await.expect("four draft jobs and the first settings lookup admitted while full");
    let admitted_in = start.elapsed();
    let ordinary = writer.publication_wait(|_| panic!("refusal must not execute"));
    let ordinary: cortex_core::CoreResult<()> = ordinary.await;
    assert_eq!(ordinary.unwrap_err().code(), "busy");
    assert!(start.elapsed() >= Duration::from_secs(2));
    assert_eq!(requests.len(), 8);
    let held_for = start.elapsed();
    release.send(()).unwrap();
    blocker.await.unwrap().unwrap();
    for job in queued {
        assert_eq!(job.await.unwrap().unwrap(), 1);
    }
    // Replenished background work also progresses without releasing the unrelated
    // project control lock or generic machine action class.
    let sibling = writer.publication(|store| store.write(|db| {
        db.execute_batch("CREATE TEMP TABLE owner_load_background(done); INSERT INTO owner_load_background VALUES(1);")?;
        Ok(1)
    })).unwrap();
    assert_eq!(sibling.await.unwrap().unwrap(), 1);
    let mut replies = Vec::new();
    let mut counts = [0; 2];
    let mut busy = [0; 2];
    let mut max = [Duration::ZERO; 2];
    tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(reply) = requests.join_next().await {
            for reply in reply.unwrap() {
                let lane = usize::from(reply.0);
                counts[lane] += usize::from(reply.2.is_ok());
                busy[lane] += usize::from(reply.2.as_ref().is_err_and(|e| e.to_string() == "owner_queue_busy"));
                max[lane] = max[lane].max(reply.3);
                replies.push(reply);
            }
        }
    }).await.expect("bounded owner acknowledgement after the writer was released");
    std::fs::write(folder.join("load-release"), "").unwrap();
    let peer_start = tokio::time::Instant::now();
    let peers = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let executions = live.clone();
            let settled = writer.read(move |db| {
                Ok(db.query_row("SELECT count(DISTINCT w.execution_id)
                    FROM workspace_executions w JOIN runtime_events e ON e.task_id=w.runtime_task_id
                    WHERE w.execution_id IN (?1, ?2, ?3, ?4) AND e.type='NATIVE_TURN_SETTLED'
                      AND json_extract(e.payload_json,'$.stop_reason')='end_turn'",
                    rusqlite::params_from_iter(executions), |row| row.get::<_, i64>(0))?)
            }).unwrap().await.unwrap().unwrap();
            if settled == live.len() as i64 {
                break settled;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("all live sibling turns settle durably with unrelated controls still held");
    eprintln!("sprint owner load: {peers}/4 ACP peers settled in {:?}", peer_start.elapsed());
    drop(generic_actions);
    drop(controls);
    assert!(max.iter().all(|elapsed| *elapsed < held_for + Duration::from_secs(2)));
    eprintln!("sprint owner load: admitted={admitted_in:?}; saturated={held_for:?}; \
        background=128+1; create/settings replies={counts:?}; busy={busy:?}; max latency={max:?}");
    let stored = writer.read(|db| {
        Ok((
            db.query_row("SELECT count(*) FROM workspace_requests WHERE request_id LIKE 'load-new-%'", [], |r| r.get::<_, i64>(0))?,
            db.query_row("SELECT count(*) FROM runtime_events WHERE type='NATIVE_TASK_SETTINGS'
                AND json_extract(payload_json,'$.settings.model') = 'load-model'", [], |r| r.get::<_, i64>(0))?,
        ))
    }).unwrap().await.unwrap().unwrap();
    eprintln!("sprint owner load: persisted new drafts and task-local settings={stored:?}");
    let errors: Vec<_> = replies.iter().filter_map(|reply| reply.2.as_ref().err()).collect();
    assert_eq!(counts, [4, 8], "{errors:?}");
    assert_eq!(stored, (4, 8));

    // Exact duplicates acknowledge the retained original and do not repeat it.
    let retry_target = replies.iter().find(|reply| reply.0).unwrap().1.clone();
    for (settings, request, result, _) in replies {
        let receipt = result.unwrap();
        assert_eq!(receipt["outcome"], "recorded", "{receipt}");
        if settings {
            assert_eq!(service.control_execution(request.clone()).await.unwrap(), receipt);
            let execution = request["executionId"].as_str().unwrap().to_owned();
            let persisted = writer.read(move |db| {
                let mut q = db.prepare("SELECT e.payload_json FROM runtime_events e
                    JOIN workspace_executions w ON w.runtime_task_id=e.task_id
                    WHERE w.execution_id=? AND e.type='NATIVE_TASK_SETTINGS' ORDER BY e.id")?;
                let rows = q.query_map([execution], |row| row.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(rows.into_iter().map(|s| serde_json::from_str::<Value>(&s)).collect::<Result<Vec<_>, _>>()?)
            }).unwrap().await.unwrap().unwrap();
            assert_eq!(persisted.len(), 2, "replay must not repeat a settings change");
            assert_eq!(persisted[0]["settings"]["effort"], "");
            assert_eq!(persisted[1]["settings"]["effort"], "high");
            let index = usize::from(request["settings"]["effort"] == "high");
            assert_eq!(persisted[index], json!({"settings":request["settings"],"activation":"next_use"}));
            let mut conflicting = request;
            conflicting["settings"]["language"] = json!("en");
            assert_eq!(service.control_execution(conflicting).await.unwrap_err().to_string(),
                "payload_conflict");
        } else {
            assert_eq!(service.workspace(request.clone()).await.unwrap(), receipt);
            let request_id = request["requestId"].as_str().unwrap().to_owned();
            let persisted = writer.read(move |db| {
                Ok(db.query_row("SELECT r.receipt_json FROM workspace_requests r
                    JOIN workspace_tasks t ON json_extract(r.receipt_json,'$.target')=t.task_id
                    WHERE r.request_id=? AND r.project_id=t.project_id", [request_id],
                    |row| row.get::<_, String>(0))?)
            }).unwrap().await.unwrap().unwrap();
            assert_eq!(serde_json::from_str::<Value>(&persisted).unwrap(), receipt);
            let mut conflicting = request;
            conflicting["payload"]["description"] = json!("Not a replay");
            assert_eq!(service.workspace(conflicting).await.unwrap()["outcome"], "payload_conflict");
            // The desktop links the replayed receipt again; membership is still unique.
            call(&service, "product.task", &project, json!({
                "action":"sprint","taskId":receipt["target"],"sprintId":sprint_id,"dependencies":[]
            })).await;
        }
    }
    let current = sprint(&service, &project, &sprint_id).await;
    assert_eq!(current["taskIds"].as_array().unwrap().len(), 10);
    let drafts = writer.read(|db| {
        Ok(db.query_row("SELECT count(*) FROM workspace_tasks WHERE description='Created during the held sprint'",
            [], |row| row.get::<_, i64>(0))?)
    }).unwrap().await.unwrap().unwrap();
    assert_eq!(drafts, 4, "duplicates must not create extra drafts");

    // If the separately persisted membership is refused, replay the same
    // create identity and link exactly that task once. Never create again.
    let request = create("load-partial-create".into(), "Retry the existing sprint draft");
    let draft = service.workspace(request.clone()).await.unwrap();
    let link = json!({"action":"sprint","taskId":draft["target"],"sprintId":sprint_id,"dependencies":[]});
    let full = service.helper.sprint_links.clone().try_acquire_many_owned(4).unwrap();
    let refused = tokio::time::timeout(Duration::from_millis(500), service.machine(
        serde_json::from_value(json!({"op":"product.task","projectId":project,"params":link})).unwrap()
    )).await.unwrap().unwrap_err();
    assert_eq!(refused.to_string(), "machine_busy");
    drop(full);
    assert!(!sprint(&service, &project, &sprint_id).await["taskIds"].as_array().unwrap().contains(&draft["target"]));
    assert_eq!(service.workspace(request).await.unwrap(), draft);
    call(&service, "product.task", &project, link.clone()).await;
    call(&service, "product.task", &project, link).await;
    let linked = sprint(&service, &project, &sprint_id).await;
    assert_eq!(linked["taskIds"].as_array().unwrap().len(), 11);
    assert_eq!(linked["taskIds"].as_array().unwrap().iter().filter(|id| **id == draft["target"]).count(), 1);

    // A failure writing the settings must also roll back the admission record.
    writer.write(|db| {
        db.execute_batch("CREATE TEMP TRIGGER load_abort_setting BEFORE INSERT ON runtime_events
            WHEN NEW.type='NATIVE_TASK_SETTINGS' BEGIN SELECT RAISE(ABORT, 'fixture failure'); END")?;
        Ok(())
    }).unwrap().await.unwrap().unwrap();
    let mut uncommitted = retry_target;
    uncommitted["requestId"] = json!("load-settings-unavailable");
    uncommitted["settings"]["model"] = json!("not-an-offered-model");
    assert_eq!(service.control_execution(uncommitted.clone()).await.unwrap_err().to_string(),
        "native_task_route_unavailable");
    uncommitted["settings"]["model"] = json!("load-model");
    uncommitted["requestId"] = json!("load-settings-rollback");
    uncommitted["settings"]["language"] = json!("en");
    let full = runtime.settings_slots.clone().try_acquire_many_owned(8).unwrap();
    assert_eq!(tokio::time::timeout(Duration::from_millis(500), service.control_execution(uncommitted.clone()))
        .await.unwrap().unwrap_err().to_string(), "owner_queue_busy");
    drop(full);
    assert!(service.control_execution(uncommitted.clone()).await.is_err());
    let rollback_count = writer.read(|db| {
        Ok(db.query_row("SELECT count(*) FROM runtime_requests WHERE request_key='load-settings-rollback'",
            [], |row| row.get::<_, i64>(0))?)
    }).unwrap().await.unwrap().unwrap();
    assert_eq!(rollback_count, 0, "failed local change must not leave partial admission");
    writer.write(|db| {
        db.execute_batch("DROP TRIGGER load_abort_setting")?;
        Ok(())
    }).unwrap().await.unwrap().unwrap();
    let retry = service.control_execution(uncommitted.clone()).await.unwrap();
    assert_eq!(retry["outcome"], "recorded");
    assert_eq!(service.control_execution(uncommitted).await.unwrap(), retry);
    eprintln!("sprint owner load: one refused link reconciled without a duplicate; \
        one settings rollback retried and replayed");
    service.stop().await.unwrap();
}

fn git_fixture(root: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("/usr/bin/git")
        .current_dir(root)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().into()
}

async fn automatic_fixture(
    target: &str,
    sprint_checkout: &str,
    task_checkout: &str,
) -> (
    Scratch,
    std::path::PathBuf,
    Arc<Service>,
    Arc<Runtime>,
    String,
    Value,
    Vec<String>,
) {
    let root = std::env::temp_dir()
        .join(format!("automatic-finish-{}", crate::identity().unwrap()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .unwrap();
    let folder = root.join("project");
    std::fs::create_dir(&folder).unwrap();
    let scratch = Scratch(root.clone());
    git_fixture(&folder, &["init", "-q", "-b", "dev"]);
    git_fixture(&folder, &["config", "user.name", "Queue fixture"]);
    git_fixture(
        &folder,
        &["config", "user.email", "fixture@example.invalid"],
    );
    git_fixture(&folder, &["config", "commit.gpgsign", "false"]);
    std::fs::write(folder.join("work.txt"), "before\n").unwrap();
    git_fixture(&folder, &["add", "work.txt"]);
    git_fixture(&folder, &["commit", "-q", "-m", "Base"]);
    git_fixture(&folder, &["branch", "main"]);
    let service = Service::bind_native(
        Config {
            runtime_dir: root.join("runtime"),
            state_dir: root.join("state"),
            allowed_root: root,
            launch: LaunchSpec::bare(),
            request_timeout: Duration::from_secs(5),
            shutdown_timeout: Duration::from_secs(2),
            event_capacity: 64,
        },
        Arc::new(Peer(None)),
    )
    .await
    .unwrap();
    let registered = service.register(&folder, "Queue fixture").await.unwrap();
    let project = registered["projectId"].as_str().unwrap().to_owned();
    service.activate(&project).await.unwrap();
    let runtime = service.state.lock().await.native[&project].clone();
    // These fixtures drive every scheduler pass themselves: the tick alone
    // continues a stopped task, never a background follower.
    runtime
        .followers
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let id = call(
        &service,
        "product.sprint",
        &project,
        json!({
            "action":"create","description":"Finish two tasks","runMode":"automatic",
            "targetBranch":target,"checkout":{"kind":sprint_checkout}
        }),
    )
    .await["sprintId"]
        .clone();
    let mut tasks = Vec::<String>::new();
    for index in 0..2 {
        let created = service.workspace(json!({"protocol":{"major":1,"minor":8},
            "requestId":format!("draft-{index}"),"projectId":project,"actorId":actor(),
            "operation":"tasks.create","target":project,"expectedVersion":"0","ownerEpoch":null,
            "payload":{"description":"Implement a part","checkout":{"kind":task_checkout}}
        })).await.unwrap();
        let task = created["target"].as_str().unwrap().to_owned();
        call(
            &service,
            "product.task",
            &project,
            json!({"action":"sprint","taskId":task,
            "sprintId":id,"dependencies":tasks}),
        )
        .await;
        tasks.push(task);
    }
    call(
        &service,
        "product.sprint",
        &project,
        json!({"action":"patch","sprintId":id,
        "patch":{"planning":"created","queueState":"running","taskIds":tasks}}),
    )
    .await;
    (scratch, folder, service, runtime, project, id, tasks)
}

#[tokio::test]
async fn sprint_live_priority_fences_stale_queue_claims_and_decisions() {
    for (mode, checkout) in [("automatic", "main"), ("hybrid", "main"), ("hybrid", "worktree")] {
        let (scratch, folder, service, runtime, project, sprint_id, ids) =
            automatic_fixture("dev", checkout, checkout).await;
        if checkout == "worktree" {
            let reply = service.machine(serde_json::from_value(json!({
                "op":"machine.update","params":{"patch":{"worktreeRoot":scratch.0.join("trees")}}
            })).unwrap()).await.unwrap();
            assert_eq!(reply["ok"], true, "{reply}");
        }
        let sprint_id = sprint_id.as_str().unwrap().to_owned();
        call(&service, "product.sprint", &project, json!({"action":"patch","sprintId":sprint_id,
            "patch":{"runMode":mode,"materializedVersion":1}})).await;
        call(&service, "product.sprint", &project, json!({"action":"patch","sprintId":sprint_id,
            "patch":{"queueState":"running"}})).await;
        let read = || call(&service, "product.sprint", &project,
            json!({"action":"live_graph","sprintId":sprint_id}));
        let initial = read().await;
        assert_eq!(initial["order"]["notStarted"][0]["taskId"], ids[0]);
        assert_eq!(initial["order"]["notStarted"][0]["next"], true);
        let old = runtime.sprint_queue_snapshot(project.clone(), sprint_id.clone()).await.unwrap();

        let moved = call(&service, "product.sprint", &project, json!({
            "action":"move_priority","sprintId":sprint_id,"taskId":ids[1],
            "direction":"up","expectedGraphRevision":initial["graphRevision"],
        })).await;
        assert_eq!(moved["changed"], true);
        let moved = read().await;
        assert_eq!(moved["priority"], json!([ids[1], ids[0]]));
        assert_eq!(moved["members"][0]["taskId"], ids[0]);
        assert_eq!(moved["order"]["notStarted"][0]["taskId"], ids[0], "priority cannot bypass edges");
        let removed = call(&service, "product.sprint", &project, json!({
            "action":"set_dependency","sprintId":sprint_id,"dependentTaskId":ids[1],
            "predecessorTaskId":ids[0],"present":false,"expectedGraphRevision":moved["graphRevision"],
        })).await;
        assert_eq!(removed["changed"], true);
        service.start_queue_task(&project, &runtime, &old, &ids[0], mode == "automatic").await.unwrap();
        service.observed_pause(&project, &old, automatic_hold(
            "settlement_blocked", Some(&ids[0]), "dependencies_blocked", "Old decision", &HoldFacts::default(),
        )).await.unwrap();
        service.complete_queue(&project, &old).await.unwrap();
        let fresh = read().await;
        assert_eq!(fresh["order"]["queueState"], "running");
        assert_eq!(fresh["order"]["notStarted"][0]["taskId"], ids[1]);
        assert_eq!(fresh["order"]["notStarted"][0]["next"], true);
        assert!(service.native_tasks(&project).await.unwrap()[&ids[0]]["executionId"].is_null());

        let newest = runtime.sprint_queue_snapshot(project.clone(), sprint_id.clone()).await.unwrap();
        service.start_queue_task(&project, &runtime, &newest, &ids[1], mode == "automatic").await.unwrap();
        let fact = service.native_tasks(&project).await.unwrap();
        let execution = fact[&ids[1]]["executionId"].clone();
        assert!(execution.is_string(), "{fact:?}");
        if checkout == "worktree" {
            let trees = git_fixture(&folder, &["worktree", "list", "--porcelain"]);
            assert!(trees.contains(scratch.0.join("trees").to_str().unwrap()), "{trees}");
        }
        service.start_queue_task(&project, &runtime, &newest, &ids[1], mode == "automatic").await.unwrap();
        let fact = service.native_tasks(&project).await.unwrap();
        assert_eq!(fact[&ids[1]]["executionId"], execution);
        assert!(fact[&ids[0]]["executionId"].is_null());
        let after = read().await;
        assert_eq!(after["order"]["queueState"], "running");
        assert_eq!(after["members"][1]["editable"], false);
        assert!(after["order"]["notStarted"].as_array().unwrap().iter().all(|r| r["next"] == false));
        for (revision, expected) in [(&fresh["graphRevision"], "stale_graph_revision"),
            (&after["graphRevision"], "sprint_member_read_only")]
        {
            let reply = service.machine(serde_json::from_value(json!({"op":"product.sprint","projectId":project,
                "params":{"action":"set_dependency","sprintId":sprint_id,"dependentTaskId":ids[1],
                    "predecessorTaskId":ids[0],"present":true,"expectedGraphRevision":revision}})).unwrap()).await.unwrap();
            assert_eq!(reply["code"], expected, "{reply}");
        }
        service.stop().await.unwrap();
    }
}

#[tokio::test]
async fn sprint_live_projection_is_suggested_in_manual_and_never_starts_work() {
    let (_scratch, _folder, service, _runtime, project, id, ids) =
        automatic_fixture("dev", "main", "main").await;
    let read = || call(&service, "product.sprint", &project,
        json!({"action":"live_graph","sprintId":id}));
    assert_eq!(read().await["order"]["notStarted"][0]["next"], true);
    let runtime = service.state.lock().await.native.remove(&project).unwrap();
    let inactive = read().await;
    assert_eq!(inactive["order"]["notStarted"][0]["next"], false);
    assert_eq!(inactive["order"]["notStarted"][0]["conditional"], true);
    service.state.lock().await.native.insert(project.clone(), runtime);
    call(&service, "product.sprint", &project, json!({"action":"patch","sprintId":id,
        "patch":{"runMode":"manual","materializedVersion":1}})).await;
    let graph = call(&service, "product.sprint", &project, json!({"action":"live_graph","sprintId":id})).await;
    assert_eq!(graph["order"]["mode"], "manual");
    assert_eq!(graph["order"]["notStarted"][1]["taskId"], ids[1]);
    assert_eq!(graph["order"]["notStarted"][1]["rank"], 2);
    assert_eq!(graph["order"]["notStarted"][0]["next"], false);
    let facts = service.native_tasks(&project).await.unwrap();
    assert!(facts[&ids[0]]["executionId"].is_null() && facts[&ids[1]]["executionId"].is_null());
    // Explicit Manual Start is still task-local; the service does not require the suggested order.
    let reply = service.workspace(json!({"protocol":{"major":1,"minor":8},"requestId":"manual-b",
        "projectId":project,"actorId":actor(),"operation":"tasks.start","target":ids[1],
        "expectedVersion":facts[&ids[1]]["version"],"ownerEpoch":null,"payload":{}})).await.unwrap();
    assert_eq!(reply["outcome"], "recorded", "{reply}");
    assert!(service.native_tasks(&project).await.unwrap()[&ids[0]]["executionId"].is_null());
    service.stop().await.unwrap();
}

async fn settled_peer(runtime: &Runtime, draft: &str) -> String {
    let draft = draft.to_owned();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(12);
    loop {
        let t = draft.clone();
        let task: Option<String> = runtime.host.executor.read(move |db| {
            use rusqlite::OptionalExtension;
            Ok(db.query_row("SELECT e.runtime_task_id FROM workspace_tasks w JOIN workspace_executions e USING(execution_id)
                JOIN runtime_tasks r ON r.task_id=e.runtime_task_id WHERE w.task_id=?
                AND r.state IN ('WAITING_USER','FAILED','CANCELLED','BLOCKED','RECOVERING')
                AND NOT EXISTS(SELECT 1 FROM capacity_reservations c WHERE c.task_id=r.task_id AND c.state!='released')",
                [&t], |r| r.get(0)).optional()?)
        }).unwrap().await.unwrap().unwrap();
        if let Some(task) = task {
            return task;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the fake ACP peer did not settle"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

// Inject the fake peer's completed result at the retained-result boundary.
// Use the real checkout digest and an actual local check; none of this
// unit fixture is live provider or release qualification.
async fn verified_fixture(runtime: &Runtime, task: String, attributed: bool) {
    verified_fixture_at(runtime, task, attributed, b"reviewed changes\n").await;
}

async fn verified_fixture_at(
    runtime: &Runtime,
    task: String,
    attributed: bool,
    checked_bytes: &[u8],
) {
    use sha2::{Digest, Sha256};
    let root = &runtime.host.root;
    let bytes = b"reviewed changes\n";
    std::fs::write(root.join("work.txt"), bytes).unwrap();
    if checked_bytes != bytes {
        std::fs::write(root.join("work.txt"), checked_bytes).unwrap();
    }
    assert!(std::process::Command::new("/usr/bin/test")
        .current_dir(root)
        .args(["-s", "work.txt"])
        .status()
        .unwrap()
        .success());
    let sha = format!("{:x}", Sha256::digest(bytes));
    let digest = crate::native_effects::observe(root.clone())
        .await
        .unwrap()
        .project_digest
        .unwrap();
    runtime.host.executor.publication(move |store| {
        let dir = store.key().directory().join(&task);
        std::fs::create_dir_all(&dir)?;
        let work = "# Execution evidence\n\nThe fixture changed work.txt.\n";
        let review = "# Independent review\n\nThe fixture's change is approved.\n";
        std::fs::write(dir.join("work.md"), work)?;
        std::fs::write(dir.join("review.md"), review)?;
        store.write(|db| {
            for (id, filename) in [("finish_work", "work.md"), ("finish_review", "review.md")] {
                db.execute("INSERT INTO reports VALUES (?,?,'report',?,'now')", (id, &task, filename))?;
            }
            db.execute("INSERT INTO runtime_nodes VALUES (?,0,'fixture-execution','worker-backend-dev','Work','[]','[\"work.txt\"]','execution','completed','fixture-execution',?)",
                (&task, json!({"report_ref":"finish_work","report_digest":format!("{:x}", Sha256::digest(work)),"checks":[]}).to_string()))?;
            db.execute("INSERT INTO runtime_nodes VALUES (?,0,'fixture-review','worker-code-reviewer','Review','[\"fixture-execution\"]','[]','review','completed','fixture-review',?)",
                (&task, json!({"report_ref":"finish_review","report_digest":format!("{:x}", Sha256::digest(review)),"verdict":"APPROVE","checks":[]}).to_string()))?;
            if attributed {
                db.execute("INSERT INTO trusted_channels(channel_id,secret_hash,task_id,principal_thread_id,role,owner_epoch,issued_at)
                    VALUES ('fixture-write','hash',?,'fixture-worker','worker',1,'now')", [&task])?;
                db.execute("INSERT INTO runtime_requests VALUES (?,'fixture-key','fixture-write','project_edit','digest',?)",
                    (&task, json!({"complete":true,
                        "reply":{"structuredContent":{"path":"work.txt","sha256":sha}}}).to_string()))?;
                cortex_core::lifecycle::event_tx(db, &task, "NATIVE_PROJECT_WRITTEN",
                    json!({"channel_id":"fixture-write","path":"work.txt","sha256":sha}))?;
            }
            let argv = json!(["/usr/bin/test", "-s", "work.txt"]);
            db.execute("UPDATE runtime_tasks SET state='COMPLETED', revision=0, blocker=NULL,
                synthesis='Verified fixture result.', plan_json=? WHERE task_id=?",
                (json!({"verification":[argv]}).to_string(), &task))?;
            cortex_core::lifecycle::event_tx(db, &task, "VERIFICATION_PASSED",
                json!({"revision":0,"revision_digest":digest,"checks":[{"argv":argv,"complete":true,
                    "exit_code":0,"revision_digest":digest}]}))?;
            Ok(())
        })
    }).unwrap().await.unwrap().unwrap();
}

fn merge_request(project: &str, execution: &Value, conflict: &Value, action: &str, id: &str) -> Value {
    json!({"projectId":project,"executionId":execution,"conflictId":conflict,"action":action,"requestId":id})
}

async fn delivered_fixture_prompt(path: &Path) -> String {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(text) = std::fs::read_to_string(path) {
                if text.ends_with('\n') {
                    assert_eq!(text.lines().count(), 1, "request was dispatched more than once");
                    let body: Value = serde_json::from_str(text.trim_end()).unwrap();
                    break body["params"]["prompt"].as_array().unwrap()
                        .iter().filter_map(|part| part["text"].as_str()).collect::<Vec<_>>().join("\n");
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("the no-paid peer did not receive a coordinator prompt")
}

struct MergeFixture {
    _scratch: Scratch,
    folder: std::path::PathBuf,
    target: std::path::PathBuf,
    target_head: String,
    service: Arc<Service>,
    runtime: Arc<Runtime>,
    project: String,
    draft: String,
    task: String,
    execution: Value,
    conflict: Value,
    sprint_id: Value,
    jobs: Jobs,
    old_actions: Value,
}

async fn merge_handoff_fixture() -> MergeFixture {
    let (_scratch, folder, service, runtime, project, sprint_id, drafts) =
        automatic_fixture("main", "main", "main").await;
    let draft = &drafts[0];
    let mut jobs = Jobs::default();
    service.sprint_pass(&project, &runtime, &mut jobs, false).await.unwrap();
    let task = settled_peer(&runtime, draft).await;
    let target = folder.with_file_name("target");
    git_fixture(&folder, &["worktree", "add", target.to_str().unwrap(), "main"]);
    std::fs::write(target.join("work.txt"), "Target conflict\n").unwrap();
    git_fixture(&target, &["commit", "-qam", "Target changes work"]);
    let target_head = git_fixture(&target, &["rev-parse", "HEAD"]);
    verified_fixture(&runtime, task.clone(), true).await;
    service.sprint_pass(&project, &runtime, &mut jobs, false).await.unwrap();
    let board = call(&service, "product.board", &project, json!({})).await;
    let accepted = &board["tasks"][draft]["acceptance"];
    assert!(accepted["digest"].is_string(), "{accepted}");
    let old_actions = call(&service, "product.read", &project, json!({"taskIds":[draft]})).await
        ["tasks"][draft]["reviewActions"].clone();
    let integrate = service.machine(serde_json::from_value(json!({
        "op":"product.integrate","projectId":project,"params":{
            "confirmed":true,"taskId":draft,"method":"squash","message":"Owner integrates",
            "digest":accepted["digest"],"requestId":"integration-conflict"}
        })).unwrap()).await.unwrap();
    assert_eq!(integrate["code"], "merge_conflict", "{integrate}");
    let record = call(&service, "product.board", &project, json!({})).await["tasks"][draft].clone();
    assert_eq!(record["conflict"], "conflict", "{record}");
    assert!(record["merge"]["handoff"].is_null());
    assert!(!folder.join(".git/merge-handoff-prompts").exists());
    let conflict = record["merge"]["conflictId"].clone();
    let execution = service.native_tasks(&project).await.unwrap()[draft]["executionId"].clone();
    MergeFixture { _scratch, folder, target, target_head, service, runtime,
        project, draft: draft.clone(), task, execution, conflict, sprint_id, jobs, old_actions }
}

#[tokio::test]
async fn owner_merge_handoff_sends_git_manifest_stop_restores_manual_finish_and_checks_request() {
    let MergeFixture { _scratch, folder, target, target_head, service, runtime, project,
        draft, task, execution, conflict, sprint_id, mut jobs, old_actions } = merge_handoff_fixture().await;
    let (draft, conflict) = (&draft, &conflict);
    let head = git_fixture(&folder, &["rev-parse", "HEAD"]);
    let handoff = merge_request(&project, &execution, conflict, "handoff_merge", "explicit-handoff");
    let started = service.control_execution(handoff.clone()).await.unwrap();
    assert_eq!(started["conflict"], "resolving", "{started}");
    let prompt = delivered_fixture_prompt(&folder.join(".git/merge-handoff-prompts")).await;
    let manifest: Value = serde_json::from_str(prompt.lines()
        .find_map(|line| line.strip_prefix("Merge manifest: ")).expect("manifest on the wire")).unwrap();
    assert_eq!(manifest, json!({
        "sourceRevision":head,"targetRevision":target_head,
        "paths":["work.txt"],"complete":true,"truncated":false
    }));
    assert!(!prompt.contains("Target conflict\n"));
    let replay = service.control_execution(handoff.clone()).await.unwrap();
    assert_eq!(replay["merge"]["handoff"]["requestId"], "explicit-handoff");
    let mut changed = handoff;
    changed["text"] = json!("Different guidance with the same request ID");
    assert_eq!(service.control_execution(changed).await.unwrap_err().to_string(), "payload_conflict");
    let duplicate = merge_request(&project, &execution, conflict, "handoff_merge", "another-handoff");
    assert_eq!(service.control_execution(duplicate).await.unwrap_err().to_string(), "stop_required");

    service.cancel_execution(&project, execution.as_str().unwrap()).await.unwrap();
    let board = call(&service, "product.board", &project, json!({})).await;
    assert_eq!(board["tasks"][draft]["conflict"], "conflict", "{board}");
    assert_eq!(board["tasks"][draft]["merge"]["handoff"]["state"], "stopped");
    assert_eq!(git_fixture(&folder, &["rev-parse", "HEAD"]), head);
    assert_eq!(git_fixture(&folder, &["rev-parse", "MERGE_HEAD"]), target_head);
    let unknown_prompt_count = std::fs::read_to_string(folder.join(".git/merge-handoff-prompts")).unwrap().lines().count();
    let inspected = service.control_execution(merge_request(
        &project, &execution, conflict, "inspect_merge", "inspect-stopped"
    )).await.unwrap();
    assert_eq!(inspected["merge"]["gitState"], "stopped");
    assert_eq!(unknown_prompt_count, 1);
    std::fs::write(folder.join("work.txt"), "Manually resolved\n").unwrap();
    git_fixture(&folder, &["add", "work.txt"]);
    call(&service, "product.task", &project, json!({
        "taskId":draft,"action":"finish_merge","requestId":"manual-finish"
    })).await;
    let resolved = git_fixture(&folder, &["rev-parse", "HEAD"]);
    let record = call(&service, "product.board", &project, json!({})).await["tasks"][draft].clone();
    assert_eq!(record["resolvedBy"], "you");
    assert_eq!(record["merge"]["freshChecks"]["revision"], resolved);
    assert_eq!(record["merge"]["freshChecks"]["state"], "not_requested");
    assert_eq!(record["acceptance"], Value::Null);
    assert_eq!(git_fixture(&target, &["rev-parse", "HEAD"]), target_head);
    // Nothing was sent on Finish or the queue pass while checks await the owner.
    service.sprint_pass(&project, &runtime, &mut jobs, false).await.unwrap();
    assert_eq!(sprint(&service, &project, &sprint_id).await["automaticQueue"]["code"], "different_target");
    assert!(!folder.join(".git/merge-fresh-check-prompts").exists());
    let old = service.machine(serde_json::from_value(json!({"op":"product.task","projectId":project,
        "params":{"action":"accept","taskId":draft,"digest":old_actions["digest"],"version":old_actions["version"]}}))
        .unwrap()).await.unwrap();
    assert_eq!(old["ok"], false, "{old}");
    let checks = merge_request(&project, &execution, conflict, "request_merge_checks", "explicit-checks");
    let request = service.control_execution(checks.clone()).await.unwrap();
    assert_eq!(request["merge"]["freshChecks"]["state"], "requested");
    let fresh = delivered_fixture_prompt(&folder.join(".git/merge-fresh-check-prompts")).await;
    assert!(fresh.contains(&resolved));
    assert!(!fresh.contains("Merge manifest:"));
    assert_eq!(service.control_execution(checks).await.unwrap()["merge"]["freshChecks"]["state"], "requested");
    let count = runtime.host.executor.read(move |db| {
        Ok(db.query_row("SELECT count(*) FROM runtime_events WHERE task_id=? AND type='USER_STEERING'
            AND json_extract(payload_json,'$.text') LIKE 'Run a fresh independent review%'", [&task],
            |row| row.get::<_,i64>(0))?)
    }).unwrap().await.unwrap().unwrap();
    assert_eq!(count, 1);
    assert_eq!(git_fixture(&target, &["rev-parse", "HEAD"]), target_head);
    service.stop().await.unwrap();
}

#[tokio::test]
async fn owner_merge_handoff_settled_completion_finishes_the_exact_staged_merge() {
    let MergeFixture { _scratch, folder, target, target_head, service, runtime, project,
        draft, task, execution, conflict, .. } = merge_handoff_fixture().await;
    let source = git_fixture(&folder, &["rev-parse", "HEAD"]);
    let started = service.control_execution(merge_request(&project, &execution, &conflict,
        "handoff_merge", "complete-handoff")).await.unwrap();
    let request = started["merge"]["handoff"]["runtimeRequestId"].as_str().unwrap();
    delivered_fixture_prompt(&folder.join(".git/merge-handoff-prompts")).await;
    std::fs::write(folder.join("work.txt"), "Resolved and staged\n").unwrap();
    git_fixture(&folder, &["add", "work.txt"]);
    let staged_tree = git_fixture(&folder, &["write-tree"]);
    let mut updates = service.events.subscribe();

    // Keep coordinator authority until the real ACP reply is recorded.
    // Marking the task COMPLETED first can close the live ACP transport.
    // Fence the next driver step so the fixture can finish before it advances.
    let driver = runtime.host.driver.lock().await;
    std::fs::write(folder.join(".git/release-merge-handoff"), "").unwrap();
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let (task, request) = (task.clone(), request.to_owned());
            let delivered: bool = runtime.host.executor.read(move |db| {
                Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_events
                    WHERE task_id=? AND type='WORKSPACE_MERGE_HANDOFF_DELIVERED'
                    AND json_extract(payload_json,'$.request')=?)",
                    (&task, &request), |row| row.get(0))?)
            }).unwrap().await.unwrap().unwrap();
            if delivered {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }).await.expect("the exact merge handoff reply was not recorded");

    // Only the completed task phase is injected; the service finishes Git.
    let fixture_task = task.clone();
    runtime.host.executor.write(move |db| {
        db.execute("UPDATE pending_questions SET status='cancelled'
            WHERE task_id=? AND status='open'", [&fixture_task])?;
        db.execute("UPDATE runtime_tasks SET state='COMPLETED', blocker=NULL,
            synthesis='Staged the resolution.' WHERE task_id=?", [&fixture_task])?;
        Ok(())
    }).unwrap().await.unwrap().unwrap();
    drop(driver);
    let observed = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let board = call(&service, "product.board", &project, json!({})).await;
            let record = &board["tasks"][&draft];
            if record["merge"]["finished"] == true {
                break record.clone();
            }
            loop {
                use tokio::sync::broadcast::error::RecvError;
                match updates.recv().await {
                    Ok(update) if update["type"] == "project_state"
                        && update["projectId"] == project => break,
                    Ok(_) => {}
                    Err(RecvError::Lagged(_)) => break,
                    Err(RecvError::Closed) => panic!("workspace updates closed before merge settlement"),
                }
            }
        }
    }).await;
    let record = match observed {
        Ok(record) => record,
        Err(_) => {
            let board = call(&service, "product.board", &project, json!({})).await;
            let record = &board["tasks"][&draft];
            let trace = runtime.host.executor.read(move |db| {
                let phase: String = db.query_row("SELECT state FROM runtime_tasks WHERE task_id=?",
                    [&task], |row| row.get(0))?;
                let mut stmt = db.prepare("SELECT type FROM runtime_events WHERE task_id=? ORDER BY id DESC LIMIT 20")?;
                let events = stmt.query_map([&task], |row| row.get::<_,String>(0))?
                    .collect::<Result<Vec<_>,_>>()?;
                Ok((phase, events))
            }).unwrap().await.unwrap().unwrap();
            panic!("merge did not settle: phase/events={trace:?}, conflict={}, handoff={}, Git={}",
                record["conflict"], record["merge"]["handoff"]["state"], record["merge"]["gitState"]);
        }
    };
    assert_eq!(record["resolvedBy"], "cortex");
    assert_eq!(record["merge"]["handoff"]["state"], "completed");
    assert_eq!(record["merge"]["freshChecks"]["state"], "not_requested");
    assert_eq!(record["acceptance"], Value::Null);
    assert_eq!(git_fixture(&folder, &["show", "-s", "--format=%P", "HEAD"]),
        format!("{source} {target_head}"));
    assert_eq!(git_fixture(&folder, &["show", "-s", "--format=%T", "HEAD"]), staged_tree);
    assert_eq!(git_fixture(&target, &["rev-parse", "HEAD"]), target_head);
    assert!(!folder.join(".git/merge-fresh-check-prompts").exists());
    service.stop().await.unwrap();
}

#[tokio::test]
async fn owner_merge_handoff_refusal_leaves_the_exact_merge_available_to_abort() {
    let MergeFixture { _scratch, folder, target, target_head, service, project,
        draft, execution, conflict, .. } = merge_handoff_fixture().await;
    let head = git_fixture(&folder, &["rev-parse", "HEAD"]);
    let mut invalid = merge_request(&project, &execution, &conflict, "handoff_merge", "rule");
    invalid["text"] = json!("deny-write: work.txt");
    assert_eq!(service.control_execution(invalid).await.unwrap_err().to_string(), "merge_guidance_write_rule");
    std::fs::write(folder.join(".git/refuse-merge-route"), "").unwrap();
    let refused = merge_request(&project, &execution, &conflict, "handoff_merge", "refused-handoff");
    assert!(service.control_execution(refused).await.is_err());
    let board = call(&service, "product.board", &project, json!({})).await;
    assert_eq!(board["tasks"][&draft]["conflict"], "conflict");
    assert_eq!(board["tasks"][&draft]["merge"]["handoff"]["state"], "refused");
    assert!(!folder.join(".git/merge-handoff-prompts").exists());
    call(&service, "product.task", &project,
        json!({"action":"abort_merge","taskId":draft,"requestId":"abort-refused"})).await;
    assert_eq!(git_fixture(&folder, &["rev-parse", "HEAD"]), head);
    assert_eq!(git_fixture(&target, &["rev-parse", "HEAD"]), target_head);
    service.stop().await.unwrap();
}

#[tokio::test]
async fn owner_merge_handoff_inspection_of_a_replaced_operation_never_dispatches() {
    let MergeFixture { _scratch, folder, target, target_head, service, project,
        draft, execution, conflict, .. } = merge_handoff_fixture().await;
    git_fixture(&folder, &["merge", "--abort"]);
    git_fixture(&folder, &["commit", "--allow-empty", "-qm", "Unrelated commit"]);
    let head = git_fixture(&folder, &["rev-parse", "HEAD"]);
    let inspected = service.control_execution(merge_request(&project, &execution, &conflict,
        "inspect_merge", "inspect-replaced")).await.unwrap();
    assert_eq!(inspected["merge"]["gitState"], "unknown");
    assert_eq!(inspected["conflict"], "failed");
    assert!(!folder.join(".git/merge-handoff-prompts").exists());
    let reply = service.machine(serde_json::from_value(json!({"op":"product.task","projectId":project,
        "params":{"action":"abort_merge","taskId":draft,"requestId":"abort-replaced"}}))
        .unwrap()).await.unwrap();
    assert_eq!(reply["ok"], false, "{reply}");
    assert_eq!(git_fixture(&folder, &["rev-parse", "HEAD"]), head);
    assert_eq!(git_fixture(&target, &["rev-parse", "HEAD"]), target_head);
    service.stop().await.unwrap();
}

#[tokio::test]
async fn legacy_merge_external_abort_cannot_restore_the_old_result() {
    let MergeFixture {
        _scratch,
        folder,
        target,
        target_head,
        service,
        runtime,
        project,
        draft,
        task,
        old_actions,
        ..
    } = merge_handoff_fixture().await;
    let clear = |request| json!({
        "op":"product.task", "projectId":project,
        "params":{"action":"abort_merge","taskId":draft,"requestId":request,"discardLegacy":true}
    });
    let bound = service.machine(serde_json::from_value(clear("bound-merge")).unwrap())
        .await.unwrap();
    assert_eq!(bound["code"], "stale_conflict", "{bound}");

    // Pair the original branch-only overlay shape with a real owning execution.
    let product_path = _scratch.0.join("state/product").join(format!("{project}.json"));
    let mut product: Value = serde_json::from_slice(&std::fs::read(&product_path).unwrap()).unwrap();
    let record = &mut product["tasks"][&draft];
    let merge = record["merge"].as_object_mut().unwrap();
    for key in ["conflictId", "kind", "sourceRevision", "targetRevision", "gitState"] {
        assert!(merge.remove(key).is_some());
    }
    let before = merge.get_mut("before").unwrap().as_object_mut().unwrap();
    before.remove("abortSnapshot");
    before.remove("requiresFreshResult");
    // Model the old UI having started a resolution without a typed handoff.
    record["conflict"] = json!("resolving");
    std::fs::write(&product_path, product.to_string()).unwrap();

    let head = git_fixture(&folder, &["rev-parse", "HEAD"]);
    git_fixture(&folder, &["merge", "--abort"]);
    let phase = |phase: &'static str| {
        let task = task.clone();
        runtime.host.executor.write(move |db| {
            db.execute("UPDATE runtime_tasks SET state=? WHERE task_id=?", (phase, task))?;
            Ok(())
        }).unwrap()
    };
    phase("REVIEWING").await.unwrap().unwrap();
    let refused = service.machine(serde_json::from_value(clear("review-in-progress")).unwrap())
        .await.unwrap();
    assert_eq!(refused["code"], "stop_required", "{refused}");
    assert_eq!(std::fs::read(&product_path).unwrap(), product.to_string().as_bytes());
    phase("COMPLETED").await.unwrap().unwrap();

    let request = clear("externally-aborted-legacy");
    let reply = service.machine(serde_json::from_value(request.clone()).unwrap()).await.unwrap();
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(service.machine(serde_json::from_value(request).unwrap()).await.unwrap(), reply);
    let product: Value = serde_json::from_slice(&std::fs::read(product_path).unwrap()).unwrap();
    let record = &product["tasks"][&draft];
    assert_eq!(record["conflict"], "none");
    assert_eq!(record["merge"], Value::Null);
    assert_eq!(record["acceptance"], Value::Null);
    assert_eq!(record["requiresFreshResult"]["after"], json!({"task":task,"revision":0}));
    assert_eq!(git_fixture(&folder, &["rev-parse", "HEAD"]), head);
    assert_eq!(git_fixture(&target, &["rev-parse", "HEAD"]), target_head);
    assert!(!folder.join(".git/merge-handoff-prompts").exists());
    assert!(!folder.join(".git/merge-fresh-check-prompts").exists());

    let old = service.machine(serde_json::from_value(json!({
        "op":"product.task","projectId":project,"params":{
            "action":"accept","taskId":draft,"digest":old_actions["digest"],
            "version":old_actions["version"]
        }
    })).unwrap()).await.unwrap();
    assert_eq!(old["code"], "review_required", "{old}");
    service.stop().await.unwrap();
}

#[tokio::test]
async fn automatic_sprint_repairs_changed_accepted_result_from_saved_hold() {
    let (_scratch, folder, service, runtime, project, id, tasks) =
        automatic_fixture("dev", "main", "main").await;
    let mut jobs = Jobs::default();
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let draft = &tasks[0];
    let task = settled_peer(&runtime, draft).await;
    verified_fixture(&runtime, task.clone(), true).await;
    let accepted = call(
        &service,
        "product.task",
        &project,
        json!({"action":"auto_ready","taskId":draft}),
    )
    .await;
    assert_eq!(accepted["accepted"], true, "{accepted}");
    std::fs::write(folder.join("work.txt"), "changed since acceptance\n").unwrap();
    let expected = sprint(&service, &project, &id).await;
    service
        .automatic_pause(
            &project,
            &expected,
            automatic_hold(
                "settlement_blocked",
                Some(draft),
                "stale_result",
                "The workspace changed.",
                &HoldFacts::default(),
            ),
        )
        .await
        .unwrap();
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let after = sprint(&service, &project, &id).await;
    let board = call(&service, "product.board", &project, json!({})).await;
    let repairs: i64 = runtime.host.executor.read(move |db| {
        Ok(db.query_row("SELECT count(*) FROM runtime_events WHERE task_id=? AND type='AUTOMATIC_REPAIR_REQUESTED'", [&task], |r| r.get(0))?)
    }).unwrap().await.unwrap().unwrap();
    assert_eq!(repairs, 1, "persisted hold: {after}");
    assert_eq!(after["queueState"], "running");
    assert!(board["tasks"][draft]["acceptance"].is_null());
    assert_ne!(board["tasks"][draft]["status"], "done");
    assert_eq!(git_fixture(&folder, &["log", "-1", "--format=%s"]), "Base");
    let peer_input = folder.join(".git/automatic-repair-input");
    tokio::time::timeout(Duration::from_secs(8), async {
        while !peer_input.exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("assigned coordinator receives the repair instruction");
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    assert_eq!(std::fs::read_to_string(peer_input).unwrap(), "repair\n");
    service.stop().await.unwrap();
}

#[tokio::test]
async fn archive_slow_driver_records_an_unknown_pause() {
    let (_scratch, _folder, service, runtime, project, _id, tasks) =
        automatic_fixture("dev", "main", "main").await;
    service.sprint_pass(&project, &runtime, &mut Jobs::default(), false).await.unwrap();
    let draft = &tasks[0];
    let task = settled_peer(&runtime, draft).await;
    let (cancel, cancelled) = tokio::sync::watch::channel(false);
    let (done, finished) = tokio::sync::watch::channel(false);
    runtime.install_pump(task.clone(), cancel, finished).await;

    // Keep the driver alive through the entire stop budget.
    tokio::time::timeout(Duration::from_secs(9), call(
        &service, "product.task", &project, json!({"action":"archive","taskId":draft}),
    )).await.expect("archive waits only a bounded time for the driver");
    assert!(*cancelled.borrow(), "the admitted Pause asked the driver to stop");
    assert!(!*done.borrow());
    let board = call(&service, "product.board", &project, json!({})).await;
    assert_eq!(board["tasks"][draft]["archive"]["stopUnconfirmed"], true);
    runtime.host.executor.read(move |db| {
        let (state, outcome, reason): (String, String, String) = db.query_row(
            "SELECT t.state,json_extract(f.result_json,'$.outcome'),json_extract(f.result_json,'$.reason')
             FROM runtime_tasks t JOIN runtime_requests i USING(task_id)
             JOIN runtime_requests f USING(task_id,request_key)
             WHERE t.task_id=? AND i.operation='native_desktop_control'
             AND i.request_key LIKE 'archive-stop-%' AND f.actor='native-desktop-finish'",
            [&task], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        assert_eq!(state, "PAUSED");
        assert_eq!(outcome, "unknown");
        assert_eq!(reason, "archive_stop_timeout");
        Ok(())
    }).unwrap().await.unwrap().unwrap();
    done.send_replace(true);
    service.stop().await.unwrap();
}

#[tokio::test]
async fn archived_then_restored_result_refuses_stale_repair_dispatch() {
    use crate::machine::task_auto_repair::{Dispatch, RepairPlan, dispatch};
    let (_scratch, _folder, service, runtime, project, id, tasks) =
        automatic_fixture("dev", "main", "main").await;
    service.sprint_pass(&project, &runtime, &mut Jobs::default(), false).await.unwrap();
    let draft = &tasks[0];
    let task = settled_peer(&runtime, draft).await;
    verified_fixture(&runtime, task.clone(), true).await;
    let accepted = call(&service, "product.task", &project,
        json!({"action":"auto_ready","taskId":draft})).await;
    assert_eq!(accepted["accepted"], true, "{accepted}");
    let selected = call(&service, "product.read", &project, json!({"taskIds":[draft]})).await;
    let version = selected["tasks"][draft]["result"]["sourceVersion"].as_str().unwrap();
    let execution = service.native_tasks(&project).await.unwrap()[draft]["executionId"].clone();
    let expected = sprint(&service, &project, &id).await;
    for action in ["archive", "restore"] {
        call(&service, "product.task", &project, json!({"action":action,"taskId":draft})).await;
    }
    let state = &service.config.state_dir;
    let before = crate::machine::native_product(state, &project).unwrap();
    assert_eq!(before["tasks"][draft]["archiveHold"], "stopped");
    assert_eq!(before["tasks"][draft]["acceptance"]["automatic"], true);
    assert_eq!(sprint(&service, &project, &id).await, expected);
    let admission = dispatch(state, &runtime, &project, &expected, draft,
        execution.as_str().unwrap(), Some(version),
        RepairPlan { prompt: "stale repair".into(), fingerprint: "stale-archive-fixture".into() },
        false).await.unwrap();
    assert!(matches!(admission, Dispatch::Refused(None)));
    assert_eq!(crate::machine::native_product(state, &project).unwrap(), before);
    assert_eq!(repairs_of(&runtime, &task).await, 0);
    assert_eq!(service.native_tasks(&project).await.unwrap()[draft]["phase"], "result");
    service.stop().await.unwrap();
}

#[tokio::test]
async fn archived_and_restored_hybrid_members_leave_a_paused_sprint_idle() {
    let (_scratch, _folder, service, runtime, project, id, tasks) =
        automatic_fixture("dev", "main", "main").await;
    call(&service, "product.sprint", &project, json!({"action":"patch","sprintId":id,
        "patch":{"runMode":"hybrid","queueState":"paused"}})).await;
    let mut jobs = Jobs::default();
    // The fixture has poll candidates before they are shelved.
    assert!(service.sprint_pass(&project, &runtime, &mut jobs, false).await.unwrap());
    for draft in &tasks {
        call(&service, "product.task", &project,
            json!({"action":"archive","taskId":draft})).await;
    }
    call(&service, "product.task", &project,
        json!({"action":"restore","taskId":tasks[1]})).await;
    let current = sprint(&service, &project, &id).await;
    assert_eq!(current["queueState"], "paused");
    assert_eq!(current["runMode"], "hybrid");
    assert!(!service.sprint_pass(&project, &runtime, &mut jobs, false).await.unwrap());
    service.stop().await.unwrap();
}

async fn repairs_of(runtime: &Runtime, task: &str) -> i64 {
    let task = task.to_owned();
    runtime.host.executor.read(move |db| {
        Ok(db.query_row("SELECT count(*) FROM runtime_events WHERE task_id=? AND type='AUTOMATIC_REPAIR_REQUESTED'", [&task], |r| r.get(0))?)
    }).unwrap().await.unwrap().unwrap()
}

async fn fail_run(runtime: &Runtime, task: &str, blocker: &str) {
    let (t, b) = (task.to_owned(), blocker.to_owned());
    runtime.host.executor.write(move |db| {
        db.execute("UPDATE runtime_tasks SET state='FAILED',blocker=? WHERE task_id=?", (&b, &t))?;
        // A failed run has no open question (the fake peer's Hybrid turn leaves one).
        db.execute("UPDATE pending_questions SET status='cancelled' WHERE task_id=? AND status='open'", [&t])?;
        cortex_core::lifecycle::event_tx(db, &t, "TASK_STATE_CHANGED", json!({"state":"FAILED","blocker":b}))?;
        Ok(())
    }).unwrap().await.unwrap().unwrap();
}

/// A run that failed with every effect known (here: an answer the runtime could
/// not certify because review was missing) goes back to the agent by itself; the
/// owner is asked only after the bounded repairs, and Allow sends it back once more.
#[tokio::test]
async fn automatic_sprint_sends_a_failed_run_back_to_the_agent_then_asks_the_owner() {
    let (_scratch, _folder, service, runtime, project, id, tasks) =
        automatic_fixture("dev", "main", "main").await;
    let mut jobs = Jobs::default();
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let task = settled_peer(&runtime, &tasks[0]).await;
    let blocker = "answer_not_certified:completion_review_missing";
    fail_run(&runtime, &task, blocker).await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let after = sprint(&service, &project, &id).await;
    assert_eq!(repairs_of(&runtime, &task).await, 1, "{after}");
    assert_eq!(after["queueState"], "running", "{after}");
    assert!(after["automaticQueue"].is_null(), "{after}");
    // The same failure again: the deduplicated, bounded repair is spent, so the
    // owner gets one decision with explicit asks and actions.
    let task = settled_peer(&runtime, &tasks[0]).await;
    fail_run(&runtime, &task, blocker).await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let held = sprint(&service, &project, &id).await;
    let hold = &held["automaticQueue"];
    assert_eq!(hold["code"], "task_failed", "{held}");
    assert_eq!(hold["nextActor"], "owner");
    assert_eq!(hold["ownerActions"], json!(["allow_repair", "close_as_is"]));
    assert!(!hold["asks"][0].as_str().unwrap().contains('_'), "{hold}");
    assert_eq!(repairs_of(&runtime, &task).await, 1);
    // Another pass without a decision changes nothing and dispatches nothing.
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    assert_eq!(sprint(&service, &project, &id).await, held);
    assert_eq!(repairs_of(&runtime, &task).await, 1);
    // A stale decision is refused; the current one is recorded once.
    let stale = service
        .machine(
            serde_json::from_value(json!({"op":"product.sprint","projectId":project,
            "params":{"action":"allow","sprintId":id,"taskId":tasks[0],"code":"task_stopped","ownerAction":"allow_repair"}}))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stale["code"], "automatic_hold_changed", "{stale}");
    call(
        &service,
        "product.sprint",
        &project,
        json!({"action":"allow","sprintId":id,"taskId":tasks[0],"code":"task_failed","ownerAction":"allow_repair"}),
    )
    .await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let resumed = sprint(&service, &project, &id).await;
    assert_eq!(repairs_of(&runtime, &task).await, 2, "{resumed}");
    assert_eq!(resumed["queueState"], "running", "{resumed}");
    assert!(resumed["automaticQueue"].is_null(), "{resumed}");
    service.stop().await.unwrap();
}

/// Hybrid follows the same next-actor rule: a failed run goes back to its agent,
/// and a cancelled prerequisite holds only its dependent, named, with actions.
#[tokio::test]
async fn hybrid_sprint_repairs_a_failed_run_and_names_a_blocked_dependent() {
    let (_scratch, _folder, service, runtime, project, id, tasks) =
        automatic_fixture("dev", "main", "main").await;
    call(
        &service,
        "product.sprint",
        &project,
        json!({"action":"patch","sprintId":id,"patch":{"runMode":"hybrid"}}),
    )
    .await;
    let mut jobs = Jobs::default();
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let task = settled_peer(&runtime, &tasks[0]).await;
    fail_run(
        &runtime,
        &task,
        "answer_not_certified:completion_nodes_incomplete",
    )
    .await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let after = sprint(&service, &project, &id).await;
    assert_eq!(repairs_of(&runtime, &task).await, 1, "{after}");
    assert_eq!(after["queueState"], "running", "{after}");
    // The owner cancels the first task: its dependent is held, by name.
    settled_peer(&runtime, &tasks[0]).await;
    call(
        &service,
        "product.task",
        &project,
        json!({"action":"close","taskId":tasks[0],"choice":"cancel","confirmed":true}),
    )
    .await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let held = sprint(&service, &project, &id).await;
    let hold = &held["automaticQueue"];
    assert_eq!(held["queueState"], "paused", "{held}");
    assert_eq!(hold["code"], "dependencies_blocked", "{held}");
    assert_eq!(hold["taskId"], tasks[1]);
    assert_eq!(hold["nextActor"], "owner");
    assert_eq!(hold["ownerActions"], json!(["close_as_is"]));
    assert!(
        hold["reason"].as_str().unwrap().contains("cancelled"),
        "{hold}"
    );
    // Closing the dependent clears the hold by itself; no Continue queue needed.
    call(
        &service,
        "product.task",
        &project,
        json!({"action":"close","taskId":tasks[1],"choice":"cancel","confirmed":true}),
    )
    .await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let done = sprint(&service, &project, &id).await;
    assert_eq!(done["queueState"], "complete", "{done}");
    service.stop().await.unwrap();
}

#[tokio::test]
async fn automatic_sprint_repairs_content_unknown_without_pausing_or_touching_unrelated_work(
) {
    let (_scratch, folder, service, runtime, project, id, tasks) =
        automatic_fixture("dev", "main", "main").await;
    let mut jobs = Jobs::default();
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let task = settled_peer(&runtime, &tasks[0]).await;
    // Capture the verified result with an unrelated staged file already
    // present. Automatic recovery must preserve it while reconciling the
    // task's recorded path.
    std::fs::write(folder.join("unrelated.txt"), "keep staged\n").unwrap();
    git_fixture(&folder, &["add", "unrelated.txt"]);
    verified_fixture_at(
        &runtime,
        task.clone(),
        true,
        b"edited independently before verification\n",
    )
    .await;
    let accepted = call(
        &service,
        "product.task",
        &project,
        json!({"action":"auto_ready","taskId":tasks[0]}),
    )
    .await;
    assert_eq!(accepted["accepted"], true, "{accepted}");

    // The recorded task file becomes unlocatable. Recovery must not commit
    // or drop the unrelated staged file.

    let facts = call(
        &service,
        "product.read",
        &project,
        json!({"taskIds":[tasks[0]]}),
    )
    .await;
    assert_eq!(
        facts["tasks"][&tasks[0]]["integrationFacts"]["reason"],
        "Task files changed again or their recorded content cannot be located. Finish in Git and use Done outside the app, or close as is."
    );

    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let repaired = sprint(&service, &project, &id).await;
    assert_eq!(repaired["queueState"], "running", "{repaired}");
    assert!(repaired["automaticQueue"].is_null(), "{repaired}");
    let repairs: i64 = runtime.host.executor.read(move |db| {
        Ok(db.query_row("SELECT count(*) FROM runtime_events WHERE task_id=? AND type='AUTOMATIC_REPAIR_REQUESTED'", [&task], |r| r.get::<_, i64>(0))?)
    }).unwrap().await.unwrap().unwrap();
    assert_eq!(repairs, 1, "{repaired}");
    let staged = git_fixture(&folder, &["diff", "--cached", "--name-only"]);
    assert_eq!(staged, "unrelated.txt");
    let peer_input = folder.join(".git/automatic-repair-input");
    tokio::time::timeout(Duration::from_secs(8), async {
        while !peer_input.exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("assigned coordinator receives the repair instruction");
    let prompt = folder.join(".git/automatic-repair-prompt");
    tokio::time::timeout(Duration::from_secs(8), async {
        while !prompt.exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("repair prompt is retained for the fixture");
    let prompt = std::fs::read_to_string(prompt).unwrap();
    assert!(prompt.contains("work.txt"), "{prompt}");
    assert!(prompt.contains("reconcile its recorded files"), "{prompt}");
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    assert_eq!(std::fs::read_to_string(peer_input).unwrap(), "repair\n");
    service.stop().await.unwrap();
}

#[tokio::test]
async fn automatic_sprint_finishes_exact_task_paths_then_starts_the_dependent() {
    let (_scratch, folder, service, runtime, project, id, tasks) =
        automatic_fixture("dev", "main", "main").await;
    let mut jobs = Jobs::default();
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let task = settled_peer(&runtime, &tasks[0]).await;
    let hook = folder.join(".git/hooks/pre-commit");
    std::fs::write(&hook, "#!/bin/sh\n: > .git/fixture-hook-ran\n").unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(folder.join("unrelated.txt"), "keep staged\n").unwrap();
    git_fixture(&folder, &["add", "unrelated.txt"]);
    verified_fixture(&runtime, task, true).await;
    let head = git_fixture(&folder, &["rev-parse", "HEAD"]);
    let wire: crate::machine::MachineCall = serde_json::from_value(json!({
        "op":"automatic.finish","projectId":project,"params":{"sprintId":id,"taskId":tasks[0]}
    })).unwrap();
    assert!(
        service.machine(wire).await.is_err(),
        "wire clients cannot invoke scheduler effects"
    );
    assert_eq!(git_fixture(&folder, &["rev-parse", "HEAD"]), head);

    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let board = call(&service, "product.board", &project, json!({})).await;
    let first = &board["tasks"][&tasks[0]];
    assert_eq!(first["status"], "done", "{first}");
    assert_eq!(first["acceptance"]["automatic"], true);
    assert_eq!(first["integration"]["automatic"], true);
    assert_eq!(first["integration"]["committed"], true);
    assert_eq!(first["integration"]["targetBranch"], "dev");
    assert_eq!(
        git_fixture(&folder, &["show", "--format=", "--name-only", "HEAD"]),
        "work.txt"
    );
    assert_eq!(
        git_fixture(&folder, &["diff", "--cached", "--name-only"]),
        "unrelated.txt"
    );
    assert!(folder.join(".git/fixture-hook-ran").exists());

    let landed = git_fixture(&folder, &["rev-parse", "HEAD"]);
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let projected = service.native_tasks(&project).await.unwrap();
    assert!(projected[&tasks[1]]["executionId"].is_string());
    assert_eq!(
        projected[&tasks[1]]["automaticContinuation"]["policy"],
        "automatic_continuation"
    );
    assert_eq!(git_fixture(&folder, &["rev-parse", "HEAD"]), landed);
    service.stop().await.unwrap();
}

#[tokio::test]
async fn automatic_sprint_skips_cancelled_and_archived_members_but_not_dependencies() {
    for archive in [false, true] {
        let (_scratch, folder, service, runtime, project, id, tasks) =
            automatic_fixture("dev", "main", "main").await;
        let mut jobs = Jobs::default();
        service
            .sprint_pass(&project, &runtime, &mut jobs, false)
            .await
            .unwrap();
        settled_peer(&runtime, &tasks[0]).await;
        call(
            &service,
            "product.task",
            &project,
            json!({
                "action":"close","taskId":tasks[0],"choice":"cancel","confirmed":true
            }),
        )
        .await;
        if archive {
            call(
                &service,
                "product.task",
                &project,
                json!({"action":"archive","taskId":tasks[0]}),
            )
            .await;
        }
        let board = call(&service, "product.board", &project, json!({})).await;
        assert_eq!(board["tasks"][&tasks[0]]["closedAs"], "cancel");
        assert_eq!(board["tasks"][&tasks[0]]["archive"].is_object(), archive);

        // A third, independent task must run even while the original
        // second member cannot use the cancelled member as a prerequisite.
        let created = service.workspace(json!({"protocol":{"major":1,"minor":8},
            "requestId":"independent-draft","projectId":project,"actorId":actor(),
            "operation":"tasks.create","target":project,"expectedVersion":"0","ownerEpoch":null,
            "payload":{"description":"Independent work","checkout":{"kind":"main"}}
        })).await.unwrap();
        let independent = created["target"].as_str().unwrap();
        call(
            &service,
            "product.task",
            &project,
            json!({"action":"sprint","taskId":independent,
            "sprintId":id,"dependencies":[]}),
        )
        .await;
        call(
            &service,
            "product.sprint",
            &project,
            json!({"action":"patch","sprintId":id,
            "patch":{"taskIds":[tasks[0],tasks[1],independent]}}),
        )
        .await;

        service
            .sprint_pass(&project, &runtime, &mut jobs, false)
            .await
            .unwrap();
        let projection = service.native_tasks(&project).await.unwrap();
        assert!(projection[independent]["executionId"].is_string());
        assert!(projection[&tasks[1]]["executionId"].is_null());
        let other = settled_peer(&runtime, independent).await;
        verified_fixture(&runtime, other, true).await;
        service
            .sprint_pass(&project, &runtime, &mut jobs, false)
            .await
            .unwrap();
        service
            .sprint_pass(&project, &runtime, &mut jobs, false)
            .await
            .unwrap();
        let held = sprint(&service, &project, &id).await;
        assert_eq!(
            held["automaticQueue"]["code"], "dependencies_blocked",
            "{held}"
        );
        assert_eq!(held["automaticQueue"]["needsOwner"], true);
        service
            .sprint_pass(&project, &runtime, &mut jobs, false)
            .await
            .unwrap();
        assert_eq!(sprint(&service, &project, &id).await, held);
        call(
            &service,
            "product.task",
            &project,
            json!({"action":"close","taskId":tasks[1],
            "choice":"cancel","confirmed":true}),
        )
        .await;
        service
            .sprint_pass(&project, &runtime, &mut jobs, false)
            .await
            .unwrap();
        assert_eq!(
            sprint(&service, &project, &id).await["queueState"],
            "running"
        );
        service
            .sprint_pass(&project, &runtime, &mut jobs, false)
            .await
            .unwrap();
        assert_eq!(
            sprint(&service, &project, &id).await["queueState"],
            "complete"
        );
        assert!(
            service.native_tasks(&project).await.unwrap()[&tasks[1]]["executionId"]
                .is_null()
        );
        assert_eq!(git_fixture(&folder, &["rev-list", "--count", "HEAD"]), "2");
        service.stop().await.unwrap();
    }
}

#[tokio::test]
async fn automatic_sprint_waits_for_confirmed_cross_branch_integration() {
    let (_scratch, folder, service, runtime, project, id, tasks) =
        automatic_fixture("main", "main", "main").await;
    let mut jobs = Jobs::default();
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let task = settled_peer(&runtime, &tasks[0]).await;
    verified_fixture(&runtime, task, true).await;
    let head = git_fixture(&folder, &["rev-parse", "HEAD"]);
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let projected = sprint(&service, &project, &id).await;
    assert_eq!(projected["queueState"], "paused", "{projected}");
    assert_eq!(projected["automaticQueue"]["kind"], "owner_integration");
    assert_eq!(projected["automaticQueue"]["needsOwner"], true);
    assert_eq!(projected["automaticQueue"]["taskId"], tasks[0]);
    assert_eq!(git_fixture(&folder, &["rev-parse", "HEAD"]), head);
    assert_eq!(git_fixture(&folder, &["rev-parse", "main"]), head);
    assert!(
        service.native_tasks(&project).await.unwrap()[&tasks[1]]["executionId"].is_null()
    );
    let board = call(&service, "product.board", &project, json!({})).await;
    let first = &board["tasks"][&tasks[0]];
    assert_eq!(first["status"], "ready_to_merge", "{first}");
    assert_eq!(first["acceptance"]["automatic"], true);
    let result = call(
        &service,
        "product.integrate",
        &project,
        json!({"confirmed":true,
        "taskId":tasks[0],"method":"squash","message":"Owner-confirmed integration",
        "digest":first["acceptance"]["digest"],"requestId":"owner-finish"}),
    )
    .await;
    assert_eq!(result["done"], true, "{result}");
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    assert_eq!(
        sprint(&service, &project, &id).await["queueState"],
        "running"
    );
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    assert!(
        service.native_tasks(&project).await.unwrap()[&tasks[1]]["executionId"].is_string()
    );
    service.stop().await.unwrap();
}

#[tokio::test]
async fn automatic_sprint_refuses_both_sprint_and_native_draft_worktrees() {
    // A sprint saved with per-task worktrees (no `shared` marker) and a worktree draft in
    // a project-folder sprint both stay refused; a shared sprint worktree is allowed.
    for (legacy_sprint, task_checkout) in [(true, "main"), (false, "worktree")] {
        let (scratch, root, service, runtime, project, id, tasks) =
            automatic_fixture("dev", "main", task_checkout).await;
        if legacy_sprint {
            let path = crate::machine::native::product_path(&scratch.0.join("state"), &project).unwrap();
            let mut saved = crate::machine::settings::read_json(&path, json!({})).unwrap();
            saved["sprints"][0]["checkout"] = json!({"kind":"worktree"});
            crate::machine::settings::write_json(&path, &saved).unwrap();
        }
        service
            .sprint_pass(&project, &runtime, &mut Jobs::default(), false)
            .await
            .unwrap();
        let projected = sprint(&service, &project, &id).await;
        assert_eq!(projected["queueState"], "paused", "{projected}");
        assert_eq!(projected["automaticQueue"]["kind"], "start_refused");
        assert_eq!(
            projected["automaticQueue"]["code"],
            "native_worktree_unavailable"
        );
        assert_eq!(projected["automaticQueue"]["needsOwner"], true);
        assert!(projected["pauseReason"]
            .as_str()
            .unwrap()
            .contains("separate worktree"));
        assert_eq!(
            git_fixture(&root, &["worktree", "list", "--porcelain"])
                .lines()
                .filter(|line| line.starts_with("worktree "))
                .count(),
            1
        );
        assert!(
            service.native_tasks(&project).await.unwrap()[&tasks[0]]["executionId"]
                .is_null()
        );
        service.stop().await.unwrap();
    }
}

/// Automatic is allowed in one worktree shared by the whole sprint: the first member starts
/// there (one linked worktree on the sprint branch, nothing in the project folder) and the
/// queue is neither paused nor refused; the dependent member waits for it.
#[tokio::test]
async fn automatic_runs_a_shared_sprint_worktree_and_starts_its_first_member_there() {
    let (scratch, root, service, runtime, project, id, tasks) =
        automatic_fixture("dev", "worktree", "worktree").await;
    let reply = service.machine(serde_json::from_value(json!({
        "op":"machine.update","params":{"patch":{"worktreeRoot":scratch.0.join("trees")}}
    })).unwrap()).await.unwrap();
    assert_eq!(reply["ok"], true, "{reply}");
    let sprint_id = id.as_str().unwrap().to_owned();
    assert_eq!(sprint(&service, &project, &id).await["checkout"], json!({"kind":"worktree","shared":true}));
    // Materialization saves this marker on every member of a shared sprint.
    for task in tasks.clone() {
        let sprint_id = sprint_id.clone();
        runtime.host.executor.write(move |db| {
            db.execute(
                "UPDATE workspace_task_options SET options_json=json_set(options_json,'$.sprintWorktree',json(?)) WHERE task_id=?",
                (json!({"sprintId":sprint_id,"title":"Shared"}).to_string(), &task),
            )?;
            Ok(())
        }).unwrap().await.unwrap().unwrap();
    }
    service.sprint_pass(&project, &runtime, &mut Jobs::default(), false).await.unwrap();
    let projected = sprint(&service, &project, &id).await;
    assert_eq!(projected["queueState"], "running", "{projected}");
    assert!(projected["automaticQueue"].is_null(), "{projected}");
    let facts = service.native_tasks(&project).await.unwrap();
    assert!(facts[&tasks[0]]["executionId"].is_string(), "{facts:?}");
    assert!(facts[&tasks[1]]["executionId"].is_null());
    let trees = git_fixture(&root, &["worktree", "list", "--porcelain"]);
    assert_eq!(trees.lines().filter(|l| l.starts_with("worktree ")).count(), 2, "{trees}");
    assert!(trees.contains("branch refs/heads/cw/shared-"), "{trees}");
    service.stop().await.unwrap();
}

#[tokio::test]
async fn a_retired_core_pause_is_resumed_by_the_runtime_not_held_for_the_owner() {
    let (_scratch, folder, service, runtime, project, id, tasks) =
        automatic_fixture("dev", "main", "main").await;
    let mut jobs = Jobs::default();
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let task = settled_peer(&runtime, &tasks[0]).await;
    let t = task.clone();
    // A pause an earlier version recorded for the owner (an attempt cap).
    runtime.host.executor.write(move |db| {
        db.execute("UPDATE runtime_tasks SET state='RECOVERING',blocker='automatic_continuation_paused' WHERE task_id=?", [&t])?;
        cortex_core::lifecycle::event_tx(db, &t, "AUTOMATIC_CONTINUATION", json!({"reason":"Retry checks"}))?;
        cortex_core::lifecycle::event_tx(db, &t, "AUTOMATIC_PAUSED",
            json!({"pause_kind":"continuation_cap","reason":"The continuation limit was reached."}))?;
        Ok(())
    }).unwrap().await.unwrap().unwrap();
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    // No-stop policy: the runtime resumes it with a changed strategy by itself.
    let resumed = runtime.host.executor.read(move |db| {
        Ok(db.query_row("SELECT count(*) FROM runtime_events WHERE task_id=? AND type='AUTOMATIC_RESUMED'",
            [&task], |row| row.get::<_, i64>(0))?)
    }).unwrap().await.unwrap().unwrap();
    assert!(resumed >= 1, "{resumed}");
    let sprint = sprint(&service, &project, &id).await;
    assert_ne!(sprint["automaticQueue"]["needsOwner"], true, "{sprint}");
    assert_eq!(git_fixture(&folder, &["rev-list", "--count", "HEAD"]), "1");
    service.stop().await.unwrap();
}

#[tokio::test]
async fn automatic_sprint_rechecks_acceptance_when_verification_becomes_available() {
    let (_scratch, folder, service, runtime, project, id, tasks) =
        automatic_fixture("dev", "main", "main").await;
    let mut jobs = Jobs::default();
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let task = settled_peer(&runtime, &tasks[0]).await;
    // A result phase alone is not verification evidence. The fixture
    // publishes the passing result only after the queue first refuses it.
    let t = task.clone();
    runtime
        .host
        .executor
        .write(move |db| {
            db.execute(
                "UPDATE runtime_tasks SET state='COMPLETED',blocker=NULL WHERE task_id=?",
                [&t],
            )?;
            Ok(())
        })
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let held = sprint(&service, &project, &id).await;
    assert_eq!(
        held["automaticQueue"]["code"], "automatic_acceptance_blocked",
        "{held}"
    );
    assert_eq!(held["automaticQueue"]["needsOwner"], true);
    assert!(
        service.native_tasks(&project).await.unwrap()[&tasks[1]]["executionId"].is_null()
    );
    assert_eq!(git_fixture(&folder, &["rev-list", "--count", "HEAD"]), "1");

    verified_fixture(&runtime, task, true).await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    assert_eq!(
        sprint(&service, &project, &id).await["queueState"],
        "running"
    );
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    assert_eq!(git_fixture(&folder, &["rev-list", "--count", "HEAD"]), "2");
    service.stop().await.unwrap();
}

#[tokio::test]
async fn automatic_sprint_requires_owner_finish_after_explicit_acceptance() {
    let (_scratch, folder, service, runtime, project, id, tasks) =
        automatic_fixture("dev", "main", "main").await;
    let mut jobs = Jobs::default();
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let task = settled_peer(&runtime, &tasks[0]).await;
    verified_fixture(&runtime, task, true).await;
    let board = call(&service, "product.board", &project, json!({})).await;
    let facts = runtime
        .owner_facts(&project, board, json!({}))
        .await
        .unwrap();
    let review = &facts["product"]["tasks"][&tasks[0]]["reviewActions"];
    assert_eq!(review["accept"], true, "{review}");
    call(&service, "product.task", &project,
        json!({"action":"accept","taskId":tasks[0],"version":review["version"],"digest":review["digest"]})).await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let held = sprint(&service, &project, &id).await;
    assert_eq!(
        held["automaticQueue"]["kind"], "owner_integration",
        "{held}"
    );
    assert_eq!(held["automaticQueue"]["code"], "owner_accepted");
    assert_eq!(git_fixture(&folder, &["rev-list", "--count", "HEAD"]), "1");
    call(&service, "product.integrate", &project,
        json!({"confirmed":true,"method":"commit","taskId":tasks[0],"digest":review["digest"],
            "message":"Finish explicitly accepted work","requestId":"owner-commit"})).await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    assert_eq!(
        sprint(&service, &project, &id).await["queueState"],
        "running"
    );
    assert_eq!(git_fixture(&folder, &["rev-list", "--count", "HEAD"]), "2");
    service.stop().await.unwrap();
}

#[tokio::test]
async fn automatic_sprint_owner_finish_consent_finishes_and_advances() {
    let (_scratch, folder, service, runtime, project, id, tasks) =
        automatic_fixture("dev", "main", "main").await;
    let mut jobs = Jobs::default();
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let task = settled_peer(&runtime, &tasks[0]).await;
    verified_fixture(&runtime, task.clone(), true).await;
    runtime.host.executor.write(move |db| {
        let checks = json!([{"name":"Staging deployment", "outcome":"not_run", "blocker":"Deployment is prohibited by policy."}]);
        db.execute("UPDATE runtime_nodes SET result_json=json_set(result_json,'$.checks',json(?))
            WHERE task_id=? AND stage='review'", (checks.to_string(), &task))?;
        Ok(())
    }).unwrap().await.unwrap().unwrap();
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    assert_eq!(
        sprint(&service, &project, &id).await["automaticQueue"]["code"],
        "automatic_acceptance_blocked"
    );
    let board = call(&service, "product.board", &project, json!({})).await;
    let facts = runtime
        .owner_facts(&project, board, json!({}))
        .await
        .unwrap();
    let review = &facts["product"]["tasks"][&tasks[0]]["reviewActions"];
    assert_eq!(review["acknowledgeOpenChecks"], true, "{review}");
    call(
        &service,
        "product.task",
        &project,
        json!({"action":"accept","taskId":tasks[0],"version":review["version"],
            "digest":review["digest"],"acknowledgeOpenChecks":true,"automaticFinish":{"sprintId":id,"targetBranch":"dev"}}),
    )
    .await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let board = call(&service, "product.board", &project, json!({})).await;
    let first = &board["tasks"][&tasks[0]];
    assert_eq!(first["status"], "done", "{first}");
    assert_eq!(first["integration"]["automatic"], true);
    assert_eq!(first["acceptance"]["open"]["not_run"], 1);
    assert_eq!(first["integration"]["targetBranch"], "dev");
    assert_eq!(
        sprint(&service, &project, &id).await["queueState"],
        "running"
    );
    assert_eq!(git_fixture(&folder, &["rev-list", "--count", "HEAD"]), "2");
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    assert!(
        service.native_tasks(&project).await.unwrap()[&tasks[1]]["executionId"].is_string()
    );
    service.stop().await.unwrap();
}

#[tokio::test]
async fn automatic_sprint_owner_finish_consent_completes_manual_owner_acceptance() {
    let (_scratch, folder, service, runtime, project, id, tasks) =
        automatic_fixture("dev", "main", "main").await;
    let mut jobs = Jobs::default();
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let task = settled_peer(&runtime, &tasks[0]).await;
    verified_fixture(&runtime, task, true).await;
    let board = call(&service, "product.board", &project, json!({})).await;
    let facts = runtime
        .owner_facts(&project, board, json!({}))
        .await
        .unwrap();
    let review = &facts["product"]["tasks"][&tasks[0]]["reviewActions"];
    call(
        &service,
        "product.task",
        &project,
        json!({"action":"accept","taskId":tasks[0],"version":review["version"],"digest":review["digest"]}),
    )
    .await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    assert_eq!(
        sprint(&service, &project, &id).await["automaticQueue"]["code"],
        "owner_accepted"
    );
    let board = call(&service, "product.board", &project, json!({})).await;
    let acceptance = board["tasks"][&tasks[0]]["acceptance"].clone();
    call(
        &service,
        "product.task",
        &project,
        json!({"action":"accept","taskId":tasks[0],"version":acceptance["version"],
            "digest":acceptance["digest"],"automaticFinish":{"sprintId":id,"targetBranch":"dev"}}),
    )
    .await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let board = call(&service, "product.board", &project, json!({})).await;
    assert_eq!(
        board["tasks"][&tasks[0]]["status"],
        "done",
        "task={:?} sprint={:?}",
        board["tasks"][&tasks[0]],
        sprint(&service, &project, &id).await
    );
    assert_eq!(git_fixture(&folder, &["rev-list", "--count", "HEAD"]), "2");
    service.stop().await.unwrap();
}

#[tokio::test]
async fn automatic_sprint_owner_finish_consent_rejects_stale_target_version_and_mode() {
    let (_scratch, _folder, service, runtime, project, id, tasks) =
        automatic_fixture("dev", "main", "main").await;
    let mut jobs = Jobs::default();
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let task = settled_peer(&runtime, &tasks[0]).await;
    let t = task.clone();
    runtime
        .host
        .executor
        .write(move |db| {
            db.execute(
                "UPDATE runtime_tasks SET state='COMPLETED',blocker=NULL WHERE task_id=?",
                [&t],
            )?;
            Ok(())
        })
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    verified_fixture(&runtime, task, true).await;
    let board = call(&service, "product.board", &project, json!({})).await;
    let facts = runtime
        .owner_facts(&project, board, json!({}))
        .await
        .unwrap();
    let review = facts["product"]["tasks"][&tasks[0]]["reviewActions"].clone();
    let wrong_target = service
        .machine(
            serde_json::from_value(json!({"op":"product.task","projectId":project,
                "params":{"action":"accept","taskId":tasks[0],"version":review["version"],
                "digest":review["digest"],"automaticFinish":{"sprintId":id,"targetBranch":"main"}}}))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(wrong_target["ok"], false);
    assert_eq!(wrong_target["code"], "stale_result");
    let wrong_version = service
        .machine(
            serde_json::from_value(json!({"op":"product.task","projectId":project,
                "params":{"action":"accept","taskId":tasks[0],"version":"wrong",
                "digest":review["digest"],"automaticFinish":{"sprintId":id,"targetBranch":"dev"}}}))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(wrong_version["ok"], false);
    assert_eq!(wrong_version["code"], "stale_result");
    call(
        &service,
        "product.sprint",
        &project,
        json!({"action":"patch","sprintId":id,"patch":{"runMode":"manual"}}),
    )
    .await;
    let wrong_mode = service
        .machine(
            serde_json::from_value(json!({"op":"product.task","projectId":project,
                "params":{"action":"accept","taskId":tasks[0],"version":review["version"],
                "digest":review["digest"],"automaticFinish":{"sprintId":id,"targetBranch":"dev"}}}))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(wrong_mode["ok"], false);
    assert_eq!(wrong_mode["code"], "automatic_finish_unavailable");
    service.stop().await.unwrap();
}

#[tokio::test]
async fn automatic_sprint_owner_close_as_is_releases_acceptance_hold() {
    let (_scratch, folder, service, runtime, project, id, tasks) =
        automatic_fixture("dev", "main", "main").await;
    let mut jobs = Jobs::default();
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let task = settled_peer(&runtime, &tasks[0]).await;
    let t = task.clone();
    runtime
        .host
        .executor
        .write(move |db| {
            db.execute(
                "UPDATE runtime_tasks SET state='COMPLETED',blocker=NULL WHERE task_id=?",
                [&t],
            )?;
            Ok(())
        })
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let held = sprint(&service, &project, &id).await;
    assert_eq!(held["queueState"], "paused", "{held}");
    assert_eq!(
        held["automaticQueue"]["code"],
        "automatic_acceptance_blocked"
    );
    let before = held["automaticQueue"]["reason"]
        .as_str()
        .unwrap()
        .to_owned();

    // The presentation command cannot release the shared folder while the
    // runtime still lacks the outcome of a dispatched write.
    let held_task = task.clone();
    runtime.host.executor.write(move |db| {
        db.execute("INSERT INTO runtime_requests VALUES (?, 'pending-check', 'fixture', 'project_edit','digest','{}')", [&held_task])?;
        Ok(())
    }).unwrap().await.unwrap().unwrap();
    let premature = service
        .machine(
            serde_json::from_value(json!({"op":"product.task","projectId":project,
        "params":{"action":"close","choice":"as_is","confirmed":true,"taskId":tasks[0]}}))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        premature["code"], "native_outside_outcome_unknown",
        "{premature}"
    );
    assert_eq!(
        sprint(&service, &project, &id).await["queueState"],
        "paused"
    );
    let held_task = task.clone();
    runtime.host.executor.write(move |db| {
        db.execute("UPDATE runtime_requests SET result_json=json_object('complete',json('true')) WHERE task_id=? AND request_key='pending-check'", [&held_task])?;
        Ok(())
    }).unwrap().await.unwrap().unwrap();

    // Exercise the same owner close route exposed by the task screen. The
    // UI first resolves the bound execution, then persists the task's
    // closed disposition. Reusing its request id must not record a second
    // owner resolution.
    let projection = service.native_tasks(&project).await.unwrap();
    let execution = projection[&tasks[0]]["executionId"].clone();
    let expected_version = projection[&tasks[0]]["version"].clone();
    let close_request = json!({
        "projectId":project,
        "executionId":execution,
        "requestId":"owner-close-as-is",
        "action":"close_task",
        "confirmed":true,
        "expectedVersion":expected_version
    });
    let first_control = runtime
        .control(project.clone(), close_request.clone())
        .await
        .unwrap();
    assert_eq!(first_control["outcome"], "recorded");
    let config = service.config.clone();
    service.stop().await.unwrap();
    drop(runtime);
    drop(service);
    let service = Service::bind_native(config, Arc::new(Peer(None)))
        .await
        .unwrap();
    service.activate(&project).await.unwrap();
    let runtime = service.state.lock().await.native[&project].clone();
    let mut jobs = Jobs::default();
    let replay = runtime
        .control(project.clone(), close_request)
        .await
        .unwrap();
    assert_eq!(replay, first_control);
    let resolved_events = runtime
        .host
        .executor
        .read(move |db| {
            Ok(db.query_row(
                "SELECT count(*) FROM runtime_events WHERE task_id=? AND type='OWNER_TASK_RESOLVED'",
                [&task],
                |row| row.get::<_, i64>(0),
            )?)
        })
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resolved_events, 1);
    call(
        &service,
        "product.task",
        &project,
        json!({"action":"close","taskId":tasks[0],"choice":"as_is","confirmed":true}),
    )
    .await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let resumed = sprint(&service, &project, &id).await;
    assert_eq!(resumed["queueState"], "running", "{resumed}");
    assert!(resumed["automaticQueue"].is_null(), "{resumed}");
    assert_eq!(resumed["pauseReason"], Value::Null);
    assert_eq!(
        call(&service, "product.board", &project, json!({})).await["tasks"][&tasks[0]]
            ["closedAs"],
        "as_is"
    );
    assert!(!before.is_empty());

    // The next pass starts the dependent once, and another pass does not
    // repeat the close or create a second execution for it.
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let first = service.native_tasks(&project).await.unwrap();
    assert!(first[&tasks[1]]["executionId"].is_string());
    let execution = first[&tasks[1]]["executionId"].clone();
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let second = service.native_tasks(&project).await.unwrap();
    assert_eq!(second[&tasks[1]]["executionId"], execution);
    assert_eq!(git_fixture(&folder, &["rev-list", "--count", "HEAD"]), "1");
    service.stop().await.unwrap();
}

#[tokio::test]
async fn automatic_sprint_rechecks_finish_preconditions_without_dispatching_a_commit() {
    let (_scratch, folder, service, runtime, project, id, tasks) =
        automatic_fixture("later", "main", "main").await;
    let mut jobs = Jobs::default();
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let task = settled_peer(&runtime, &tasks[0]).await;
    verified_fixture(&runtime, task, true).await;
    let head = git_fixture(&folder, &["rev-parse", "HEAD"]);
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let held = sprint(&service, &project, &id).await;
    assert_eq!(held["automaticQueue"]["code"], "unknown_branch", "{held}");

    git_fixture(&folder, &["branch", "later"]);
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let next = sprint(&service, &project, &id).await;
    assert_eq!(
        next["automaticQueue"]["kind"], "owner_integration",
        "{next}"
    );
    assert_eq!(git_fixture(&folder, &["rev-parse", "HEAD"]), head);
    assert!(
        service.native_tasks(&project).await.unwrap()[&tasks[1]]["executionId"].is_null()
    );
    service.stop().await.unwrap();
}

/// A folder without Git has nothing to commit: a verified, automatically
/// accepted result reaches Done by itself (closed in place with its checks
/// kept), never a "close it yourself" hold, and the queue starts the next task.
#[tokio::test]
async fn automatic_sprint_finishes_in_a_folder_without_git_by_itself() {
    let (_scratch, folder, service, runtime, project, id, tasks) =
        automatic_fixture("dev", "main", "main").await;
    std::fs::remove_dir_all(folder.join(".git")).unwrap();
    let mut jobs = Jobs::default();
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let task = settled_peer(&runtime, &tasks[0]).await;
    verified_fixture(&runtime, task, true).await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let board = call(&service, "product.board", &project, json!({})).await;
    let first = &board["tasks"][&tasks[0]];
    assert_eq!(first["status"], "done", "{first}");
    assert_eq!(first["closedAs"], "as_is");
    assert_eq!(first["notVerifiedByCortex"], false);
    assert_eq!(first["acceptance"]["automatic"], true);
    let held = sprint(&service, &project, &id).await;
    assert_eq!(held["queueState"], "running", "{held}");
    assert!(held["automaticQueue"].is_null(), "{held}");
    assert!(!folder.join(".git").exists(), "no repository is created");
    // The next pass starts the dependent task: nothing waited for the owner.
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    assert!(
        !service.native_tasks(&project).await.unwrap()[&tasks[1]]["executionId"].is_null()
    );
    service.stop().await.unwrap();
}

#[tokio::test]
async fn automatic_sprint_closes_already_landed_work_without_an_extra_commit() {
    let (_scratch, folder, service, runtime, project, id, tasks) =
        automatic_fixture("dev", "main", "main").await;
    let mut jobs = Jobs::default();
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let task = settled_peer(&runtime, &tasks[0]).await;
    std::fs::write(folder.join("work.txt"), "reviewed changes\n").unwrap();
    git_fixture(&folder, &["add", "work.txt"]);
    git_fixture(&folder, &["commit", "-q", "-m", "Already landed"]);
    std::fs::write(folder.join("unrelated.txt"), "keep staged\n").unwrap();
    git_fixture(&folder, &["add", "unrelated.txt"]);
    let head = git_fixture(&folder, &["rev-parse", "HEAD"]);
    verified_fixture(&runtime, task, true).await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let board = call(&service, "product.board", &project, json!({})).await;
    let first = &board["tasks"][&tasks[0]];
    assert_eq!(first["status"], "done", "{first}");
    assert_eq!(first["integration"]["automatic"], true);
    assert_eq!(first["integration"]["committed"], false);
    assert_eq!(git_fixture(&folder, &["rev-parse", "HEAD"]), head);
    assert_eq!(
        git_fixture(&folder, &["diff", "--cached", "--name-only"]),
        "unrelated.txt"
    );
    assert_eq!(
        sprint(&service, &project, &id).await["queueState"],
        "running"
    );
    service.stop().await.unwrap();
}

#[tokio::test]
async fn automatic_sprint_never_commits_unattributed_changes() {
    let (_scratch, folder, service, runtime, project, id, tasks) =
        automatic_fixture("dev", "main", "main").await;
    let mut jobs = Jobs::default();
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let task = settled_peer(&runtime, &tasks[0]).await;
    verified_fixture(&runtime, task, false).await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let held = sprint(&service, &project, &id).await;
    assert_eq!(held["queueState"], "paused", "{held}");
    assert_eq!(held["automaticQueue"]["code"], "task_content_unknown");
    assert_eq!(held["automaticQueue"]["needsOwner"], true);
    assert!(
        service.native_tasks(&project).await.unwrap()[&tasks[1]]["executionId"].is_null()
    );
    assert_eq!(git_fixture(&folder, &["rev-list", "--count", "HEAD"]), "1");
    assert_eq!(
        std::fs::read(folder.join("work.txt")).unwrap(),
        b"reviewed changes\n"
    );
    service.stop().await.unwrap();
}

#[tokio::test]
async fn automatic_sprint_does_not_repeat_a_dispatched_failed_commit() {
    let (_scratch, folder, service, runtime, project, id, tasks) =
        automatic_fixture("dev", "main", "main").await;
    let mut jobs = Jobs::default();
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let task = settled_peer(&runtime, &tasks[0]).await;
    let hook = folder.join(".git/hooks/pre-commit");
    std::fs::write(
        &hook,
        "#!/bin/sh\nprintf 'attempt\\n' >> .git/fixture-attempts\nexit 1\n",
    )
    .unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(folder.join("unrelated.txt"), "keep staged\n").unwrap();
    git_fixture(&folder, &["add", "unrelated.txt"]);
    verified_fixture(&runtime, task, true).await;

    for _ in 0..2 {
        service
            .sprint_pass(&project, &runtime, &mut jobs, false)
            .await
            .unwrap();
        let held = sprint(&service, &project, &id).await;
        assert_eq!(
            held["automaticQueue"]["code"], "integration_failed",
            "{held}"
        );
        assert_eq!(held["automaticQueue"]["needsOwner"], true);
        // Even an explicit queue restart cannot dispatch the same result's
        // integration a second time behind the durable intent.
        call(
            &service,
            "product.sprint",
            &project,
            json!({"action":"patch","sprintId":id,
            "patch":{"queueState":"running","pauseReason":null}}),
        )
        .await;
    }
    assert_eq!(
        std::fs::read_to_string(folder.join(".git/fixture-attempts")).unwrap(),
        "attempt\n"
    );
    assert_eq!(git_fixture(&folder, &["rev-list", "--count", "HEAD"]), "1");
    assert_eq!(
        git_fixture(&folder, &["diff", "--cached", "--name-only"]),
        "unrelated.txt"
    );
    assert!(
        service.native_tasks(&project).await.unwrap()[&tasks[1]]["executionId"].is_null()
    );
    service.stop().await.unwrap();
}

async fn allow(service: &Service, project: &str, id: &Value, hold: &Value) -> Value {
    service
        .machine(
            serde_json::from_value(json!({"op":"product.sprint","projectId":project,
            "params":{"action":"allow","sprintId":id,"taskId":hold["taskId"],"code":hold["code"],
                "holdId":hold["holdId"],"ownerAction":"allow_repair"}}))
            .unwrap(),
        )
        .await
        .unwrap()
}

/// H2: a busy runtime decides nothing. Without consent no owner hold is written
/// (the agent goes first on the next pass); with consent the consent is kept.
#[tokio::test]
async fn a_busy_runtime_keeps_agent_first_repair_and_the_owners_consent() {
    let (_scratch, _folder, service, runtime, project, id, tasks) =
        automatic_fixture("dev", "main", "main").await;
    let mut jobs = Jobs::default();
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let task = settled_peer(&runtime, &tasks[0]).await;
    fail_run(
        &runtime,
        &task,
        "answer_not_certified:completion_review_missing",
    )
    .await;
    let fact = service.native_tasks(&project).await.unwrap()[&tasks[0]].clone();
    let running = sprint(&service, &project, &id).await;
    {
        let _busy = runtime.controls.lock().await;
        let repair = service
            .failed_repair(&project, &runtime, &running, &tasks[0], &fact)
            .await
            .unwrap();
        assert!(matches!(repair, Repair::Later), "busy is not a refusal");
    }
    assert_eq!(repairs_of(&runtime, &task).await, 0);
    assert_eq!(sprint(&service, &project, &id).await, running);
    // The owner allowed a repair; the runtime is busy when the queue acts on it.
    service
        .automatic_pause(
            &project,
            &running,
            automatic_hold(
                "settlement_blocked",
                Some(&tasks[0]),
                "task_failed",
                "The last run of this task ended as failed.",
                &HoldFacts {
                    repair: true,
                    ..HoldFacts::default()
                },
            ),
        )
        .await
        .unwrap();
    let held = sprint(&service, &project, &id).await;
    let reply = allow(&service, &project, &id, &held["automaticQueue"]).await;
    assert_eq!(reply["ok"], true, "{reply}");
    let allowed = sprint(&service, &project, &id).await;
    assert!(allowed["automaticQueue"]["allowedAt"].is_i64(), "{allowed}");
    {
        let _busy = runtime.controls.lock().await;
        let repair = service
            .failed_repair(&project, &runtime, &allowed, &tasks[0], &fact)
            .await
            .unwrap();
        assert!(matches!(repair, Repair::Later), "busy is not a refusal");
    }
    assert_eq!(
        sprint(&service, &project, &id).await,
        allowed,
        "consent kept"
    );
    assert_eq!(repairs_of(&runtime, &task).await, 0);
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let resumed = sprint(&service, &project, &id).await;
    assert_eq!(repairs_of(&runtime, &task).await, 1, "{resumed}");
    assert_eq!(resumed["queueState"], "running", "{resumed}");
    assert!(resumed["automaticQueue"].is_null(), "{resumed}");
    service.stop().await.unwrap();
}

/// M4: a failed run with an effect whose outcome is unknown is never offered to
/// the agent; the runtime's refusal reaches the owner's hold.
#[tokio::test]
async fn a_failed_run_with_an_unknown_effect_is_never_offered_to_the_agent() {
    let (_scratch, _folder, service, runtime, project, id, tasks) =
        automatic_fixture("dev", "main", "main").await;
    let mut jobs = Jobs::default();
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let task = settled_peer(&runtime, &tasks[0]).await;
    let t = task.clone();
    runtime.host.executor.write(move |db| {
        db.execute("INSERT INTO trusted_channels(channel_id,secret_hash,task_id,principal_thread_id,role,owner_epoch,issued_at)
            VALUES ('fixture-unknown','hash',?,'fixture-worker','worker',1,'now')", [&t])?;
        db.execute("INSERT INTO runtime_requests VALUES (?,'fixture-unknown-key','fixture-unknown','project_write','digest',?)",
            (&t, json!({"complete":false}).to_string()))?;
        Ok(())
    }).unwrap().await.unwrap().unwrap();
    fail_run(
        &runtime,
        &task,
        "answer_not_certified:completion_review_missing",
    )
    .await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let held = sprint(&service, &project, &id).await;
    let hold = &held["automaticQueue"];
    assert_eq!(hold["code"], "task_failed", "{held}");
    // A write whose outcome is unknown can never be settled: the owner decides,
    // in plain words, with only the close the service accepts.
    assert_eq!(hold["nextActor"], "owner");
    assert_eq!(hold["ownerActions"], json!(["close_as_is"]), "{hold}");
    assert_eq!(hold["reason"], UNSETTLED_REASON);
    assert_eq!(hold["asks"], json!([UNSETTLED_CLOSE_ASK]));
    assert_eq!(
        hold["settling"]["hold"]["asks"],
        json!([UNKNOWN_EFFECT_ASK])
    );
    assert_eq!(repairs_of(&runtime, &task).await, 0);
    // A command whose process still runs, which the runtime settles by itself
    // once it ends: the runtime acts next and the owner is asked nothing.
    let t = task.clone();
    runtime.host.executor.write(move |db| {
        db.execute("DELETE FROM runtime_requests WHERE task_id=? AND request_key='fixture-unknown-key'", [&t])?;
        Ok(())
    }).unwrap().await.unwrap().unwrap();
    let child = live_command(&runtime, &task, "fixture-unknown-key").await;
    for _ in 0..2 {
        service
            .sprint_pass(&project, &runtime, &mut jobs, false)
            .await
            .unwrap();
        let waiting = sprint(&service, &project, &id).await["automaticQueue"].clone();
        assert_eq!(waiting["nextActor"], "runtime", "{waiting}");
        assert_eq!(waiting["reason"], SETTLING_REASON);
        assert_eq!(waiting["ownerActions"], json!([]));
        assert_eq!(waiting["settling"]["since"], hold["settling"]["since"]);
    }
    // Once it settles the queue derives the failed run again: the agent repairs it.
    drop(child);
    let t = task.clone();
    runtime.host.executor.write(move |db| {
        db.execute("UPDATE runtime_requests SET result_json=? WHERE task_id=? AND request_key='fixture-unknown-key'",
            (json!({"complete":false,"settled":true,"failure":"command_outcome_unknown"}).to_string(), &t))?;
        Ok(())
    }).unwrap().await.unwrap().unwrap();
    for _ in 0..2 {
        service
            .sprint_pass(&project, &runtime, &mut jobs, false)
            .await
            .unwrap();
    }
    let resumed = sprint(&service, &project, &id).await;
    assert!(resumed["automaticQueue"]["settling"].is_null(), "{resumed}");
    assert_eq!(repairs_of(&runtime, &task).await, 1, "{resumed}");
    service.stop().await.unwrap();
}

/// The owner's incident: a stale automatic acceptance whose task has an unknown
/// effect. While the runtime can settle it nothing is offered; once it cannot,
/// Close as is is offered and accepted (preflight and close), recording the
/// effect as unknown, never as done.
#[tokio::test]
async fn an_unknown_effect_hold_offers_only_a_close_the_service_accepts() {
    let (_scratch, _folder, service, runtime, project, id, tasks) =
        automatic_fixture("dev", "main", "main").await;
    let mut jobs = Jobs::default();
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let task = settled_peer(&runtime, &tasks[0]).await;
    let t = task.clone();
    runtime
        .host
        .executor
        .write(move |db| {
            db.execute(
                "UPDATE runtime_tasks SET state='COMPLETED',blocker=NULL WHERE task_id=?",
                [&t],
            )?;
            Ok(())
        })
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    let child = live_command(&runtime, &task, "pending-check").await;
    for _ in 0..2 {
        service
            .sprint_pass(&project, &runtime, &mut jobs, false)
            .await
            .unwrap();
    }
    let waiting = sprint(&service, &project, &id).await["automaticQueue"].clone();
    assert_eq!(waiting["code"], "automatic_acceptance_blocked", "{waiting}");
    assert_eq!(waiting["nextActor"], "runtime", "{waiting}");
    assert_eq!(waiting["ownerActions"], json!([]));
    let close = |preflight: bool| {
        json!({"op":"product.task","projectId":project,"params":{"action":"close","choice":"as_is",
            "confirmed":true,"taskId":tasks[0],"preflight":preflight}})
    };
    // Closing while the runtime can still settle the command is refused, by the
    // preflight as by the close.
    for preflight in [true, false] {
        let refused = service
            .machine(serde_json::from_value(close(preflight)).unwrap())
            .await
            .unwrap();
        assert_eq!(
            refused["code"], "native_outside_outcome_unknown",
            "{refused}"
        );
    }
    // An effect the runtime can never confirm goes to the owner.
    let t = task.clone();
    runtime.host.executor.write(move |db| {
        db.execute("UPDATE runtime_requests SET operation='project_edit' WHERE task_id=? AND request_key='pending-check'", [&t])?;
        Ok(())
    }).unwrap().await.unwrap().unwrap();
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let held = sprint(&service, &project, &id).await["automaticQueue"].clone();
    assert_eq!(held["nextActor"], "owner", "{held}");
    assert_eq!(held["ownerActions"], json!(["close_as_is"]), "{held}");
    assert_eq!(held["reason"], UNSETTLED_REASON);
    assert_eq!(held["settling"]["since"], waiting["settling"]["since"]);
    let refused = allow(&service, &project, &id, &held).await;
    assert_eq!(refused["code"], "automatic_hold_changed", "{refused}");
    let checked = service
        .machine(serde_json::from_value(close(true)).unwrap())
        .await
        .unwrap();
    assert_eq!(checked["result"]["preflight"], true, "{checked}");
    let closed = service
        .machine(serde_json::from_value(close(false)).unwrap())
        .await
        .unwrap();
    assert_eq!(closed["ok"], true, "{closed}");
    let t = task.clone();
    let (settled_by, acknowledged): (String, i64) = runtime.host.executor.read(move |db| {
        Ok((db.query_row("SELECT json_extract(result_json,'$.settled_by') FROM runtime_requests WHERE task_id=? AND request_key='pending-check'", [&t], |r| r.get(0))?,
            db.query_row("SELECT count(*) FROM runtime_events WHERE task_id=? AND type='OWNER_ACKNOWLEDGED_UNKNOWN_EFFECTS'", [&t], |r| r.get(0))?))
    }).unwrap().await.unwrap().unwrap();
    assert_eq!(
        (settled_by.as_str(), acknowledged),
        ("owner_close_as_is", 1)
    );
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let resumed = sprint(&service, &project, &id).await;
    assert!(resumed["automaticQueue"].is_null(), "{resumed}");
    service.stop().await.unwrap();
}

/// M1: Hybrid keeps a refused start for the owner's Try again instead of
/// clearing it and retrying the start on every pass.
#[tokio::test]
async fn hybrid_keeps_a_refused_start_until_the_owner_tries_again() {
    let (_scratch, _folder, service, runtime, project, id, tasks) =
        automatic_fixture("dev", "main", "main").await;
    call(
        &service,
        "product.sprint",
        &project,
        json!({"action":"patch","sprintId":id,"patch":{"runMode":"hybrid"}}),
    )
    .await;
    let expected = sprint(&service, &project, &id).await;
    service
        .automatic_pause(
            &project,
            &expected,
            automatic_hold(
                "start_refused",
                Some(&tasks[0]),
                "maintenance_busy",
                &start_reason("maintenance_busy"),
                &HoldFacts::default(),
            ),
        )
        .await
        .unwrap();
    let held = sprint(&service, &project, &id).await;
    assert_eq!(
        held["automaticQueue"]["ownerActions"],
        json!(["retry", "close_as_is"])
    );
    let mut jobs = Jobs::default();
    for _ in 0..2 {
        service
            .sprint_pass(&project, &runtime, &mut jobs, false)
            .await
            .unwrap();
        assert_eq!(sprint(&service, &project, &id).await, held);
    }
    assert!(
        service.native_tasks(&project).await.unwrap()[&tasks[0]]["executionId"].is_null()
    );
    // Try again is Continue queue: the queue starts the task.
    call(
        &service,
        "product.sprint",
        &project,
        json!({"action":"patch","sprintId":id,"patch":{"queueState":"running","pauseReason":null}}),
    )
    .await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    assert!(
        service.native_tasks(&project).await.unwrap()[&tasks[0]]["executionId"].is_string()
    );
    service.stop().await.unwrap();
}

/// L5: the repaired result's acceptance is cleared before the runtime reopens
/// it; a repair that does not start leaves the record exactly as it was.
#[tokio::test]
async fn a_repair_that_does_not_start_leaves_the_acceptance_as_it_was() {
    let (_scratch, folder, service, runtime, project, id, tasks) =
        automatic_fixture("dev", "main", "main").await;
    let mut jobs = Jobs::default();
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let draft = &tasks[0];
    let task = settled_peer(&runtime, draft).await;
    verified_fixture(&runtime, task.clone(), true).await;
    let accepted = call(
        &service,
        "product.task",
        &project,
        json!({"action":"auto_ready","taskId":draft}),
    )
    .await;
    assert_eq!(accepted["accepted"], true, "{accepted}");
    std::fs::write(folder.join("work.txt"), "changed since acceptance\n").unwrap();
    let expected = sprint(&service, &project, &id).await;
    let hold = automatic_hold(
        "settlement_blocked",
        Some(draft),
        "stale_result",
        "The workspace changed.",
        &HoldFacts::default(),
    );
    service
        .automatic_pause(&project, &expected, hold)
        .await
        .unwrap();
    let held = sprint(&service, &project, &id).await;
    let before =
        call(&service, "product.board", &project, json!({})).await["tasks"][draft].clone();
    let execution =
        service.native_tasks(&project).await.unwrap()[draft]["executionId"].clone();
    {
        let _busy = runtime.controls.lock().await;
        let repair = service
            .automatic_repair(
                &project,
                &runtime,
                &held,
                draft,
                &execution,
                None,
                "stale_result",
                "The workspace changed.",
                true,
            )
            .await
            .unwrap();
        assert!(matches!(repair, Repair::Later));
    }
    let after =
        call(&service, "product.board", &project, json!({})).await["tasks"][draft].clone();
    assert_eq!(after["acceptance"], before["acceptance"]);
    assert_eq!(after["status"], before["status"]);
    assert_eq!(sprint(&service, &project, &id).await, held);
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let board = call(&service, "product.board", &project, json!({})).await;
    assert_eq!(repairs_of(&runtime, &task).await, 1);
    assert!(board["tasks"][draft]["acceptance"].is_null());
    service.stop().await.unwrap();
}

/// L3: a failed commit says nothing about a Commit retry no hold offers; the
/// owner's consent sends it back to the agent for a fresh result.
#[tokio::test]
async fn a_failed_commit_offers_the_agent_repair_it_describes() {
    let (_scratch, folder, service, runtime, project, id, tasks) =
        automatic_fixture("dev", "main", "main").await;
    let mut jobs = Jobs::default();
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let task = settled_peer(&runtime, &tasks[0]).await;
    let hook = folder.join(".git/hooks/pre-commit");
    std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o700)).unwrap();
    verified_fixture(&runtime, task.clone(), true).await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let held = sprint(&service, &project, &id).await;
    let hold = &held["automaticQueue"];
    assert_eq!(hold["code"], "integration_failed", "{held}");
    assert_eq!(hold["reason"], INTEGRATION_FAILED);
    assert_eq!(held["pauseReason"], INTEGRATION_FAILED);
    assert_eq!(
        hold["ownerActions"],
        json!(["allow_repair", "close_as_is"]),
        "{hold}"
    );
    std::fs::remove_file(&hook).unwrap();
    let reply = allow(&service, &project, &id, hold).await;
    assert_eq!(reply["ok"], true, "{reply}");
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let resumed = sprint(&service, &project, &id).await;
    assert_eq!(repairs_of(&runtime, &task).await, 1, "{resumed}");
    assert_eq!(resumed["queueState"], "running", "{resumed}");
    service.stop().await.unwrap();
}

/// L2/L7: every owner hold the scheduler can persist answers Allow only for
/// itself, Try again releases it, and every runtime-actor hold is rechecked
/// (released here, where nothing blocks) by the next pass.
#[tokio::test]
async fn every_hold_action_reaches_its_handler_and_every_runtime_hold_is_rechecked() {
    use crate::native_driver::sprint_tests::automatic_repair_tests::{
        HOLDS, OWNER_ACTIONS,
    };
    let (_scratch, _folder, service, runtime, project, id, tasks) =
        automatic_fixture("dev", "main", "main").await;
    let mut jobs = Jobs::default();
    // How the conversation card sends each action (see the card in the UI):
    // product.sprint allow, a sprint patch, a task control, the accept or close
    // flows, or a link to the place the owner acts.
    let routes = [
        ("allow_repair", "product.sprint allow"),
        ("retry", "product.sprint patch queueState running"),
        ("allow_retries", "task continue"),
        ("resume_task", "task continue"),
        ("confirm_and_continue", "task retry"),
        ("allow_finish", "accept with finish consent"),
        ("accept_result", "accept"),
        ("close_as_is", "product.task close"),
        ("confirm_integration", "task changes"),
        ("resolve_conflict", "task conflict"),
        ("change_run_mode", "sprint settings"),
    ];
    for action in OWNER_ACTIONS {
        assert!(
            routes.iter().any(|(a, _)| a == action),
            "{action} has no route"
        );
    }
    let facts = [
        HoldFacts::default(),
        HoldFacts {
            repair: true,
            ..HoldFacts::default()
        },
    ];
    for &(kind, code) in HOLDS {
        for task in [Some(tasks[0].as_str()), None] {
            for facts in &facts {
                let hold = automatic_hold(kind, task, code, "A plain reason.", facts);
                let expected = sprint(&service, &project, &id).await;
                service
                    .automatic_pause(&project, &expected, hold.clone())
                    .await
                    .unwrap();
                let saved = sprint(&service, &project, &id).await["automaticQueue"].clone();
                assert!(saved["holdId"].is_string(), "{saved}");
                if hold["nextActor"] == BY_OWNER {
                    let offers = hold["ownerActions"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|a| a == "allow_repair");
                    // An older card's decision never applies to this hold.
                    let mut stale = saved.clone();
                    stale["holdId"] = json!("h_older");
                    let refused = allow(&service, &project, &id, &stale).await;
                    assert_eq!(refused["code"], "automatic_hold_changed", "{saved}");
                    let reply = allow(&service, &project, &id, &saved).await;
                    assert_eq!(reply["ok"] == true, offers, "{saved}: {reply}");
                    let after = sprint(&service, &project, &id).await;
                    assert_eq!(after["automaticQueue"]["allowedAt"].is_i64(), offers);
                    // Try again releases the hold through the sprint patch.
                    call(
                        &service,
                        "product.sprint",
                        &project,
                        json!({"action":"patch","sprintId":id,
                        "patch":{"queueState":"running","pauseReason":null}}),
                    )
                    .await;
                    assert!(
                        sprint(&service, &project, &id).await["automaticQueue"].is_null()
                    );
                } else if task.is_some() || kind != "continuation" {
                    // A continuation hold always names its task.
                    service
                        .sprint_pass(&project, &runtime, &mut jobs, false)
                        .await
                        .unwrap();
                    let after = sprint(&service, &project, &id).await;
                    assert!(
                        after["automaticQueue"].is_null(),
                        "{kind}/{code} is never rechecked: {after}"
                    );
                    assert_eq!(after["queueState"], "running");
                } else {
                    call(
                        &service,
                        "product.sprint",
                        &project,
                        json!({"action":"patch","sprintId":id,
                        "patch":{"queueState":"running","pauseReason":null}}),
                    )
                    .await;
                }
            }
        }
    }
    // No hold the scheduler persists names the agent: while an agent repair runs
    // the queue runs and no hold is saved (see `BY_AGENT`).
    assert!(
        service.native_tasks(&project).await.unwrap()[&tasks[0]]["executionId"].is_null()
    );
    service.stop().await.unwrap();

    // With an unknown effect present, every hold of a started task is decided
    // again: a command the runtime can settle is the runtime's (asks nothing),
    // any other effect is the owner's, and each action offered is accepted by its
    // handler. Allow a repair is never offered; Close as is passes its handler's
    // own preflight, including the settled-execution guard of an automatic close.
    let (_scratch, _folder, service, runtime, project, id, tasks) =
        automatic_fixture("dev", "main", "main").await;
    service
        .sprint_pass(&project, &runtime, &mut jobs, false)
        .await
        .unwrap();
    let task = settled_peer(&runtime, &tasks[0]).await;
    // A command whose process still runs (the runtime settles it once it ends),
    // then a write the runtime can never confirm.
    let child = live_command(&runtime, &task, "invariant-unknown").await;
    for settles in [true, false] {
        if !settles {
            let t = task.clone();
            runtime.host.executor.write(move |db| {
                db.execute("UPDATE runtime_requests SET operation='project_edit',result_json='{\"complete\":false}'
                    WHERE task_id=? AND request_key='invariant-unknown'", [&t])?;
                Ok(())
            }).unwrap().await.unwrap().unwrap();
        }
        for &(kind, code) in HOLDS {
            for facts in &facts {
                let hold =
                    automatic_hold(kind, Some(&tasks[0]), code, "A plain reason.", facts);
                let expected = sprint(&service, &project, &id).await;
                service
                    .automatic_pause(&project, &expected, hold.clone())
                    .await
                    .unwrap();
                let saved = sprint(&service, &project, &id).await["automaticQueue"].clone();
                let actions: Vec<&str> = saved["ownerActions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|a| a.as_str().unwrap())
                    .collect();
                if kind == "start_refused" {
                    assert_eq!(saved["ownerActions"], hold["ownerActions"], "{saved}");
                } else if settles {
                    assert_eq!(saved["nextActor"], BY_RUNTIME, "{kind}/{code}: {saved}");
                    assert_eq!(saved["needsOwner"], false);
                    assert_eq!(saved["reason"], SETTLING_REASON);
                    assert!(actions.is_empty() && saved["asks"] == json!([]), "{saved}");
                    // Rechecked on every pass, and kept while it stays unknown.
                    service
                        .sprint_pass(&project, &runtime, &mut jobs, false)
                        .await
                        .unwrap();
                    let kept =
                        sprint(&service, &project, &id).await["automaticQueue"].clone();
                    assert_eq!(kept["nextActor"], BY_RUNTIME, "{kind}/{code}: {kept}");
                    assert_eq!(kept["settling"]["hold"]["code"], code);
                } else if saved["settling"].is_null() {
                    // No-stop policy: the task's continuation already settled the
                    // write for re-verification by its coordinator, so the hold is
                    // decided as if every effect were known.
                    let t = task.clone();
                    let settled_by: String = runtime.host.executor.read(move |db| {
                        Ok(db.query_row("SELECT coalesce(json_extract(result_json,'$.settled_by'),'') FROM runtime_requests
                            WHERE task_id=? AND request_key='invariant-unknown'", [&t], |r| r.get(0))?)
                    }).unwrap().await.unwrap().unwrap();
                    assert_eq!(settled_by, "runtime_reverify", "{kind}/{code}: {saved}");
                    assert_eq!(
                        saved["reason"], "A plain reason.",
                        "{kind}/{code}: {saved}"
                    );
                } else {
                    assert_eq!(saved["nextActor"], BY_OWNER, "{kind}/{code}: {saved}");
                    assert_eq!(saved["reason"], UNSETTLED_REASON, "{kind}/{code}: {saved}");
                    assert_eq!(actions.last(), Some(&"close_as_is"), "{saved}");
                    for action in &actions {
                        assert!(ACCEPTED_UNSETTLED.contains(action), "{action}: {saved}");
                        assert!(routes.iter().any(|(a, _)| a == action), "{action}");
                    }
                    let refused = allow(&service, &project, &id, &saved).await;
                    assert_eq!(refused["code"], "automatic_hold_changed", "{saved}");
                    let checked = service
                        .machine(
                            serde_json::from_value(json!({"op":"product.task","projectId":project,
                                "params":{"action":"close","choice":"as_is","confirmed":true,
                                    "taskId":tasks[0],"preflight":true}}))
                            .unwrap(),
                        )
                        .await
                        .unwrap();
                    assert_eq!(checked["ok"], true, "{kind}/{code}: {checked}");
                }
                call(
                    &service,
                    "product.sprint",
                    &project,
                    json!({"action":"patch","sprintId":id,
                    "patch":{"queueState":"running","pauseReason":null}}),
                )
                .await;
            }
        }
    }
    drop(child);
    service.stop().await.unwrap();
}

/// A command receipt the runtime cannot settle yet: the process group it
/// recorded still runs. Dropping the returned child ends it.
async fn live_command(runtime: &Runtime, task: &str, key: &str) -> tokio::process::Child {
    let child = tokio::process::Command::new("/bin/sleep")
        .arg("120")
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let pid = child.id().unwrap();
    let (t, k) = (task.to_owned(), key.to_owned());
    runtime.host.executor.write(move |db| {
        let reference = format!("native-command:{k}");
        db.execute("INSERT INTO runtime_requests VALUES (?,?,'fixture-live','command_run','digest',?)",
            (&t, &k, json!({"complete":false,"tool_call_ref":reference}).to_string()))?;
        cortex_core::lifecycle::event_tx(db, &t, "NATIVE_COMMAND_PROCESS",
            json!({"ref":reference,"process_identity":{"pid":pid,"start_ticks":1}}))?;
        Ok(())
    }).unwrap().await.unwrap().unwrap();
    child
}

/// The settling decision itself: the runtime waits a bounded time for commands,
/// any other unknown effect is the owner's at once, and a known one changes nothing.
#[test]
fn a_settling_hold_waits_a_bounded_time_and_keeps_the_replaced_decision() {
    let facts = HoldFacts {
        repair: true,
        ..HoldFacts::default()
    };
    let hold = automatic_hold(
        "settlement_blocked",
        Some("t"),
        "stale_result",
        "Changed.",
        &facts,
    );
    assert_eq!(hold["ownerActions"], json!(["allow_repair", "close_as_is"]));
    assert_eq!(settling_hold(hold.clone(), &Value::Null, None, 5), hold);
    let waiting = settling_hold(hold.clone(), &Value::Null, Some(Unsettled::Commands), 5);
    assert_eq!(waiting["nextActor"], BY_RUNTIME);
    assert_eq!(waiting["settling"]["since"], 5);
    assert_eq!(waiting["settling"]["hold"], hold);
    // The same hold decided again keeps its first wait and its replaced decision.
    let again = settling_hold(waiting.clone(), &waiting, Some(Unsettled::Commands), 6);
    assert_eq!(again, waiting);
    // No time cap: commands stay the runtime's to settle, however long it takes.
    let late = settling_hold(
        hold.clone(),
        &waiting,
        Some(Unsettled::Commands),
        5 + 24 * 60 * 60 * 1000,
    );
    assert_eq!(late["nextActor"], BY_RUNTIME);
    assert_eq!(late["ownerActions"], json!([]));
    assert_eq!(late["settling"]["hold"], hold);
    let never = settling_hold(hold.clone(), &Value::Null, Some(Unsettled::Effects), 5);
    assert_eq!(never["ownerActions"], json!(["close_as_is"]));
    assert_eq!(never["asks"], json!([UNSETTLED_CLOSE_ASK]));
    // Settled again: the replaced decision comes back unchanged.
    assert_eq!(settling_hold(never, &late, None, 9), hold);
    // An action the service accepts with an unknown effect stays offered first.
    let recovery = automatic_hold(
        "settlement_blocked",
        Some("t"),
        "task_recovery",
        "Stopped.",
        &HoldFacts::default(),
    );
    let owner = settling_hold(recovery, &Value::Null, Some(Unsettled::Effects), 5);
    assert_eq!(
        owner["ownerActions"],
        json!(["confirm_and_continue", "close_as_is"])
    );
    assert_eq!(owner["asks"].as_array().unwrap().len(), 2);
    // A refused start and a hold without a task are never changed.
    let start = automatic_hold(
        "start_refused",
        Some("t"),
        "refused",
        "No.",
        &HoldFacts::default(),
    );
    assert_eq!(
        settling_hold(start.clone(), &Value::Null, Some(Unsettled::Effects), 5),
        start
    );
    let none = automatic_hold(
        "settlement_blocked",
        None,
        "git_required",
        "No.",
        &HoldFacts::default(),
    );
    assert_eq!(
        settling_hold(none.clone(), &Value::Null, Some(Unsettled::Effects), 5),
        none
    );
}
