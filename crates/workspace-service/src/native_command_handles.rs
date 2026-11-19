//! Durable supervisor attachment, output settlement and safe owner-stop routing.
use crate::{
    native_command_supervisor::{self as supervisor, Spec},
    native_commands::{self, Running},
};
use cortex_core::{
    access,
    channels::{self, Credential, Identity},
    lifecycle,
    store::executor::StoreExecutor,
    CoreError, CoreResult, Store,
};
use serde_json::{json, Value};
use std::{
    fs::OpenOptions, io::Read, os::unix::fs::OpenOptionsExt, path::PathBuf, process::Stdio,
    sync::Arc, time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
    sync::watch,
};

async fn recv<T>(r: tokio::sync::oneshot::Receiver<CoreResult<T>>) -> CoreResult<T> {
    r.await
        .map_err(|_| CoreError::Refused("native_host_closed"))?
}
fn directory(store: &Store, task: &str) -> PathBuf {
    store.key().directory().join(task).join(".native-output")
}
fn leaf<'a>(h: &'a Value, field: &str) -> CoreResult<&'a str> {
    let name = h[field]
        .as_str()
        .ok_or(CoreError::Refused("invalid_command_handle"))?;
    if name.contains('/') || name.contains("..") || name.is_empty() {
        return Err(CoreError::Refused("invalid_command_handle"));
    }
    Ok(name)
}
fn observe(dir: &std::path::Path, h: &Value) -> CoreResult<Value> {
    let status = supervisor::status(&dir.join(leaf(h, "status")?))?;
    if status["ref"] != h["reference"]
        || status["supervisor"] != h["identity"]
        || !h["identity"].is_object()
    {
        return Err(CoreError::Refused("command_handle_identity_mismatch"));
    }
    if status["state"] != "finished" {
        let pid = h["identity"]["pid"]
            .as_u64()
            .and_then(|p| u32::try_from(p).ok())
            .ok_or(CoreError::Refused("invalid_command_handle"))?;
        if supervisor::identity(pid).as_ref() != Some(&h["identity"]) {
            return Err(CoreError::Refused("command_outcome_unknown"));
        }
    }
    if !matches!(status["state"].as_str(), Some("ready" | "finished")) {
        return Err(CoreError::Refused("invalid_command_handle"));
    }
    Ok(status)
}
struct StopOnDrop {
    dir: PathBuf,
    handle: Value,
}
impl Drop for StopOnDrop {
    fn drop(&mut self) {
        if observe(&self.dir, &self.handle).is_ok_and(|s| s["state"] != "finished") {
            let _ = stop(&self.dir, &self.handle);
        }
    }
}
/// Armed while a launch has not yet released its payload. A call dropped before the
/// release (for example when its session ends) settles the receipt as not started;
/// the write is queued on the store executor and needs no awaiting caller.
struct Unreleased {
    ex: Option<Arc<StoreExecutor>>,
    task: String,
    actor: String,
    key: String,
    reference: String,
}
impl Drop for Unreleased {
    fn drop(&mut self) {
        if let Some(ex) = self.ex.take() {
            let (t, a, k, r) = (
                self.task.clone(),
                self.actor.clone(),
                self.key.clone(),
                self.reference.clone(),
            );
            let _ = ex.write(move |db| {
                native_commands::settle_unrecorded_tx(
                    db,
                    &t,
                    &a,
                    &k,
                    Some(&r),
                    "command_not_started",
                    None,
                    "payload_never_released",
                )
            });
        }
    }
}
fn stop(dir: &std::path::Path, h: &Value) -> CoreResult<()> {
    // No PID signalling from a recovered observer. Only this random private
    // launch's supervisor interprets the stop marker.
    let path = dir.join(leaf(h, "stop")?);
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
    {
        Ok(f) => f.sync_all()?,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }
    Ok(())
}
async fn authorize(
    ex: &StoreExecutor,
    c: &Credential,
    mode: &str,
    denied: &[String],
    readonly: bool,
    reference: &str,
    preparation: bool,
) -> CoreResult<()> {
    let c = c.clone();
    let mode = mode.to_owned();
    let denied = denied.to_vec();
    let r = reference.to_owned();
    recv(ex.write(move |db| {
        let i = channels::authenticate(db, &c.channel_id, c.secret(), true)?;
        if access::current(db, &i.task)? != mode || native_commands::denials(db, &i.task)? != denied
        {
            return Err(CoreError::Refused("access_changed"));
        }
        if !readonly {
            crate::native_project::writable(db, &i, None)?;
        }
        if preparation {
            lifecycle::event_tx(
                db,
                &i.task,
                "NATIVE_COMMAND_PREPARATION",
                json!({"ref":r,"possible":true,"preparation":true}),
            )?;
        }
        Ok(())
    })?)
    .await
}
fn settle(
    store: &Store,
    task: &str,
    actor: &str,
    key: &str,
    h: &Value,
    status: &Value,
    revision: Option<String>,
) -> CoreResult<Value> {
    let dir = directory(store, task);
    let filename = leaf(h, "output")?;
    let mut file = cortex_core::store::locks::open_private(&dir.join(filename), false, false)?;
    let (hash, size) = native_commands::fingerprint(&mut file)?;
    if status["hash"] != hash || status["byte_count"] != size || status["state"] != "finished" {
        return Err(CoreError::Refused("output_integrity_mismatch"));
    }
    use std::io::{Seek, SeekFrom};
    file.seek(SeekFrom::Start(0))?;
    let mut prefix = Vec::new();
    file.take(65536).read_to_end(&mut prefix)?;
    let code = status["exit_code"].as_i64();
    let signal = status["signal"].as_i64();
    let failure = status["failure"].as_str().map(str::to_owned);
    let value = json!({"tool_call_ref":h["reference"],"argv":h["argv"],"cwd":h["cwd"],
        "profile":h["profile"],"access_mode":h["mode"],"revision_digest":revision,
        "exit_code":code,"signal":signal,"failure":failure,"settled":true,
        "complete":failure.is_none()&&code.is_some(),"output":String::from_utf8_lossy(&prefix),
        "output_truncated":size>65536,"byte_count":size,"large_output_id":h["id"]});
    store.write(|db|{
        let raw:String=db.query_row("SELECT result_json FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=?",
            (task,actor,key),|r|r.get(0))?;
        let prior:Value=serde_json::from_str(&raw)?;
        if prior["settled"]==true {return Ok(prior);}
        if prior["handle"]!=*h {return Err(CoreError::Refused("command_handle_identity_mismatch"));}
        db.execute("INSERT INTO large_outputs VALUES (?,?,?,'terminal',?,?,?,?,?,?,?,?)",
            rusqlite::params![h["id"].as_str(),task,actor,hash,filename,size as i64,0,code,signal.map(|s|s.to_string()),failure,cortex_core::now()])?;
        db.execute("UPDATE runtime_requests SET result_json=? WHERE task_id=? AND actor=? AND request_key=?",
            (value.to_string(),task,actor,key))?;
        let mut observation=value.clone();observation.as_object_mut().unwrap().remove("output");
        lifecycle::event_tx(db,task,"NATIVE_COMMAND_FINISHED",json!({"receipt":{
            "tool_call_ref":h["reference"],"argv":h["argv"],"observation":observation}}))?;
        Ok(value)
    })
}
fn unknown(store: &Store, task: &str, h: &Value) -> CoreResult<()> {
    store.write(|db| {
        let seen: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=?
            AND type='NATIVE_COMMAND_RECOVERED' AND json_extract(payload_json,'$.ref')=?
            AND json_extract(payload_json,'$.handle_state')='unknown')",
            (task, h["reference"].as_str()),
            |r| r.get(0),
        )?;
        if !seen {
            lifecycle::event_tx(
                db,
                task,
                "NATIVE_COMMAND_RECOVERED",
                json!({
            "ref":h["reference"],"process_identity":h["identity"],"handle_state":"unknown",
            "outcome":"unknown","replayed":false}),
            )?;
        }
        Ok(())
    })
}
#[allow(clippy::too_many_arguments)]
pub(super) async fn execute(
    ex: Arc<StoreExecutor>,
    c: Credential,
    running: Running,
    i: Identity,
    root: PathBuf,
    cwd: PathBuf,
    argv: Vec<String>,
    mode: String,
    readonly: bool,
    denied: Vec<String>,
    writes: Vec<PathBuf>,
    protected: Vec<PathBuf>,
    dir: PathBuf,
    output: String,
    id: String,
    reference: String,
    args: Value,
    profile: String,
) -> CoreResult<Value> {
    let status_file = format!("{id}.status");
    let stopping = format!("{id}.stop");
    let spec = Spec {
        root: root.clone(),
        cwd: cwd.clone(),
        argv: argv.clone(),
        profile: profile.clone(),
        writes,
        protected,
        output: dir.join(&output),
        status: dir.join(&status_file),
        stop: dir.join(&stopping),
        reference: reference.clone(),
        timeout: args["timeout_seconds"].as_u64().unwrap_or(1800),
    };
    // Until the final `go` reaches the supervisor the payload cannot start. A call
    // that fails (or is dropped) before then settles its receipt as not started.
    let mut unreleased = Unreleased {
        ex: Some(ex.clone()),
        task: i.task.clone(),
        actor: c.channel_id.clone(),
        key: args["request_key"].as_str().unwrap_or_default().to_owned(),
        reference: reference.clone(),
    };
    let launched = async {
    let before = native_commands::check_digest(root.clone(), &argv).await;
    let mut launcher = Command::new(std::env::current_exe()?);
    // Read now, so a changed tool search path applies from the next command on.
    if let Some(path) = crate::tool_path::current() {
        launcher.env("PATH", path);
    }
    let mut child = launcher
        .arg(supervisor::MODE)
        .env_remove("CORTEX_CHANNEL_SECRET")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()?;
    let mut input = child.stdin.take().unwrap();
    input.write_all(&serde_json::to_vec(&spec)?).await?;
    input.write_all(b"\n").await?;
    let mut messages = BufReader::new(child.stdout.take().unwrap()).lines();
    let h;
    loop {
        let line = tokio::time::timeout(Duration::from_secs(15), messages.next_line())
            .await
            .map_err(|_| CoreError::Refused("native_command_sandbox_unavailable"))??
            .ok_or(CoreError::Refused("native_command_sandbox_unavailable"))?;
        let v: Value = serde_json::from_str(&line)?;
        authorize(
            &ex,
            &c,
            &mode,
            &denied,
            readonly,
            &reference,
            v["authorize"] == true,
        )
        .await?;
        if v["ready"].is_object() {
            if supervisor::identity(child.id().unwrap()).as_ref() != Some(&v["ready"]) {
                return Err(CoreError::Refused("command_handle_identity_mismatch"));
            }
            h = json!({"reference":reference,"identity":v["ready"],"status":status_file,"stop":stopping,
                "output":output,"id":id,"argv":argv,"cwd":cwd,"profile":profile,"mode":mode,"denied":denied});
            let handle = h.clone();
            let t = i.task.clone();
            let actor = c.channel_id.clone();
            let key = args["request_key"].as_str().unwrap().to_owned();
            recv(ex.write(move|db|{
                // A receipt the runtime already settled (its call was lost) never launches.
                let bound = db.execute("UPDATE runtime_requests SET result_json=json_set(result_json,'$.handle',json(?))
                    WHERE task_id=? AND actor=? AND request_key=? AND operation='command_run'
                    AND json_extract(result_json,'$.settled') IS NOT 1",
                    (handle.to_string(),&t,&actor,&key))?;
                if bound != 1 {
                    return Err(CoreError::Refused("command_outcome_unknown"));
                }
                lifecycle::event_tx(db,&t,"NATIVE_COMMAND_PROCESS",json!({
                    "ref":handle["reference"],"process_identity":handle["identity"],"supervised":true}))?;
                Ok(())
            })?).await?;
            input.write_all(b"go\n").await?;
            break;
        }
        if v["authorize"] != true {
            return Err(CoreError::Refused("invalid_command_supervisor"));
        }
        input.write_all(b"go\n").await?;
    }
    drop(input);
    Ok::<_, CoreError>((before, child, messages, h))
    }
    .await;
    let (before, mut child, _messages, h) = match launched {
        Ok(launched) => {
            unreleased.ex = None;
            launched
        }
        Err(error) => {
            // The supervisor never received the final `go` (an unwritable pipe means
            // it is gone): the payload provably never started. Settle before replying.
            unreleased.ex = None;
            let (t, a, k, r) = (
                unreleased.task.clone(),
                unreleased.actor.clone(),
                unreleased.key.clone(),
                unreleased.reference.clone(),
            );
            if let Ok(reply) = ex.write(move |db| {
                native_commands::settle_unrecorded_tx(
                    db,
                    &t,
                    &a,
                    &k,
                    Some(&r),
                    "command_not_started",
                    None,
                    "payload_never_released",
                )
            }) {
                let _ = reply.await;
            }
            return Err(error);
        }
    };
    let guard = StopOnDrop {
        dir: dir.clone(),
        handle: h.clone(),
    };
    let unknown_task = i.task.clone();
    let (cancel, mut cancelled) = watch::channel(false);
    let (done, finished) = watch::channel(false);
    running.lock().await.insert(
        reference.clone(),
        crate::native_owner_controls::RunningCommand {
            task: i.task.clone(),
            cancel,
            done: finished,
        },
    );
    let status_executor = ex.clone();
    let status_credential = c.clone();
    let pending_reference = reference.clone();
    let diagnostic_channel = c.clone();
    let diagnostic_key = args["request_key"].as_str().unwrap().to_owned();
    let monitor = tokio::spawn(async move {
        let mut phase = "process_observation";
        let result = async {
            let status = loop {
                let status = observe(&dir, &h)?;
                if status["state"] == "finished" {
                    break status;
                }
                if *cancelled.borrow()
                    || authorize(&ex, &c, &mode, &denied, readonly, &reference, false)
                        .await
                        .is_err()
                {
                    stop(&dir, &h)?;
                }
                tokio::select! { _=tokio::time::sleep(Duration::from_millis(50))=>{},
                _=cancelled.changed()=>{} }
            };
            phase = "settlement";
            let after = native_commands::check_digest(root, &argv).await;
            let revision = if before == after { after } else { None };
            let actor = c.channel_id;
            let task = i.task;
            let key = args["request_key"].as_str().unwrap().to_owned();
            let h = h.clone();
            recv(ex.publication(move |s| settle(s, &task, &actor, &key, &h, &status, revision))?)
                .await
        }
        .await;
        // A child handle is only reaped here; its independent supervisor is not
        // killed on service death. A lost observer cannot invent a command result.
        let _ = tokio::time::timeout(Duration::from_secs(3), child.wait()).await;
        if result.is_ok() {
            done.send_replace(true);
        } else {
            if let Err(error) = &result {
                native_commands::record_failure(
                    &ex,
                    &diagnostic_channel,
                    &diagnostic_key,
                    phase,
                    error,
                )
                .await;
            }
            let h = guard.handle.clone();
            let t = unknown_task.clone();
            let _ = recv(ex.publication(move |s| unknown(s, &t, &h))?).await;
            // Settle the receipt at once when its process is provably gone (a live
            // supervisor keeps it open for the re-settle of orphaned receipts).
            let actor = diagnostic_channel.channel_id.clone();
            let key = diagnostic_key.clone();
            let _ =
                recv(ex.publication(move |s| resettle_row(s, &unknown_task, &actor, &key, false))?)
                    .await;
        }
        drop(guard);
        running.lock().await.remove(&reference);
        result.map_err(|_| CoreError::Refused("command_outcome_unknown"))
    });
    match tokio::time::timeout(Duration::from_secs(5), monitor).await {
        Ok(joined) => joined
            .map_err(|_| CoreError::Refused("command_outcome_unknown"))?
            .map(crate::native_sources::reply),
        Err(_) => {
            recv(status_executor.publication(move |store| {
                let c = status_credential;
                let identity =
                    store.read(|db| channels::authenticate(db, &c.channel_id, c.secret(), true))?;
                let response = status(store, &identity.task, &c.channel_id, &pending_reference)?;
                if identity.assignment.is_some() && response["structuredContent"]["settled"] == false {
                    // A command can finish before the worker ends its turn. Remember
                    // which worker actually received a pending reply in this turn.
                    store.write(|db| lifecycle::event_tx(db, &identity.task, "NATIVE_COMMAND_PENDING_REPLY",
                        json!({"ref":pending_reference,"channel_id":c.channel_id,"attempt":identity.attempt})))?;
                }
                Ok(response)
            })?)
            .await
        }
    }
}
pub(super) fn public_status(store: &Store, task: &str, record: &Value) -> CoreResult<Value> {
    if record["settled"] == true {
        let mut value = record.clone();
        value
            .as_object_mut()
            .ok_or(CoreError::Refused("invalid_command_handle"))?
            .remove("handle");
        return Ok(value);
    }
    let h = &record["handle"];
    if !h.is_object() || h["reference"] != record["tool_call_ref"] {
        return Err(CoreError::Refused("command_outcome_unknown"));
    }
    if store.read(|db| Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='NATIVE_COMMAND_RECOVERED' AND json_extract(payload_json,'$.ref')=? AND json_extract(payload_json,'$.handle_state')='unknown')",(task,h["reference"].as_str()),|r| r.get::<_,bool>(0))?))? {
        return Err(CoreError::Refused("command_outcome_unknown"));
    }
    let status = observe(&directory(store, task), h)
        .map_err(|_| CoreError::Refused("command_outcome_unknown"))?;
    Ok(
        json!({"state":if status["state"]=="finished" {"settling"} else {"running"},
        "tool_call_ref":h["reference"],"complete":false,"settled":false}),
    )
}

