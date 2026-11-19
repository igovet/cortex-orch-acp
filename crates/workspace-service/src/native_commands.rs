//! Durable command identities, private full output and owner-only running handles.
//! Brokered execution uses bubblewrap filesystem/process views; no uncontained fallback.
use crate::{
    native_project::{digest, safe, writable},
    native_sessions::ClientPermissions,
};
use cortex_core::{
    access,
    channels::{self, Credential, Identity},
    lifecycle::{self, waits},
    scheduler::policy,
    store::executor::StoreExecutor,
    CoreError, CoreResult, Store,
};
use cortex_engine::engine::PermissionDecision;
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    sync::{watch, Mutex},
};
pub(super) type Running =
    Arc<Mutex<std::collections::HashMap<String, crate::native_owner_controls::RunningCommand>>>;
/// References of command calls still running in this process, from before their
/// intent row until the call returns (or its future is dropped). The re-settle of
/// orphaned receipts never touches them.
static IN_FLIGHT: std::sync::Mutex<std::collections::BTreeSet<String>> =
    std::sync::Mutex::new(std::collections::BTreeSet::new());
pub(super) struct Flight(String);
impl Flight {
    pub(super) fn new(reference: &str) -> Self {
        IN_FLIGHT
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(reference.to_owned());
        Self(reference.to_owned())
    }
}
impl Drop for Flight {
    fn drop(&mut self) {
        IN_FLIGHT
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
    }
}
pub(super) fn in_flight(reference: &str) -> bool {
    IN_FLIGHT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(reference)
}
/// Settles a `command_run`/`outside_command` receipt whose outcome was never
/// recorded: `settled:true` with `complete:false` and the failure, never a success.
/// `command_not_started` means the payload provably never launched; any other
/// failure leaves the outcome unknown (`outcome:"unknown"`). Only an unsettled,
/// incomplete row bound to `reference` changes; returns whether it did. A finish
/// event is added unless that command already has one.
pub(super) fn settle_unrecorded_tx(
    db: &rusqlite::Connection,
    task: &str,
    actor: &str,
    key: &str,
    reference: Option<&str>,
    failure: &str,
    exit_code: Option<i64>,
    evidence: &str,
) -> CoreResult<bool> {
    let row: Option<(String, String)> = db.query_row("SELECT operation,result_json FROM runtime_requests
        WHERE task_id=? AND actor=? AND request_key=? AND operation IN ('command_run','outside_command')",
        (task, actor, key), |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
    let Some((operation, raw)) = row else {
        return Ok(false);
    };
    let mut value: Value = serde_json::from_str(&raw)?;
    if value["settled"] == true || value["complete"] == true {
        return Ok(false);
    }
    if value["tool_call_ref"].as_str() != reference {
        return Err(CoreError::Refused("command_handle_identity_mismatch"));
    }
    let started = failure != "command_not_started";
    let object = value
        .as_object_mut()
        .ok_or(CoreError::Refused("invalid_command_receipt"))?;
    for (field, entry) in [
        ("settled", json!(true)),
        ("complete", json!(false)),
        ("failure", json!(failure)),
        ("exit_code", json!(exit_code)),
        (
            "outcome",
            json!(if started { "unknown" } else { "not_started" }),
        ),
        ("launched", if started { Value::Null } else { json!(false) }),
        ("settlement_evidence", json!(evidence)),
    ] {
        object.insert(field.to_owned(), entry);
    }
    db.execute(
        "UPDATE runtime_requests SET result_json=? WHERE task_id=? AND actor=? AND request_key=? AND result_json=?",
        (value.to_string(), task, actor, key, &raw),
    )?;
    let finished: bool = match reference {
        Some(reference) => db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='NATIVE_COMMAND_FINISHED'
            AND json_extract(payload_json,'$.receipt.tool_call_ref')=?)", (task, reference), |r| r.get(0))?,
        None => false,
    };
    if !finished {
        let mut observation = value.clone();
        if let Some(o) = observation.as_object_mut() {
            o.remove("output");
            o.remove("handle");
        }
        lifecycle::event_tx(
            db,
            task,
            "NATIVE_COMMAND_FINISHED",
            json!({"receipt":{
            "tool_call_ref":reference,"argv":value["argv"],"request_key":key,"operation":operation,
            "observation":observation}}),
        )?;
    }
    Ok(true)
}
/// Retain value-free provenance when a launch or settlement fails. A store failure
/// can also prevent recording the diagnostic; the original error still propagates.
pub(super) async fn record_failure(
    ex: &StoreExecutor,
    credential: &Credential,
    key: &str,
    phase: &'static str,
    error: &CoreError,
) {
    let class = match error {
        CoreError::Io(e) => format!("io_{:?}", e.kind()),
        CoreError::Sql(rusqlite::Error::SqliteFailure(e, _)) => format!("sqlite_{:?}", e.code),
        CoreError::Sql(_) => "sqlite".into(),
        CoreError::Refused(
            code @ ("invalid_cwd"
            | "command_output_create_failed"
            | "output_integrity_mismatch"
            | "command_handle_identity_mismatch"),
        ) => (*code).into(),
        _ => return,
    };
    let c = credential.clone();
    let key = key.to_owned();
    if let Ok(receiver) = ex.write(move |db| {
        let i = channels::authenticate(db,&c.channel_id,c.secret(),true)?;
        let reference: Option<String> = db.query_row("SELECT json_extract(result_json,'$.tool_call_ref') FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=? AND operation='command_run'",(&i.task,&c.channel_id,&key),|r|r.get(0)).optional()?.flatten();
        let observed = match &reference {
            None => "before_command_intent",
            Some(reference) => {
                let launched: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='NATIVE_COMMAND_PROCESS' AND json_extract(payload_json,'$.ref')=?)",(&i.task,reference),|r|r.get(0))?;
                if launched { "after_process_registration" } else { "after_command_intent" }
            }
        };
        lifecycle::event_tx(db,&i.task,"NATIVE_COMMAND_DIAGNOSTIC",json!({"class":class,"phase":phase,"stage":observed,"channel_id":c.channel_id,"ref":reference}))?;
        Ok(())
    }) { let _ = receiver.await; }
}
pub(super) async fn check_digest(root: PathBuf, argv: &[String]) -> Option<String> {
    // Reads/no-ops cannot verify a check: avoid a worktree scan for them.
    if cortex_core::lifecycle::completion::trivial_command(argv) {
        return None;
    }
    crate::native_effects::observe(root)
        .await
        .ok()
        .and_then(|e| e.project_digest)
}
pub(super) fn supports(n: &str) -> bool {
    matches!(n, "command_run" | "project_git")
}
fn create_command_output(output_dir: &std::path::Path, file: &str) -> CoreResult<File> {
    OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(output_dir.join(file))
        .map_err(|_| CoreError::Refused("command_output_create_failed"))
}
async fn recv<T>(r: tokio::sync::oneshot::Receiver<CoreResult<T>>) -> CoreResult<T> {
    r.await
        .map_err(|_| CoreError::Refused("native_host_closed"))?
}
pub(super) fn fingerprint(file: &mut File) -> CoreResult<(String, u64)> {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    let mut count = 0;
    let mut chunk = [0; 65536];
    file.seek(SeekFrom::Start(0))?;
    loop {
        let n = file.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        count += n as u64;
        hash.update(&chunk[..n]);
    }
    Ok((format!("{:x}", hash.finalize()), count))
}
/// Retain oversized authorized read replies, without clipping their JSON content.
pub(super) fn defer_json(store: &Store, i: &Identity, value: Value) -> CoreResult<Value> {
    defer_json_over(store, i, value, 32768)
}
/// `defer_json` with an explicit inline byte budget (read_batch items use 6144).
pub(super) fn defer_json_over(
    store: &Store,
    i: &Identity,
    value: Value,
    inline: usize,
) -> CoreResult<Value> {
    let bytes = serde_json::to_vec(&value)?;
    if bytes.len() <= inline {
        return Ok(value);
    }
    let dir = store.key().directory().join(&i.task).join(".native-output");
    cortex_core::store::locks::private_directory(&store.key().directory().join(&i.task), true)?;
    cortex_core::store::locks::private_directory(&dir, true)?;
    let id = format!("lo_{}", &crate::native_effects::identity()?[..12]);
    let filename = format!("{id}.bin");
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(dir.join(&filename))?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    File::open(&dir)?.sync_all()?;
    store.write(|db| {
        db.execute(
            "INSERT INTO large_outputs VALUES (?,?,?,'tool_result',?,?,?,?,NULL,NULL,NULL,?)",
            rusqlite::params![
                id,
                i.task,
                i.assignment.as_deref().unwrap_or("coordinator"),
                digest(&bytes),
                filename,
                bytes.len() as i64,
                0,
                cortex_core::now()
            ],
        )?;
        Ok(())
    })?;
    Ok(
        json!({"deferred":true,"large_output_id":id,"byte_count":bytes.len(),"encoding":"utf-8","complete":false,
        "reason":"response_exceeds_inline_budget"}),
    )
}
pub(super) fn read_output(
    store: &Store,
    i: &Identity,
    c: &Credential,
    args: &Value,
) -> CoreResult<Value> {
    let id = args["large_output_id"]
        .as_str()
        .ok_or(CoreError::Refused("invalid_output"))?;
    let scope = i.assignment.as_deref().unwrap_or("coordinator");
    let (file,count,hash,kind,truncated,code,signal,failure):(String,i64,Option<String>,String,bool,Option<i64>,Option<String>,Option<String>)=store.read(|db|Ok(db.query_row(
        "SELECT filename,byte_count,source_ref,source_kind,truncated,exit_code,signal,failure FROM large_outputs l WHERE task_id=? AND id=?
        AND (owner_scope=? OR owner_scope=? OR EXISTS(SELECT 1 FROM trusted_channels old WHERE old.task_id=l.task_id
            AND old.channel_id=l.owner_scope AND ((?='coordinator' AND old.role='coordinator') OR old.assignment_id=?)))",
        rusqlite::params![i.task,id,c.channel_id,scope,scope,i.assignment],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?)))?))?;
    if file.contains('/') || file.contains("..") {
        return Err(CoreError::Refused("invalid_output"));
    }
    let path = store
        .key()
        .directory()
        .join(&i.task)
        .join(".native-output")
        .join(&file);
    let (mut f, native) = match cortex_core::store::locks::open_private(&path, false, false) {
        Ok(file) => (file, true),
        Err(CoreError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => (
            cortex_core::store::locks::open_private(
                &store
                    .key()
                    .directory()
                    .join(&i.task)
                    .join("large_outputs")
                    .join(&file),
                false,
                false,
            )?,
            false,
        ),
        Err(e) => return Err(e),
    };
    let (actual, size) = fingerprint(&mut f)?;
    if size as i64 != count || native && hash.as_deref() != Some(&actual) {
        return Err(CoreError::Refused("output_integrity_mismatch"));
    }
    let offset = args["offset"].as_u64().unwrap_or(0);
    let limit = args["limit"].as_u64().unwrap_or(65536);
    f.seek(SeekFrom::Start(offset))?;
    let mut b = Vec::new();
    f.take(limit).read_to_end(&mut b)?;
    use base64::Engine;
    let (chunk, encoding) = match std::str::from_utf8(&b) {
        Ok(s) => (s.to_owned(), "utf-8"),
        Err(_) => (
            base64::engine::general_purpose::STANDARD.encode(&b),
            "base64",
        ),
    };
    Ok(crate::native_sources::reply(
        json!({"id":id,"large_output_id":id,"offset":offset,"output":String::from_utf8_lossy(&b),"chunk":chunk,"encoding":encoding,"byte_count":count,
        "truncated":truncated,"exit_status":{"exit_code":code,"signal":signal},"failure":failure,"source_kind":kind,"source_ref":if native{Value::Null}else{json!(hash)},
        "integrity_verified":native,"next_offset":if offset+(b.len() as u64)<count as u64{json!(offset+b.len() as u64)}else{Value::Null},"complete":offset==0&&b.len() as i64==count}),
    ))
}
/// Called only after native_access has committed consent for the exact request.
pub(super) async fn outside_capture(
    executor: Arc<StoreExecutor>,
    credential: Credential,
    running: Running,
    root: PathBuf,
    args: &Value,
    mut cancel: watch::Receiver<bool>,
) -> CoreResult<Value> {
    let c = credential.clone();
    let (task, dir, project_root) = recv(executor.publication(move |store| {
        let i = store.read(|db| channels::authenticate(db, &c.channel_id, c.secret(), true))?;
        store.write(|db| writable(db, &i, None))?;
        let dir = store.key().directory().join(&i.task).join(".native-output");
        cortex_core::store::locks::private_directory(&store.key().directory().join(&i.task), true)?;
        cortex_core::store::locks::private_directory(&dir, true)?;
        Ok((i.task, dir, store.key().root().to_path_buf()))
    })?)
    .await?;
    let argv: Vec<String> = serde_json::from_value(args["argv"].clone())?;
    let reference = format!(
        "native-command:{}",
        &crate::native_effects::identity()?[..24]
    );
    let id = format!("lo_{}", &crate::native_effects::identity()?[..12]);
    let filename = format!("{id}.bin");
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(dir.join(&filename))?;
    let t = task.clone();
    let r = reference.clone();
    let a = argv.clone();
    let actor = credential.channel_id.clone();
    let key = args["request_key"]
        .as_str()
        .ok_or(CoreError::Refused("invalid_request"))?
        .to_owned();
    let _flight = Flight::new(&reference);
    recv(executor.write(move |db| {
        // A receipt the runtime already settled (its call was lost) never launches.
        let bound = db.execute("UPDATE runtime_requests SET result_json=json_set(result_json,'$.tool_call_ref',?,'$.argv',json(?))
            WHERE task_id=? AND actor=? AND request_key=? AND operation='outside_command' AND json_extract(result_json,'$.complete')=0
            AND json_extract(result_json,'$.settled') IS NOT 1",
            (&r,serde_json::to_string(&a)?,&t,&actor,&key))?;
        if bound != 1 {
            return Err(CoreError::Refused("command_outcome_unknown"));
        }
        lifecycle::event_tx(db, &t, "NATIVE_COMMAND_STARTED", json!({"ref":r,"argv":a,"channel_id":actor}))?;
        Ok(())
    })?)
    .await?;
    let started_digest = check_digest(project_root.clone(), &argv).await;
    let (stop, mut stopped) = watch::channel(false);
    let (done, finished) = watch::channel(false);
    running.lock().await.insert(
        reference.clone(),
        crate::native_owner_controls::RunningCommand {
            task: task.clone(),
            cancel: stop,
            done: finished,
        },
    );
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .current_dir(&root)
        .env_remove("CORTEX_CHANNEL_SECRET")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .process_group(0);
    let output = Arc::new(Mutex::new(file));
    let deadline = Duration::from_secs(args["timeout_seconds"].as_u64().unwrap_or(600));
    let result=async{
        let mut child=cmd.spawn()?;let pid=child.id().unwrap();let mut group=Group(Some(pid));
        retain_process(&executor,&task,&reference,pid).await?;
        let mut out=tokio::spawn(drain(child.stdout.take().unwrap(),output.clone()));
        let mut err=tokio::spawn(drain(child.stderr.take().unwrap(),output.clone()));
        let mut failure=None;
        let code=tokio::select!{
            result=tokio::time::timeout(deadline,child.wait())=>match result{Ok(Ok(s))=>s.code(),_=>{failure=Some("interrupted_or_timeout");None}},
            _=cancel.changed()=>{failure=Some("authority_changed");None},
            _=stopped.changed()=>{failure=Some("owner_stopped");None},
        };
        group.kill();let _=child.wait().await;
        let drained=tokio::time::timeout(Duration::from_secs(2),async{(&mut out).await.map_err(|_|CoreError::Refused("output_lost"))??;(&mut err).await.map_err(|_|CoreError::Refused("output_lost"))??;Ok::<_,CoreError>(())}).await;
        if !matches!(drained,Ok(Ok(()))){out.abort();err.abort();failure=Some("output_incomplete");}
        Ok::<_,CoreError>((code,failure))
    }.await;
    let result=async{
        output.lock().await.sync_all()?;
        let (code,failure)=result.unwrap_or((None,Some("spawn_or_capture_failed")));
        let mut file=File::open(dir.join(&filename))?;let (hash,size)=fingerprint(&mut file)?;file.seek(SeekFrom::Start(0))?;
        let mut prefix=Vec::new();file.take(65536).read_to_end(&mut prefix)?;
        let ended_digest = check_digest(project_root, &argv).await;
        let revision_digest = if started_digest == ended_digest { ended_digest } else { None };
        let value=json!({"revision_digest":revision_digest,"tool_call_ref":reference,"argv":argv,"cwd":root,"profile":"outside_confirmed","exit_code":code,"complete":failure.is_none()&&code.is_some(),
            "settled":true,"failure":failure,"output":String::from_utf8_lossy(&prefix),"output_truncated":size>65536,"byte_count":size,"large_output_id":id});
        let c=credential.clone();let mut event=value.clone();event.as_object_mut().unwrap().remove("output");
        recv(executor.write(move|db|{
            db.execute("INSERT INTO large_outputs VALUES (?,?,?,'terminal',?,?,?,?,?,NULL,?,?)",
                rusqlite::params![id,task,c.channel_id,hash,filename,size as i64,0,code,failure,cortex_core::now()])?;
            lifecycle::event_tx(db,&task,"NATIVE_COMMAND_FINISHED",json!({"receipt":{"tool_call_ref":event["tool_call_ref"],"argv":event["argv"],"observation":event}}))?;Ok(())
        })?).await?;
        Ok::<_,CoreError>(value)
    }.await;
    done.send_replace(true);
    running.lock().await.remove(&reference);
    result
}

