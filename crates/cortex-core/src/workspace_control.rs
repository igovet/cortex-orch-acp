//! Native desktop owner adapter. Bindings, CAS, intent and receipts are authoritative.
#[cfg(test)]
mod coordinator_card_tests {
    use super::*;
    #[test]
    fn activity_does_not_stale_a_pending_coordinator_card() {
        let root = std::env::temp_dir().join(format!("coordinator-card-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let key = crate::ProjectKey::new(&root, crate::StoreChannel::Dev).unwrap();
        let store = Store::open(key, crate::OpenMode::Initialize).unwrap();
        let machine = lifecycle::TaskMachine::new(&store);
        let owner = machine.create("t", "g", "o").unwrap();
        machine
            .transition(
                "t",
                &owner,
                "plan",
                TaskState::Planning,
                None,
                &lifecycle::FoundationGuards,
            )
            .unwrap();
        store
            .write(|db| {
                let q = lifecycle::skill_proposals::request(
                    db,
                    &root,
                    "t",
                    "c",
                    "ask",
                    &json!({"question":"Continue?","options":["Yes","No"]}),
                )?;
                let mut b = Binding {
                    task: "t".into(),
                    draft: "d".into(),
                    session: "s".into(),
                    owner: owner.clone(),
                    version: 1,
                    execution_version: 1,
                    goal: "g".into(),
                };
                let first = decisions_tx(db, &b)?[0].clone();
                lifecycle::event_tx(
                    db,
                    "t",
                    "NATIVE_ACTIVITY_CHANGED",
                    json!({"status":"completed"}),
                )?;
                b.execution_version += 100;
                let second = decisions_tx(db, &b)?[0].clone();
                assert_eq!(first["version"], second["version"]);
                assert_eq!(second["status"], "pending");
                waits::answer_tx(
                    db,
                    "t",
                    q["question_id"].as_str().unwrap(),
                    "Yes",
                    "text",
                    None,
                )?;
                assert_eq!(decisions_tx(db, &b)?[0]["status"], "answered");
                Ok(())
            })
            .unwrap();
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn stop_option_of_a_coordinator_question_is_an_answer_but_the_runtime_stop_question_finishes() {
        let root = std::env::temp_dir().join(format!("stop-answer-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let key = crate::ProjectKey::new(&root, crate::StoreChannel::Dev).unwrap();
        let store = Store::open(key, crate::OpenMode::Initialize).unwrap();
        let owner = lifecycle::TaskMachine::new(&store).create("t", "g", "o").unwrap();
        lifecycle::TaskMachine::new(&store)
            .transition("t", &owner, "plan", TaskState::Planning, None, &lifecycle::FoundationGuards)
            .unwrap();
        store
            .write(|db| {
                for answer in ["Stop", "stop", "finish now", "Стоп"] {
                    let q = lifecycle::skill_proposals::request(
                        db,
                        &root,
                        "t",
                        "c",
                        &format!("ask-{answer}"),
                        &json!({"question":"Stop the daemon first or keep it?","options":["Stop","Keep"]}),
                    )?;
                    assert!(waits::answer_tx(db, "t", q["question_id"].as_str().unwrap(), answer, "text", None)?);
                    assert_ne!(domain::task_tx(db, "t")?.0, TaskState::Cancelled, "{answer} cancelled the task");
                    let forced: i64 = db.query_row(
                        "SELECT count(*) FROM runtime_events WHERE task_id='t' AND type='USER_FORCE_FINISHED'",
                        [],
                        |r| r.get(0),
                    )?;
                    assert_eq!(forced, 0);
                }
                // The runtime's own stop question keeps its finish keywords.
                domain::state_tx(db, "t", TaskState::Executing, None)?;
                domain::stop_tx(db, "t", "native_session_start_failed")?;
                domain::state_tx(db, "t", TaskState::WaitingUser, None)?;
                let id: String = db.query_row(
                    "SELECT id FROM pending_questions WHERE task_id='t' AND status='open' AND dedup_key LIKE 'runtime:stop:%'",
                    [],
                    |r| r.get(0),
                )?;
                assert!(waits::answer_tx(db, "t", &id, "Stop", "text", None)?);
                assert_eq!(domain::task_tx(db, "t")?.0, TaskState::Cancelled);
                Ok(())
            })
            .unwrap();
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn the_runtime_stop_question_speaks_the_task_language_and_names_the_reason() {
        let root = std::env::temp_dir().join(format!("stop-ru-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let key = crate::ProjectKey::new(&root, crate::StoreChannel::Dev).unwrap();
        let store = Store::open(key, crate::OpenMode::Initialize).unwrap();
        for (task, settings, goal, ru) in [
            ("ru", Some("ru"), "Write the guide", true),
            ("en", Some("en"), "Напиши руководство", false),
            ("auto", Some("auto"), "Изучи аватарку и опиши стиль канала", true),
            ("none", None, "Describe the channel style", false),
        ] {
            lifecycle::TaskMachine::new(&store).create(task, "g", "o").unwrap();
            store
                .write(|db| {
                    lifecycle::event_tx(db, task, "NATIVE_START_ADMITTED", json!({"goal":goal}))?;
                    if let Some(language) = settings {
                        lifecycle::event_tx(db, task, "NATIVE_TASK_SETTINGS", json!({"settings":{"language":language}}))?;
                    }
                    domain::state_tx(db, task, TaskState::Executing, None)?;
                    domain::stop_tx(db, task, "native_session_start_failed")?;
                    let (text, options): (String, String) = db.query_row(
                        "SELECT text,options_json FROM pending_questions WHERE task_id=? AND status='open'",
                        [task],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )?;
                    assert!(!text.contains("native_session_start_failed"), "{text}");
                    if ru {
                        assert_eq!(text, "Задача остановлена: не удалось запустить сессию воркера. Что делать дальше?");
                        assert_eq!(options, json!(domain::STOP_OPTIONS_RU).to_string());
                    } else {
                        assert_eq!(text, "The task stopped: a worker session could not start. What should happen next?");
                        assert_eq!(options, json!(domain::STOP_OPTIONS_EN).to_string());
                    }
                    Ok(())
                })
                .unwrap();
        }
        // The Russian finish option, exactly as offered, finishes the task.
        store
            .write(|db| {
                let id: String = db.query_row(
                    "SELECT id FROM pending_questions WHERE task_id='ru' AND status='open'",
                    [],
                    |r| r.get(0),
                )?;
                assert!(waits::answer_tx(db, "ru", &id, domain::STOP_OPTIONS_RU[1], "text", None)?);
                assert_eq!(domain::task_tx(db, "ru")?.0, TaskState::Cancelled);
                Ok(())
            })
            .unwrap();
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn activity_does_not_stale_a_pending_worker_card_but_revocation_still_fences_it() {
        let root = std::env::temp_dir().join(format!("worker-card-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let store = Store::open(
            crate::ProjectKey::new(&root, crate::StoreChannel::Dev).unwrap(),
            crate::OpenMode::Initialize,
        )
        .unwrap();
        let owner = lifecycle::TaskMachine::new(&store)
            .create("t", "g", "o")
            .unwrap();
        store.write(|db|{
            db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='t'",[])?;
            db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'work','worker-backend-dev','Work','[]','[\"src\"]','execution','running','assignment',NULL)",[])?;
            let c=crate::channels::issue_tx(db,"t",&owner,crate::channels::Principal::Worker{assignment:"assignment"})?;
            crate::channels::bind_session_tx(db,&c,"engine")?;
            waits::request_tx_bound(db,"t",&c.channel_id,"question",&[waits::QuestionForm{question:"Continue?".into(),options:vec!["Yes".into(),"No".into()]}])?;
            let mut b=Binding{task:"t".into(),draft:"d".into(),session:"s".into(),owner:owner.clone(),version:1,execution_version:1,goal:"g".into()};
            let first=decisions_tx(db,&b)?[0].clone();
            lifecycle::event_tx(db,"t","NATIVE_TURN_SETTLED",json!({"status":"completed"}))?;
            b.execution_version+=100;
            let second=decisions_tx(db,&b)?[0].clone();
            assert_eq!(first["version"],second["version"]);
            assert_eq!(second["status"],"pending");
            crate::channels::revoke_tx(db,&c.channel_id)?;
            assert_eq!(decisions_tx(db,&b)?[0]["status"],"superseded");
            Ok(())
        }).unwrap();
        // The refused desktop answer commits the authority-loss cancellation separately,
        // so the question is not left pending but unanswerable.
        let open = |store: &Store| store.read(|db| Ok(db.query_row("SELECT count(*) FROM pending_questions WHERE status='open'",[],|r|r.get::<_,i64>(0))?)).unwrap();
        assert_eq!(open(&store), 1);
        assert_eq!(store.write(|db| waits::cancel_unauthorized_tx(db, "t")).unwrap(), 1);
        assert_eq!(open(&store), 0);
        assert_eq!(store.write(|db| waits::cancel_unauthorized_tx(db, "t")).unwrap(), 0);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}
use crate::{
    error::refuse,
    lifecycle::{self, domain, waits},
    ownership::OwnerEpoch,
    CoreError, CoreResult, Store, TaskState,
};
use rusqlite::{Connection, OptionalExtension, Transaction};
use serde_json::{json, Value};

#[derive(Clone)]
pub struct Binding {
    pub task: String,
    pub draft: String,
    pub session: String,
    pub owner: OwnerEpoch,
    pub version: i64,
    pub execution_version: i64,
    pub goal: String,
}
pub fn binding(
    db: &Connection,
    root: &str,
    project: &str,
    actor: &str,
    execution: &str,
) -> CoreResult<Binding> {
    binding_with(db, root, project, actor, execution, true)
}
/// Read-only binding: the same project/root/owner/session identity, without
/// requiring the Rust host to own the task. Completed tasks the Python host ran
/// are never adopted, but their questions must still be readable (never answerable:
/// a terminal task offers no answer operation).
fn read_binding(db: &Connection, root: &str, project: &str, actor: &str, execution: &str) -> CoreResult<Binding> {
    binding_with(db, root, project, actor, execution, false)
}
fn binding_with(
    db: &Connection,
    root: &str,
    project: &str,
    actor: &str,
    execution: &str,
    native: bool,
) -> CoreResult<Binding> {
    let row = db.query_row(
        "SELECT e.runtime_task_id,t.task_id,e.acp_session_id,o.owner_ref,o.epoch,t.version,e.version,r.goal
         FROM workspace_executions e JOIN workspace_tasks t USING(task_id)
         JOIN workspace_projects p USING(project_id) JOIN runtime_tasks r ON r.task_id=e.runtime_task_id
         JOIN tasks rt ON rt.id=r.task_id JOIN task_owners o ON o.task_id=r.task_id
         JOIN acp_host_sessions s ON s.session_id=e.acp_session_id
         WHERE p.project_id=? AND p.project_root=? AND p.activated_by=? AND rt.project_root=p.project_root
         AND e.execution_id=? AND e.owner_epoch=o.epoch AND s.task_id=r.task_id AND s.project_root=p.project_root
         AND (?5=0 OR EXISTS(SELECT 1 FROM runtime_events v WHERE v.task_id=r.task_id AND v.type IN ('TASK_CREATED','NATIVE_TASK_ADOPTED')
         AND json_extract(v.payload_json,'$.execution_host')='rust'))",
        (project,root,actor,execution,native), |r| Ok(Binding {
            task:r.get(0)?,draft:r.get(1)?,session:r.get(2)?,
            owner:OwnerEpoch {owner:r.get(3)?,epoch:r.get(4)?},version:r.get(5)?,
            execution_version:r.get(6)?,goal:r.get(7)?,
        })).optional()?;
    row.ok_or(CoreError::Refused("native_execution_binding_invalid"))
}
fn key(request: &Value) -> CoreResult<&str> {
    let key = request["requestId"]
        .as_str()
        .ok_or(CoreError::Refused("invalid_request"))?;
    workspace_protocol::validate("Identity", &json!(key))
        .map_err(|_| CoreError::Refused("invalid_request"))?;
    Ok(key)
}
fn prior(db: &Connection, b: &Binding, request: &Value) -> CoreResult<Option<Value>> {
    let row: Option<(String,String)> = db.query_row(
        "SELECT digest,result_json FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=? AND operation='native_desktop_control'",
        (&b.task,&b.owner.owner,key(request)?), |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
    let Some((digest, _)) = row else {
        return Ok(None);
    };
    // request_tx uses the canonical JSON digest.
    if digest
        != crate::digest(&serde_json::to_vec(
            &json!({"operation":"native_desktop_control","payload":request}),
        )?)
    {
        return refuse("payload_conflict");
    }
    let finished: Option<String> = db.query_row(
        "SELECT result_json FROM runtime_requests WHERE task_id=? AND actor='native-desktop-finish' AND request_key=?",
        (&b.task,key(request)?), |r| r.get(0)).optional()?;
    let encoded = finished.ok_or(CoreError::Refused("native_control_outcome_unknown"))?;
    let receipt: Value = serde_json::from_str(&encoded)?;
    // An explicitly settled unknown outcome never claims the control happened.
    if receipt["outcome"] == "unknown" {
        return refuse("native_control_outcome_unknown");
    }
    Ok(Some(receipt))
}
/// Bound an admitted control that never reached its finish receipt. The finish
/// slot is closed with an explicit `outcome: unknown` receipt (revision and time
/// recorded), the task is held in a retryable recovery state and the request
/// keeps replaying as `native_control_outcome_unknown`. Nothing is claimed to
/// have happened, and later restarts no longer re-raise this admission.
pub fn settle_unknown(
    store: &Store,
    project: &str,
    actor: &str,
    request: &Value,
    reason: &str,
) -> CoreResult<()> {
    store.write(|db| {
        let b = binding(
            db,
            &store.key().root().to_string_lossy(),
            project,
            actor,
            request["executionId"]
                .as_str()
                .ok_or(CoreError::Refused("invalid_execution"))?,
        )?;
        let key = key(request)?;
        let admitted: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=? AND operation='native_desktop_control')",
            (&b.task, &b.owner.owner, key),
            |r| r.get(0),
        )?;
        let finished: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM runtime_requests WHERE task_id=? AND actor='native-desktop-finish' AND request_key=?)",
            (&b.task, key),
            |r| r.get(0),
        )?;
        if !admitted || finished {
            return Ok(());
        }
        let revision = domain::task_tx(db, &b.task)?.1;
        lifecycle::request_tx(
            db,
            &b.task,
            "native-desktop-finish",
            key,
            "native_desktop_finished",
            request,
            || {
                lifecycle::event_tx(
                    db,
                    &b.task,
                    "NATIVE_CONTROL_OUTCOME_UNKNOWN",
                    json!({"request":key,"action":request["action"],"reason":reason,"revision":revision}),
                )?;
                // Pause/stop are visible in the state itself; other actions leave the
                // task unobservable until the owner retries.
                if !matches!(
                    request["action"].as_str(),
                    Some("task_settings" | "worker_message" | "rerun_check" | "pause" | "stop")
                ) {
                    domain::native_recovery_required_tx(
                        db,
                        &b.task,
                        "native_control_outcome_unknown",
                    )?;
                }
                Ok(json!({"outcome":"unknown","native":true,"reason":reason,
                    "revision":revision,"settledAt":crate::now()}))
            },
        )?;
        Ok(())
    })
}
pub fn replay(
    store: &Store,
    project: &str,
    actor: &str,
    request: &Value,
) -> CoreResult<Option<Value>> {
    store.read(|db| {
        let b = binding(
            db,
            &store.key().root().to_string_lossy(),
            project,
            actor,
            request["executionId"]
                .as_str()
                .ok_or(CoreError::Refused("invalid_execution"))?,
        )?;
        prior(db, &b, request)
    })
}
pub fn admit(store: &Store, project: &str, actor: &str, request: &Value) -> CoreResult<Binding> {
    admit_input(store, project, actor, request, None)
}
/// Build the trusted request identity once, before replay, admission and finish.
/// Client-provided fingerprints are not accepted.
pub fn canonical_request(
    mut request: Value,
    capture: Option<&crate::input::Capture>,
) -> CoreResult<Value> {
    if !request.is_object() || request.get("capture").is_some() {
        return refuse("invalid_request");
    }
    if let Some(c) = capture.filter(|c| !c.blocks.is_empty()) {
        if !matches!(request["action"].as_str(), Some("prompt" | "incoming_prompt")) {
            return refuse("native_attachments_unavailable");
        }
        request["capture"] = c.fingerprint();
    }
    Ok(request)
}
/// An owned process that has not been confirmed gone.
fn unsettled_tx(db: &rusqlite::Transaction<'_>, task: &str) -> CoreResult<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM delegation_provisioning WHERE task_id=? AND status!='terminated')
         OR EXISTS(SELECT 1 FROM capacity_reservations WHERE task_id=? AND state!='released')",
        (task, task),
        |r| r.get(0),
    )?)
}
/// Admit a canonical request from `canonical_request`. Captured attachments
/// become the next source revision in the same transaction as steering and
/// transcript rows. Replay and finish must receive this same request.
pub fn admit_input(
    store: &Store,
    project: &str,
    actor: &str,
    request: &Value,
    capture: Option<&crate::input::Capture>,
) -> CoreResult<Binding> {
    let capture = capture.filter(|c| !c.blocks.is_empty());
    if capture.is_some() && !matches!(request["action"].as_str(), Some("prompt" | "incoming_prompt")) {
        return refuse("native_attachments_unavailable");
    }
    // Fail closed if a caller did not bind the exact capture before replay.
    if request.get("capture") != capture.map(|c| c.fingerprint()).as_ref() {
        return refuse("invalid_input_capture");
    }
    store.write(|db| {
        let b = binding(db,&store.key().root().to_string_lossy(),project,actor,
            request["executionId"].as_str().ok_or(CoreError::Refused("invalid_execution"))?)?;
        if prior(db,&b,request)?.is_some() { return refuse("native_control_already_recorded"); }
        let action = request["action"].as_str().ok_or(CoreError::Refused("invalid_action"))?;
        let mut state = domain::task_tx(db,&b.task)?.0;
        // An owner message to a finished result (Request changes) reopens it for a
        // fresh plan, as Python's steering does. Cancelled tasks stay ended.
        if matches!(action,"prompt"|"incoming_prompt") && domain::reopen_tx(db,&b.task)? {
            state = TaskState::Recovering;
        }
        if state.terminal() && !matches!(action,"close_task"|"mark_resolved") { return refuse("task_not_resumable"); }
        lifecycle::request_tx(db,&b.task,&b.owner.owner,key(request)?,"native_desktop_control",request,|| {
            match action {
                "pause" | "stop" => {
                    domain::owner_pause_tx(db,&b.task,&b.owner,&format!("desktop:{}",key(request)?),false,b.version)?;
                    db.execute("UPDATE trusted_channels SET revoked_at=? WHERE task_id=? AND revoked_at IS NULL",(crate::now(),&b.task))?;
                },
                "resume" => {
                    // Refuse before any commit while an owned process is unconfirmed, so a
                    // refusal never leaves a resumed task without a driver.
                    if unsettled_tx(db,&b.task)? {return refuse("native_shutdown_unconfirmed");}
                    let version = request["expectedVersion"].as_str().and_then(|s| s.parse().ok()).ok_or(CoreError::Refused("stale_version"))?;
                    domain::owner_pause_tx(db,&b.task,&b.owner,&format!("desktop:{}",key(request)?),true,version)?;
                },
                "retry" => {
                    if !matches!(state,TaskState::Recovering|TaskState::WaitingUser|TaskState::Blocked) { return refuse("retry_not_expected"); }
                    // Retry is the owner's explicit acknowledgement of effects whose outcome
                    // stayed unknown (e.g. a command interrupted by a restart). Record them as
                    // settled-unknown, never as success, so recovery stops re-raising them on
                    // every service restart.
                    let acknowledged=db.execute("UPDATE runtime_requests SET result_json=json_set(CASE WHEN json_valid(result_json) THEN result_json ELSE '{}' END,
                        '$.settled',1,'$.outcome_unknown',1,'$.settled_by','owner_retry','$.settled_at',?)
                        WHERE task_id=? AND operation IN ('outside_command','outside_write','command_run','project_write','project_edit','project_mkdir','project_delete','project_skill_link','draft_write','external_call')
                        AND json_extract(result_json,'$.complete') IS NOT 1 AND json_extract(result_json,'$.status') IS NOT 'complete' AND json_extract(result_json,'$.settled') IS NOT 1",
                        (crate::now(),&b.task))?;
                    if acknowledged>0 {
                        lifecycle::event_tx(db,&b.task,"OWNER_ACKNOWLEDGED_UNKNOWN_EFFECTS",json!({"count":acknowledged,"request":key(request)?}))?;
                    }
                    domain::interrupt_tx(db,&b.task)?;
                },
                "prompt" | "incoming_prompt" => {
                    let text=request["text"].as_str().filter(|s|!s.trim().is_empty()&&s.chars().count()<=32_000).ok_or(CoreError::Refused("invalid_message"))?;
                    if request.get("attachments").is_some_and(|a|a.as_array().is_none_or(|a|!a.is_empty())) {return refuse("native_attachments_unavailable");}
                    if state==TaskState::Paused {
                        if action!="incoming_prompt" {return refuse("task_paused");}
                        // A new owner message explicitly resumes the ACP chat. Resume
                        // and source capture commit together, before any engine starts.
                        // Same pre-check as Resume: a refusal commits nothing.
                        if unsettled_tx(db,&b.task)? {return refuse("native_shutdown_unconfirmed");}
                        domain::owner_pause_tx(db,&b.task,&b.owner,&format!("incoming-resume:{}",key(request)?),true,b.version)?;
                    }
                    let sessions=db.prepare("SELECT session_id FROM acp_host_sessions WHERE task_id=?
                        AND json_extract(client_info,'$.name')='cortex-rust-acp'")?
                        .query_map([&b.task],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
                    if sessions.len()>1 {return refuse("incoming_session_binding_conflict");}
                    let attachments=match capture {
                        Some(c)=>Some(c.persist(store,db,&b.task,sessions.first().unwrap_or(&b.session),text)?),
                        None=>None,
                    };
                    let mut steering=json!({"text":text,"request":key(request)?});
                    if let Some(a)=&attachments {steering["attachments"]=a.clone();}
                    lifecycle::event_tx(db,&b.task,"USER_STEERING",steering)?;
                    let attachments=attachments.map(|a|a.to_string());
                    let at=crate::now();
                    for session in std::iter::once(&b.session).chain(sessions.iter()) {
                        let seq:i64=db.query_row("SELECT coalesce(max(seq),-1)+1 FROM (
                            SELECT seq FROM host_transcript WHERE session_id=? UNION ALL SELECT seq FROM host_updates WHERE session_id=?)",
                            (session,session),|r|r.get(0))?;
                        db.execute("INSERT INTO host_transcript(session_id,seq,kind,text,attachments_json,created_at) VALUES (?,?,'user_message',?,?,?)",
                            (session,seq,text,&attachments,&at))?;
                    }
                    domain::interrupt_tx(db,&b.task)?;
                },
                "close_task" | "mark_resolved" => {
                    if request["confirmed"]!=true {return refuse("confirmation_required");}
                    if request.get("expectedVersion").is_some_and(|v|*v!=b.version.to_string()) {return refuse("stale_version");}
                    if state!=TaskState::Completed { domain::cancel_task_tx(db,&b.task,"owner_resolved_without_certification")?; }
                },
                _ => return refuse("native_operation_unavailable"),
            }
            Ok(json!({"admitted":true}))
        })?;
        Ok(b)
    })
}
pub fn finish(
    store: &Store,
    project: &str,
    actor: &str,
    request: &Value,
    restart: bool,
) -> CoreResult<Value> {
    store.write(|db| {
        let b=binding(db,&store.key().root().to_string_lossy(),project,actor,request["executionId"].as_str().unwrap())?;
        lifecycle::request_tx(db,&b.task,"native-desktop-finish",key(request)?,"native_desktop_finished",request,|| {
            let unsettled: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM delegation_provisioning WHERE task_id=? AND status!='terminated')
                 OR EXISTS(SELECT 1 FROM capacity_reservations WHERE task_id=? AND state!='released')",
                (&b.task,&b.task), |r|r.get(0))?;
            let closing=matches!(request["action"].as_str(),Some("close_task"|"mark_resolved"));
            // A close already committed its cancellation at admission (or the task had
            // finished): the task is closed whatever an earlier attempt's process did, so
            // the close is reported done. The unconfirmed process stays recorded and keeps
            // deletion refused until it is settled; nothing claims it exited.
            let state=domain::task_tx(db,&b.task)?.0;
            if unsettled && !(closing && state.terminal()) {return refuse("native_shutdown_unconfirmed");}
            if restart {domain::native_continue_tx(db,&b.task)?;}
            if closing {
                let mut resolved=json!({"certified":state==TaskState::Completed});
                if unsettled {resolved["processes_unconfirmed"]=json!(true);}
                lifecycle::event_tx(db,&b.task,"OWNER_TASK_RESOLVED",resolved)?;
            }
            let version=domain::pause_version_tx(db,&b.task)?;
            let mut receipt=json!({"outcome":"recorded","native":true,"state":domain::task_tx(db,&b.task)?.0,"entityVersion":version.to_string()});
            if closing && unsettled {receipt["processesUnconfirmed"]=json!(true);}
            Ok(receipt)
        })
    })
}
fn decisions_tx(db: &Transaction<'_>, b: &Binding) -> CoreResult<Vec<Value>> {
    let waits = waits::projection_tx(db, &b.task)?;
    let rows=db.prepare("SELECT id,text,options_json,status,created_at,answered_at,answer_text,assignment_ref FROM pending_questions WHERE task_id=? ORDER BY rowid LIMIT 1001")?
        .query_map([&b.task],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<String>>(2)?,r.get::<_,String>(3)?,
            r.get::<_,String>(4)?,r.get::<_,Option<String>>(5)?,r.get::<_,Option<String>>(6)?,r.get::<_,Option<String>>(7)?)))?
        .collect::<Result<Vec<_>,_>>()?;
    if rows.len() > 1000 {
        return refuse("native_decision_history_limit");
    }
    let requirement: i64 = db.query_row(
        "SELECT coalesce((SELECT requirement_revision FROM task_orchestration WHERE task_id=?),0)",
        [&b.task],
        |r| r.get(0),
    )?;
    let state = domain::task_tx(db, &b.task)?.0;
    // A paused task keeps the coordinator question it was waiting on, shown but not
    // answerable until Resume; the question itself is not superseded by Pause.
    let paused_saved = if state == TaskState::Paused { domain::pause_saved_tx(db, &b.task)? } else { None };
    rows.into_iter().map(|(id,text,options,status,created,answered,answer,assignment)| {
        let w=waits.iter().rev().find(|w|w["question_id"]==id||w["questions"].as_array().is_some_and(|qs|qs.iter().any(|q|q["question_id"]==id)));
        let resumable=match w {Some(w)=>waits::valid_tx(db,w)?,None=>false};
        let interrupted=w.is_some_and(|w| w["reason"]=="native_session_lost"
            && matches!(w["status"].as_str(),Some("open"|"ready"))
            && w["continuation_required"]==true) && !resumable;
        let paused_question=paused_saved.as_ref().is_some_and(|saved|saved["resumeState"]=="WAITING_USER"
            &&saved["questionIds"].as_array().is_some_and(|ids|ids.contains(&json!(id))));
        let valid=match w {Some(w)=>w["status"]=="open"&&(interrupted||resumable),None=>assignment.is_none()&&(state==TaskState::WaitingUser||paused_question)};
        let status=match status.as_str() {"answered"=>"answered","open" if valid=>"pending",_=>"superseded"};
        let labels:Vec<String>=serde_json::from_str(options.as_deref().unwrap_or("[]"))?;
        let options:Vec<_>=labels.into_iter().enumerate().map(|(i,label)|json!({"optionId":format!("wo_{}", &crate::digest(format!("{id}:{i}:{label}").as_bytes())[..40]),"label":label})).collect();
        let saved_batch: Option<String> = if w.is_none() {
            db.query_row("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='USER_QUESTION_REQUESTED'
                AND EXISTS(SELECT 1 FROM json_each(json_extract(payload_json,'$.questions'))
                    WHERE json_extract(value,'$.question_id')=?) ORDER BY id DESC LIMIT 1",
                (&b.task,&id),|r|r.get(0)).optional()?
        } else { None };
        let saved_batch: Value = saved_batch.map(|s|serde_json::from_str(&s)).transpose()?.unwrap_or(Value::Null);
        let batch=w.and_then(|w|w.get("batch_id")).cloned().unwrap_or_else(||saved_batch["batch_id"].clone());
        // Each card binds its own question/wait, never unrelated turn or
        // activity events arriving between display and the answer transaction.
        let question_version: Option<i64> = if let Some(w)=w {
            w["event_id"].as_i64()
        } else if assignment.is_none() {
            db.query_row("SELECT id FROM runtime_events WHERE task_id=? AND type='USER_QUESTION_REQUESTED'
                AND (json_extract(payload_json,'$.question_id')=? OR EXISTS(
                    SELECT 1 FROM json_each(json_extract(payload_json,'$.questions'))
                    WHERE json_extract(value,'$.question_id')=?)) ORDER BY id DESC LIMIT 1",
                (&b.task,&id,&id),|r|r.get(0)).optional()?
        } else { None };
        let version=question_version.unwrap_or(b.execution_version).to_string();
        let mut value=json!({"decisionId":format!("wd_{id}"),"questionId":id,"kind":if batch.is_null(){"question"}else{"question_batch"},
            "batchId":batch,"executionId":Value::Null,"acpSessionId":b.session,"scope":assignment.unwrap_or("task".into()),"prompt":text,
            "options":options,"version":version,"ownerEpoch":b.owner.epoch.to_string(),"requirementRevision":requirement.to_string(),
            "status":status,"allowedOperations":if status=="pending"&&state!=TaskState::Paused&&!interrupted{vec!["decisions.answer"]}else{vec![]},
            "createdAt":created,"answeredAt":answered,"answerText":answer});
        if !batch.is_null() {value["questionIds"]=json!(w.unwrap_or(&saved_batch)["questions"].as_array().unwrap().iter().map(|q|q["question_id"].clone()).collect::<Vec<_>>());}
        Ok(value)
    }).collect()
}
pub fn decisions(store: &Store, project: &str, actor: &str, request: &Value) -> CoreResult<Value> {
    store.write(|db|{
        let target=request["target"].as_str().ok_or(CoreError::Refused("invalid_target"))?;
        let execution=if request["operation"]=="decisions.get" {
            db.query_row("SELECT e.execution_id FROM pending_questions q JOIN workspace_executions e ON e.runtime_task_id=q.task_id WHERE q.id=?",
                [target.strip_prefix("wd_").ok_or(CoreError::Refused("not_found"))?],|r|r.get::<_,String>(0)).optional()?.ok_or(CoreError::Refused("not_found"))?
        }else{target.to_owned()};
        let b=read_binding(db,&store.key().root().to_string_lossy(),project,actor,&execution)?;
        let native=binding(db,&store.key().root().to_string_lossy(),project,actor,&execution).is_ok();
        let mut items=decisions_tx(db,&b)?;
        // A task the Rust host does not own is shown, never answered here.
        if !native { for item in &mut items { item["allowedOperations"]=json!([]); } }
        for item in &mut items {item["executionId"]=json!(execution);}
        if request["operation"]=="decisions.get" {return items.into_iter().find(|i|i["decisionId"]==target).ok_or(CoreError::Refused("not_found"));}
        let limit=request["limit"].as_u64().unwrap_or(100) as usize;
        let token=crate::digest(format!("{project}:{execution}:{}:{limit}",b.execution_version).as_bytes());
        let after=if let Some(c)=request["cursor"].as_str() {c.strip_prefix(&format!("{token}.")).and_then(|s|s.parse::<usize>().ok()).ok_or(CoreError::Refused("invalid_cursor"))?}else{0};
        if after>items.len(){return refuse("invalid_cursor");}
        let next=(after+limit).min(items.len());
        let watermark:i64=db.query_row("SELECT coalesce((SELECT high_watermark FROM workspace_outbox_state WHERE project_id=?),0)",[project],|r|r.get(0))?;
        let result=json!({"items":items[after..next],"nextCursor":if next<items.len(){Some(format!("{token}.{next}"))}else{None},"highWatermark":watermark.to_string()});
        workspace_protocol::validate("DecisionPage",&result).map_err(|_|CoreError::Refused("native_decision_page_unavailable"))?;
        Ok(result)
    })
}
pub fn answer(store: &Store, project: &str, actor: &str, command: &Value) -> CoreResult<Value> {
    answer_from_session(store, project, actor, command, None)
}
/// The answering transport supplies its real session separately from client payload.
pub fn answer_from_session(
    store: &Store,
    project: &str,
    actor: &str,
    command: &Value,
    session: Option<&str>,
) -> CoreResult<Value> {
    answer_with_input(store, project, actor, command, session, None)
}
/// An answer whose attached files were captured by the transport. They become the
/// task's next source revision in the answer's own transaction, so the answered turn
/// carries them and a replay of the same answer retains nothing twice. The command
/// must carry `payload.capture`, the capture's fingerprint.
pub fn answer_with_input(
    store: &Store,
    project: &str,
    actor: &str,
    command: &Value,
    session: Option<&str>,
    capture: Option<&crate::input::Capture>,
) -> CoreResult<Value> {
    let capture = capture.filter(|c| !c.blocks.is_empty());
    if command["payload"].get("capture") != capture.map(|c| c.fingerprint()).as_ref() {
        return refuse("invalid_input_capture");
    }
    let outcome = answer_in_transaction(store, project, actor, command, session, capture);
    // A superseded refusal rolls back answer_tx's authority-loss cancellation.
    // Commit it separately so the question cannot stay pending but unanswerable.
    if matches!(&outcome, Err(CoreError::Refused("superseded"))) {
        let _ = store.write(|db| {
            let execution = command["payload"]["executionId"]
                .as_str()
                .ok_or(CoreError::Refused("invalid_execution"))?;
            let b = binding(db, &store.key().root().to_string_lossy(), project, actor, execution)?;
            waits::cancel_unauthorized_tx(db, &b.task)
        });
    }
    outcome
}
fn answer_in_transaction(
    store: &Store,
    project: &str,
    actor: &str,
    command: &Value,
    session: Option<&str>,
    capture: Option<&crate::input::Capture>,
) -> CoreResult<Value> {
    store.write(|db| {
        let p = &command["payload"];
        let execution = p["executionId"]
            .as_str()
            .ok_or(CoreError::Refused("invalid_execution"))?;
        let b = binding(
            db,
            &store.key().root().to_string_lossy(),
            project,
            actor,
            execution,
        )?;
        if let Some(session) = session {
            let bound: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM acp_host_sessions WHERE session_id=? AND task_id=?)",
                (session, &b.task),
                |r| r.get(0),
            )?;
            if !bound {
                return refuse("question_answer_session_mismatch");
            }
        }
        lifecycle::request_tx(
            db,
            &b.task,
            actor,
            key(command)?,
            "native_desktop_answer",
            command,
            || {
                if command["ownerEpoch"] != b.owner.epoch.to_string() {
                    return refuse("stale_epoch");
                }
                if p["acpSessionId"] != b.session {
                    return refuse("invalid_request");
                }
                if domain::task_tx(db, &b.task)?.0 == TaskState::Paused {
                    return refuse("task_paused");
                }
                let decisions = decisions_tx(db, &b)?;
                let members: Vec<_> = decisions
                    .iter()
                    .filter(|d| {
                        if p["batchId"].is_null() {
                            d["batchId"].is_null() && d["decisionId"] == command["target"]
                        } else {
                            d["batchId"] == p["batchId"] && command["target"] == p["batchId"]
                        }
                    })
                    .collect();
                let answers = p["answers"]
                    .as_array()
                    .ok_or(CoreError::Refused("invalid_request"))?;
                if members.is_empty() || members.len() != answers.len() {
                    return refuse("invalid_request");
                }
                let mut converted = Vec::new();
                for d in members {
                    let matching: Vec<_> = answers
                        .iter()
                        .filter(|a| {
                            a["decisionId"] == d["decisionId"] && a["questionId"] == d["questionId"]
                        })
                        .collect();
                    if matching.len() != 1
                        || d["status"] != "pending"
                        || !d["allowedOperations"]
                            .as_array()
                            .is_some_and(|a| a.contains(&json!("decisions.answer")))
                    {
                        return refuse("superseded");
                    }
                    let a = matching[0];
                    if a.get("attachments")
                        .is_some_and(|a| a.as_array().is_none_or(|a| !a.is_empty()))
                    {
                        return refuse("native_attachments_unavailable");
                    }
                    if a["expectedVersion"] != d["version"]
                        || command["expectedVersion"] != d["version"]
                    {
                        return refuse("stale_version");
                    }
                    let text = if a["optionId"].is_null() {
                        a["text"].as_str().unwrap_or("").to_owned()
                    } else {
                        d["options"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .find(|o| o["optionId"] == a["optionId"])
                            .and_then(|o| o["label"].as_str())
                            .ok_or(CoreError::Refused("invalid_request"))?
                            .to_owned()
                    };
                    if text.trim().is_empty()
                        || !a["optionId"].is_null()
                            && a["text"]
                                .as_str()
                                .is_some_and(|s| !s.is_empty() && s != text)
                    {
                        return refuse("invalid_request");
                    }
                    converted.push((d["questionId"].as_str().unwrap().to_owned(), text));
                }
                let answered_text = converted.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>().join("\n");
                for (q, text) in converted {
                    if !waits::answer_tx(
                        db,
                        &b.task,
                        &q,
                        &text,
                        "text",
                        Some(session.unwrap_or(&b.session)),
                    )? {
                        return refuse("superseded");
                    }
                }
                if let Some(capture) = capture {
                    let descriptors = capture.persist(store, db, &b.task, session.unwrap_or(&b.session), &answered_text)?;
                    lifecycle::event_tx(db, &b.task, "USER_ANSWER_ATTACHMENTS", json!({"attachments":descriptors}))?;
                }
                Ok(super::receipt(
                    command,
                    "recorded",
                    command["target"].as_str().unwrap(),
                    b.execution_version,
                ))
            },
        )
    })
}
