//! Durable assignment-local waits. Python: orchestration/worker_waits.py:18-292.
//! A claimed delivery is never replayed. Process recovery orphans it; owner
//! pause preserves decisions, but cannot revive the old channel or session.
use super::{domain, event_tx, request_tx};
use crate::{error::refuse, CoreError, CoreResult, TaskState};
use rusqlite::{OptionalExtension, Transaction};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub(crate) fn field<'a>(v: &'a Value, key: &str) -> CoreResult<&'a str> {
    v[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or(CoreError::Refused("invalid_worker_wait"))
}
pub fn projection_tx(db: &Transaction<'_>, task: &str) -> CoreResult<Vec<Value>> {
    let rows=db.prepare("SELECT id,payload_json FROM runtime_events WHERE task_id=? AND type='WORKER_WAIT_CHANGED' ORDER BY id")?
        .query_map([task],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?)))?.collect::<Result<Vec<_>,_>>()?;
    let mut map = BTreeMap::new();
    for (id, s) in rows {
        let mut v: Value = serde_json::from_str(&s)?;
        v["event_id"] = json!(id);
        map.insert(field(&v, "wait_id")?.to_owned(), v);
    }
    let mut values: Vec<_> = map.into_values().collect();
    values.sort_by_key(|v| v["event_id"].as_i64().unwrap_or(0));
    Ok(values)
}
pub fn current_tx(db: &Transaction<'_>, task: &str, assignment: &str) -> CoreResult<Option<Value>> {
    Ok(projection_tx(db, task)?
        .into_iter()
        .rev()
        .find(|v| v["assignment_ref"] == assignment && active(v)))
}
fn active(v: &Value) -> bool {
    ["open", "ready", "resuming"].contains(&v["status"].as_str().unwrap_or(""))
}
pub fn change_tx(db: &Transaction<'_>, wait: &mut Value) -> CoreResult<()> {
    let task = field(wait, "task_id")?.to_owned();
    if let Some(o) = wait.as_object_mut() {
        o.remove("event_id");
    }
    event_tx(db, &task, "WORKER_WAIT_CHANGED", wait.clone())?;
    Ok(())
}
pub fn preserved_tx(db: &Transaction<'_>, w: &Value) -> CoreResult<bool> {
    if w["continuation_required"] == true
        && w["reason"] == "native_session_lost"
        && (w["kind"].is_null() || w["kind"] == "question")
    {
        // A new native owner may retain an answer for a positively settled
        // legacy worker. This does not revive its revoked channel or session.
        let adopted: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM runtime_events
            WHERE task_id=? AND type='NATIVE_TASK_ADOPTED'
            AND json_extract(payload_json,'$.owner_epoch')=?) AND EXISTS(
            SELECT 1 FROM delegation_provisioning WHERE task_id=? AND idempotency_key=?
            AND attempt_id=? AND status='terminated') AND NOT EXISTS(
            SELECT 1 FROM capacity_reservations WHERE task_id=? AND state!='released')",
            rusqlite::params![
                field(w, "task_id")?,
                w["owner_epoch"].as_i64(),
                field(w, "task_id")?,
                field(w, "assignment_ref")?,
                field(w, "attempt_id")?,
                field(w, "task_id")?
            ],
            |r| r.get(0),
        )?;
        if adopted {
            return Ok(true);
        }
    }
    if w["owner_paused"] != true || w["continuation_required"] != true {
        return Ok(false);
    }
    Ok(
        domain::pause_saved_tx(db, field(w, "task_id")?)?.is_some_and(|saved| {
            saved["waitIds"]
                .as_array()
                .is_some_and(|ids| ids.contains(&w["wait_id"]))
        }),
    )
}
pub fn valid_tx(db: &Transaction<'_>, w: &Value) -> CoreResult<bool> {
    let task = field(w, "task_id")?;
    let (state, revision) = domain::task_tx(db, task)?;
    let preserved = preserved_tx(db, w)?;
    if !(matches!(state, TaskState::Executing | TaskState::Reviewing)
        || preserved && state == TaskState::Paused)
        || w["revision"].as_i64() != Some(revision)
    {
        return Ok(false);
    }
    let epoch: Option<i64> = db
        .query_row(
            "SELECT epoch FROM task_owners WHERE task_id=?",
            [task],
            |r| r.get(0),
        )
        .optional()?;
    if epoch != w["owner_epoch"].as_i64() {
        return Ok(false);
    }
    let stopped: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM task_stopped WHERE task_id=?)",
        [task],
        |r| r.get(0),
    )?;
    if stopped {
        return Ok(false);
    }
    let assignment = field(w, "assignment_ref")?;
    if preserved {
        return Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_nodes WHERE task_id=? AND assignment_ref=? AND revision=? AND status='running')",
            (task,assignment,revision),|r|r.get(0))?);
    }
    Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_nodes n JOIN delegation_provisioning p
        ON n.task_id=p.task_id AND n.assignment_ref=p.idempotency_key JOIN trusted_channels c ON c.channel_id=p.channel_id
        WHERE n.task_id=? AND n.assignment_ref=? AND n.revision=? AND n.status='running'
        AND p.attempt_id=? AND p.engine_session_ref IS ? AND c.channel_id=? AND c.owner_epoch=?
        AND c.role='worker' AND c.revoked_at IS NULL AND (c.expires_at IS NULL OR c.expires_at>?))",
        rusqlite::params![task,assignment,revision,field(w,"attempt_id")?,field(w,"engine_session_ref")?,
            field(w,"channel_id")?,w["owner_epoch"].as_i64(),crate::now()],|r|r.get(0))?)
}
pub fn check_mutation_tx(db: &Transaction<'_>, task: &str, assignment: &str) -> CoreResult<()> {
    if current_tx(db, task, assignment)?
        .is_some_and(|w| ["open", "ready"].contains(&w["status"].as_str().unwrap_or("")))
    {
        return refuse("worker_waiting_for_answer");
    }
    Ok(())
}
#[derive(Debug, Clone)]
pub struct QuestionForm {
    pub question: String,
    pub options: Vec<String>,
}
fn validate_forms(forms: &[QuestionForm]) -> CoreResult<()> {
    if forms.is_empty() || forms.len() > 8 {
        return refuse("invalid_question_batch");
    }
    for f in forms {
        if f.question.trim().is_empty()
            || f.question.chars().count() > 8000
            || f.options.len() < 2
            || f.options.len() > 8
            || f.options
                .iter()
                .any(|s| s.trim().is_empty() || s.chars().count() > 8000)
        {
            return refuse("invalid_question_form");
        }
    }
    Ok(())
}
/// Channel identity is obtained by the adapter, not accepted as model-selected
/// authority. Recheck its exact task/attempt/epoch and assignment in this transaction.
pub fn request_tx_bound(
    db: &Transaction<'_>,
    task: &str,
    channel: &str,
    key: &str,
    forms: &[QuestionForm],
) -> CoreResult<Value> {
    request_tx_bound_topics(db, task, channel, key, forms, &[])
}
/// `request_tx_bound` with each form's optional short `topic` label (1 to 24
/// characters), kept with the question in the wait record and its question
/// event, where the decision projection and batch question forms read it.
pub fn request_tx_bound_topics(
    db: &Transaction<'_>,
    task: &str,
    channel: &str,
    key: &str,
    forms: &[QuestionForm],
    topics: &[Option<String>],
) -> CoreResult<Value> {
    validate_forms(forms)?;
    if topics.len() > forms.len()
        || topics
            .iter()
            .flatten()
            .any(|t| t.trim().is_empty() || t.chars().count() > 24)
    {
        return refuse("question_topic_invalid");
    }
    let topic = |i: usize| topics.get(i).cloned().flatten();
    let row:Option<(String,String,Option<String>,i64)>=db.query_row(
        "SELECT n.assignment_ref,p.attempt_id,p.engine_session_ref,c.owner_epoch FROM trusted_channels c
        JOIN delegation_provisioning p ON p.channel_id=c.channel_id AND p.task_id=c.task_id AND p.attempt_id=c.attempt_id
        JOIN runtime_nodes n ON n.task_id=p.task_id AND n.assignment_ref=p.idempotency_key
        JOIN runtime_tasks t ON t.task_id=n.task_id AND t.revision=n.revision
        JOIN task_owners o ON o.task_id=c.task_id AND o.epoch=c.owner_epoch
        WHERE c.task_id=? AND c.channel_id=? AND c.role='worker' AND c.revoked_at IS NULL
        AND (c.expires_at IS NULL OR c.expires_at>?) AND n.status='running' AND t.state IN ('EXECUTING','REVIEWING')",
        (task,channel,crate::now()),|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    let (assignment, attempt, session, epoch) =
        row.ok_or(CoreError::Refused("stale_worker_authority"))?;
    // A form without a topic keeps the exact payload (and replay digest) it had before.
    let payload = json!({"forms":forms.iter().enumerate().map(|(i,f)|{
        let mut v=json!({"question":f.question,"options":f.options});
        if let Some(t)=topic(i) {v["topic"]=json!(t);}
        v}).collect::<Vec<_>>()});
    request_tx(db, task, channel, key, "ask_question", &payload, || {
        check_mutation_tx(db, task, &assignment)?;
        let (state, revision) = domain::task_tx(db, task)?;
        let mut questions = Vec::new();
        for (i, f) in forms.iter().enumerate() {
            let id = format!("pq_{}", uuid::Uuid::new_v4().simple());
            db.execute("INSERT INTO pending_questions(id,task_id,assignment_ref,dedup_key,text,options_json,status,created_at)
                VALUES (?,?,?,?,?,?,'open',?)",
                (&id,task,&assignment,format!("{channel}:{key}:{i}"),&f.question,json!(f.options).to_string(),crate::now()))?;
            let mut q = json!({"question_id":id,"question":f.question,"options":f.options});
            if let Some(t) = topic(i) {
                q["topic"] = json!(t);
            }
            questions.push(q);
        }
        let mut w = json!({"wait_id":format!("ww_{}",uuid::Uuid::new_v4().simple()),"task_id":task,
            "assignment_ref":assignment,"attempt_id":attempt,"channel_id":channel,"owner_epoch":epoch,
            "engine_session_ref":session,"revision":revision,"created_at":crate::now(),"status":"open",
            "question_id":questions[0]["question_id"],"question":forms[0].question,"options":forms[0].options});
        if forms.len() > 1 {
            w["batch_id"] = json!(format!("qb_{}", uuid::Uuid::new_v4().simple()));
            w["questions"] = json!(questions);
        }
        if session.as_deref().is_none_or(str::is_empty) {
            event_tx(
                db,
                task,
                "USER_QUESTION_REQUESTED",
                json!({"question_id":w["question_id"],"resume_state":state,"questions":questions}),
            )?;
            domain::state_tx(db, task, TaskState::WaitingUser, None)?;
            return Ok(
                json!({"question_id":w["question_id"],"state":"WAITING_USER","complete":true}),
            );
        }
        change_tx(db, &mut w)?;
        Ok(
            json!({"question_id":w["question_id"],"wait_id":w["wait_id"],"status":"waiting_for_answer","complete":true,"questions":questions}),
        )
    })
}
fn question_ids(w: &Value) -> Vec<String> {
    match w["questions"].as_array() {
        Some(q) => q
            .iter()
            .filter_map(|q| q["question_id"].as_str().map(str::to_owned))
            .collect(),
        None => w["question_id"]
            .as_str()
            .map(str::to_owned)
            .into_iter()
            .collect(),
    }
}
pub fn cancel_tx(
    db: &Transaction<'_>,
    task: &str,
    assignment: Option<&str>,
    reason: &str,
    status: &str,
) -> CoreResult<()> {
    for mut w in projection_tx(db, task)? {
        if active(&w) && assignment.is_none_or(|a| w["assignment_ref"] == a) {
            w["status"] = json!(status);
            w["reason"] = json!(reason);
            change_tx(db, &mut w)?;
            for q in question_ids(&w) {
                db.execute(
                    "UPDATE pending_questions SET status='cancelled' WHERE id=? AND status='open'",
                    [q],
                )?;
            }
        }
    }
    Ok(())
}
/// Authority-loss cleanup that survives a refused answer. `answer_tx` cancels a
/// wait whose authority is gone, but a caller that then refuses (desktop
/// `superseded`) rolls that cancellation back with its transaction, leaving a
/// question pending that can never be answered and a wait the driver keeps
/// waiting on. The refusing caller runs this in its own committed transaction.
pub fn cancel_unauthorized_tx(db: &Transaction<'_>, task: &str) -> CoreResult<usize> {
    let mut cancelled = 0;
    for mut w in projection_tx(db, task)? {
        if active(&w) && !valid_tx(db, &w)? {
            w["status"] = json!("cancelled");
            w["reason"] = json!("authority_lost");
            change_tx(db, &mut w)?;
            for q in question_ids(&w) {
                db.execute(
                    "UPDATE pending_questions SET status='cancelled' WHERE id=? AND status='open'",
                    [q],
                )?;
            }
            cancelled += 1;
        }
    }
    Ok(cancelled)
}
/// Exact question answer and wait readiness commit together, including batches.
pub fn answer_tx(
    db: &Transaction<'_>,
    task: &str,
    question: &str,
    text: &str,
    source: &str,
    session: Option<&str>,
) -> CoreResult<bool> {
    if !["text", "elicitation"].contains(&source) || text.trim().is_empty() {
        return refuse("question_answer_invalid");
    }
    let open: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM pending_questions WHERE task_id=? AND id=? AND status='open')",
        (task, question),
        |r| r.get(0),
    )?;
    if !open {
        return Ok(false);
    }
    let state = domain::task_tx(db, task)?.0;
    if state == TaskState::Paused {
        return Ok(false);
    }
    let mut wait = projection_tx(db, task)?
        .into_iter()
        .rev()
        .find(|w| question_ids(w).iter().any(|q| q == question));
    if let Some(w) = wait.as_mut() {
        if w["status"] != "open" || !valid_tx(db, w)? {
            if active(w) {
                w["status"] = json!("cancelled");
                w["reason"] = json!("authority_lost");
                change_tx(db, w)?;
            }
            for q in question_ids(w) {
                db.execute(
                    "UPDATE pending_questions SET status='cancelled' WHERE id=? AND status='open'",
                    [q],
                )?;
            }
            return Ok(false);
        }
        if w["kind"] == "access" {
            return Ok(false);
        }
    } else if state.terminal() {
        return Ok(false);
    }
    db.execute("UPDATE pending_questions SET status='answered',answer_text=?,answer_source=?,answered_at=? WHERE id=?",
        (text,source,crate::now(),question))?;
    event_tx(
        db,
        task,
        "NATIVE_QUESTION_ANSWERED",
        json!({"question_id":question,"session_id":session,"answer_source":source}),
    )?;
    if let Some(w) = wait.as_mut() {
        for q in question_ids(w) {
            let answered: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM pending_questions WHERE id=? AND status='answered')",
                [q],
                |r| r.get(0),
            )?;
            if !answered {
                return Ok(true);
            }
        }
        w["status"] = json!("ready");
        w["answer_source"] = json!(source);
        w["answer_session_id"] = json!(session);
        w["answer_ref"] = json!(format!("{}:answer", field(w, "question_id")?));
        change_tx(db, w)?;
        event_tx(
            db,
            task,
            "WORKER_USER_ANSWER_RECEIVED",
            json!({"wait_id":w["wait_id"],"question_id":w["question_id"],
            "assignment_ref":w["assignment_ref"],"answer_source":source}),
        )?;
    } else {
        let saved:Option<String>=db.query_row("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='USER_QUESTION_REQUESTED' ORDER BY id DESC LIMIT 1",
            [task],|r|r.get(0)).optional()?;
        let saved: Value =
            serde_json::from_str(&saved.ok_or(CoreError::Refused("runtime_question_missing"))?)?;
        let members = if saved["questions"].is_array() {
            question_ids(&saved)
        } else {
            vec![field(&saved, "question_id")?.to_owned()]
        };
        if !members.iter().any(|q| q == question) {
            return refuse("runtime_question_mismatch");
        }
        for q in &members {
            let answered: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM pending_questions WHERE id=? AND status='answered')",
                [q],
                |r| r.get(0),
            )?;
            if !answered {
                return Ok(true);
            }
        }
        event_tx(
            db,
            task,
            "USER_ANSWER_RECEIVED",
            json!({"question_id":saved["question_id"],"answer_source":source}),
        )?;
        super::skill_proposals::record_decision(db, task, &saved, text)?;
        let normalized = text
            .trim()
            .to_lowercase()
            .trim_end_matches(['.', '!'])
            .to_owned();
        // Force-finish keywords belong only to the runtime's own stop question.
        // A coordinator or worker question may legitimately offer "Stop" as an
        // option; its text is an ordinary answer and never cancels the task.
        let runtime_stop = saved["runtime_stop"] == true;
        let force = runtime_stop
            && [
                "/cortex-finish",
                "закрыть как есть",
                "завершить задачу",
                "заверши задачу",
                "закрыть задачу",
                "закрой задачу",
                "finish the task",
                "close the task",
                "close as is",
                "finish now",
                "стоп",
                "остановить",
                "остановить задачу",
                "останови задачу",
                "stop",
                "stop the task",
            ]
            .contains(&normalized.as_str());
        let stop_option = runtime_stop
            && [
                "2",
                "finish",
                "завершить",
                "finish: close the task as is, without certifying unfinished work",
                "завершить: закрыть задачу как есть, без подтверждения незавершённой работы",
            ]
            .contains(&normalized.as_str());
        if members.len() == 1 && (force || stop_option) {
            db.execute(
                "INSERT OR IGNORE INTO task_stopped(task_id,stopped_at) VALUES (?,?)",
                (task, crate::now()),
            )?;
            domain::cancel_task_tx(db, task, "user_force_finished")?;
            event_tx(
                db,
                task,
                "USER_FORCE_FINISHED",
                json!({"question_id":question}),
            )?;
        } else if state == TaskState::WaitingUser {
            let target: TaskState = field(&saved, "resume_state")?
                .parse()
                .map_err(CoreError::Refused)?;
            if !super::allowed(state, target) && target != TaskState::Recovering {
                return refuse("invalid_resume_state");
            }
            domain::state_tx(db, task, target, None)?;
        }
    }
    Ok(true)
}
pub fn claim_tx(
    db: &Transaction<'_>,
    task: &str,
    assignment: &str,
    session: &str,
) -> CoreResult<Option<Value>> {
    let Some(mut w) = current_tx(db, task, assignment)? else {
        return Ok(None);
    };
    if w["status"] != "ready"
        || domain::task_tx(db, task)?.0 == TaskState::Paused
        || w["continuation_required"] == true
    {
        return Ok(None);
    }
    if !valid_tx(db, &w)? || w["engine_session_ref"] != session {
        w["status"] = json!("orphaned");
        w["reason"] = json!("continuation_binding_lost");
        change_tx(db, &mut w)?;
        return Ok(None);
    }
    let answer = if w["kind"] == "access" {
        // Access decisions must be freshly revalidated by the policy provider.
        crate::scheduler::policy::revalidate_access_tx(db, &mut w)?;
        w["access_decision"].clone()
    } else {
        let mut answers = Vec::new();
        for q in question_ids(&w) {
            let a:Option<(String,String)>=db.query_row("SELECT answer_text,answer_source FROM pending_questions WHERE id=? AND status='answered'",
                [&q],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
            let (text, source) = a.ok_or(CoreError::Refused("worker_wait_answer_missing"))?;
            if !["text", "elicitation"].contains(&source.as_str()) {
                return refuse("worker_wait_answer_missing");
            }
            answers.push(json!({"question_id":q,"answer":text,"answer_ref":format!("{q}:answer")}));
        }
        if answers.len() == 1 {
            answers[0]["answer"].clone()
        } else {
            json!(answers)
        }
    };
    w["status"] = json!("resuming");
    w["delivery_id"] = json!(format!("wc_{}", uuid::Uuid::new_v4().simple()));
    change_tx(db, &mut w)?;
    event_tx(db, task, "WORKER_CONTINUATION_STARTED", w.clone())?;
    Ok(Some(json!({"wait":w,"answer":answer})))
}
pub fn finish_tx(
    db: &Transaction<'_>,
    task: &str,
    wait_id: &str,
    delivery_id: &str,
    failure: Option<&str>,
) -> CoreResult<bool> {
    for mut w in projection_tx(db, task)? {
        if w["wait_id"] == wait_id && w["status"] == "resuming" && w["delivery_id"] == delivery_id {
            w["status"] = json!(if failure.is_some() {
                "orphaned"
            } else {
                "resumed"
            });
            w["failure_kind"] = json!(failure);
            change_tx(db, &mut w)?;
            return Ok(true);
        }
    }
    Ok(false)
}
pub fn recover_deliveries_tx(db: &Transaction<'_>, task: &str) -> CoreResult<()> {
    for mut w in projection_tx(db, task)? {
        if w["status"] == "resuming" {
            w["status"] = json!("orphaned");
            w["reason"] = json!("answer_continuation_unconfirmed");
            change_tx(db, &mut w)?;
        }
    }
    Ok(())
}