#[cfg(test)]
mod output_tests {
    use super::create_command_output;
    use cortex_core::CoreError;
    use std::path::Path;

    #[test]
    fn output_create_failure_is_reported_before_process_registration() {
        let result = create_command_output(Path::new("/dev/null"), "output.bin");
        assert!(matches!(
            result,
            Err(CoreError::Refused("command_output_create_failed"))
        ));
    }
}
// PID alone is not an identity: boot ID and /proc start ticks fence reuse.
fn process_identity(pid: u32) -> Option<Value> {
    let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let fields = stat
        .rsplit_once(") ")?
        .1
        .split_whitespace()
        .collect::<Vec<_>>();
    let start = fields.get(19)?.parse::<u64>().ok()?;
    Some(json!({"pid":pid,"boot_id":boot.trim(),"start_ticks":start}))
}
async fn retain_process(
    executor: &StoreExecutor,
    task: &str,
    reference: &str,
    pid: u32,
) -> CoreResult<()> {
    let identity = process_identity(pid);
    let task = task.to_owned();
    let reference = reference.to_owned();
    recv(executor.write(move |db| {
        lifecycle::event_tx(
            db,
            &task,
            "NATIVE_COMMAND_PROCESS",
            json!({"ref":reference,"process_identity":identity}),
        )?;
        Ok(())
    })?)
    .await
}
/// A non-child cannot regain wait status or lost pipe bytes after restart.
/// Report that handle as unknown, with a fresh leader observation, never as a
/// successful command or a safe process-group stop target. Do not signal a PID
/// which could have been recycled, and never launch the payload again.
pub(super) fn recover_commands(store: &Store) -> CoreResult<()> {
    store.write(|db| {
        let rows = db.prepare("SELECT task_id,request_key,actor,result_json FROM runtime_requests
            WHERE operation IN ('command_run','outside_command') AND json_extract(result_json,'$.settled') IS NOT 1 AND json_extract(result_json,'$.complete') IS NOT 1")?
            .query_map([], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?)))?
            .collect::<Result<Vec<_>,_>>()?;
        for (task, key, actor, raw) in rows {
            let intent: Value = serde_json::from_str(&raw)?;
            if intent["handle"].is_object() { continue; }
            let Some(reference) = intent["tool_call_ref"].as_str() else { continue };
            let prior: Option<String> = db.query_row("SELECT payload_json FROM runtime_events
                WHERE task_id=? AND type='NATIVE_COMMAND_PROCESS' AND json_extract(payload_json,'$.ref')=? ORDER BY id DESC LIMIT 1",
                (&task, reference), |r| r.get(0)).optional()?;
            let identity = prior.and_then(|p|serde_json::from_str::<Value>(&p).ok())
                .map(|p|p["process_identity"].clone()).unwrap_or(Value::Null);
            let observation = identity["pid"].as_u64().and_then(|p|u32::try_from(p).ok()).map(|pid| {
                match process_identity(pid) {
                    Some(current) if current == identity => "live",
                    Some(_) => "identity_mismatch",
                    None => "not_observable",
                }
            }).unwrap_or("identity_unavailable");
            let seen: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=?
                AND type='NATIVE_COMMAND_RECOVERED' AND json_extract(payload_json,'$.ref')=?)",
                (&task, reference), |r| r.get(0))?;
            if !seen {
                lifecycle::event_tx(db, &task, "NATIVE_COMMAND_RECOVERED",
                    json!({"ref":reference,"request_key":key,"channel_id":actor,
                        "process_identity":identity,"leader_observation":observation,
                        "handle_state":"unknown","outcome":"unknown","replayed":false}))?;
            }
        }
        Ok(())
    })
}
#[cfg(test)]
mod recovery_tests {
    use super::*;
    use cortex_core::{lifecycle::TaskMachine, OpenMode, ProjectKey, StoreChannel};
    #[tokio::test]
    async fn restart_keeps_exact_process_identity_and_unknown_handle_without_relaunch() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!(
            "command-recovery-{}",
            crate::native_effects::identity().unwrap()
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let key = ProjectKey::new(&root, StoreChannel::Dev).unwrap();
        let store = Store::open(key.clone(), OpenMode::Initialize).unwrap();
        TaskMachine::new(&store)
            .create("t", "recovery", "owner")
            .unwrap();
        let mut child = Command::new("/bin/sleep")
            .arg("30")
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let pid = child.id().unwrap();
        let identity = process_identity(pid).unwrap();
        let mut reused = identity.clone();
        reused["start_ticks"] = json!(0);
        store.write(|db|{
            for (reference,identity) in [("live",identity.clone()),("reused",reused),("unbound",Value::Null)] {
                db.execute("INSERT INTO runtime_requests VALUES ('t',?,'old-channel','command_run','bound',?)",
                    (reference,json!({"complete":false,"tool_call_ref":reference,"argv":["/bin/false"]}).to_string()))?;
                lifecycle::event_tx(db,"t","NATIVE_COMMAND_PROCESS",json!({"ref":reference,"process_identity":identity}))?;
            }
            Ok(())
        }).unwrap();
        drop(store);
        let store = Store::open(key, OpenMode::RuntimeValidate).unwrap();
        recover_commands(&store).unwrap();
        recover_commands(&store).unwrap();
        let values:Vec<Value>=store.read(|db| Ok(db.prepare("SELECT payload_json FROM runtime_events WHERE type='NATIVE_COMMAND_RECOVERED' ORDER BY id")?
            .query_map([],|r|Ok(serde_json::from_str::<Value>(&r.get::<_,String>(0)?).unwrap()))?
            .collect::<Result<Vec<_>,_>>()?)).unwrap();
        assert_eq!(values.len(), 3);
        assert_eq!(values[0]["leader_observation"], "live");
        assert_eq!(values[0]["process_identity"], identity);
        assert_eq!(values[1]["leader_observation"], "identity_mismatch");
        assert_eq!(values[2]["leader_observation"], "identity_unavailable");
        assert!(values
            .iter()
            .all(|v| v["handle_state"] == "unknown" && v["replayed"] == false));
        assert!(
            child.try_wait().unwrap().is_none(),
            "recovery must not signal an unadopted process"
        );
        child.kill().await.unwrap();
        child.wait().await.unwrap();
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}
struct Group(Option<u32>);
impl Group {
    fn kill(&mut self) {
        if let Some(pid) = self.0.take() {
            unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
        }
    }
}
impl Drop for Group {
    fn drop(&mut self) {
        self.kill();
    }
}
async fn drain(mut r: impl AsyncRead + Unpin, output: Arc<Mutex<File>>) -> CoreResult<()> {
    let mut buf = [0; 8192];
    loop {
        let n = r.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        output.lock().await.write_all(&buf[..n])?;
    }
    Ok(())
}
pub(super) fn denials(db: &rusqlite::Connection, task: &str) -> CoreResult<Vec<String>> {
    // The owner's write denials in force (lifted ones excluded), sorted.
    let paths = cortex_core::lifecycle::steering::denials(db, task)?;
    for path in &paths {
        safe(path)?;
    }
    Ok(paths)
}
fn deletion(argv: &[String]) -> bool {
    if regex::Regex::new(r"\b(rm|unlink|rmdir|shred)\b|--delete|-delete|\bgit\b.*\b(clean|rm)\b")
        .unwrap()
        .is_match(&argv.join(" "))
    {
        return true;
    }
    // Conservative consent detector, never a hostile-code parser.
    argv.iter().any(|a| {
        a.split(|c: char| c.is_whitespace() || ";|&()".contains(c))
            .any(|s| {
                ["rm", "unlink", "rmdir", "shred", "-delete", "--delete"].contains(&s)
                    || s.ends_with("/rm")
            })
    }) || argv
        .windows(2)
        .any(|p| p[0] == "git" && ["clean", "rm"].contains(&p[1].as_str()))
}
pub(super) async fn execute(
    executor: Arc<StoreExecutor>,
    credential: Credential,
    root: PathBuf,
    permissions: Option<Arc<dyn ClientPermissions>>,
    protected_roots: Vec<PathBuf>,
    running: Running,
    name: &str,
    args: Value,
) -> CoreResult<Value> {
    let name = name.to_owned();
    let c = credential.clone();
    let a = args.clone();
    let n = name.clone();
    let hash = digest(&serde_json::to_vec(&json!([name, args]))?);
    let h = hash.clone();
    let (i,mode,scopes,saved,output_dir)=recv(executor.publication(move|store|{
        let i=store.read(|db|channels::authenticate(db,&c.channel_id,c.secret(),true))?;
        let mode=store.read(|db|access::current(db,&i.task))?;
        let saved:Option<(String,String,String)>=store.read(|db|Ok(db.query_row("SELECT operation,digest,result_json FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=?",
            (&i.task,&c.channel_id,a["request_key"].as_str()),|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?))?;
        if let Some((op,hash,v))=saved {if op!=n||hash!=h{return Err(CoreError::Refused("request_key_conflict"));}let v:Value=serde_json::from_str(&v)?;
            let value = if n == "command_run" {
                crate::native_command_handles::public_status(store,&i.task,&v)?
            } else if v["settled"]==true { v } else {return Err(CoreError::Refused("command_outcome_unknown"));};
            return Ok((i,mode,Vec::new(),Some(value),PathBuf::new()));
        }
        let scopes=store.write(|db|{
            if n=="command_run" {if i.role!="worker"||i.profile.as_deref()==Some("worker-senior-consultant"){return Err(CoreError::Refused("worker_required"));}
                if let Some(a)=i.assignment.as_deref(){waits::check_mutation_tx(db,&i.task,a)?;}
            }
            let raw:Option<String>=db.query_row("SELECT scope_json FROM runtime_nodes WHERE task_id=? AND assignment_ref=?",(&i.task,&i.assignment),|r|r.get(0)).optional()?;
            let scopes=policy::effective_scope_tx(db,&i.task,i.assignment.as_deref(),raw.as_deref().unwrap_or("[]"))?;
            if n=="command_run"&&!policy::read_only(i.profile.as_deref().unwrap_or(""),i.stage.as_deref().unwrap_or("")){
                writable(db,&i,None)?;
            }
            Ok(scopes)
        })?;
        let dir=store.key().directory().join(&i.task).join(".native-output");
        cortex_core::store::locks::private_directory(&store.key().directory().join(&i.task),true)?;
        cortex_core::store::locks::private_directory(&dir,true)?;
        Ok((i,mode,scopes,None,dir))
    })?).await?;
    if let Some(v) = saved {
        return Ok(crate::native_sources::reply(v));
    }
    let readonly = i.role != "worker"
        || policy::read_only(
            i.profile.as_deref().unwrap_or(""),
            i.stage.as_deref().unwrap_or(""),
        );
    let mut argv: Vec<String> = if name == "project_git" {
        if args["kind"] == "status" {
            vec![
                "git".into(),
                "-c".into(),
                "core.fsmonitor=false".into(),
                "status".into(),
                "--short".into(),
            ]
        } else {
            vec![
                "git".into(),
                "-c".into(),
                "core.fsmonitor=false".into(),
                "diff".into(),
                "--no-ext-diff".into(),
                "--no-textconv".into(),
                "HEAD".into(),
                "--".into(),
            ]
        }
    } else {
        serde_json::from_value(args["argv"].clone())?
    };
    if name == "project_git" {
        let mut probe = Command::new("git");
        probe
            .args([
                "config",
                "--null",
                "--name-only",
                "--get-regexp",
                r"^filter\.",
            ])
            .current_dir(&root)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", "/nonexistent")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .stdin(Stdio::null())
            .kill_on_drop(true);
        let out = tokio::time::timeout(Duration::from_secs(5), probe.output())
            .await
            .map_err(|_| CoreError::Refused("git_filter_probe_timeout"))??;
        if !matches!(out.status.code(), Some(0 | 1))
            || out.stdout.len() > 65536
            || !out.stderr.is_empty()
        {
            return Err(CoreError::Refused("git_filter_probe_failed"));
        }
        let mut extra = vec![
            "-c".to_owned(),
            "core.hooksPath=/dev/null".into(),
            "-c".into(),
            "core.untrackedCache=false".into(),
        ];
        let names = String::from_utf8(out.stdout)
            .map_err(|_| CoreError::Refused("git_filter_probe_failed"))?;
        let drivers = names
            .split('\0')
            .filter_map(|s| s.rsplit_once('.').map(|(s, _)| s))
            .collect::<std::collections::BTreeSet<_>>();
        for driver in drivers {
            for (key, value) in [
                ("clean", ""),
                ("smudge", ""),
                ("process", ""),
                ("required", "false"),
            ] {
                extra.extend(["-c".to_owned(), format!("{driver}.{key}={value}")]);
            }
        }
        argv.splice(1..1, extra);
    }
    let cwd = args["cwd"]
        .as_str()
        .map(PathBuf::from)
        .unwrap_or(root.clone());
    // Access mode selects outside-cwd availability, never a role's write authority.
    if !cwd.is_absolute()
        || cwd
            .components()
            .any(|p| matches!(p, std::path::Component::ParentDir))
        || cwd
            .canonicalize()
            .map_err(|_| CoreError::Refused("invalid_cwd"))?
            != cwd
        || !cwd.is_dir()
    {
        return Err(CoreError::Refused("invalid_cwd"));
    }
    if let Ok(relative) = cwd.strip_prefix(&root) {
        if cwd != root {
            safe(relative.to_str().ok_or(CoreError::Refused("invalid_cwd"))?)?;
        }
    } else if mode == "guarded"
        || crate::native_access::protected(&cwd)
        || crate::native_command_sandbox::policy::protected_target(&root, &cwd)
        || protected_roots
            .iter()
            .any(|p| p.canonicalize().map(|p| cwd.starts_with(p)).unwrap_or(true))
    {
        return Err(CoreError::Refused("outside_cwd_denied"));
    }
    let is_delete = name == "command_run" && deletion(&argv);
    if is_delete && readonly {
        return Err(CoreError::Refused("readonly_delete_refused"));
    }
    if is_delete && mode != "auto" {
        let c = credential.clone();
        let task = i.task.clone();
        let session=recv(executor.read(move|db|Ok(db.query_row("SELECT session_id FROM acp_host_sessions WHERE task_id=? AND json_extract(client_info,'$.name')='cortex-rust-acp'",[task],|r|r.get::<_,String>(0)).optional()?))?).await?;
        let request = json!({"toolCall":{"toolCallId":args["request_key"],"title":"Delete project data","kind":"execute","rawInput":args},
            "options":[{"optionId":"allow","name":"Allow once","kind":"allow_once"},{"optionId":"deny","name":"Deny","kind":"reject_once"}]});
        let allowed = match (permissions, session) {
            (Some(p), Some(s)) => {
                matches!(p.decide(&s,&request).await,Ok(PermissionDecision::Selected(id)) if id=="allow")
            }
            _ => false,
        };
        if !allowed {
            let h = hash.clone();
            let n = name.clone();
            let a = args.clone();
            recv(executor.write(move|db|{let i=channels::authenticate(db,&c.channel_id,c.secret(),true)?;
                db.execute("INSERT INTO runtime_requests VALUES (?,?,?,?,?,?)",(&i.task,a["request_key"].as_str(),&c.channel_id,n,h,json!({"complete":false,"settled":true,"exit_code":null,"failure":"deletion_not_confirmed"}).to_string()))?;Ok(())})?).await?;
            return Err(CoreError::Refused("deletion_not_confirmed"));
        }
    }
    let reference = format!(
        "native-command:{}",
        &crate::native_effects::identity()?[..24]
    );
    let id = format!("lo_{}", &crate::native_effects::identity()?[..12]);
    let file = format!("{id}.bin");
    let out = create_command_output(&output_dir, &file)?;
    let profile = if name == "project_git" {
        "git_context"
    } else if readonly {
        "read_only"
    } else if mode == "guarded" {
        "guarded"
    } else {
        "development"
    };
    // Reservations coordinate writers; they are not the project's write boundary.
    let _ = scopes;
    let writes = if !readonly && name != "project_git" {
        vec![root.clone()]
    } else {
        Vec::new()
    };
    let task = i.task.clone();
    let denied = recv(executor.read(move |db| denials(db, &task))?).await?;
    let c = credential.clone();
    let a = args.clone();
    let n = name.clone();
    let h = hash.clone();
    let r = reference.clone();
    let mode_copy = mode.clone();
    let denied_copy = denied.clone();
    let expected = i.clone();
    let _flight = Flight::new(&reference);
    recv(executor.write(move |db| {
        let i = expected;
        let current = channels::authenticate(db, &c.channel_id, c.secret(), true)?;
        if current != i
            || access::current(db, &i.task)? != mode_copy
            || denials(db, &i.task)? != denied_copy
        {
            return Err(CoreError::Refused("channel_or_access_changed"));
        }
        if n == "command_run" && !readonly {
            writable(db, &i, None)?;
        }
        db.execute(
            "INSERT INTO runtime_requests VALUES (?,?,?,?,?,?)",
            (
                &i.task,
                a["request_key"].as_str(),
                &c.channel_id,
                n,
                h,
                json!({"complete":false,"tool_call_ref":r,"argv":a["argv"]}).to_string(),
            ),
        )?;
        lifecycle::event_tx(
            db,
            &i.task,
            "NATIVE_COMMAND_STARTED",
            json!({"ref":r,"channel_id":c.channel_id,"argv":a["argv"]}),
        )?;
        Ok(())
    })?)
    .await?;
    if name == "command_run" {
        drop(out);
        let mut protected = output_dir
            .parent()
            .and_then(|p| p.parent())
            .map(PathBuf::from)
            .into_iter()
            .collect::<Vec<_>>();
        protected.extend(protected_roots);
        protected.extend(denied.iter().map(|p| root.join(p)));
        return crate::native_command_handles::execute(
            executor,
            credential,
            running,
            i,
            root,
            cwd,
            argv,
            mode,
            readonly,
            denied,
            writes,
            protected,
            output_dir,
            file,
            id,
            reference,
            args,
            profile.to_owned(),
        )
        .await;
    }
    let (cancel, mut cancelled) = watch::channel(false);
    let (done, finished) = watch::channel(false);
    running.lock().await.insert(
        reference.clone(),
        crate::native_owner_controls::RunningCommand {
            task: i.task.clone(),
            cancel: cancel.clone(),
            done: finished,
        },
    );
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]);
    cmd.current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .process_group(0)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null");
    let started_digest = check_digest(root.clone(), &argv).await;
    let output = Arc::new(Mutex::new(out));
    let result=async{
        let mut child=cmd.spawn()?;let pid=child.id().unwrap();let mut group=Group(Some(pid));retain_process(&executor,&i.task,&reference,pid).await?;let mut out=tokio::spawn(drain(child.stdout.take().unwrap(),output.clone()));let mut err=tokio::spawn(drain(child.stderr.take().unwrap(),output.clone()));
        let deadline=tokio::time::sleep(Duration::from_secs(args["timeout_seconds"].as_u64().unwrap_or(1800)));tokio::pin!(deadline);
        let mut failure=None;let code=loop {
            tokio::select!{
                status=child.wait()=>break status?.code(),
                _=&mut deadline=>{failure=Some("timeout");break None;},
                _=cancelled.changed()=>{if *cancelled.borrow(){failure=Some("owner_stopped");break None;}},
                _=tokio::time::sleep(Duration::from_millis(50))=>{
                    let c=credential.clone();let expected=mode.clone();let denied=denied.clone();let valid=recv(executor.write(move|db|{
                        let i=channels::authenticate(db,&c.channel_id,c.secret(),true)?;
                        // A new denial stops a running writer; a lifted one (fewer denials) does not.
                        if !readonly && (access::current(db,&i.task)?!=expected || denials(db,&i.task)?.iter().any(|p|!denied.contains(p))){return Err(CoreError::Refused("access_changed"));}
                        if !readonly {writable(db,&i,None)?;} Ok(())
                    })?).await;
                    if valid.is_err(){failure=Some("authority_changed");break None;}
                }
            }
        };
        group.kill();let _=child.wait().await;
        let drained=tokio::time::timeout(Duration::from_secs(2),async{(&mut out).await.map_err(|_|CoreError::Refused("output_lost"))??;(&mut err).await.map_err(|_|CoreError::Refused("output_lost"))??;Ok::<_,CoreError>(())}).await;
        if !matches!(drained,Ok(Ok(()))){out.abort();err.abort();failure=Some("output_incomplete");}
        Ok::<_,CoreError>((code,failure))
    }.await;
    let result=async {
    output.lock().await.sync_all()?;
    let (code,failure)=result.unwrap_or((None,Some("spawn_or_capture_failed")));
    let mut retained=File::open(output_dir.join(&file))?;let (full,size)=fingerprint(&mut retained)?;
    retained.seek(SeekFrom::Start(0))?;let mut prefix=Vec::new();retained.take(65536).read_to_end(&mut prefix)?;
    let inline=String::from_utf8_lossy(&prefix).into_owned();
    let ended_digest = check_digest(root, &argv).await;
    let revision_digest = if started_digest == ended_digest { ended_digest } else { None };
    let value=json!({"revision_digest":revision_digest,"tool_call_ref":reference,"argv":argv,"cwd":cwd,"profile":profile,"access_mode":mode,"exit_code":code,
        "complete":failure.is_none()&&code.is_some(),"settled":true,"failure":failure,"output":inline,"output_truncated":size>65536,"byte_count":size,"large_output_id":id});
    let c=credential.clone();let v=value.clone();let n=name;let task=i.task;let key=args["request_key"].as_str().unwrap().to_owned();
    let recorded=recv(executor.write(move|db|{
        db.execute("INSERT INTO large_outputs VALUES (?,?,?,'terminal',?,?,?,?,?,NULL,?,?)",
            rusqlite::params![id,task,c.channel_id,full,file,size as i64,0,code,failure,cortex_core::now()])?;
        db.execute("UPDATE runtime_requests SET result_json=? WHERE task_id=? AND actor=? AND request_key=? AND operation=?",
            (v.to_string(),&task,&c.channel_id,key,n))?;
        let mut observation=v.clone();observation.as_object_mut().unwrap().remove("output");
        lifecycle::event_tx(db,&task,"NATIVE_COMMAND_FINISHED",json!({"receipt":{"tool_call_ref":v["tool_call_ref"],"argv":v["argv"],"observation":observation}}))?;Ok(())
    })?).await;
    recorded?;
    Ok(crate::native_sources::reply(value))
    }.await;
    done.send_replace(true);
    running.lock().await.remove(&reference);
    result
}
