//! Opening a project with rows the retired Python host (or an earlier native build)
//! left behind never breaks it, and every current owner action applies to such tasks.
//! Fixtures are legacy-shaped rows on a disposable store; no engine or model runs
//! (`/bin/false` exits before initialize).
use super::*;
use crate::{actor, native_runtime::NativeRoutes, native_sessions::SessionRoute};
use cortex_core::workspace;
use cortex_engine::environment::{EngineKind, LaunchSpec};
use std::{collections::BTreeMap, path::Path, time::Duration};

struct FailingStart;
impl NativeRoutes for FailingStart {
    fn route(&self, root: &Path, _: &str) -> CoreResult<SessionRoute> {
        Ok(SessionRoute {
            launch: LaunchSpec {
                engine: EngineKind::Codex,
                executable: "/bin/false".into(),
                args: vec![],
                cwd: root.into(),
                native_home: root.into(),
                credential_directory: root.into(),
                base_environment: BTreeMap::new(),
                codex_launcher: None,
            },
            relay: "/bin/false".into(),
            broker_parent: root.join(if workspace_protocol::channel::current().is_dev() {
                ".cortex-dev"
            } else {
                ".cortex"
            }),
            connection: "fixture".into(),
            connection_limit: 4,
            model: "fixture-model".into(),
            parameters: BTreeMap::new(),
            instructions: "No-paid fixture.".into(),
            deadline: Duration::from_secs(2),
        })
    }
}

