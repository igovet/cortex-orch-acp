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
    let status = format!("{id}.status");
    let stopping = format!("{id}.stop");
    let spec = Spec {
        root: root.clone(),
        cwd: cwd.clone(),
        argv: argv.clone(),
        profile: profile.clone(),
        writes,
        protected,
        output: dir.join(&output),
        status: dir.join(&status),
        stop: dir.join(&stopping),
        reference: reference.clone(),
        timeout: args["timeout_seconds"].as_u64().unwrap_or(1800),
    };
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
            h = json!({"reference":reference,"identity":v["ready"],"status":status,"stop":stopping,
                "output":output,"id":id,"argv":argv,"cwd":cwd,"profile":profile,"mode":mode,"denied":denied});
            let handle = h.clone();
            let t = i.task.clone();
            let actor = c.channel_id.clone();
            let key = args["request_key"].as_str().unwrap().to_owned();
            recv(ex.write(move|db|{
                db.execute("UPDATE runtime_requests SET result_json=json_set(result_json,'$.handle',json(?))
                    WHERE task_id=? AND actor=? AND request_key=? AND operation='command_run'",
                    (handle.to_string(),&t,&actor,&key))?;
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
        let after = native_commands::check_digest(root, &argv).await;
        let revision = if before == after { after } else { None };
        let actor = c.channel_id;
        let task = i.task;
        let key = args["request_key"].as_str().unwrap().to_owned();
        let h = h.clone();
        recv(ex.publication(move |s| settle(s, &task, &actor, &key, &h, &status, revision))?).await
    }
    .await;
    // A child handle is only reaped here; its independent supervisor is not
    // killed on service death. A lost observer cannot invent a command result.
    let _ = tokio::time::timeout(Duration::from_secs(3), child.wait()).await;
    if result.is_ok() {
        done.send_replace(true);
    } else {
        let h = guard.handle.clone();
        let _ = recv(ex.publication(move |s| unknown(s, &unknown_task, &h))?).await;
    }
    drop(guard);
    running.lock().await.remove(&reference);
    result.map(crate::native_sources::reply)
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