pub(super) fn status(
    store: &Store,
    task: &str,
    channel: &str,
    reference: &str,
) -> CoreResult<Value> {
    use rusqlite::OptionalExtension;
    let raw: String = store.read(|db| Ok(db.query_row(
        "SELECT result_json FROM runtime_requests WHERE task_id=? AND actor=? AND operation='command_run' AND json_extract(result_json,'$.tool_call_ref')=?",
        (task,channel,reference), |row| row.get::<_,String>(0)).optional()?))?
        .ok_or(CoreError::Refused("unknown_command_ref"))?;
    public_status(store, task, &serde_json::from_str(&raw)?).map(crate::native_sources::reply)
}

pub(super) async fn recover(runtime: &crate::native_runtime::Runtime) -> CoreResult<()> {
    let ex = runtime.host.executor.clone();
    let running = runtime.host.commands.clone();
    let rows=recv(ex.publication(|s|s.read(|db|Ok(db.prepare(
        "SELECT task_id,actor,request_key,result_json FROM runtime_requests WHERE operation='command_run'
         AND json_extract(result_json,'$.settled') IS NOT 1 AND json_type(result_json,'$.handle')='object'")?
        .query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?)))?
        .collect::<Result<Vec<_>,_>>()?)))?).await?;
    for (task, actor, key, raw) in rows {
        let intent: Value = serde_json::from_str(&raw)?;
        let h = intent["handle"].clone();
        let reference = h["reference"]
            .as_str()
            .ok_or(CoreError::Refused("invalid_command_handle"))?
            .to_owned();
        if running.lock().await.contains_key(&reference) {
            continue;
        }
        let t = task.clone();
        let handle = h.clone();
        let attached = recv(ex.publication(move |s| {
            let status = match observe(&directory(s, &t), &handle) {
                Ok(v) => v,
                Err(_) => {
                    unknown(s, &t, &handle)?;
                    return Ok(false);
                }
            };
            Ok(status["state"] == "finished" || status["state"] == "ready")
        })?)
        .await?;
        if !attached {
            continue;
        }
        let (cancel, mut cancelled) = watch::channel(false);
        let (done, finished) = watch::channel(false);
        running.lock().await.insert(
            reference.clone(),
            crate::native_owner_controls::RunningCommand {
                task: task.clone(),
                cancel,
                done: finished,
            },
        );
        let ex = ex.clone();
        let running = running.clone();
        let shutdown = runtime.stop.subscribe();
        runtime.spawn_recovery(async move {
            let mut confirmed=false;
            loop {
                let t=task.clone();let handle=h.clone();let a=actor.clone();let k=key.clone();let stop_requested=*cancelled.borrow() || *shutdown.borrow();
                let receiver=ex.publication(move|s|{
                    let dir=directory(s,&t);
                    match observe(&dir,&handle) {
                        Ok(status) if status["state"]=="finished" => {
                            let confirmed=settle(s,&t,&a,&k,&handle,&status,None).is_ok();
                            if !confirmed {unknown(s,&t,&handle)?;}
                            Ok(Some(confirmed))
                        },
                        Ok(_) => {
                            let state:String=s.read(|db|Ok(db.query_row("SELECT state FROM runtime_tasks WHERE task_id=?",[&t],|r|r.get(0))?))?;
                            let policy_changed=s.read(|db|Ok(access::current(db,&t)?!=handle["mode"].as_str().unwrap_or("")
                                || json!(native_commands::denials(db,&t)?)!=handle["denied"]))?;
                            if stop_requested || policy_changed || matches!(state.as_str(),"PAUSED"|"CANCELLED"|"FAILED"|"COMPLETED") {
                                stop(&dir,&handle)?;
                            }
                            Ok(None)
                        },
                        Err(_) => {unknown(s,&t,&handle)?;Ok(Some(false))},
                    }
                });
                let Ok(receiver)=receiver else {break};
                let step=recv(receiver).await;
                match step {Ok(Some(value))=>{confirmed=value;break},Ok(None)=>{},Err(_)=>break}
                tokio::select!{_=tokio::time::sleep(Duration::from_millis(50))=>{},_=cancelled.changed()=>{}}
            }
            if confirmed {done.send_replace(true);}
            running.lock().await.remove(&reference);
        }).await;
    }
    Ok(())
}