/// One started desktop task: (root, runtime, execution, runtime task).
async fn seeded() -> (std::path::PathBuf, Arc<Runtime>, String, String) {
    let root = std::env::temp_dir().join(format!("legacy-open-{}", crate::identity().unwrap()));
    std::fs::create_dir(&root).unwrap();
    let path = root.clone();
    let runtime = Arc::new(
        tokio::task::spawn_blocking(move || {
            Runtime::open(&path, "project", Arc::new(FailingStart))
        })
        .await
        .unwrap()
        .unwrap(),
    );
    let execution = runtime
        .host
        .executor
        .publication(move |store| {
            let command = json!({"protocol":{"major":1,"minor":8},"requestId":"draft",
                "projectId":"project","actorId":actor(),"operation":"tasks.create","target":"project",
                "expectedVersion":"0","ownerEpoch":null,
                "payload":{"description":"Legacy fixture","checkout":{"kind":"main"}}});
            let draft = workspace::create_draft(store, "project", &actor(), &command)?.receipt;
            let start = json!({"protocol":{"major":1,"minor":8},"requestId":"start",
                "projectId":"project","actorId":actor(),"operation":"tasks.start",
                "target":draft["target"],"expectedVersion":draft["entityVersion"],
                "ownerEpoch":null,"payload":{}});
            let started = workspace::start_native(store, "project", &actor(), &start)?;
            Ok(started["receipt"]["target"].as_str().unwrap().to_owned())
        })
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    let task = sql(&runtime, "SELECT task_id FROM runtime_tasks").await;
    (root, runtime, execution, task)
}
async fn sql(runtime: &Runtime, query: &'static str) -> String {
    runtime
        .host
        .executor
        .read(move |db| {
            Ok(db
                .query_row(query, [], |r| r.get::<_, rusqlite::types::Value>(0))
                .map(|v| match v {
                    rusqlite::types::Value::Text(s) => s,
                    rusqlite::types::Value::Integer(i) => i.to_string(),
                    rusqlite::types::Value::Null => String::new(),
                    other => format!("{other:?}"),
                })?)
        })
        .unwrap()
        .await
        .unwrap()
        .unwrap()
}
async fn exec(runtime: &Runtime, query: String) {
    runtime
        .host
        .executor
        .write(move |db| {
            db.execute_batch(&query)?;
            Ok(())
        })
        .unwrap()
        .await
        .unwrap()
        .unwrap();
}
async fn close(root: std::path::PathBuf, runtime: Arc<Runtime>) {
    runtime.shutdown().await;
    drop(runtime);
    std::fs::remove_dir_all(root).unwrap();
}
fn boot() -> String {
    std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .unwrap()
        .trim()
        .to_owned()
}
async fn relaunched(runtime: &Runtime) {
    for _ in 0..200 {
        if sql(
            runtime,
            "SELECT count(*) FROM runtime_events WHERE type='NATIVE_SESSION_START_FAILED'",
        )
        .await
            != "0"
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the coordinator was not relaunched");
}
async fn purge(runtime: &Runtime) -> CoreResult<()> {
    let draft = sql(runtime, "SELECT task_id FROM workspace_tasks").await;
    runtime
        .host
        .executor
        .publication(move |store| {
            store
                .purge_workspace_task("project", &draft, false)
                .map(|_| ())
        })
        .unwrap()
        .await
        .unwrap()
}

/// test-ai: worker rows of tasks the Python host ran before runtime records existed
/// made the recovery transaction fail (`FOREIGN KEY constraint failed`), and with it
/// the whole project. A task whose state no longer parses failed it the same way.
#[tokio::test(flavor = "multi_thread")]
async fn tasks_without_a_runtime_record_or_with_a_damaged_state_never_fail_the_project() {
    let (root, runtime, _, _) = seeded().await;
    let r = root.to_string_lossy().into_owned();
    exec(&runtime, format!("
        INSERT INTO tasks VALUES ('legacy-old','{r}','0');
        INSERT INTO delegation_provisioning VALUES ('old-ready','legacy-old',NULL,'old-a1','worker-explorer','ready','ready',NULL,NULL,NULL,NULL,'engine-1',NULL,NULL,1,'0','0');
        INSERT INTO attempt_process_registry VALUES ('old-a1','legacy-old','worker',NULL,NULL,4242,4242,'100','an-earlier-boot','d','0','1','process_exited');
        INSERT INTO capacity_reservations VALUES ('old-r1','fixture','legacy-old',NULL,'old-a1','worker','released','0','0','process_exited');
        INSERT INTO delegation_provisioning VALUES ('old-failed','legacy-old',NULL,'old-a2','worker-qa-engineer','claimed:engine_session','failed',NULL,NULL,NULL,NULL,NULL,NULL,NULL,1,'0','0');
        INSERT INTO capacity_reservations VALUES ('old-r2','fixture','legacy-old',NULL,'old-a2','worker','released','0','0','never_started');
        INSERT INTO delegation_provisioning VALUES ('old-open','legacy-old',NULL,'old-a3','worker-explorer','starting','in_progress',NULL,NULL,NULL,NULL,NULL,NULL,NULL,1,'0','0');
        INSERT INTO capacity_reservations VALUES ('old-r3','fixture','legacy-old',NULL,'old-a3','worker','ambiguous','0','0','termination_unconfirmed');
        INSERT INTO tasks VALUES ('damaged','{r}','0');
        INSERT INTO runtime_tasks(task_id,state,goal,created_at,updated_at) VALUES ('damaged','NOT_A_STATE','g','0','0');
        INSERT INTO task_owners VALUES ('damaged','python-owner',1,'0');")).await;
    runtime.recover().await.unwrap();
    // Positive evidence settles the old task's rows; no runtime event is written for it.
    assert_eq!(sql(&runtime, "SELECT group_concat(idempotency_key||':'||status,',') FROM (SELECT * FROM delegation_provisioning WHERE task_id='legacy-old' ORDER BY idempotency_key)").await,
        "old-failed:terminated,old-open:in_progress,old-ready:terminated");
    assert_eq!(
        sql(
            &runtime,
            "SELECT count(*) FROM runtime_events WHERE task_id IN ('legacy-old','damaged')"
        )
        .await,
        "0"
    );
    // The damaged task is left exactly as it was; the project opened.
    assert_eq!(
        sql(
            &runtime,
            "SELECT state||':'||coalesce(blocker,'') FROM runtime_tasks WHERE task_id='damaged'"
        )
        .await,
        "NOT_A_STATE:"
    );
    // A later restart is just as quiet.
    runtime.recover().await.unwrap();
    close(root, runtime).await;
}

/// Business/Cortex/test-dev: a finished task the Python host ran was never adopted,
/// so Request changes, a message, Continue, Retry and Close all refused
/// `native_execution_binding_invalid`, and its result read "binding unavailable".
#[tokio::test(flavor = "multi_thread")]
async fn a_finished_python_task_is_adopted_and_its_result_is_shown_as_an_earlier_version() {
    let (root, runtime, execution, task) = seeded().await;
    exec(&runtime, format!("
        UPDATE runtime_events SET payload_json=json_set(payload_json,'$.execution_host','python') WHERE type='TASK_CREATED';
        UPDATE runtime_tasks SET state='COMPLETED',synthesis=NULL WHERE task_id='{task}';")).await;
    let message = |key: &str| json!({"projectId":"project","executionId":execution,"requestId":key,"action":"prompt","text":"Please change it."});
    assert_eq!(
        runtime
            .control("project".into(), message("before"))
            .await
            .unwrap_err()
            .to_string(),
        "native_execution_binding_invalid"
    );
    runtime.recover().await.unwrap();
    assert_eq!(sql(&runtime, "SELECT json_extract(payload_json,'$.finished')||':'||json_extract(payload_json,'$.replayed') FROM runtime_events WHERE type='NATIVE_TASK_ADOPTED'").await, "1:0");
    assert_eq!(sql(&runtime, "SELECT o.epoch=e.owner_epoch AND o.epoch=2 FROM task_owners o JOIN workspace_executions e ON e.runtime_task_id=o.task_id").await, "1");
    // The finished result reads as a read-only result of an earlier version, with its
    // own identity; it is never acceptable here, and Request changes is offered.
    let (actions, _) = crate::machine::task_facts::native_review(
        &root,
        "project",
        &execution,
        "COMPLETED",
        &json!({}),
        None,
    );
    assert_eq!(actions["accept"], false);
    assert_eq!(actions["unblockAction"], "request_changes");
    assert_eq!(actions["earlierVersion"], true);
    assert!(
        actions["reason"]
            .as_str()
            .unwrap()
            .contains("earlier version of the app"),
        "{actions}"
    );
    let version = sql(&runtime, "SELECT max(id) FROM runtime_events").await;
    assert_eq!(actions["version"], version);
    assert_eq!(
        actions["digest"],
        format!("result:wr_{execution}:{version}")
    );
    // A closed card is closed, not "waiting for Cortex".
    let (closed, _) = crate::machine::task_facts::native_review(
        &root,
        "project",
        &execution,
        "COMPLETED",
        &json!({"status":"done"}),
        None,
    );
    assert_eq!(closed["reason"], "This task is closed.");
    assert_eq!(closed["unblockAction"], Value::Null);
    let (stopped, _) = crate::machine::task_facts::native_review(
        &root,
        "project",
        &execution,
        "CANCELLED",
        &json!({}),
        None,
    );
    assert_eq!(
        stopped["reason"],
        "This task stopped without a result to review."
    );
    // Request changes now reopens it and relaunches the coordinator.
    let receipt = runtime
        .control("project".into(), message("after"))
        .await
        .unwrap();
    assert_eq!(receipt["outcome"], "recorded");
    assert_eq!(
        sql(
            &runtime,
            "SELECT count(*) FROM runtime_events WHERE type='TASK_REOPENED'"
        )
        .await,
        "1"
    );
    relaunched(&runtime).await;
    // Adoption happens once.
    runtime.recover().await.unwrap();
    assert_eq!(
        sql(
            &runtime,
            "SELECT count(*) FROM runtime_events WHERE type='NATIVE_TASK_ADOPTED'"
        )
        .await,
        "1"
    );
    close(root, runtime).await;
}

/// Cortex/Business: process records of the Python host never got an end time, so old
/// tasks kept reading as running and Delete was refused `active_execution` forever.
#[tokio::test(flavor = "multi_thread")]
async fn process_records_end_only_when_their_process_cannot_run() {
    let (root, runtime, execution, task) = seeded().await;
    let close_task = json!({"projectId":"project","executionId":execution,"requestId":"close","action":"close_task","confirmed":true});
    runtime.control("project".into(), close_task).await.unwrap();
    let own = std::fs::read_to_string("/proc/self/stat").unwrap();
    let fields: Vec<&str> = own[own.rfind(')').unwrap() + 1..]
        .split_whitespace()
        .collect();
    let (pid, pgid, start) = (
        std::process::id(),
        fields[2].to_owned(),
        fields[19].to_owned(),
    );
    let mut child = std::process::Command::new("/bin/true").spawn().unwrap();
    let dead = child.id();
    child.wait().unwrap();
    let b = boot();
    exec(&runtime, format!("
        INSERT INTO attempt_process_registry VALUES ('p-old','{task}','acp_host',NULL,NULL,1,1,'1','an-earlier-boot','d','0',NULL,NULL);
        INSERT INTO attempt_process_registry VALUES ('p-dead','{task}','acp_host',NULL,NULL,{dead},{dead},'1','{b}','d','0',NULL,NULL);
        INSERT INTO attempt_process_registry VALUES ('p-live','{task}','acp_host',NULL,NULL,{pid},{pgid},'{start}','{b}','d','0',NULL,NULL);")).await;
    assert_eq!(
        purge(&runtime).await.unwrap_err().code(),
        "active_execution"
    );
    runtime.recover().await.unwrap();
    assert_eq!(sql(&runtime, "SELECT group_concat(attempt_id||':'||coalesce(termination_reason,'open'),',') FROM (SELECT * FROM attempt_process_registry ORDER BY attempt_id)").await,
        "p-dead:process_gone,p-live:open,p-old:previous_boot");
    // The live process still blocks deletion; once it ends, Delete proceeds.
    assert_eq!(
        purge(&runtime).await.unwrap_err().code(),
        "active_execution"
    );
    exec(&runtime, "UPDATE attempt_process_registry SET terminated_at='1',termination_reason='process_exited' WHERE attempt_id='p-live'".into()).await;
    purge(&runtime).await.unwrap();
    close(root, runtime).await;
}

/// A record from an earlier boot never blocks deletion, even before any recovery.
#[tokio::test(flavor = "multi_thread")]
async fn a_process_record_from_an_earlier_boot_never_blocks_deletion() {
    let (root, runtime, execution, task) = seeded().await;
    let close_task = json!({"projectId":"project","executionId":execution,"requestId":"close","action":"close_task","confirmed":true});
    runtime.control("project".into(), close_task).await.unwrap();
    exec(&runtime, format!("INSERT INTO attempt_process_registry VALUES ('p-old','{task}','worker',NULL,NULL,1,1,'1','an-earlier-boot','d','0',NULL,NULL);")).await;
    purge(&runtime).await.unwrap();
    close(root, runtime).await;
}

/// algotrading/frontier: Python `failed`/`in_progress` worker rows with no engine
/// session and no process record, whose capacity that host had already released,
/// were flipped to `ambiguous` ("outcome unknown") on every restart. Held tasks were
/// re-recorded as "recovery required" on every restart.
#[tokio::test(flavor = "multi_thread")]
async fn never_started_legacy_rows_settle_and_restarts_record_nothing_new() {
    let (root, runtime, _, task) = seeded().await;
    exec(&runtime, format!("
        UPDATE runtime_tasks SET state='EXECUTING' WHERE task_id='{task}';
        INSERT INTO delegation_provisioning VALUES ('py-failed','{task}',NULL,'py-a1','worker-backend-dev','claimed:engine_session','failed',NULL,NULL,NULL,NULL,NULL,NULL,NULL,1,'0','0');
        INSERT INTO capacity_reservations VALUES ('py-r1','fixture','{task}',NULL,'py-a1','worker','released','0','0','process_exited');
        INSERT INTO delegation_provisioning VALUES ('py-progress','{task}',NULL,'py-a2','worker-backend-dev','claimed:engine_session','in_progress',NULL,NULL,NULL,NULL,NULL,NULL,NULL,1,'0','0');
        INSERT INTO capacity_reservations VALUES ('py-r2','fixture','{task}',NULL,'py-a2','worker','released','0','0','never_started');
        INSERT INTO delegation_provisioning VALUES ('unconfirmed','{task}',NULL,'x-a3','worker-backend-dev','ready','ambiguous',NULL,NULL,NULL,NULL,NULL,NULL,NULL,1,'0','0');
        INSERT INTO capacity_reservations VALUES ('x-r3','fixture','{task}',NULL,'x-a3','worker','ambiguous','0','0','termination_unconfirmed');")).await;
    runtime.recover().await.unwrap();
    assert_eq!(sql(&runtime, "SELECT group_concat(idempotency_key||':'||status,',') FROM (SELECT * FROM delegation_provisioning ORDER BY idempotency_key)").await,
        "py-failed:terminated,py-progress:terminated,unconfirmed:terminated");
    // The unconfirmed attempt's host is gone and no engine works in the project.
    assert_eq!(sql(&runtime, "SELECT state||':'||release_observation FROM capacity_reservations WHERE reservation_id='x-r3'").await,
        "released:no_engine_process");
    assert_eq!(sql(&runtime, "SELECT json_extract(payload_json,'$.evidence')||':'||json_extract(payload_json,'$.count') FROM runtime_events WHERE type='NATIVE_PROCESS_SETTLED'").await,
        "no_engine_session_or_process_and_capacity_released:2");
    assert_eq!(
        sql(&runtime, "SELECT state||':'||blocker FROM runtime_tasks").await,
        "RECOVERING:native_session_outcome_unknown"
    );
    let counted = "SELECT count(*) FROM runtime_events";
    let before = sql(&runtime, counted).await;
    runtime.recover().await.unwrap();
    runtime.recover().await.unwrap();
    assert_eq!(
        sql(&runtime, counted).await,
        before,
        "a restart re-recorded an unchanged recovery"
    );
    assert_eq!(
        sql(
            &runtime,
            "SELECT count(*) FROM runtime_events WHERE type='NATIVE_RECOVERY_REQUIRED'"
        )
        .await,
        "1"
    );
    close(root, runtime).await;
}

/// Live run 2026-11-17: an earlier build held a verified, synthesized result as
/// "interrupted" (`completion_worker_check_unverified`) because a reviewer's
/// descriptive check command (`python3 -c "..."`) differed from its receipt. The
/// next start checks such a result again instead of leaving it held; a hold whose
/// latest verification failed, or any other completion hold, is left as it was.
#[tokio::test(flavor = "multi_thread")]
async fn a_result_held_only_for_a_descriptive_check_command_is_checked_again() {
    for (verification, blocker, rechecked) in [
        (
            "VERIFICATION_PASSED",
            "completion_worker_check_unverified",
            true,
        ),
        (
            "VERIFICATION_FAILED",
            "completion_worker_check_unverified",
            false,
        ),
        (
            "VERIFICATION_PASSED",
            "completion_artifact_changed_or_missing",
            false,
        ),
    ] {
        let (root, runtime, _, task) = seeded().await;
        exec(&runtime, format!("
            UPDATE runtime_tasks SET state='RECOVERING',blocker='{blocker}',synthesis='Done.' WHERE task_id='{task}';
            INSERT INTO runtime_events(task_id,type,actor,payload_json,created_at) VALUES ('{task}','{verification}','runtime','{{}}','0');")).await;
        runtime.recover().await.unwrap();
        let resumed = "SELECT count(*) FROM runtime_events WHERE type='NATIVE_RECOVERY_RESUMED' AND json_extract(payload_json,'$.reason')='completion_rechecked' AND json_extract(payload_json,'$.state')='RECONCILING'";
        assert_eq!(
            sql(&runtime, resumed).await,
            if rechecked { "1" } else { "0" },
            "{verification}/{blocker}"
        );
        if !rechecked {
            assert_eq!(
                sql(&runtime, "SELECT state||':'||blocker FROM runtime_tasks").await,
                format!("RECOVERING:{blocker}")
            );
        }
        close(root, runtime).await;
    }
}

/// A service killed while a session ran never removed its broker directory, and the
/// project's store kept it forever. Recovery removes a broker directory no socket is
/// bound in once it is a minute old; a fresh one and anything else are kept.
#[tokio::test(flavor = "multi_thread")]
async fn a_dead_services_broker_directory_is_removed_on_recovery() {
    let (root, runtime, _, _) = seeded().await;
    let store = root.join(if workspace_protocol::channel::current().is_dev() {
        ".cortex-dev"
    } else {
        ".cortex"
    });
    let stale = store.join("cortex-mcp-47b3baad186df5cc42d2cd46");
    let fresh = store.join("cortex-mcp-0123456789abcdef01234567");
    let other = store.join("cortex-mcp-not-a-broker");
    for dir in [&stale, &fresh, &other] {
        std::fs::create_dir(dir).unwrap();
        std::fs::write(dir.join("relay.token"), "fixture").unwrap();
    }
    let old = std::time::SystemTime::now() - Duration::from_secs(120);
    for dir in [&stale, &other] {
        std::fs::File::open(dir).unwrap().set_modified(old).unwrap();
    }
    runtime.recover().await.unwrap();
    assert!(
        !stale.exists(),
        "the dead service's broker directory stayed"
    );
    assert!(fresh.exists() && other.exists());
    close(root, runtime).await;
}

/// Owner store 2026-11-17: a worker's turn ended without an accepted result, the retry
/// gave up (`lost_worker_action_outcome_unknown`) and the stop left the task in
/// RECOVERING for a Retry no screen offers, with no question. A stop for such a work
/// blocker now asks the owner at once with the runtime's stop question.
#[tokio::test(flavor = "multi_thread")]
async fn a_work_stop_asks_the_owner_with_concrete_answers() {
    let (root, runtime, _, task) = seeded().await;
    exec(
        &runtime,
        format!("UPDATE runtime_tasks SET state='EXECUTING',blocker=NULL WHERE task_id='{task}';"),
    )
    .await;
    let (owner, epoch) = (
        sql(&runtime, "SELECT owner_ref FROM task_owners").await,
        sql(&runtime, "SELECT epoch FROM task_owners").await,
    );
    let owner = OwnerEpoch {
        owner,
        epoch: epoch.parse().unwrap(),
    };
    let (_keep, cancelled) = tokio::sync::watch::channel(false);
    runtime
        .stop_after_error(
            &task,
            &owner,
            CoreError::Refused("lost_worker_action_outcome_unknown"),
            cancelled,
        )
        .await;
    assert_eq!(
        sql(&runtime, "SELECT state||':'||blocker FROM runtime_tasks").await,
        "WAITING_USER:lost_worker_action_outcome_unknown"
    );
    assert_eq!(sql(&runtime, "SELECT count(*) FROM pending_questions WHERE status='open' AND dedup_key LIKE 'runtime:stop:%'").await, "1");
    let question = sql(
        &runtime,
        "SELECT text FROM pending_questions WHERE status='open'",
    )
    .await;
    assert!(
        question.contains("without an accepted result"),
        "{question}"
    );
    assert!(!question.contains("lost_worker"), "{question}");
    close(root, runtime).await;
}

/// The owner's task stuck that way by an earlier build (RECOVERING, no blocker, the
/// last ASK_USER_REQUIRED unanswered and nothing after it) gets the question on the
/// next start; a task where anything followed the stop is left alone.
#[tokio::test(flavor = "multi_thread")]
async fn an_unasked_legacy_work_stop_gets_its_question_on_start() {
    for followed in [false, true] {
        let (root, runtime, _, task) = seeded().await;
        exec(&runtime, format!("
            UPDATE runtime_tasks SET state='RECOVERING',blocker=NULL WHERE task_id='{task}';
            UPDATE pending_questions SET status='answered' WHERE task_id='{task}';
            INSERT INTO runtime_events(task_id,type,actor,payload_json,created_at) VALUES ('{task}','NATIVE_DRIVER_STOPPED','runtime','{{\"reason\":\"lost_worker_action_outcome_unknown\"}}','0');
            INSERT INTO runtime_events(task_id,type,actor,payload_json,created_at) VALUES ('{task}','ASK_USER_REQUIRED','runtime','{{\"blocker\":\"lost_worker_action_outcome_unknown\"}}','0');
            INSERT INTO runtime_events(task_id,type,actor,payload_json,created_at) VALUES ('{task}','TASK_STATE_CHANGED','runtime','{{\"state\":\"RECOVERING\"}}','0');")).await;
        if followed {
            exec(&runtime, format!("INSERT INTO runtime_events(task_id,type,actor,payload_json,created_at) VALUES ('{task}','USER_STEERING','runtime','{{}}','0');")).await;
        }
        runtime.recover().await.unwrap();
        let state = sql(
            &runtime,
            "SELECT state||':'||coalesce(blocker,'') FROM runtime_tasks",
        )
        .await;
        let open = sql(&runtime, "SELECT count(*) FROM pending_questions WHERE status='open' AND dedup_key LIKE 'runtime:stop:%'").await;
        if followed {
            assert_eq!((state.as_str(), open.as_str()), ("RECOVERING:", "0"));
        } else {
            assert_eq!(
                (state.as_str(), open.as_str()),
                ("WAITING_USER:lost_worker_action_outcome_unknown", "1")
            );
            // A later start does not ask twice.
            runtime.recover().await.unwrap();
            assert_eq!(
                sql(
                    &runtime,
                    "SELECT count(*) FROM pending_questions WHERE dedup_key LIKE 'runtime:stop:%'"
                )
                .await,
                "1"
            );
        }
        close(root, runtime).await;
    }
}
