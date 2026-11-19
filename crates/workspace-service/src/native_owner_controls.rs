//! Authenticated owner effects and bounded, native control observations.
use crate::{
    actor, native_effects,
    native_runtime::Runtime,
    native_sessions::{NativeHost, SessionDriver},
};
use cortex_core::{
    lifecycle::{self, domain},
    ownership,
    workspace::control,
    CoreError, CoreResult,
};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
use tokio::sync::watch;

pub(super) struct RunningCommand {
    pub(super) task: String,
    pub(super) cancel: watch::Sender<bool>,
    pub(super) done: watch::Receiver<bool>,
}
fn refused<T>(code: &'static str) -> CoreResult<T> {
    Err(CoreError::Refused(code))
}
async fn recv<T>(r: tokio::sync::oneshot::Receiver<CoreResult<T>>) -> CoreResult<T> {
    r.await
        .map_err(|_| CoreError::Refused("native_host_closed"))?
}
fn text<'a>(r: &'a Value, key: &str) -> CoreResult<&'a str> {
    r[key]
        .as_str()
        .filter(|s| !s.trim().is_empty() && s.chars().count() <= 32_000)
        .ok_or(CoreError::Refused("invalid_request"))
}
/// The stop dialogs offer the reason as optional: absent is fine, present must be text.
fn optional_text(r: &Value, key: &str) -> CoreResult<()> {
    if r.get(key).is_some_and(|v| !v.is_null()) {
        text(r, key)?;
    }
    Ok(())
}
fn validate(r: &Value) -> CoreResult<()> {
    let extras: &[&str] = match r["action"].as_str() {
        Some("worker_message") => {
            text(r, "text")?;
            text(r, "workerId")?;
            if r.get("attachments")
                .is_some_and(|a| a.as_array().is_none_or(|a| !a.is_empty()))
            {
                return refused("native_attachments_unavailable");
            }
            &["workerId", "text", "attachments"]
        }
        Some("worker_stop") => {
            optional_text(r, "reason")?;
            text(r, "workerId")?;
            &["workerId", "reason"]
        }
        Some("command_stop") => {
            optional_text(r, "reason")?;
            text(r, "toolCallRef")?;
            &["toolCallRef", "reason"]
        }
        Some("rerun_check") => {
            if r["checkIndex"].as_u64().is_none() {
                return refused("invalid_request");
            }
            text(r, "digest")?;
            text(r, "resultVersion")?;
            &["checkIndex", "checkName", "digest", "resultVersion"]
        }
        Some("task_settings") => {
            let s = &r["settings"];
            if s.as_object().is_none_or(|m| {
                m.len() != 4
                    || m.keys()
                        .any(|k| !["model", "effort", "mode", "language"].contains(&k.as_str()))
            }) || s["model"].as_str().is_none_or(|s| s.len() > 512)
                || s["effort"].as_str().is_none_or(|s| s.len() > 128)
                || !matches!(s["language"].as_str(), Some("auto" | "en" | "ru"))
            {
                return refused("invalid_settings");
            }
            if !s["mode"]
                .as_str()
                .is_some_and(|m| cortex_core::access::MODES.contains(&m))
            {
                return refused("native_access_mode_unavailable");
            }
            &["settings"]
        }
        _ => return refused("invalid_action"),
    };
    if r.as_object().is_none_or(|m| {
        m.keys().any(|k| {
            !["projectId", "executionId", "requestId", "action"].contains(&k.as_str())
                && !extras.contains(&k.as_str())
        })
    }) {
        return refused("invalid_request");
    }
    Ok(())
}
/// Owner guidance is always framed as guidance, never as authority to expand scope.
pub(super) fn guidance_prompt(message: &Value) -> String {
    format!(
        "Owner guidance (not authority to expand scope): {}",
        message["text"].as_str().unwrap_or_default()
    )
}
impl Runtime {
    pub(super) async fn task_settings(&self, task: &str) -> CoreResult<Value> {
        let task = task.to_owned();
        recv(self.host.executor.read(move |db| {
            let raw:Option<String>=db.query_row("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='NATIVE_TASK_SETTINGS' ORDER BY id DESC LIMIT 1",
                [&task],|r|r.get(0)).optional()?;
            Ok(raw.map(|s|serde_json::from_str::<Value>(&s)).transpose()?.unwrap_or(Value::Null)["settings"].clone())
        })?).await
    }
    pub(super) async fn guided_prompt(
        &self,
        driver: &SessionDriver,
        initial: &str,
        guidance: &mut tokio::sync::mpsc::Receiver<Value>,
    ) -> CoreResult<()> {
        let mut prompt = initial.to_owned();
        let mut delivery = None;
        loop {
            let next = {
                let current = prompt.clone();
                let pending = driver.prompt(&current);
                tokio::pin!(pending);
                tokio::select! {
                    result=&mut pending => {
                        if let Some(message) = delivery.take() { driver.guidance_delivery(message, result.is_ok(), false).await?; }
                        let turn=result?;
                        if turn.compacted && driver.needs_compaction_continuation().await? {
                            prompt="Continue from the rebuilt recovery capsule after observed compaction. Do not replay effects.".into();
                            continue;
                        }
                        return Ok(());
                    },
                    Some(text)=guidance.recv()=> {
                        // Capsule/attachment preparation may still be running.
                        // Cancelling a ready session poisons it before any prompt
                        // can reply. Keep polling preparation and wire writes while
                        // waiting to cancel an actually active prompt.
                        // The guidance being replaced reached the engine when its prompt
                        // was active or finished; only then is it recorded as delivered.
                        let reached=std::sync::atomic::AtomicBool::new(false);
                        let settled=tokio::time::timeout(Duration::from_secs(3), async {
                            let cancel=async {
                                while !driver.prompt_active() { tokio::time::sleep(Duration::from_millis(1)).await; }
                                reached.store(true,std::sync::atomic::Ordering::SeqCst);
                                driver.cancel().await
                            };
                            tokio::pin!(cancel);
                            tokio::select! {
                                result=&mut pending => { reached.store(true,std::sync::atomic::Ordering::SeqCst); result?; },
                                result=&mut cancel => { result?; (&mut pending).await?; },
                            }
                            Ok::<_,CoreError>(())
                        }).await;
                        if let Some(message)=delivery.take() {
                            driver.guidance_delivery(message, reached.load(std::sync::atomic::Ordering::SeqCst), true).await?;
                        }
                        settled.map_err(|_|CoreError::Refused("native_guidance_delivery_unknown"))??;
                        text
                    }
                }
            };
            prompt = guidance_prompt(&next);
            delivery = Some(next);
        }
    }
    pub(super) async fn owner_control(
        self: &Arc<Self>,
        project: String,
        r: Value,
    ) -> CoreResult<Value> {
        // Resolve binding/replay before capability validation, as for other native controls.
        let p = project.clone();
        let q = r.clone();
        if let Some(receipt) = recv(
            self.host
                .executor
                .publication(move |s| control::replay(s, &p, &actor(), &q))?,
        )
        .await?
        {
            return Ok(receipt);
        }
        validate(&r)?;
        let p = project.clone();
        let q = r.clone();
        let (b,worker)=recv(self.host.executor.publication(move |s|s.read(|db|{
            let b=control::binding(db,&s.key().root().to_string_lossy(),&p,&actor(),text(&q,"executionId")?)?;
            let worker=if matches!(q["action"].as_str(),Some("worker_message"|"worker_stop")) {
                let rows=db.prepare("SELECT p.idempotency_key FROM delegation_provisioning p JOIN runtime_nodes n ON n.task_id=p.task_id AND n.assignment_ref=p.idempotency_key
                    JOIN task_owners o ON o.task_id=p.task_id AND o.epoch=p.owner_epoch
                    WHERE p.task_id=? AND (p.idempotency_key=? OR p.attempt_id=?) AND p.status='ready' AND n.status='running' AND n.result_json IS NULL")?
                    .query_map((&b.task,text(&q,"workerId")?,text(&q,"workerId")?),|r|r.get::<_,String>(0))?
                    .collect::<Result<Vec<_>,_>>()?;
                if rows.len()!=1 { return refused("native_worker_not_live"); }
                Some(rows[0].clone())
            } else {None};
            Ok((b,worker))
        }))?).await?;
        let sender = if r["action"] == "worker_message" {
            Some(
                self.guidance
                    .lock()
                    .await
                    .get(worker.as_ref().unwrap())
                    .cloned()
                    .ok_or(CoreError::Refused("native_worker_not_live"))?,
            )
        } else {
            None
        };
        let permit = if let Some(sender) = sender.as_ref() {
            Some(
                sender
                    .try_reserve()
                    .map_err(|_| CoreError::Refused("native_guidance_busy"))?,
            )
        } else {
            None
        };
        let worker_stop = if r["action"] == "worker_stop" {
            Some(
                self.worker_stops
                    .lock()
                    .await
                    .get(worker.as_ref().unwrap())
                    .cloned()
                    .ok_or(CoreError::Refused("native_worker_not_live"))?,
            )
        } else {
            None
        };
        if r["action"] == "task_settings" {
            self.routes.task_route(&self.host.root, &r["settings"])?;
        }
        let command = if r["action"] == "command_stop" {
            let task = b.task.clone();
            let reference = text(&r, "toolCallRef")?.to_owned();
            let recovered = recv(self.host.executor.read(move |db| {
                Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='NATIVE_COMMAND_RECOVERED'
                    AND json_extract(payload_json,'$.ref')=? AND json_extract(payload_json,'$.handle_state')='unknown')",
                    (task,reference), |r|r.get::<_,bool>(0))?)
            })?).await?;
            if recovered {
                return refused("native_command_outcome_unknown");
            }
            let commands = self.host.commands.lock().await;
            let c = commands
                .get(text(&r, "toolCallRef")?)
                .filter(|c| c.task == b.task && !*c.done.borrow())
                .ok_or(CoreError::Refused("native_command_not_live"))?;
            Some((c.cancel.clone(), c.done.clone()))
        } else {
            None
        };
        let rerun = if r["action"] == "rerun_check" {
            // The same result identity the owner read presented (and Accept checks).
            let identity = self
                .review(&project, &b.task, &b.draft, text(&r, "executionId")?)
                .await?;
            let t = b.task.clone();
            let q = r.clone();
            Some(recv(self.host.executor.read(move |db|{
                let live:bool=db.query_row("SELECT state IN ('PLANNING','PLAN_READY','EXECUTING','REVIEWING','VERIFYING','RECONCILING') FROM runtime_tasks WHERE task_id=?",[&t],|r|r.get(0))?;
                if live {return refused("native_check_still_running");}
                let mut checks=checks(db,&t)?;
                if let Some((actions, _)) = &identity { identify(&mut checks, actions); }
                if checks["digest"]!=q["digest"] || checks["sourceVersion"]!=q["resultVersion"] {return refused("stale_result");}
                let check=checks["checks"].get(q["checkIndex"].as_u64().unwrap() as usize).ok_or(CoreError::Refused("invalid_check"))?;
                if check["name"]!=q["checkName"] {return refused("stale_result");}
                if check["rerunnable"]!=true {return refused("invalid_check");}
                serde_json::from_value::<Vec<String>>(check["command"].clone()).map_err(|_|CoreError::Refused("invalid_check"))
            })?).await?)
        } else {
            None
        };
        if let Some(argv) = &rerun {
            let fresh = native_effects::observe(self.host.root.clone()).await?;
            if fresh.project_digest.as_deref() != r["digest"].as_str() {
                return refused("stale_result");
            }
            if argv.is_empty() {
                return refused("invalid_check");
            }
        }
        let q = r.clone();
        let t = b.task.clone();
        let o = b.owner.clone();
        let stop_assignment = worker.clone();
        recv(self.host.executor.write(move |db|{
            ownership::check_tx(db,&t,&o)?;
            if matches!(q["action"].as_str(),Some("worker_stop"|"worker_message")) {
                let live:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM delegation_provisioning p JOIN runtime_nodes n ON n.task_id=p.task_id AND n.assignment_ref=p.idempotency_key
                    WHERE p.task_id=? AND (p.idempotency_key=? OR p.attempt_id=?) AND p.status='ready' AND n.status='running' AND n.result_json IS NULL)",
                    (&t,q["workerId"].as_str(),q["workerId"].as_str()),|r|r.get(0))?;
                if !live {return refused("native_worker_not_live");}
            }
            lifecycle::request_tx(db,&t,&o.owner,text(&q,"requestId")?,"native_desktop_control",&q,|| {
                if q["action"]=="worker_stop" { domain::stop_assignment_tx(db,&t,stop_assignment.as_deref().unwrap(),q["reason"].as_str().unwrap_or("Stopped by the owner."))?; }
                if q["action"]=="worker_message" {
                    lifecycle::event_tx(db,&t,"NATIVE_GUIDANCE_QUEUED",json!({"worker":q["workerId"],"text":q["text"],"request":q["requestId"]}))?;
                }
                Ok(json!({"admitted":true}))
            })?;
            Ok(())
        })?).await?;
        let settled_request = r.clone();
        let outcome: CoreResult<Value> = async {
        let mut effect = json!({});
        match r["action"].as_str().unwrap() {
            "worker_message" => {
                permit.unwrap().send(r.clone());
                effect = json!({"delivery":"queued"});
            }
            "worker_stop" => {
                let (cancel, mut done) = worker_stop.unwrap();
                cancel.send_replace(true);
                tokio::time::timeout(Duration::from_secs(5), async {
                    while !*done.borrow_and_update() {
                        done.changed()
                            .await
                            .map_err(|_| CoreError::Refused("native_worker_stop_unknown"))?;
                    }
                    Ok::<_, CoreError>(())
                })
                .await
                .map_err(|_| CoreError::Refused("native_worker_stop_unknown"))??;
            }
            "command_stop" => {
                let (cancel, mut done) = command.unwrap();
                cancel.send_replace(true);
                tokio::time::timeout(Duration::from_secs(5), async {
                    while !*done.borrow_and_update() {
                        done.changed()
                            .await
                            .map_err(|_| CoreError::Refused("native_command_stop_unknown"))?;
                    }
                    Ok::<_, CoreError>(())
                })
                .await
                .map_err(|_| CoreError::Refused("native_command_stop_unknown"))??;
            }
            "rerun_check" => {
                let argv = rerun.unwrap();
                effect = self.host.owner_verify(&b.task, &b.owner, &argv).await?;
                let fresh = native_effects::observe(self.host.root.clone()).await?;
                effect["revision_digest"] = r["digest"].clone();
                if fresh.project_digest.as_deref() != r["digest"].as_str() {
                    effect["observation"]["complete"] = json!(false);
                    effect["observation"]["failure"] =
                        json!("verification_review_revision_changed");
                }
            }
            "task_settings" => {}
            _ => unreachable!(),
        }
        let q = r.clone();
        let t = b.task.clone();
        let o = b.owner.clone();
        recv(self.host.executor.write(move |db|{
            ownership::check_tx(db,&t,&o)?;
            lifecycle::request_tx(db,&t,"native-desktop-finish",text(&q,"requestId")?,"native_desktop_finished",&q,||{
                match q["action"].as_str().unwrap() {
                    "worker_stop" => {},
                    "task_settings" => {
                        let mode=q["settings"]["mode"].as_str().ok_or(CoreError::Refused("invalid_settings"))?;
                        if cortex_core::access::current(db,&t)?!=mode {
                            cortex_core::access::switch_tx(db,&t,mode,&b.session,"native_owner_settings")?;
                        }
                        lifecycle::event_tx(db,&t,"NATIVE_TASK_SETTINGS",json!({"settings":q["settings"],"activation":"next_use"}))?;
                    },
                    "rerun_check" => { lifecycle::event_tx(db,&t,"NATIVE_CHECK_RERUN",json!({"index":q["checkIndex"],"receipt":effect,"digest":q["digest"]}))?; },
                    _=>{},
                }
                lifecycle::event_tx(db,&t,"NATIVE_OWNER_CONTROL",json!({"action":q["action"],"worker":worker,"request":q["requestId"],"effect":effect}))?;
                Ok(json!({"outcome":"recorded","native":true,"effect":effect}))
            })
        })?).await
        }
        .await
        .and_then(|v| {
            if v["outcome"] == "unknown" {
                refused("native_control_outcome_unknown")
            } else {
                Ok(v)
            }
        });
        // An admitted control that could not reach its finish receipt is settled as an
        // explicit unknown outcome, never left to block every later restart.
        if let Err(error) = &outcome {
            let (p, q, reason) = (project.clone(), settled_request, error.code().to_owned());
            if let Ok(settled) = self
                .host
                .executor
                .publication(move |s| control::settle_unknown(s, &p, &actor(), &q, &reason))
            {
                let _ = settled.await;
            }
        }
        outcome
    }
}
/// Worker-page tool steps take sequences above every runtime event id, so a step
/// and a command or edit never share one; rows are ordered by time first.
const STEP_SEQUENCE: i64 = 1 << 40;
/// The timeline's selected tool-step text (`cortex_core::projections::tool_step`).
const STEP_PREFIX: &str = "Workspace step: ";
/// Rows of one worker-page read. Older rows are reached with the `activityCursor` it returns.
const PAGE: usize = 200;
/// Host check receipts of one result that one read presents. Beyond it the result is marked
/// `checksPartial` and cannot be accepted from this view.
const CHECK_RECEIPTS: usize = 1000;
/// A worker-page cursor: rows strictly older than a (time, sequence) key. `na1.{at}.{sequence}` bounds the activity rows;
/// `na2.{at}.{sequence}.{at}.{sequence}` bounds the worker's queued messages separately. (0, 0) bounds nothing: exhausted.
fn parse_cursor(cursor: &str) -> Option<((i64, i64), (i64, i64))> {
    let parts: Vec<&str> = cursor.split('.').collect();
    let number = |n: usize| parts.get(n)?.parse::<i64>().ok();
    match (parts.first().copied(), parts.len()) {
        (Some("na1"), 3) => Some(((number(1)?, number(2)?), (0, 0))),
        (Some("na2"), 5) => Some(((number(1)?, number(2)?), (number(3)?, number(4)?))),
        _ => None,
    }
}
fn cursor_text(activity: Option<(i64, i64)>, messages: Option<(i64, i64)>) -> Option<String> {
    match (activity, messages) {
        (None, None) => None,
        (a, None) => a.map(|a| format!("na1.{}.{}", a.0, a.1)),
        (a, Some(m)) => {
            let a = a.unwrap_or((0, 0));
            Some(format!("na2.{}.{}.{}.{}", a.0, a.1, m.0, m.1))
        }
    }
}
/// One task's queued guidance messages (optionally one worker's), the newest `PAGE` older than `before`, oldest first, with
/// the oldest returned key and whether older ones remain.
fn guidance(
    db: &rusqlite::Connection,
    task: &str,
    draft: &str,
    worker: Option<&str>,
    before: Option<(i64, i64)>,
) -> CoreResult<(Vec<Value>, Option<(i64, i64)>, bool)> {
    let (at, id) = before.unwrap_or((i64::MAX, i64::MAX));
    let mut rows = db.prepare(&format!("SELECT id,payload_json,coalesce({ms},0) FROM runtime_events WHERE task_id=? AND type='NATIVE_GUIDANCE_QUEUED'
        AND (?4 IS NULL OR json_extract(payload_json,'$.worker')=?4) AND (coalesce({ms},0),id)<(?2,?3) ORDER BY id DESC LIMIT {}", PAGE + 1, ms = ms("created_at")))?
        .query_map(rusqlite::params![task, at, id, worker], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    let more = rows.len() > PAGE;
    rows.truncate(PAGE);
    rows.reverse();
    let oldest = rows.first().map(|(id, _, at)| (*at, *id));
    let mut out = Vec::new();
    for (_, raw, _) in rows {
        let v: Value = serde_json::from_str(&raw)?;
        let delivered: Option<bool> = db.query_row("SELECT json_extract(payload_json,'$.delivered') FROM runtime_events WHERE task_id=? AND type='NATIVE_GUIDANCE_SETTLED' AND json_extract(payload_json,'$.request')=? ORDER BY id DESC LIMIT 1",
            (task, v["request"].as_str()), |r| r.get(0)).optional()?;
        out.push(json!({"id":v["request"],"taskId":draft,"workerId":v["worker"],"text":v["text"],"mode":"send_now",
            "delivery":match delivered {Some(true)=>"delivered",Some(false)=>"unverified",None=>"queued"},
            "at":null,"deliveredAt":null,"failure":null,"attachments":[]}));
    }
    Ok((out, oldest, more))
}
/// Narration messages of one worker page, newest first before the cut.
const NARRATION_MESSAGES: usize = 60;
/// Longest narration text of one message, in characters.
const NARRATION_CHARS: usize = 4000;
/// Retained chunks one read looks at, so a long session cannot make the page unbounded.
const NARRATION_CHUNKS: i64 = 4000;
/// A worker's own narration: the retained agent messages whose identity is `{attempt}:…`, one note per message, in the
/// worker page's order (time, then sequence above every runtime event id, like its tool steps). Written by the worker
/// into the same session history the Conversation reads, so nothing here is text the Conversation did not already
/// hold. The newest messages fill the bound; a cut says so. Older builds' rows carry the same prefix and read the same.
fn narration(
    db: &rusqlite::Connection,
    session: &str,
    attempt: &str,
    budget: &mut usize,
    before: Option<(i64, i64)>,
) -> CoreResult<(Vec<Value>, bool)> {
    let prefix = format!("{attempt}:");
    // A page after a cursor reads from the cursor's time on (with slack for a message's chunks), then keeps exact keys below.
    let until = before.map_or(i64::MAX, |(at, _)| at.saturating_add(600_000));
    let rows = db.prepare(&format!("SELECT seq,message_id,text,{ms} FROM host_transcript
            WHERE session_id=? AND kind='agent_message' AND substr(message_id,1,?)=? AND coalesce({ms},0)<=? ORDER BY seq DESC LIMIT {}", NARRATION_CHUNKS + 1, ms = ms("created_at")))?
        .query_map(rusqlite::params![session, prefix.chars().count() as i64, prefix, until], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, Option<i64>>(3)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut partial = rows.len() as i64 > NARRATION_CHUNKS;
    // Chunks of one message are adjacent and share an identity; walk newest to oldest and join them back in order.
    let mut messages: Vec<(i64, Option<i64>, String, Vec<String>)> = Vec::new();
    for (seq, id, chunk, at) in rows.into_iter().take(NARRATION_CHUNKS as usize) {
        let Some(chunk) = chunk else { continue };
        match messages.last_mut() {
            Some(m) if m.2 == id => {
                m.0 = seq;
                m.1 = at.or(m.1);
                m.3.push(chunk);
            }
            _ => messages.push((seq, at, id, vec![chunk])),
        }
    }
    let mut out = Vec::new();
    for (first, at, _, mut chunks) in messages {
        chunks.reverse();
        let body = chunks.concat();
        let body = body.trim();
        if body.is_empty() {
            continue;
        }
        if before.is_some_and(|(a, s)| (at.unwrap_or(0), STEP_SEQUENCE + first) >= (a, s)) {
            continue;
        }
        let body: String = body.chars().take(NARRATION_CHARS).collect();
        if out.len() >= NARRATION_MESSAGES || body.len() > *budget {
            partial = true;
            break;
        }
        *budget -= body.len();
        out.push(json!({"sequence":STEP_SEQUENCE+first,"kind":"note","text":body,"at":at}));
    }
    out.reverse();
    Ok((out, partial))
}
/// Milliseconds since the epoch of a stored RFC 3339 time column, or NULL.
fn ms(column: &str) -> String {
    format!("CAST(round((julianday({column})-2440587.5)*86400000) AS INTEGER)")
}
/// Runtime facts of the owner's messages to the coordinator and write denials, read
/// with the task's product facts (and so after each of its runtime events): every
/// recorded message carries `recorded` (waiting for the coordinator's next step) or
/// `delivered` with `deliveredAt`; a saved queue row with the same id takes that state.
/// `writeDenials` lists the owner's `deny-write` paths in force with when each was set.
fn owner_steering(
    db: &rusqlite::Connection,
    task: &str,
    draft: &str,
    wanted: bool,
    product: &mut Value,
) -> CoreResult<()> {
    let millis = |at: &Value| -> CoreResult<Value> {
        Ok(match at.as_str() {
            Some(at) => json!(db.query_row(&format!("SELECT {}", ms("?1")), [at], |r| r
                .get::<_, Option<i64>>(0))?),
            None => Value::Null,
        })
    };
    let denials = cortex_core::lifecycle::steering::denial_records(db, task)?;
    if !denials.is_empty() || product["tasks"][draft].is_object() {
        let mut listed = Vec::new();
        for d in denials {
            listed.push(json!({"path":d.path,"setAt":millis(&json!(d.set_at))?}));
        }
        product["tasks"][draft]["writeDenials"] = json!(listed);
    }
    // Message states only for the tasks a read asks about (the task on view).
    if !wanted {
        return Ok(());
    }
    let states = cortex_core::lifecycle::steering::message_states(db, task)?;
    let Some(messages) = product["messages"].as_array_mut() else {
        return Ok(());
    };
    for s in states {
        let delivered = millis(&s["deliveredAt"])?;
        let row = json!({"id":s["request"],"taskId":draft,"workerId":null,"text":s["text"],"mode":"send_now",
            "delivery":s["delivery"],"at":millis(&s["at"])?,"deliveredAt":delivered,"failure":null});
        match messages
            .iter_mut()
            .find(|m| m["id"] == s["request"] && m["workerId"].is_null())
        {
            Some(saved) => {
                saved["delivery"] = row["delivery"].clone();
                saved["deliveredAt"] = delivered;
            }
            None => messages.push(row),
        }
    }
    Ok(())
}
/// The host check receipts of a task's current result. A reopened task keeps its
/// plan revision until its next execution baseline, so receipts recorded before
/// the latest reopen belong to the superseded result and are never presented again.
pub(super) fn checks(db: &rusqlite::Connection, task: &str) -> CoreResult<Value> {
    let mut items = Vec::new();
    let mut digest = Value::Null;
    let mut version = String::from("0");
    let mut query=db.prepare("SELECT id,payload_json FROM runtime_events WHERE task_id=? AND type IN ('NATIVE_COMMAND_FINISHED','NATIVE_CHECK_RERUN')
        AND json_extract(payload_json,'$.receipt.revision')=(SELECT revision FROM runtime_tasks WHERE task_id=runtime_events.task_id)
        AND id>coalesce((SELECT max(r.id) FROM runtime_events r WHERE r.task_id=runtime_events.task_id AND r.type='TASK_REOPENED'),0)
        ORDER BY id DESC LIMIT ?")?;
    // The newest receipts decide each check. A task with more than the bound is never refused: the
    // result is marked `checksPartial` (older receipts were not read) and the presenter keeps it unacceptable.
    let mut receipts = query
        .query_map(rusqlite::params![task, CHECK_RECEIPTS as i64 + 1], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let partial = receipts.len() > CHECK_RECEIPTS;
    receipts.truncate(CHECK_RECEIPTS);
    receipts.reverse();
    for (id, raw) in receipts {
        let v: Value = serde_json::from_str(&raw)?;
        let receipt = &v["receipt"];
        let argv = receipt["argv"].clone();
        let observation = &receipt["observation"];
        let reason = observation["failure"].as_str().unwrap_or("");
        // This code is returned before the verifier starts the command. A
        // generic incomplete receipt (timeout, cancellation, etc.) is not a skip.
        let skipped = observation["exit_code"].is_null()
            && reason == "native_verification_sandbox_unavailable";
        let outcome = if skipped {
            "not_run"
        } else if observation["complete"] == true && observation["exit_code"] == 0 {
            "passed"
        } else {
            "failed"
        };
        let mut item = json!({"name":argv.as_array().into_iter().flatten().filter_map(Value::as_str).collect::<Vec<_>>().join(" "),
            "command":argv,"sourceDigest":receipt["revision_digest"],"kind":"automated","source":"runtime","rerunnable":!skipped,"outcome":outcome,
            "detail":reason,"toolCallRef":receipt["tool_call_ref"]});
        if skipped {
            item["technicalReason"] = json!(reason);
        }
        if let Some(index) = items.iter().position(|i: &Value| i["command"] == argv) {
            items[index] = item;
        } else {
            items.push(item);
        }
        digest = receipt["revision_digest"].clone();
        version = id.to_string();
    }
    for item in &mut items {
        if item["sourceDigest"] != digest {
            item["outcome"] = json!("unverified");
            item["detail"] = json!("verification_review_revision_changed");
            item.as_object_mut().unwrap().remove("technicalReason");
        }
        item.as_object_mut().unwrap().remove("sourceDigest");
    }
    let mut snapshot = json!({"checks":items,"digest":digest,"sourceVersion":version});
    if partial {
        snapshot["checksPartial"] = json!(true);
    }
    Ok(snapshot)
}
/// Whether the current result has more host check receipts than one read presents. Accept is never decided from such a set.
pub(crate) fn receipts_partial(db: &rusqlite::Connection, task: &str) -> CoreResult<bool> {
    let count: i64 = db.query_row(
        "SELECT count(*) FROM runtime_events WHERE task_id=? AND type IN ('NATIVE_COMMAND_FINISHED','NATIVE_CHECK_RERUN')
        AND json_extract(payload_json,'$.receipt.revision')=(SELECT revision FROM runtime_tasks WHERE task_id=runtime_events.task_id)
        AND id>coalesce((SELECT max(r.id) FROM runtime_events r WHERE r.task_id=runtime_events.task_id AND r.type='TASK_REOPENED'),0)",
        [task],
        |r| r.get(0),
    )?;
    Ok(count > CHECK_RECEIPTS as i64)
}
pub(crate) const RECEIPTS_PARTIAL_REASON: &str = "This result has more check receipts than can be shown here. Request changes for a fresh, reviewable result.";
/// Brings a facts reply under the frame without refusing it: drops the oldest half of the largest worker
/// activity (naming the cursor that reads it back), then the oldest queued messages. True if anything moved.
fn fit_frame(value: &mut Value) -> CoreResult<bool> {
    let limit = workspace_protocol::FRAME_BYTES - 2048;
    let key = |a: &Value| {
        (
            a["at"].as_i64().unwrap_or(0),
            a["sequence"].as_i64().unwrap_or(0),
        )
    };
    let mut trimmed = false;
    while serde_json::to_vec(&*value)?.len() > limit {
        trimmed = true;
        let largest = value["workers"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(n, w)| {
                (
                    n,
                    w["activity"]
                        .as_array()
                        .map_or(0, |a| a.iter().map(|i| i.to_string().len()).sum::<usize>()),
                )
            })
            .filter(|(n, size)| {
                *size > 0
                    && value["workers"][*n]["activity"]
                        .as_array()
                        .is_some_and(|a| !a.is_empty())
            })
            .max_by_key(|(_, size)| *size);
        if let Some((n, _)) = largest {
            let worker = &mut value["workers"][n];
            let items = worker["activity"].as_array_mut().expect("measured above");
            let drop = items.len().div_ceil(2);
            let dropped: Vec<Value> = items.drain(..drop).collect();
            let boundary = match items.first() {
                Some(first) => key(first),
                None => {
                    let last = key(dropped.last().expect("one dropped"));
                    (last.0, last.1 + 1)
                }
            };
            worker["activityPartial"] = json!(true);
            // The queued-message bound a cursor already carries stays: only the activity bound moves.
            let messages = worker["activityCursor"]
                .as_str()
                .and_then(parse_cursor)
                .map(|c| c.1)
                .filter(|m| *m != (0, 0));
            worker["activityCursor"] = json!(cursor_text(Some(boundary), messages));
            continue;
        }
        match value["product"]["messages"].as_array_mut() {
            Some(m) if !m.is_empty() => {
                let n = m.len().div_ceil(2);
                m.drain(..n);
            }
            _ => return refused("native_facts_limit"),
        }
    }
    Ok(trimmed)
}
/// The owner-facing number of a task's result: 1 for the first result, plus one for each
/// time the task was reopened (Request changes or steering) up to event `upto`. Event ids
/// and revisions move with every runtime event, so they are never shown as a version; they
/// stay in `sourceVersion`, which commands bind to.
pub(super) fn result_ordinal(
    db: &rusqlite::Connection,
    task: &str,
    upto: Option<i64>,
) -> CoreResult<u64> {
    let reopened: i64 = db.query_row(
        "SELECT count(*) FROM runtime_events WHERE task_id=? AND type='TASK_REOPENED' AND id<=?",
        (task, upto.unwrap_or(i64::MAX)),
        |r| r.get(0),
    )?;
    Ok(reopened as u64 + 1)
}
/// A node's worker status for the owner. A node the runtime still records as running is
/// working while its channel is starting or ready; only a recorded end state is failed.
/// Lines one worker-page edit shows. A Cortex write records its diff as the
/// whole previous text removed and the whole new text added (`native_project`);
/// this keeps only the changed lines, with up to three unchanged lines around
/// them, as one hunk whose counts match the recorded `add`/`del`. Anything that
/// is not that shape, or no change at all, shows no lines. Bounded, redacted.
pub(super) fn edit_hunk(recorded: &str) -> Vec<String> {
    const CONTEXT: usize = 3;
    const LINES: usize = 200;
    let mut rows = recorded.lines();
    let header = (rows.next(), rows.next(), rows.next());
    let (Some(a), Some(b), Some(h)) = header else {
        return Vec::new();
    };
    if !a.starts_with("--- ") || !b.starts_with("+++ ") || !h.starts_with("@@ ") {
        return Vec::new();
    }
    let (mut old, mut new) = (Vec::new(), Vec::new());
    for row in rows {
        match (row.strip_prefix('-'), row.strip_prefix('+')) {
            (Some(l), _) if new.is_empty() => old.push(l),
            (_, Some(l)) => new.push(l),
            _ => return Vec::new(),
        }
    }
    let head = old.iter().zip(&new).take_while(|(x, y)| x == y).count();
    let tail = old[head..]
        .iter()
        .rev()
        .zip(new[head..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (old_end, new_end) = (old.len() - tail, new.len() - tail);
    if head == old_end && head == new_end {
        return Vec::new();
    }
    let (before, after) = (head.min(CONTEXT), tail.min(CONTEXT));
    let start = head - before;
    let (old_count, new_count) = (old_end - start + after, new_end - start + after);
    let at = |count: usize| if count == 0 { start } else { start + 1 };
    let mut out = vec![format!(
        "@@ -{},{old_count} +{},{new_count} @@",
        at(old_count),
        at(new_count)
    )];
    out.extend(old[start..head].iter().map(|l| format!(" {l}")));
    out.extend(old[head..old_end].iter().map(|l| format!("-{l}")));
    out.extend(new[head..new_end].iter().map(|l| format!("+{l}")));
    out.extend(
        old[old_end..old_end + after]
            .iter()
            .map(|l| format!(" {l}")),
    );
    if out.len() > LINES + 1 {
        let more = out.len() - LINES - 1;
        out.truncate(LINES + 1);
        out.push(format!(
            "… {more} more line{} in Changes",
            if more == 1 { "" } else { "s" }
        ));
    }
    out.into_iter()
        .map(|l| crate::machine::redact_display_text(&l.chars().take(2000).collect::<String>()))
        .collect()
}
/// The engine, model and effort a worker's engine session was configured with, from the
/// record its session start wrote once the engine confirmed them, newest session of this
/// worker first. Null while no session of it was configured (not started yet, an older
/// store without the record): never another session's route, such as the coordinator's.
pub(super) fn worker_route(
    db: &rusqlite::Connection,
    task: &str,
    assignment: &str,
) -> CoreResult<Value> {
    let raw: Option<String> = db.query_row("SELECT e.payload_json FROM runtime_events e JOIN trusted_channels c
        ON c.task_id=e.task_id AND c.channel_id=json_extract(e.payload_json,'$.channel_id')
        WHERE e.task_id=? AND e.type='NATIVE_SESSION_CONFIGURED' AND c.role='worker' AND c.assignment_id=? ORDER BY e.id DESC LIMIT 1",
        (task, assignment), |r| r.get(0)).optional()?;
    let Some(p) = raw
        .map(|raw| serde_json::from_str::<Value>(&raw))
        .transpose()?
    else {
        return Ok(Value::Null);
    };
    let (Some(engine), Some(model)) = (
        p["engine"].as_str(),
        p["model"].as_str().filter(|m| !m.is_empty()),
    ) else {
        return Ok(Value::Null);
    };
    let effort = ["thought_level", "effort", "reasoning_effort"]
        .iter()
        .find_map(|k| p["parameters"][k].as_str().filter(|s| !s.is_empty()))
        .unwrap_or("");
    let mut route = json!({"engine":engine,"model":model,"effort":effort});
    // The engine's own name for the model, recorded with the configured session; an older
    // record has none and the desktop resolves the id through its catalogue.
    if let Some(label) = p["model_label"].as_str().filter(|l| !l.is_empty()) {
        route["modelLabel"] = json!(label);
    }
    // Why a reviewer got its route, from the recorded route decision.
    let rule: Option<String> = db.query_row("SELECT json_extract(payload_json,'$.decision.review_route.rule') FROM runtime_events
        WHERE task_id=? AND type='NATIVE_ROUTE_SELECTED' AND json_extract(payload_json,'$.assignment_ref')=? ORDER BY id DESC LIMIT 1",
        (task, assignment), |r| r.get(0)).optional()?.flatten();
    let why = match rule.as_deref() {
        Some("different_from_all_authors") => Some("A model you prefer for review, different from every model that wrote the work"),
        Some("different_from_dominant_author") => Some("A model you prefer for review, different from the model that wrote most of the change"),
        Some("same_model_fresh_session") => Some("A model you prefer for review that also wrote part of the work, reviewing in a fresh session: no other review model was usable"),
        Some("no_review_preferred_model") => Some("No model you prefer for review was usable, so another model reviews"),
        Some("no_authors") => Some("A model you prefer for review"),
        _ => None,
    };
    if let Some(why) = why {
        route["why"] = json!(why);
    }
    Ok(route)
}
pub(super) fn worker_status(node: &str, process: &str, task_state: &str) -> &'static str {
    match (node, process) {
        ("running", "ready" | "starting") => "working",
        ("completed", _) => "completed",
        ("cancelled", _) => "cancelled",
        _ if task_state == "WAITING_USER" => "cancelled",
        _ => "failed",
    }
}
/// A finished result's identity is its Python-parity review identity (Python
/// `result_projection`: version = the task's latest event), which Accept, owner
/// reruns and the presented result share. Without one, the check identity stays.
/// How long a deferring presentation read (`defer`) waits for the folder observation
/// before it answers with pending review facts (`pending_reviews`).
pub(crate) const OBSERVATION_WAIT: std::time::Duration = std::time::Duration::from_millis(500);
/// Why Accept is off while the folder observation of a deferring read is still running.
pub(crate) const CHECKING_REASON: &str = "Checking the result against the project\u{2026}";
/// The completed results of a deferring read whose folder observation has not finished:
/// each keeps its own record (summary, version, runtime checks) and carries a pending
/// review with Accept off. No digest, staleness or acceptance is claimed from it.
fn pending_reviews(facts: &mut Value, completed: &[(String, String, Option<String>, u64)]) {
    for (draft, _, synthesis, ordinal) in completed {
        let task = &mut facts["product"]["tasks"][draft];
        if !task["result"].is_object() {
            task["result"] = json!({"checks":[]});
        }
        let result = &mut task["result"];
        result["digest"] = Value::Null;
        result["acceptanceAvailable"] = json!(false);
        result["summary"] = json!(synthesis
            .as_deref()
            .unwrap_or("Native result summary unavailable."));
        result["version"] = json!(ordinal);
        task["reviewActions"] = json!({"accept":false,"pending":true,"requestChanges":true,
            "reason":CHECKING_REASON,"unblockAction":null,"acknowledgeOpenChecks":false});
        task["workspaceDigest"] = Value::Null;
    }
}
fn identify(result: &mut Value, actions: &Value) {
    if let (Some(version), Some(digest)) = (actions["version"].as_str(), actions["digest"].as_str())
    {
        result["sourceVersion"] = json!(version);
        result["digest"] = json!(digest);
    }
}
impl Runtime {
    /// Python-parity review actions of a COMPLETED native result, bound to the
    /// native verifier's digest of the project folder. None if not completed.
    async fn review(
        &self,
        project: &str,
        task: &str,
        draft: &str,
        execution: &str,
    ) -> CoreResult<Option<(Value, bool)>> {
        let t = task.to_owned();
        let completed: bool = recv(self.host.executor.read(move |db| {
            Ok(db.query_row(
                "SELECT EXISTS(SELECT 1 FROM runtime_tasks WHERE task_id=? AND state='COMPLETED')",
                [&t],
                |r| r.get(0),
            )?)
        })?)
        .await?;
        if !completed {
            return Ok(None);
        }
        let record = match &self.maintenance_state {
            Some(state) => crate::machine::native_product(state, project)
                .map(|p| p["tasks"][draft].clone())
                .unwrap_or(Value::Null),
            None => Value::Null,
        };
        // The rerun-check identity check decides on the folder as it is now.
        let native = crate::machine::task_facts::native_observation(&self.host.root).await;
        self.reviews(project, vec![(execution.to_owned(), record)], native)
            .await
            .map(|mut r| r.pop())
    }
    async fn reviews(
        &self,
        project: &str,
        completed: Vec<(String, Value)>,
        native: Option<String>,
    ) -> CoreResult<Vec<(Value, bool)>> {
        let root = self.host.root.clone();
        let project = project.to_owned();
        tokio::task::spawn_blocking(move || {
            crate::machine::task_facts::native_reviews(
                &root,
                &project,
                &completed,
                native.as_deref(),
            )
        })
        .await
        .map_err(|_| CoreError::Refused("native_facts_unavailable"))
    }
    /// Existing presentation DTOs, read from the native owner's one store, with a fresh
    /// folder observation. No helper. The service reads `owner_facts_observed`.
    #[cfg(test)]
    pub(super) async fn owner_facts(
        &self,
        project: &str,
        product: Value,
        params: Value,
    ) -> CoreResult<Value> {
        self.owner_read(project, product, params, false, None).await
    }
    /// `owner_facts` with its folder observation already started (the service passes
    /// `machine::task_facts::recent_observation` of the project root),
    /// so the folder is observed while the saved records and the store are read. An
    /// observation the read turns out not to need is stopped.
    pub(crate) async fn owner_facts_observed(
        &self,
        project: &str,
        product: Value,
        params: Value,
        observing: tokio::task::JoinHandle<CoreResult<String>>,
    ) -> CoreResult<Value> {
        self.owner_read(project, product, params, false, Some(observing))
            .await
    }
    /// The worker rows of `owner_facts` alone (`workers`): the same rows, cursors and
    /// pages, without the product facts, folder observation and result reviews that a
    /// worker read never returns. The frame bound applies to the rows themselves.
    pub(crate) async fn owner_workers(&self, project: &str, params: Value) -> CoreResult<Value> {
        let facts = self
            .owner_read(
                project,
                json!({"tasks":{},"sprints":[],"messages":[]}),
                params,
                true,
                None,
            )
            .await?;
        Ok(facts["workers"].clone())
    }
    async fn owner_read(
        &self,
        project: &str,
        mut product: Value,
        params: Value,
        workers_only: bool,
        observing: Option<tokio::task::JoinHandle<CoreResult<String>>>,
    ) -> CoreResult<Value> {
        // A started observation never outlives this read, whatever it returns, except
        // that a deferring read (`defer`) that stops waiting for it leaves it to finish:
        // it is a presentation observation, and the read that follows takes it over.
        let defer = params["defer"] == true;
        struct Started(Option<tokio::task::JoinHandle<CoreResult<String>>>);
        impl Drop for Started {
            fn drop(&mut self) {
                if let Some(observing) = self.0.take() {
                    observing.abort();
                }
            }
        }
        let mut observing = Started(observing);
        let (mut facts, completed) = recv(self.host.executor.read(move |db| {
            let tasks=db.prepare("SELECT t.task_id,e.execution_id,e.runtime_task_id,r.state,r.synthesis FROM workspace_tasks t
                JOIN workspace_executions e ON e.execution_id=t.execution_id JOIN runtime_tasks r ON r.task_id=e.runtime_task_id ORDER BY t.rowid DESC LIMIT 1001")?
                .query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,Option<String>>(4)?)))?
                .collect::<Result<Vec<_>,_>>()?;
            // A large project is never refused: the newest 1000 tasks are read and the rest is named in `partial`.
            let mut partial=serde_json::Map::new();
            let mut tasks=tasks;
            if tasks.len()>1000 {tasks.truncate(1000);partial.insert("tasks".into(),json!(true));}
            tasks.reverse();
            let cursors:Option<((i64,i64),(i64,i64))>=match params["cursor"].as_str() {
                None=>None,
                Some(c)=>Some(parse_cursor(c).ok_or(CoreError::Refused("invalid_cursor"))?),
            };
            let (cursor,msg_cursor)=(cursors.map(|c|c.0),cursors.map(|c|c.1));
            let mut workers=Vec::new();
            let mut completed=Vec::new();
            // Edit lines share one bound per read, well inside the frame; beyond it an edit shows its counts only.
            let mut diff_budget:usize=1_000_000;
            let mut step_budget:usize=2_000_000;
            let mut narration_budget:usize=400_000;
            for (draft,execution,task,state,synthesis) in tasks {
                let wanted=!params["taskIds"].as_array().is_some_and(|a|!a.contains(&json!(draft)));
                // A worker read returns no product: only its requested tasks' rows are read.
                if workers_only && !wanted {continue;}
                if !workers_only {
                let ordinal=result_ordinal(db,&task,None)?;
                if state=="COMPLETED" { completed.push((draft.clone(),execution.clone(),synthesis.clone(),ordinal)); }
                // Acceptance records the event id it accepted; the owner reads the result number.
                if let Some(event)=product["tasks"][&draft]["acceptance"]["version"].as_str().and_then(|s|s.parse::<i64>().ok())
                    .or_else(||product["tasks"][&draft]["acceptance"]["version"].as_i64()) {
                    product["tasks"][&draft]["acceptance"]["version"]=json!(result_ordinal(db,&task,Some(event))?);
                }
                let settings:Option<String>=db.query_row("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='NATIVE_TASK_SETTINGS' ORDER BY id DESC LIMIT 1",
                    [&task],|r|r.get(0)).optional()?;
                if let Some(raw)=settings {product["tasks"][&draft]["settings"]=serde_json::from_str::<Value>(&raw)?["settings"].clone();}
                owner_steering(db,&task,&draft,wanted,&mut product)?;
                // Only this result's host checks: a reopened task shows none until its next run records them.
                let mut result=checks(db,&task)?;
                if result["checks"].as_array().is_some_and(|a|!a.is_empty()) {
                    result["kind"]=json!("work");result["acceptanceAvailable"]=json!(false);
                    result["summary"]=json!(if state=="COMPLETED" {
                        synthesis.as_deref().unwrap_or("Native result summary unavailable.")
                    } else {"Native host command observations; owner reruns do not certify completion."});
                    result["version"]=json!(ordinal);
                    product["tasks"][&draft]["result"]=result;
                }
                }
                // The newest queued guidance messages ride with the product; older ones are read, per worker,
                // behind the cursor its page returns (`guidance_floor` is where the product's window ends).
                let (queued,oldest,more_queued)=guidance(db,&task,&draft,None,None)?;
                let guidance_floor=if more_queued {oldest} else {None};
                if !workers_only {product["messages"].as_array_mut().ok_or(CoreError::Refused("state_unreadable"))?.extend(queued);}
                if !wanted {continue;}
                let nodes=db.prepare("SELECT n.assignment_ref,n.role,n.objective,n.status,p.status,n.scope_json,n.result_json,n.revision,p.attempt_id FROM runtime_nodes n
                    JOIN delegation_provisioning p ON p.task_id=n.task_id AND p.idempotency_key=n.assignment_ref
                    WHERE n.task_id=? ORDER BY n.rowid DESC LIMIT 1001")?
                    .query_map([&task],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?,r.get::<_,Option<String>>(6)?,r.get::<_,i64>(7)?,r.get::<_,Option<String>>(8)?)))?
                    .collect::<Result<Vec<_>,_>>()?;
                let mut nodes=nodes;
                if nodes.len()>1000 {nodes.truncate(1000);partial.insert("workers".into(),json!(true));}
                nodes.reverse();
                // A rerun event is the runtime's authoritative link between the
                // finished review and its follow-up. Keep it beside the worker
                // projection so Work details can explain both rows without
                // parsing model prose.
                let review_reruns:Vec<Value>=db.prepare("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='REVIEW_RERUN' ORDER BY id")?
                    .query_map([&task],|r|r.get::<_,String>(0))?
                    .collect::<Result<Vec<_>,_>>()?.into_iter()
                    .map(|raw|serde_json::from_str::<Value>(&raw).map_err(|_|rusqlite::Error::InvalidQuery))
                    .collect::<Result<Vec<_>,_>>()?;
                let host_worker=nodes.iter().rev().find(|n|n.3=="completed").map(|n|n.0.clone());
                // A worker of an earlier version (the plan revision moved on, or the task was
                // reopened after it) is history: its checks are not the current version's.
                let (current_revision,reopened):(i64,Option<i64>)=db.query_row("SELECT revision,(SELECT max(json_extract(payload_json,'$.revision')) FROM runtime_events WHERE task_id=runtime_tasks.task_id AND type='TASK_REOPENED') FROM runtime_tasks WHERE task_id=?",
                    [&task],|r|Ok((r.get(0)?,r.get(1)?)))?;
                // The task timeline's own retained tool rows, grouped by call: a worker's
                // page shows its reads, searches, skill reads and other steps from them.
                let session:Option<String>=db.query_row("SELECT acp_session_id FROM workspace_executions WHERE execution_id=?",[&execution],|r|r.get(0)).optional()?;
                let mut calls=std::collections::HashMap::<String,Vec<(String,i64,i64,Option<i64>)>>::new();
                if let Some(session)=&session {
                    for row in db.prepare(&format!("SELECT json_extract(update_json,'$.toolCallId'),min(seq),max(seq),min({}) FROM host_updates
                        WHERE session_id=? AND json_valid(update_json) AND json_type(update_json,'$.toolCallId')='text'
                        GROUP BY 1 ORDER BY 2 DESC",ms("created_at")))?
                        .query_map([session],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?,r.get::<_,Option<i64>>(3)?)))? {
                        let (call,first,last,at)=row?;
                        if let Some((attempt,_))=call.split_once(':') { calls.entry(attempt.to_owned()).or_default().push((call.clone(),first,last,at)); }
                    }
                }
                // Every retained update of the presented workers' calls, read in one pass over the session
                // (oldest first, with its seq) instead of one session scan per call.
                let mut updates=std::collections::HashMap::<String,(Vec<i64>,Vec<String>)>::new();
                if let Some(session)=&session {
                    let needed:std::collections::HashSet<&str>=nodes.iter()
                        .filter(|n|params["workerId"].as_str().is_none_or(|w|w==n.0))
                        .filter_map(|n|n.8.as_deref().and_then(|a|calls.get(a)))
                        .flatten().map(|c|c.0.as_str()).collect();
                    if !needed.is_empty() {
                        let mut q=db.prepare("SELECT seq,json_extract(update_json,'$.toolCallId'),update_json FROM host_updates
                            WHERE session_id=? AND json_valid(update_json) AND json_type(update_json,'$.toolCallId')='text' ORDER BY seq")?;
                        let mut rows=q.query([session])?;
                        while let Some(r)=rows.next()? {
                            let call:String=r.get(1)?;
                            if needed.contains(call.as_str()) { let u=updates.entry(call).or_default(); u.0.push(r.get(0)?); u.1.push(r.get(2)?); }
                        }
                    }
                }
                // The task's command receipts, read once for all its workers (each worker filters its own below).
                let mut command_rows:Option<Vec<(i64,String,Value,Option<String>,Option<i64>,Option<String>)>>=None;
                for (id,role,goal,status,process,scope,node_result,revision,attempt) in nodes {
                    if params["workerId"].as_str().is_some_and(|w|w!=id) {continue;}
                    let superseded=revision<current_revision || reopened.is_some_and(|r|revision<=r);
                    let mut activity=Vec::new();
                    // A command started on a worker's own channel belongs to that worker (and
                    // is stoppable there); any other (host verification) is shown on one
                    // settled worker.
                    let mut owners=std::collections::HashMap::<String,Option<String>>::new();
                    // When each command started: a finished one keeps its place in time.
                    let mut began=std::collections::HashMap::<String,Option<i64>>::new();
                    // Every receipt is read in order (nothing is cut here); the page below takes the window.
                    if command_rows.is_none() {
                        let mut rows=Vec::new();
                        for row in db.prepare(&format!("SELECT e.id,e.type,e.payload_json,c.assignment_id,{},c.channel_id FROM runtime_events e
                            LEFT JOIN trusted_channels c ON e.type='NATIVE_COMMAND_STARTED' AND c.task_id=e.task_id AND c.channel_id=json_extract(e.payload_json,'$.channel_id')
                            WHERE e.task_id=? AND e.type IN ('NATIVE_COMMAND_STARTED','NATIVE_COMMAND_FINISHED') ORDER BY e.id",ms("e.created_at")))?
                            .query_map([&task],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,Option<String>>(3)?,r.get::<_,Option<i64>>(4)?,r.get::<_,Option<String>>(5)?)))? {
                            let (event,kind,raw,assignment,stamp,channel)=row?;
                            rows.push((event,kind,serde_json::from_str::<Value>(&raw)?,assignment,stamp,channel));
                        }
                        command_rows=Some(rows);
                    }
                    for (event,kind,v,assignment,stamp,channel) in command_rows.iter().flatten() {
                        let (event,stamp)=(*event,*stamp);
                        let reference=if kind=="NATIVE_COMMAND_STARTED" {&v["ref"]}else{&v["receipt"]["tool_call_ref"]};
                        let key=reference.as_str().unwrap_or_default().to_owned();
                        // A command on a channel that is no worker's (the coordinator's own) is never shown on a worker page.
                        let assignment=if kind=="NATIVE_COMMAND_STARTED" && assignment.is_none() && channel.is_some() {Some("\u{0}coordinator".to_owned())} else {assignment.clone()};
                        if kind=="NATIVE_COMMAND_STARTED" { owners.insert(key.clone(),assignment); began.insert(key.clone(),stamp); }
                        let stamp=began.get(&key).copied().flatten().or(stamp);
                        let owner=owners.get(&key).cloned().flatten();
                        let own=owner.as_deref()==Some(id.as_str());
                        if !own && (owner.is_some() || host_worker.as_deref()!=Some(&id)) {continue;}
                        if kind=="NATIVE_COMMAND_FINISHED" { activity.retain(|a:&Value|a["toolCallRef"]!=*reference); }
                        let running=kind=="NATIVE_COMMAND_STARTED";
                        let argv=if running {&v["argv"]} else {&v["receipt"]["argv"]};
                        let text:String=if own {
                            argv.as_array().filter(|a|!a.is_empty()&&a.iter().all(Value::is_string))
                                .map(|a|a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ").chars().take(2000).collect())
                                .unwrap_or_else(||"Command".into())
                        } else {"Host verification".into()};
                        activity.push(json!({"sequence":event,"kind":"command","text":text,"at":stamp,
                            "toolCallRef":reference,"command":{"state":if running{"running"}else if v["receipt"]["observation"]["complete"]==true && v["receipt"]["observation"]["exit_code"]==0 {"passed"}else if v["receipt"]["observation"]["failure"]=="owner_stopped" {"stopped"}else{"failed"},
                            "exit":v["receipt"]["observation"]["exit_code"],"took":null,"tail":"","reason":v["receipt"]["observation"]["failure"]}}));
                    }
                    // Project writes the worker's own channel recorded (the host verifier never edits).
                    let mut edits=Vec::<(String,u64,u64)>::new();
                    // The n-th write of one content to one path by one channel is that
                    // channel's n-th completed request recording it (both commit together).
                    let mut seen=std::collections::HashMap::<(String,String,String),i64>::new();
                    let writes=db.prepare(&format!("SELECT e.id,e.payload_json,{} FROM runtime_events e JOIN trusted_channels c ON c.task_id=e.task_id AND c.channel_id=json_extract(e.payload_json,'$.channel_id')
                        WHERE e.task_id=? AND e.type='NATIVE_PROJECT_WRITTEN' AND c.assignment_id=? ORDER BY e.id",ms("e.created_at")))?
                        .query_map((&task,&id),|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<i64>>(2)?)))?
                        .collect::<Result<Vec<_>,_>>()?;
                    // Totals and the n-th-write counting cover every write; only the newest rows before the
                    // cursor are looked up in full and listed (older ones are read on a later page).
                    let before=|event:i64,stamp:Option<i64>|cursor.is_none_or(|(a,s)|(stamp.unwrap_or(0),event)<(a,s));
                    let listed_from=writes.iter().filter(|(e,_,s)|before(*e,*s)).count().saturating_sub(PAGE);
                    let mut rank=0usize;
                    for (event,raw,stamp) in writes {
                        let v:Value=serde_json::from_str(&raw)?;
                        let listed=before(event,stamp) && {rank+=1;rank>listed_from};
                        let Some(path)=v["path"].as_str() else {continue};
                        let (add,del)=(v["add"].as_u64().unwrap_or(0),v["del"].as_u64().unwrap_or(0));
                        let (channel,sha)=(v["channel_id"].as_str().unwrap_or_default().to_owned(),v["sha256"].as_str().unwrap_or_default().to_owned());
                        let nth=seen.entry((channel.clone(),path.to_owned(),sha.clone())).or_insert(0);
                        let recorded:Option<(Option<String>,Option<bool>)>=if listed {db.query_row("SELECT json_extract(result_json,'$.reply.structuredContent.diff'),json_extract(result_json,'$.reply.structuredContent.diff_truncated')
                            FROM runtime_requests WHERE task_id=? AND actor=? AND operation IN ('project_write','project_edit') AND json_valid(result_json)
                              AND json_extract(result_json,'$.complete')=1 AND json_extract(result_json,'$.reply.structuredContent.path')=?
                              AND json_extract(result_json,'$.reply.structuredContent.sha256')=? ORDER BY rowid LIMIT 1 OFFSET ?",
                            rusqlite::params![&task,&channel,path,&sha,*nth],|r|Ok((r.get(0)?,r.get(1)?))).optional()?} else {None};
                        *nth+=1;
                        match edits.iter_mut().find(|e|e.0==path) {
                            Some(e)=>{e.1+=add;e.2+=del;}
                            None=>edits.push((path.to_owned(),add,del)),
                        }
                        if !listed {continue;}
                        let lines=match recorded {
                            Some((Some(diff),Some(false))) if diff_budget>0 => edit_hunk(&diff),
                            _ => Vec::new(),
                        };
                        diff_budget=diff_budget.saturating_sub(lines.iter().map(|l|l.len()+8).sum());
                        activity.push(json!({"sequence":event,"kind":"edit","text":"Edited","code":path,"at":stamp,
                            "diff":{"add":add,"del":del,"lines":lines}}));
                    }
                    // Commands and edits are windowed together below; a cut before the window is more pages, never a refusal.
                    let mut more=listed_from>0;
                    let mut floor:Option<(i64,i64)>=if more {activity.iter().filter(|a|a["kind"]=="edit").map(|a|(a["at"].as_i64().unwrap_or(0),a["sequence"].as_i64().unwrap_or(0))).min()} else {None};
                    // Its other tool steps (reads, skill reads, searches, Git, engine tools) as
                    // the task timeline shows them. Its Cortex commands and writes are above
                    // with their receipts, so only a refused one is repeated as a step. The
                    // newest steps fill what the bound leaves; a cut keeps `activityPartial` and its `activityCursor`.
                    // Refusals of this attempt's result submissions, by code only, oldest first.
                    let refusals:Vec<String>=match attempt.as_deref() {
                        Some(a)=>db.prepare("SELECT json_extract(payload_json,'$.code') FROM runtime_events WHERE task_id=? AND type='WORKER_RESULT_REFUSED'
                            AND json_extract(payload_json,'$.attempt')=? AND json_type(payload_json,'$.code')='text' ORDER BY id LIMIT 101")?
                            .query_map((&task,a),|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?,
                        None=>Vec::new(),
                    };
                    if let (Some(session),Some(own))=(&session,attempt.as_deref().and_then(|a|calls.get(a))) {
                        let mut steps=Vec::new();
                        for (call,first,last,stamp) in own {
                            if cursor.is_some_and(|(a,s)|(stamp.unwrap_or(0),STEP_SEQUENCE+first)>=(a,s)) {continue;}
                            if steps.len()==PAGE {more=true;floor=floor.max(Some(steps.iter().map(|i:&Value|(i["at"].as_i64().unwrap_or(0),i["sequence"].as_i64().unwrap_or(0))).min().unwrap()));break;}
                            let step=match updates.get(call.as_str()).filter(|u|u.0.last()==Some(last)) {
                                Some(read)=>{
                                    cortex_core::projections::tool_step_from(db,session,*last,read.1.last().expect("filtered above"),call,&read.1)?
                                }
                                None=>{
                                    let raw:Option<String>=db.query_row("SELECT update_json FROM host_updates WHERE session_id=? AND seq=?",(session,last),|r|r.get(0)).optional()?;
                                    raw.map(|raw|cortex_core::projections::tool_step(db,session,*last,&raw)).transpose()?.flatten()
                                }
                            };
                            let Some((text,status))=step else {continue};
                            let mut item=json!({"sequence":STEP_SEQUENCE+first,"kind":"tool","text":text,"at":stamp,"toolCallRef":call});
                            if let Some(body)=text.strip_prefix(STEP_PREFIX) {
                                let mut d:Value=serde_json::from_str(body)?;
                                let title=d["title"].as_str().unwrap_or("").to_owned();
                                let operation=title.rsplit("__").next().unwrap_or("").rsplit('.').next().unwrap_or("").trim().to_lowercase();
                                if matches!(operation.as_str(),"command_run"|"project_edit"|"project_write") && d["failure"].as_str().is_none_or(|f|f.trim().is_empty()) {continue;}
                                // Edit text and command output share one bound per read; beyond it a step names its target only.
                                let size=body.len();
                                if size>step_budget { for key in ["oldText","newText","output"] { if let Some(m)=d.as_object_mut() {m.remove(key);} } }
                                let args=d.to_string();
                                step_budget=step_budget.saturating_sub(args.len());
                                item["text"]=json!(if title.is_empty() {"Tool activity"} else {title.as_str()});
                                item["args"]=json!(args);
                            }
                            item["error"]=json!(status.as_deref()==Some("failed"));
                            steps.push(item);
                        }
                        // A refused result submission names its recorded refusal only when the
                        // binding is exact: as many failed submissions as recorded refusals of
                        // this attempt, and one refusal code (or one submission). Otherwise the
                        // step stays as recorded and the worker card carries the refusals.
                        let mut submits:Vec<usize>=steps.iter().enumerate().filter(|(_,s)|s["error"]==true
                            && s["text"].as_str().unwrap_or("").rsplit("__").next().unwrap_or("").rsplit('.').next().unwrap_or("").trim()=="result_submit")
                            .map(|(n,_)|n).collect();
                        submits.sort_by_key(|n|steps[*n]["sequence"].as_i64().unwrap_or(0));
                        if !submits.is_empty() && submits.len()==refusals.len() && (refusals.len()==1 || refusals.iter().all(|c|c==&refusals[0])) {
                            for (n,code) in submits.into_iter().zip(&refusals) { steps[n]["refusal"]=json!(code); }
                        }
                        activity.extend(steps);
                    }
                    if let (Some(session),Some(a))=(&session,attempt.as_deref()) {
                        let (notes,cut)=narration(db,session,a,&mut narration_budget,cursor)?;
                        if cut {
                            more=true;
                            if let Some(oldest)=notes.iter().map(|i|(i["at"].as_i64().unwrap_or(0),i["sequence"].as_i64().unwrap_or(0))).min() {floor=floor.max(Some(oldest));}
                        }
                        activity.extend(notes);
                    }
                    let key=|a:&Value|(a["at"].as_i64().unwrap_or(0),a["sequence"].as_i64().unwrap_or(0));
                    activity.retain(|a|cursor.is_none_or(|c|key(a)<c));
                    activity.sort_by_key(key);
                    // A page is the newest PAGE rows. Nothing is cut for good: the rest is read from `activityCursor`,
                    // and a row below a cut source's oldest supplied row waits for that source's next page.
                    if let Some(f)=floor {activity.retain(|a|key(a)>=f);}
                    if activity.len()>PAGE {activity.drain(..activity.len()-PAGE);more=true;}
                    // This worker's queued messages older than the product's window (or than the cursor's), the same way.
                    let (worker_messages,message_next)=match msg_cursor {
                        Some(before)=>{let (found,oldest,more_found)=guidance(db,&task,&draft,Some(&id),Some(before))?;(found,if more_found {oldest} else {None})}
                        None=>(Vec::new(),guidance_floor),
                    };
                    let page_cursor=cursor_text(if more {activity.first().map(key).or(floor)} else {None},message_next);
                    let worker_result=node_result.as_deref().and_then(|raw|serde_json::from_str::<Value>(raw).ok())
                        .filter(|r|r.is_object() && (status=="completed" || (status=="failed" && r["assignment_outcome"]=="failed")))
                        .map(|r|{
                            let reported=r["checks"].as_array().map(Vec::as_slice).unwrap_or(&[]);
                            // A failure the runtime's fresh assessment still derives as closed by a later passing run of the same check,
                            // retracted or superseded is the check's history, not its state: the worker's current checks leave it out.
                            // A persisted reference alone is never taken on trust.
                            let settled=crate::machine::withdrawn_checks(db,&task,Some(id.as_str()),&r,reported);
                            let checks=reported.iter().zip(settled).filter(|(c,gone)|c.is_object() && !gone).map(|(c,_)|json!({"name":c["name"].as_str().unwrap_or(""),
                                "outcome":match c["outcome"].as_str(){Some(s @ ("passed"|"failed"|"not_run"|"unverified"))=>s,_=>"unverified"},
                                "detail":c["blocker"].as_str().unwrap_or(""),"source":"worker"})).collect::<Vec<_>>();
                            let summary=r["summary"].as_str().filter(|s|!s.is_empty()).map(|s|s.chars().take(8000).collect::<String>())
                                .unwrap_or_else(||if r["report_ref"].is_string() {"The worker's report holds its result.".to_owned()} else {String::new()});
                            let mut result=json!({"summary":summary,"checks":checks,
                                "edits":edits.iter().map(|(path,add,del)|json!({"path":path,"add":add,"del":del})).collect::<Vec<_>>()});
                            if let Some(report)=r["report_ref"].as_str() {result["reportId"]=json!(report);}
                            // The worker itself reported the assignment failed: the card says so.
                            if status=="failed" {result["outcome"]=json!("failed");}
                            result
                        })
                        .filter(|r|!r["summary"].as_str().unwrap_or("").is_empty());
                    let review_info = if role.contains("review") {
                        let recorded = node_result.as_deref().and_then(|raw|serde_json::from_str::<Value>(raw).ok());
                        let outgoing = review_reruns.iter().find(|event| event["review"].as_str() == Some(id.as_str()));
                        let incoming = review_reruns.iter().find(|event| event["rerun"].as_str() == Some(id.as_str()));
                        let mut info = serde_json::Map::new();
                        if let Some(verdict)=recorded.as_ref().and_then(|r|r["verdict"].as_str()).filter(|v| matches!(*v,"APPROVE"|"REQUEST_CHANGES"|"BLOCK"|"INCOMPLETE")) {
                            info.insert("verdict".into(),json!(verdict));
                        }
                        if let Some(items)=recorded.as_ref().and_then(|r|r["not_covered"].as_array()) {
                            let covered=items.iter().filter_map(|item|Some(json!({"item":item["item"].as_str()?.to_owned(),"reason":item["reason"].as_str()?.to_owned()}))).collect::<Vec<_>>();
                            if !covered.is_empty() { info.insert("notCovered".into(),json!(covered)); }
                        }
                        if let Some(event)=outgoing.or(incoming) {
                            info.insert("rerun".into(),json!({
                                "reason":event["reason"].as_str().unwrap_or("The previous review did not cover the whole change."),
                                "previousWorkerId":event["review"].as_str().unwrap_or(""),
                                "followUpWorkerId":event["rerun"].as_str().unwrap_or(""),
                            }));
                        }
                        if info.is_empty() { Value::Null } else { Value::Object(info) }
                    } else { Value::Null };
                    let route=worker_route(db,&task,&id)?;
                    workers.push(json!({"id":id,"taskId":draft,"executionId":execution,"role":role.trim_start_matches("worker-"),
                        "status":worker_status(&status,&process,&state),
                        "lane":if role.contains("review"){"review"}else{"execution"},"goal":goal,"now":null,"waitsFor":null,
                        "scope":serde_json::from_str::<Value>(&scope)?,"route":route,"attempt":null,"elapsedMs":null,"runningSince":null,
                        "context":null,"usage":null,"activity":activity,"result":worker_result,"review":review_info,"messages":[]}));
                    // Present only when they apply, so a current worker keeps its recorded shape.
                    let worker=workers.last_mut().expect("pushed above");
                    if superseded {worker["superseded"]=json!(true);}
                    if let Some(next)=&page_cursor {worker["activityPartial"]=json!(true);worker["activityCursor"]=json!(next);}
                    if !worker_messages.is_empty() {worker["messages"]=json!(worker_messages);}
                    // Why a worker with no result ended, from recorded facts only. An older
                    // store without the event simply has no reason.
                    if worker["result"].is_null() && (status=="cancelled" || status=="failed") {
                        let stopped:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='WORKER_STOPPED_BY_OWNER' AND json_extract(payload_json,'$.assignment_ref')=?)",(&task,&id),|r|r.get(0))?;
                        let lost:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='ASSIGNMENT_RETRIED' AND json_extract(payload_json,'$.previous_assignment')=? AND json_extract(payload_json,'$.failure_class')='worker_lost')",(&task,&id),|r|r.get(0))?;
                        let ended=if stopped {Some("stopped_by_owner")} else if status=="cancelled" {Some("cancelled_by_coordinator")} else if lost {Some("worker_lost")} else {None};
                        if let Some(ended) = ended {worker["endedReason"]=json!(ended);}
                    }
                    if let Some(last)=refusals.last() {worker["resultRefusals"]=json!({"count":refusals.len(),"code":last});}
                }
            }
            let mut value=json!({"product":product,"workers":workers});
            // Over the frame, the oldest rows of the largest worker pages wait for their cursor; only a read
            // that cannot be brought under the frame by that is refused.
            if fit_frame(&mut value)? {partial.insert("size".into(),json!(true));}
            if !partial.is_empty() {value["partial"]=Value::Object(partial);}
            Ok((value,completed))
        })?).await?;
        // Native check receipts and Git presentation facts use different digest
        // domains. Compare native results with a fresh native observation only.
        if !completed.is_empty()
            || facts["product"]["tasks"]
                .as_object()
                .is_some_and(|tasks| tasks.values().any(|task| task["result"].is_object()))
        {
            let current = match observing.0.take() {
                // A deferring read waits for the folder observation only briefly. A result's
                // own record (summary, version, runtime checks) is shown at once; what
                // depends on the folder (Accept, staleness, the review notes) is pending
                // until the read that follows, never computed from a missing observation.
                Some(mut started) if defer => {
                    match tokio::time::timeout(OBSERVATION_WAIT, &mut started).await {
                        Ok(observed) => observed.ok().and_then(|observed| observed.ok()),
                        Err(_) => {
                            drop(started);
                            pending_reviews(&mut facts, &completed);
                            return Ok(facts);
                        }
                    }
                }
                Some(started) => started.await.ok().and_then(|observed| observed.ok()),
                None => native_effects::observe_digest(self.host.root.clone())
                    .await
                    .ok(),
            };
            // A finished result carries the same review actions and acceptance
            // availability as Python's `result_projection`, never a fixed refusal.
            let records = completed
                .iter()
                .map(|(draft, execution, _, _)| {
                    (execution.clone(), facts["product"]["tasks"][draft].clone())
                })
                .collect();
            let reviews = self.reviews(project, records, current.clone()).await?;
            for ((draft, _, synthesis, ordinal), (mut actions, work)) in
                completed.iter().zip(reviews)
            {
                let task = &mut facts["product"]["tasks"][draft];
                if !task["result"].is_object() {
                    task["result"] = json!({"checks":[],"digest":null,"sourceVersion":"0"});
                }
                let result = &mut task["result"];
                identify(result, &actions);
                if let Some(Value::Array(mut skipped)) = actions
                    .as_object_mut()
                    .and_then(|actions| actions.remove("notRunChecks"))
                {
                    // Keep receipt indices stable for owner reruns. These appended rows
                    // describe checks that did not execute and are not rerun targets.
                    if let Some(checks) = result["checks"].as_array_mut() {
                        checks.append(&mut skipped);
                    }
                }
                result["kind"] = json!(if work { "work" } else { "answer" });
                result["acceptanceAvailable"] = json!(actions["accept"] == true);
                // A result from an earlier version is read-only and keeps its identity
                // (see `task_facts::earlier_version`); its summary may not exist here.
                let earlier = actions["earlierVersion"] == true;
                result["summary"] = json!(synthesis.as_deref().unwrap_or(if earlier {
                    "This result is from an earlier version of the app. Its summary is not available here."
                } else {
                    "Native result summary unavailable."
                }));
                result["version"] = json!(ordinal);
                if let Some(notes) = actions["reviewWarnings"]
                    .as_array()
                    .filter(|a| !a.is_empty())
                {
                    task["result"]["warnings"] = json!(notes);
                }
                task["reviewActions"] = actions;
                // Check receipts past the bound were not read: Accept is never computed from a partial set.
                if task["result"]["checksPartial"] == true {
                    task["result"]["acceptanceAvailable"] = json!(false);
                    task["reviewActions"]["accept"] = json!(false);
                    task["reviewActions"]["reason"] = json!(RECEIPTS_PARTIAL_REASON);
                    task["reviewActions"]["unblockAction"] = json!("request_changes");
                }
            }
            if let Some(tasks) = facts["product"]["tasks"].as_object_mut() {
                for task in tasks.values_mut().filter(|task| task["result"].is_object()) {
                    // A result with no recorded workspace digest (an answer, or work not yet
                    // verified) carries an identity placeholder. It is not a claim about the
                    // files, so it is never labelled stale; a verified digest that no longer
                    // matches the folder stays stale.
                    let placeholder = task["reviewActions"]["workspaceChanged"] != true
                        && task["result"]["digest"]
                            .as_str()
                            .is_some_and(|d| d.starts_with("result:"));
                    // A project-folder sprint task whose own recorded writes still hold
                    // what it wrote: later tasks' writes do not make its result stale.
                    let own_writes = task["reviewActions"]["ownWritesOnly"] == true;
                    // The app's own commit of exactly this result moves HEAD, which the
                    // folder's digest covers; the committed result itself is unchanged.
                    let committed = task["work"] == "merged"
                        && task["integration"]["method"] == "commit"
                        && task["integration"]["digest"].is_string()
                        && task["integration"]["digest"] == task["result"]["digest"];
                    if let Some(actions) = task["reviewActions"].as_object_mut() {
                        actions.remove("ownWritesOnly");
                    }
                    task["workspaceDigest"] = if placeholder || own_writes || committed {
                        task["result"]["digest"].clone()
                    } else {
                        json!(current)
                    };
                }
            }
        }
        Ok(facts)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    /// A worker's page holds that worker's own narration, from the history the Conversation no longer shows it in: only
    /// messages whose identity carries its attempt (older builds' rows too), chunks of one message joined, in order with its steps.
    #[test]
    fn a_workers_page_carries_only_its_own_narration() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE host_transcript(session_id TEXT, seq INTEGER, kind TEXT, text TEXT, message_id TEXT, created_at TEXT);").unwrap();
        for (seq, kind, text, id) in [
            (0, "agent_message", "Coordinator says hi.", Some("xa_c:m1")),
            (1, "agent_message", "Running the focused ", Some("xa_w:m2")),
            (2, "agent_message", "test suites.", Some("xa_w:m2")),
            (3, "agent_message", "Other worker.", Some("xa_x:m3")),
            (4, "agent_message", "Then the typecheck.", Some("xa_w:m4")),
            (5, "agent_message", "No identity.", None),
            (6, "user_message", "Owner.", None),
            (7, "agent_message", "Lookalike attempt.", Some("xa_wx:m5")),
        ] {
            db.execute("INSERT INTO host_transcript VALUES ('s',?,?,?,?,'2026-11-15T10:00:00.000000+00:00')", rusqlite::params![seq, kind, text, id]).unwrap();
        }
        let mut budget = 10_000;
        let (notes, partial) = narration(&db, "s", "xa_w", &mut budget, None).unwrap();
        assert!(!partial);
        let texts: Vec<_> = notes.iter().map(|n| n["text"].as_str().unwrap()).collect();
        assert_eq!(
            texts,
            ["Running the focused test suites.", "Then the typecheck."]
        );
        assert!(notes
            .iter()
            .all(|n| n["kind"] == "note" && n["sequence"].as_i64().unwrap() > STEP_SEQUENCE));
        assert!(notes[0]["sequence"].as_i64() < notes[1]["sequence"].as_i64());
        // A small shared budget keeps the newest message and says the page is partial.
        let mut budget = 25;
        let (notes, partial) = narration(&db, "s", "xa_w", &mut budget, None).unwrap();
        assert!(partial);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0]["text"], "Then the typecheck.");
    }
    /// A reply over the frame loses its oldest rows to a cursor, never to a refusal; the cursor reads them back.
    #[test]
    fn an_oversized_reply_trims_the_oldest_activity_and_names_its_cursor() {
        let big = "x".repeat(200_000);
        let items: Vec<Value> = (0..60)
            .map(|i| json!({"sequence":i,"at":1000+i,"kind":"note","text":big}))
            .collect();
        let mut value = json!({"product":{"messages":[]},"workers":[{"id":"w","activity":items}]});
        assert!(serde_json::to_vec(&value).unwrap().len() > workspace_protocol::FRAME_BYTES);
        assert!(fit_frame(&mut value).unwrap());
        let kept = value["workers"][0]["activity"].as_array().unwrap();
        assert!(!kept.is_empty() && kept.len() < 60);
        assert_eq!(kept.last().unwrap()["sequence"], 59, "the newest rows stay");
        assert_eq!(value["workers"][0]["activityPartial"], true);
        let ((at, sequence), _) =
            parse_cursor(value["workers"][0]["activityCursor"].as_str().unwrap()).unwrap();
        assert_eq!(
            (at, sequence),
            (
                kept[0]["at"].as_i64().unwrap(),
                kept[0]["sequence"].as_i64().unwrap()
            )
        );
        assert!(
            serde_json::to_vec(&value).unwrap().len() <= workspace_protocol::FRAME_BYTES - 2048
        );
        assert!(parse_cursor("na1.1.x").is_none() && parse_cursor("nh2.1.2").is_none());
    }
    #[test]
    fn check_snapshot_excludes_old_revisions_and_never_relabels_stale_passes() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE runtime_tasks(task_id TEXT,revision INTEGER);
            INSERT INTO runtime_tasks VALUES ('t',2);
            CREATE TABLE runtime_events(id INTEGER PRIMARY KEY,task_id TEXT,type TEXT,payload_json TEXT);").unwrap();
        for (id, revision, argv, digest) in [
            (1, 1, "old", "old"),
            (2, 2, "first", "first"),
            (3, 2, "second", "second"),
        ] {
            let event = json!({"receipt":{"revision":revision,"argv":[argv],"revision_digest":digest,
                "tool_call_ref":format!("c-{id}"),"observation":{"complete":true,"exit_code":0}}});
            db.execute(
                "INSERT INTO runtime_events VALUES (?,'t','NATIVE_COMMAND_FINISHED',?)",
                (id, event.to_string()),
            )
            .unwrap();
        }
        let snapshot = checks(&db, "t").unwrap();
        assert_eq!(snapshot["checks"].as_array().unwrap().len(), 2);
        assert_eq!(snapshot["checks"][0]["name"], "first");
        assert_eq!(snapshot["checks"][0]["outcome"], "unverified");
        assert_eq!(snapshot["checks"][1]["outcome"], "passed");
        assert_eq!(snapshot["digest"], "second");
        assert_eq!(snapshot["sourceVersion"], "3");
    }
    /// Live run: right after Request changes the task kept its plan revision until
    /// its next baseline, so the accepted result's host check came back as a
    /// phantom "Result v2". Receipts before the latest reopen are not presented.
    #[test]
    fn a_reopened_task_presents_no_checks_of_its_superseded_result() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE runtime_tasks(task_id TEXT,revision INTEGER);
            INSERT INTO runtime_tasks VALUES ('t',1);
            CREATE TABLE runtime_events(id INTEGER PRIMARY KEY,task_id TEXT,type TEXT,payload_json TEXT);").unwrap();
        let receipt = |id: i64, argv: &str| {
            let event = json!({"receipt":{"revision":1,"argv":[argv],"revision_digest":"d",
                "tool_call_ref":format!("c-{id}"),"observation":{"complete":true,"exit_code":0}}});
            db.execute(
                "INSERT INTO runtime_events VALUES (?,'t','NATIVE_COMMAND_FINISHED',?)",
                (id, event.to_string()),
            )
            .unwrap();
        };
        receipt(394, "node");
        assert_eq!(
            checks(&db, "t").unwrap()["checks"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        db.execute("INSERT INTO runtime_events VALUES (408,'t','TASK_REOPENED','{\"previous\":\"COMPLETED\",\"revision\":1}')", []).unwrap();
        // Another task's reopen fences nothing here.
        db.execute(
            "INSERT INTO runtime_events VALUES (409,'other','TASK_REOPENED','{}')",
            [],
        )
        .unwrap();
        let reopened = checks(&db, "t").unwrap();
        assert_eq!(reopened["checks"], json!([]), "{reopened}");
        assert_eq!(reopened["digest"], Value::Null);
        // The next result's own receipt is presented again.
        receipt(478, "node-again");
        let next = checks(&db, "t").unwrap();
        assert_eq!(next["checks"].as_array().unwrap().len(), 1);
        assert_eq!(next["checks"][0]["name"], "node-again");
        assert_eq!(next["sourceVersion"], "478");
    }
    /// The recorded diff of a Cortex write, as `native_project` formats it.
    fn recorded(path: &str, old: &str, new: &str) -> String {
        format!(
            "--- {path}\n+++ {path}\n@@ -1,{} +1,{} @@\n{}{}",
            old.lines().count(),
            new.lines().count(),
            old.lines().map(|l| format!("-{l}\n")).collect::<String>(),
            new.lines().map(|l| format!("+{l}\n")).collect::<String>()
        )
    }
    #[test]
    fn an_edit_shows_its_changed_lines_with_a_little_context() {
        // A new file is all additions.
        assert_eq!(
            edit_hunk(&recorded("ARCHITECTURE.md", "", "# Architecture\n\n- a\n")),
            ["@@ -0,0 +1,3 @@", "+# Architecture", "+", "+- a"]
        );
        // One replaced line in the middle keeps three lines on each side.
        let old = (1..=10).map(|n| format!("line {n}\n")).collect::<String>();
        let new = old.replace("line 5\n", "line five\n-dash\n");
        assert_eq!(
            edit_hunk(&recorded("a.txt", &old, &new)),
            [
                "@@ -2,7 +2,8 @@",
                " line 2",
                " line 3",
                " line 4",
                "-line 5",
                "+line five",
                "+-dash",
                " line 6",
                " line 7",
                " line 8"
            ]
        );
        // Appended at the end, near the start of the file.
        assert_eq!(
            edit_hunk(&recorded("b.txt", "one\n", "one\ntwo\n")),
            ["@@ -1,1 +1,2 @@", " one", "+two"]
        );
        // No change, another shape, or nothing recorded: no lines, never an empty hunk.
        assert!(edit_hunk(&recorded("c.txt", "same\n", "same\n")).is_empty());
        assert!(edit_hunk("--- a\n+++ a\n@@ -1,1 +1,1 @@\n+new\n-old\n").is_empty());
        assert!(edit_hunk("").is_empty());
        // Bounded and redacted.
        let long = edit_hunk(&recorded(
            "d.txt",
            "",
            &(0..500).map(|n| format!("{n}\n")).collect::<String>(),
        ));
        assert_eq!(long.len(), 202);
        assert_eq!(long[201], "… 300 more lines in Changes");
        let secret = edit_hunk(&recorded(
            "e.env",
            "",
            "OPENAI_API_KEY=sk-proj-abcdefghijklmnopqrstuvwxyz0123456789\n",
        ));
        assert!(
            !secret
                .concat()
                .contains("abcdefghijklmnopqrstuvwxyz0123456789"),
            "{secret:?}"
        );
    }
    #[test]
    fn result_number_counts_results_not_events() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE runtime_events(id INTEGER PRIMARY KEY,task_id TEXT,type TEXT,payload_json TEXT);").unwrap();
        // Event ids 55 (first result), 57 (reopened), 117 (second result) and later noise.
        for (id, kind) in [
            (1, "TASK_CREATED"),
            (55, "SYNTHESIS_RUNTIME_COMPOSED"),
            (57, "TASK_REOPENED"),
            (117, "SYNTHESIS_SUBMITTED"),
            (119, "NATIVE_TURN_SETTLED"),
        ] {
            db.execute(
                "INSERT INTO runtime_events VALUES (?,'t',?,'{}')",
                (id, kind),
            )
            .unwrap();
        }
        db.execute(
            "INSERT INTO runtime_events VALUES (200,'other','TASK_REOPENED','{}')",
            [],
        )
        .unwrap();
        assert_eq!(result_ordinal(&db, "t", Some(56)).unwrap(), 1);
        assert_eq!(result_ordinal(&db, "t", Some(52)).unwrap(), 1);
        assert_eq!(result_ordinal(&db, "t", Some(119)).unwrap(), 2);
        assert_eq!(result_ordinal(&db, "t", None).unwrap(), 2);
    }
    #[test]
    fn running_worker_is_never_failed_while_its_channel_starts() {
        assert_eq!(worker_status("running", "starting", "RUNNING"), "working");
        assert_eq!(worker_status("running", "ready", "RUNNING"), "working");
        assert_eq!(
            worker_status("completed", "terminated", "RUNNING"),
            "completed"
        );
        assert_eq!(
            worker_status("cancelled", "terminated", "PAUSED"),
            "cancelled"
        );
        assert_eq!(worker_status("running", "terminated", "RUNNING"), "failed");
        assert_eq!(worker_status("failed", "terminated", "RUNNING"), "failed");
        assert_eq!(
            worker_status("running", "terminated", "WAITING_USER"),
            "cancelled"
        );
    }
}
impl NativeHost {
    pub(super) async fn owner_verify(
        &self,
        task: &str,
        owner: &ownership::OwnerEpoch,
        argv: &[String],
    ) -> CoreResult<Value> {
        let reference = format!("native-command:{}", native_effects::identity()?);
        let (cancel, receiver) = watch::channel(false);
        let (done, finished) = watch::channel(false);
        let t = task.to_owned();
        let o = owner.clone();
        let a = argv.to_vec();
        let reference_copy = reference.clone();
        self.commands.lock().await.insert(
            reference.clone(),
            RunningCommand {
                task: task.into(),
                cancel: cancel.clone(),
                done: finished,
            },
        );
        let prepared = async {
            recv(self.executor.write(move |db| {
                ownership::check_tx(db, &t, &o)?;
                let revision = domain::task_tx(db, &t)?.1;
                lifecycle::event_tx(
                    db,
                    &t,
                    "NATIVE_COMMAND_STARTED",
                    json!({"ref":reference_copy,"argv":a,"revision":revision}),
                )?;
                Ok(revision)
            })?)
            .await
        }
        .await;
        let revision = match prepared {
            Ok(revision) => revision,
            Err(error) => {
                done.send_replace(true);
                self.commands.lock().await.remove(&reference);
                return Err(error);
            }
        };
        let result = native_effects::verify_cancelled(self.root.clone(), argv, receiver).await;
        let mut receipt = match result {
            Ok(r) => r,
            Err(e) => {
                json!({"argv":argv,"observation":{"complete":false,"exit_code":null,"failure":e.code()}})
            }
        };
        receipt["tool_call_ref"] = json!(reference);
        receipt["owner_epoch"] = json!(owner.epoch);
        receipt["revision"] = json!(revision);
        match native_effects::observe(self.root.clone()).await {
            Ok(fresh) => {
                receipt["revision_digest"] = json!(fresh.project_digest.unwrap_or_default())
            }
            Err(error) => {
                receipt["revision_digest"] = json!("");
                receipt["observation"]["complete"] = json!(false);
                receipt["observation"]["failure"] = json!(error.code());
            }
        }
        if *cancel.borrow() {
            receipt["observation"]["complete"] = json!(false);
            receipt["observation"]["failure"] = json!("owner_stopped");
        }
        let t = task.to_owned();
        let raw = receipt.clone();
        let recorded = recv(self.executor.write(move |db| {
            lifecycle::event_tx(db, &t, "NATIVE_COMMAND_FINISHED", json!({"receipt":raw}))?;
            Ok(())
        })?)
        .await;
        done.send_replace(true);
        self.commands.lock().await.remove(&reference);
        recorded?;
        Ok(receipt)
    }
}