/// What one check of an orphaned command receipt did.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Resettled {
    /// The receipt is settled now (with its recorded outcome, or as not started/unknown).
    Settled,
    /// Its process may still run, its absence cannot be proven, or its call may still act.
    Left,
}

fn boot_id() -> Option<String> {
    std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .ok()
        .map(|b| b.trim().to_owned())
        .filter(|b| !b.is_empty())
}

/// Whether the actor channel could still authenticate: the rows
/// `channels::authenticate` requires, without its secret. A call whose channel
/// cannot authenticate can neither launch nor settle its own receipt any more.
fn channel_may_act(db: &rusqlite::Connection, channel: &str) -> CoreResult<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM trusted_channels c JOIN task_owners o ON o.task_id=c.task_id AND o.epoch=c.owner_epoch
        JOIN runtime_tasks t ON t.task_id=c.task_id
        JOIN delegation_provisioning p ON p.task_id=c.task_id AND p.channel_id=c.channel_id
            AND p.attempt_id=c.attempt_id AND p.owner_epoch=c.owner_epoch
        LEFT JOIN runtime_nodes n ON n.task_id=c.task_id AND n.assignment_ref=p.idempotency_key
        WHERE c.channel_id=? AND c.revoked_at IS NULL AND (c.expires_at IS NULL OR c.expires_at>?)
        AND NOT EXISTS(SELECT 1 FROM task_stopped WHERE task_id=c.task_id)
        AND (c.role='coordinator' AND p.role_id='orchestrator' OR
            c.role='worker' AND n.status='running' AND n.result_json IS NULL AND n.revision=t.revision
            AND c.assignment_id=n.assignment_ref AND p.role_id=n.role))",
        (channel, cortex_core::now()),
        |r| r.get(0),
    )?)
}

/// The recorded process identity (`NATIVE_COMMAND_PROCESS`) of an unsupervised
/// launch: the direct child, which leads its own process group.
fn process_record(
    db: &rusqlite::Connection,
    task: &str,
    reference: &str,
) -> CoreResult<Option<Value>> {
    use rusqlite::OptionalExtension;
    let raw: Option<String> = db.query_row("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='NATIVE_COMMAND_PROCESS'
        AND json_extract(payload_json,'$.ref')=? ORDER BY id DESC LIMIT 1", (task, reference), |r| r.get(0)).optional()?;
    Ok(raw
        .and_then(|p| serde_json::from_str::<Value>(&p).ok())
        .map(|p| p["process_identity"].clone())
        .filter(Value::is_object))
}

/// An unsupervised process is gone when it ran in an earlier boot, or when neither
/// its exact process (pid with the recorded start time) nor any member of its
/// process group exists: the group is the boundary the runtime itself stops.
fn inline_gone(identity: &Value, boot: Option<&str>) -> Option<&'static str> {
    if boot.is_some()
        && identity["boot_id"]
            .as_str()
            .is_some_and(|b| Some(b) != boot)
    {
        return Some("previous_boot");
    }
    let pid = identity["pid"].as_i64()?;
    let start = identity["start_ticks"].as_u64()?.to_string();
    (!crate::native_recovery::recorded_process_alive(pid, pid, &start))
        .then_some("process_group_gone")
}

/// Settles one unsettled `command_run`/`outside_command` receipt only on proof
/// that nothing can still run it; it never signals, stops or relaunches anything
/// and never records a success it did not observe:
/// - a supervisor whose status is `finished` settles the receipt with its recorded
///   outcome, or (output unverifiable) as `outcome_not_recorded` with its exit code;
/// - a supervised launch whose supervisor is gone is settled `outcome_not_recorded`
///   when it ran in an earlier boot, when the payload's recorded process group has
///   no member, or for the read-only profile, whose bubblewrap runs with
///   `--die-with-parent --unshare-pid` (the supervisor's death ends the whole PID
///   namespace); otherwise it stays unknown;
/// - a `command_run` receipt without a bound supervisor never launched (the payload
///   is released only after the handle commits): `command_not_started`;
/// - an unsupervised process (outside commands, legacy launches) is gone per
///   [`inline_gone`], or its finish event already recorded the exit;
/// - an outside command receipt changes only once its channel can no longer act
///   (or at startup, when no call of an earlier service can still run); one
///   without a command reference never launched.
pub(super) fn resettle_row(
    store: &Store,
    task: &str,
    actor: &str,
    key: &str,
    startup: bool,
) -> CoreResult<Resettled> {
    use rusqlite::OptionalExtension;
    let row: Option<(String, String)> = store.read(|db| Ok(db.query_row("SELECT operation,result_json FROM runtime_requests
        WHERE task_id=? AND actor=? AND request_key=? AND operation IN ('command_run','outside_command')",
        (task, actor, key), |r| Ok((r.get(0)?, r.get(1)?))).optional()?))?;
    let Some((operation, raw)) = row else {
        return Ok(Resettled::Left);
    };
    let v: Value = serde_json::from_str(&raw)?;
    if v["settled"] == true || v["complete"] == true {
        return Ok(Resettled::Left);
    }
    let reference = v["tool_call_ref"].as_str().map(str::to_owned);
    let boot = boot_id();
    let recorded_exit = |reference: &str| -> CoreResult<Option<Option<i64>>> {
        let raw: Option<String> = store.read(|db| Ok(db.query_row("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='NATIVE_COMMAND_FINISHED'
            AND json_extract(payload_json,'$.receipt.tool_call_ref')=? ORDER BY id DESC LIMIT 1", (task, reference), |r| r.get(0)).optional()?))?;
        Ok(raw.map(|p| {
            serde_json::from_str::<Value>(&p)
                .ok()
                .and_then(|p| p["receipt"]["observation"]["exit_code"].as_i64())
        }))
    };
    let decision: Option<(&str, Option<i64>, &str)> = if operation == "command_run" {
        let h = &v["handle"];
        if h.is_object() {
            if h["reference"].as_str() != reference.as_deref() {
                return Ok(Resettled::Left);
            }
            let status = leaf(h, "status")
                .ok()
                .and_then(|f| supervisor::status(&directory(store, task).join(f)).ok())
                .filter(|s| s["ref"] == h["reference"] && s["supervisor"] == h["identity"]);
            if let Some(status) = status.as_ref().filter(|s| s["state"] == "finished") {
                if settle(store, task, actor, key, h, status, None).is_ok() {
                    return Ok(Resettled::Settled);
                }
                Some((
                    "outcome_not_recorded",
                    status["exit_code"].as_i64(),
                    "supervisor_finished",
                ))
            } else {
                let identity = &h["identity"];
                let pid = identity["pid"].as_u64().and_then(|p| u32::try_from(p).ok());
                if boot.is_some()
                    && identity["boot_id"]
                        .as_str()
                        .is_some_and(|b| Some(b) != boot.as_deref())
                {
                    Some(("outcome_not_recorded", None, "previous_boot"))
                } else if pid.is_none()
                    || supervisor::identity(pid.unwrap()).as_ref() == Some(identity)
                {
                    None
                } else if let Some(group) = status.as_ref().and_then(|s| s["group"].as_i64()) {
                    (!crate::native_recovery::recorded_process_alive(group, group, "")).then_some((
                        "outcome_not_recorded",
                        None,
                        "supervisor_and_process_group_gone",
                    ))
                } else if h["profile"] == "read_only" {
                    Some((
                        "outcome_not_recorded",
                        None,
                        "supervisor_gone_pid_namespace",
                    ))
                } else {
                    None
                }
            }
        } else if let Some(reference) = reference.as_deref() {
            match store.read(|db| process_record(db, task, reference))? {
                Some(identity) => match inline_gone(&identity, boot.as_deref()) {
                    Some(e) => Some((
                        "outcome_not_recorded",
                        recorded_exit(reference)?.flatten(),
                        e,
                    )),
                    None => None,
                },
                None => Some(("command_not_started", None, "no_supervisor_bound")),
            }
        } else {
            None
        }
    } else {
        match reference.as_deref() {
            None => Some(("command_not_started", None, "not_launched")),
            Some(reference) => match recorded_exit(reference)? {
                Some(code) => Some(("outcome_not_recorded", code, "finished_receipt")),
                None => match store.read(|db| process_record(db, task, reference))? {
                    Some(identity) => inline_gone(&identity, boot.as_deref())
                        .map(|e| ("outcome_not_recorded", None, e)),
                    None => None,
                },
            },
        }
    };
    let Some((failure, code, evidence)) = decision else {
        return Ok(Resettled::Left);
    };
    let outside = operation == "outside_command";
    let changed = store.write(|db| {
        if outside && !startup && channel_may_act(db, actor)? {
            return Ok(false);
        }
        native_commands::settle_unrecorded_tx(
            db,
            task,
            actor,
            key,
            reference.as_deref(),
            failure,
            code,
            evidence,
        )
    })?;
    Ok(if changed {
        Resettled::Settled
    } else {
        Resettled::Left
    })
}

/// Unsettled, incomplete command receipts: (task, actor, request key, command reference).
pub(super) fn orphans(
    db: &rusqlite::Connection,
) -> CoreResult<Vec<(String, String, String, Option<String>)>> {
    Ok(db
        .prepare(
            "SELECT task_id,actor,request_key,json_extract(result_json,'$.tool_call_ref') FROM runtime_requests
            WHERE operation IN ('command_run','outside_command') AND json_extract(result_json,'$.settled') IS NOT 1
            AND json_extract(result_json,'$.complete') IS NOT 1 ORDER BY rowid",
        )?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<Result<Vec<_>, _>>()?)
}

/// Checks every orphaned receipt with [`resettle_row`], skipping commands this
/// process still runs or observes (`busy` and in-flight calls). Each receipt is
/// decided alone. Returns how many were settled and the rows left open.
pub(super) fn resettle(
    store: &Store,
    busy: &std::collections::BTreeSet<String>,
    startup: bool,
) -> CoreResult<(usize, Vec<String>)> {
    let mut settled = 0;
    let mut left = Vec::new();
    for (task, actor, key, reference) in store.read(orphans)? {
        if reference
            .as_deref()
            .is_some_and(|r| busy.contains(r) || native_commands::in_flight(r))
        {
            continue;
        }
        match resettle_row(store, &task, &actor, &key, startup) {
            Ok(Resettled::Settled) => settled += 1,
            Ok(Resettled::Left) | Err(_) => left.push(format!("{task}\u{0}{actor}\u{0}{key}")),
        }
    }
    Ok((settled, left))
}

#[cfg(test)]
mod resettle_tests {
    use super::*;
    use cortex_core::{lifecycle::TaskMachine, OpenMode, ProjectKey, StoreChannel};
    use std::os::unix::fs::PermissionsExt;

    fn row(db: &rusqlite::Connection, op: &str, key: &str, value: Value) -> CoreResult<()> {
        db.execute(
            "INSERT INTO runtime_requests VALUES ('t',?,'gone-channel',?,'d',?)",
            (key, op, value.to_string()),
        )?;
        Ok(())
    }
    fn receipt(store: &Store, key: &str) -> Value {
        store
            .read(|db| {
                let raw: String = db.query_row(
                    "SELECT result_json FROM runtime_requests WHERE request_key=?",
                    [key],
                    |r| r.get(0),
                )?;
                Ok(serde_json::from_str(&raw)?)
            })
            .unwrap()
    }
    async fn sleeper() -> tokio::process::Child {
        Command::new("/bin/sleep")
            .arg("30")
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .unwrap()
    }
    async fn end(mut child: tokio::process::Child) {
        child.kill().await.unwrap();
        child.wait().await.unwrap();
    }

    #[tokio::test]
    async fn orphaned_receipts_settle_only_once_their_process_is_provably_gone() {
        let root = std::env::temp_dir().join(format!(
            "command-resettle-{}",
            crate::native_effects::identity().unwrap()
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let key = ProjectKey::new(&root, StoreChannel::Dev).unwrap();
        let store = Store::open(key, OpenMode::Initialize).unwrap();
        TaskMachine::new(&store)
            .create("t", "resettle", "owner")
            .unwrap();
        let dir = directory(&store, "t");
        cortex_core::store::locks::private_directory(&store.key().directory().join("t"), true)
            .unwrap();
        cortex_core::store::locks::private_directory(&dir, true).unwrap();
        // A supervisor (alive, then gone) whose payload group outlives it.
        let supervisor_process = sleeper().await;
        let payload = sleeper().await;
        let identity = supervisor::identity(supervisor_process.id().unwrap()).unwrap();
        let handle = json!({"reference":"native-command:supervised","identity":identity,"status":"s.status",
            "stop":"s.stop","output":"s.bin","id":"lo_s","argv":["/bin/true"],"cwd":"/","profile":"development",
            "mode":"development","denied":[]});
        let status = json!({"ref":"native-command:supervised","supervisor":identity,"state":"ready",
            "group":payload.id().unwrap()});
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(dir.join("s.status"))
            .unwrap();
        std::io::Write::write_all(&mut file, status.to_string().as_bytes()).unwrap();
        drop(file);
        // An outside command with a recorded, live process.
        let outside = sleeper().await;
        let outside_identity = json!({"pid":outside.id().unwrap(),"boot_id":boot_id().unwrap(),
            "start_ticks":std::fs::read_to_string(format!("/proc/{}/stat",outside.id().unwrap())).unwrap()
                .rsplit_once(") ").unwrap().1.split_whitespace().nth(19).unwrap().parse::<u64>().unwrap()});
        store.write(|db| {
            // A launch whose call ended (refused, channel gone) before a supervisor was bound.
            row(db, "command_run", "lost-launch", json!({"complete":false,"tool_call_ref":"native-command:lost","argv":["npm","run","typecheck"]}))?;
            row(db, "command_run", "in-flight", json!({"complete":false,"tool_call_ref":"native-command:flying","argv":["/bin/true"]}))?;
            row(db, "command_run", "supervised", json!({"complete":false,"tool_call_ref":"native-command:supervised","argv":["/bin/true"],"handle":handle}))?;
            row(db, "outside_command", "outside-live", json!({"complete":false,"mode":"auto","tool_call_ref":"native-command:outside","argv":["/bin/true"]}))?;
            lifecycle::event_tx(db, "t", "NATIVE_COMMAND_PROCESS", json!({"ref":"native-command:outside","process_identity":outside_identity}))?;
            row(db, "outside_command", "outside-unlaunched", json!({"complete":false,"mode":"auto"}))?;
            Ok(())
        }).unwrap();
        let unknown = |store: &Store| {
            store
                .read(|db| cortex_core::lifecycle::automatic::require_settled_execution(db, "t"))
                .err()
                .map(|e| e.code().to_owned())
        };
        assert_eq!(
            unknown(&store).as_deref(),
            Some("native_outside_outcome_unknown")
        );
        let busy = std::collections::BTreeSet::new();
        let flight = native_commands::Flight::new("native-command:flying");
        let (settled, left) = resettle(&store, &busy, false).unwrap();
        assert_eq!((settled, left.len()), (2, 2), "{left:?}");
        assert_eq!(
            receipt(&store, "lost-launch")["failure"],
            "command_not_started"
        );
        assert_eq!(
            receipt(&store, "lost-launch")["settlement_evidence"],
            "no_supervisor_bound"
        );
        assert_eq!(
            receipt(&store, "outside-unlaunched")["settlement_evidence"],
            "not_launched"
        );
        for key in ["in-flight", "supervised", "outside-live"] {
            assert_ne!(receipt(&store, key)["settled"], true, "{key} may still run");
        }
        // The supervisor is gone but its payload's process group is not.
        end(supervisor_process).await;
        drop(flight);
        end(outside).await;
        let (settled, left) = resettle(&store, &busy, false).unwrap();
        assert_eq!((settled, left.len()), (2, 1), "{left:?}");
        assert_eq!(
            receipt(&store, "in-flight")["failure"],
            "command_not_started"
        );
        assert_eq!(
            receipt(&store, "outside-live")["failure"],
            "outcome_not_recorded"
        );
        assert_eq!(
            receipt(&store, "outside-live")["settlement_evidence"],
            "process_group_gone"
        );
        assert_ne!(receipt(&store, "supervised")["settled"], true);
        assert_eq!(
            unknown(&store).as_deref(),
            Some("native_outside_outcome_unknown")
        );
        end(payload).await;
        assert_eq!(resettle(&store, &busy, false).unwrap(), (1, Vec::new()));
        let supervised = receipt(&store, "supervised");
        assert_eq!(
            supervised["settlement_evidence"],
            "supervisor_and_process_group_gone"
        );
        assert_eq!(supervised["outcome"], "unknown");
        // Nothing open is left: close-as-is, repair and resume are no longer refused.
        assert_eq!(unknown(&store), None);
        store.read(|db| {
            assert_eq!(db.query_row("SELECT count(*) FROM runtime_requests WHERE json_extract(result_json,'$.complete') IS 1",[],|r|r.get::<_,i64>(0))?, 0);
            assert_eq!(db.query_row("SELECT count(*) FROM runtime_events WHERE type='NATIVE_COMMAND_FINISHED'",[],|r|r.get::<_,i64>(0))?, 5);
            Ok(())
        }).unwrap();
        // A second pass records nothing again.
        assert_eq!(resettle(&store, &busy, true).unwrap(), (0, Vec::new()));
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn a_finished_supervisor_settles_with_its_recorded_exit_and_busy_commands_are_left() {
        let root = std::env::temp_dir().join(format!(
            "command-resettle-finished-{}",
            crate::native_effects::identity().unwrap()
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let key = ProjectKey::new(&root, StoreChannel::Dev).unwrap();
        let store = Store::open(key, OpenMode::Initialize).unwrap();
        TaskMachine::new(&store)
            .create("t", "resettle", "owner")
            .unwrap();
        let dir = directory(&store, "t");
        cortex_core::store::locks::private_directory(&store.key().directory().join("t"), true)
            .unwrap();
        cortex_core::store::locks::private_directory(&dir, true).unwrap();
        let identity = json!({"pid":1,"boot_id":boot_id().unwrap(),"start_ticks":0});
        let handle = json!({"reference":"native-command:done","identity":identity,"status":"f.status",
            "stop":"f.stop","output":"f.bin","id":"lo_f","argv":["/bin/false"],"cwd":"/","profile":"read_only",
            "mode":"development","denied":[]});
        // The recorded output no longer matches: the exit is known, the receipt is not.
        let status = json!({"ref":"native-command:done","supervisor":identity,"state":"finished",
            "exit_code":3,"signal":null,"failure":null,"hash":"0","byte_count":0});
        for (name, body) in [("f.status", status.to_string()), ("f.bin", String::new())] {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(dir.join(name))
                .unwrap();
            std::io::Write::write_all(&mut file, body.as_bytes()).unwrap();
        }
        store.write(|db| {
            row(db, "command_run", "done", json!({"complete":false,"tool_call_ref":"native-command:done","argv":["/bin/false"],"handle":handle}))?;
            row(db, "command_run", "observed", json!({"complete":false,"tool_call_ref":"native-command:observed","argv":["/bin/true"]}))?;
            Ok(())
        }).unwrap();
        let busy = std::collections::BTreeSet::from(["native-command:observed".to_owned()]);
        assert_eq!(resettle(&store, &busy, false).unwrap(), (1, Vec::new()));
        let done = receipt(&store, "done");
        assert_eq!(done["settled"], true);
        assert_eq!(done["complete"], false);
        assert_eq!(done["exit_code"], 3);
        assert_eq!(done["failure"], "outcome_not_recorded");
        assert_eq!(done["settlement_evidence"], "supervisor_finished");
        assert_ne!(receipt(&store, "observed")["settled"], true);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}
