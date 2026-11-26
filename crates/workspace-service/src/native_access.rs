//! Outside effects require a live writing channel and durable one-shot identity.
//! These are unsandboxed owner-policy routes; they do not retry a started payload.
use crate::native_sessions::ClientPermissions;
use cortex_core::{
    access,
    channels::{self, Credential, Identity},
    lifecycle::{self, waits},
    scheduler::policy,
    store::executor::StoreExecutor,
    CoreError, CoreResult,
};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    path::{Component, Path, PathBuf},
    sync::Arc,
};

pub(super) fn supports(name: &str) -> bool {
    matches!(name, "outside_command" | "outside_write")
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
async fn recv<T>(r: tokio::sync::oneshot::Receiver<CoreResult<T>>) -> CoreResult<T> {
    r.await
        .map_err(|_| CoreError::Refused("native_host_closed"))?
}
fn writer(db: &rusqlite::Transaction<'_>, c: &Credential) -> CoreResult<Identity> {
    let i = channels::authenticate(db, &c.channel_id, c.secret(), true)?;
    if i.role != "worker"
        || policy::read_only(
            i.profile.as_deref().unwrap_or(""),
            i.stage.as_deref().unwrap_or(""),
        )
    {
        return Err(CoreError::Refused("writing_worker_required"));
    }
    let assignment = i
        .assignment
        .as_deref()
        .ok_or(CoreError::Refused("writing_worker_required"))?;
    let scope: String = db.query_row(
        "SELECT scope_json FROM runtime_nodes WHERE task_id=? AND assignment_ref=?",
        (&i.task, assignment),
        |r| r.get(0),
    )?;
    if serde_json::from_str::<Vec<String>>(&scope)?.is_empty() {
        return Err(CoreError::Refused("writing_scope_required"));
    }
    waits::check_mutation_tx(db, &i.task, assignment)?;
    Ok(i)
}
pub(super) fn still_confirmed(
    db: &rusqlite::Transaction<'_>,
    credential: &Credential,
    expected_mode: &str,
) -> CoreResult<()> {
    let identity = writer(db, credential)?;
    if access::current(db, &identity.task)? != expected_mode {
        return Err(CoreError::Refused("access_mode_changed"));
    }
    Ok(())
}

fn absolute(path: &Path) -> CoreResult<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(CoreError::Refused("outside_path_must_be_absolute"));
    }
    Ok(())
}
pub(super) fn protected(path: &Path) -> bool {
    path.components().any(|c| {
        matches!(c, Component::Normal(s) if [
        ".git",".gitconfig",".git-credentials",".cortex",".cortex-dev",".codex",".claude",
        ".ssh",".gnupg",".paseo",".paseo-dev","cortex","cortex-dev","Paseo","Paseo-dev",
        ".cortex-dev-owner.json",".cortex-workspace-dev.json",
    ].iter().any(|p|s==*p))
    })
}
/// Reject symlink aliases rather than risk crossing the project/control-plane boundary.
fn target(root: &Path, args: &Value, write: bool) -> CoreResult<PathBuf> {
    let p = PathBuf::from(
        if write {
            args["path"].as_str()
        } else {
            args["cwd"].as_str()
        }
        .unwrap_or_else(|| root.to_str().unwrap()),
    );
    absolute(&p)?;
    if write && crate::native_engine_policy::live_store_file(&p) {
        return Err(CoreError::Refused("cortex_live_store"));
    }
    let mut at = PathBuf::from("/");
    for part in p.components().skip(1) {
        at.push(part.as_os_str());
        match at.symlink_metadata() {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(CoreError::Refused("outside_symlink_refused"))
            }
            Ok(_) => {}
            Err(e) if write && at == p && e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(CoreError::Refused("outside_path_unavailable")),
        }
    }
    if write {
        if p.starts_with(root) {
            return Err(CoreError::Refused("outside_path_inside_project"));
        }
        if !p.parent().is_some_and(Path::is_dir) {
            return Err(CoreError::Refused("outside_parent_unavailable"));
        }
    } else if !p.is_dir() {
        return Err(CoreError::Refused("outside_cwd_unavailable"));
    }
    Ok(p)
}
#[allow(clippy::too_many_arguments)]
pub(super) async fn execute(
    executor: Arc<StoreExecutor>,
    credential: Credential,
    root: PathBuf,
    permissions: Option<Arc<dyn ClientPermissions>>,
    protected_roots: Vec<PathBuf>,
    running: crate::native_commands::Running,
    name: &str,
    args: Value,
    scoped: bool,
) -> CoreResult<Value> {
    execute_as(
        executor,
        credential,
        root,
        permissions,
        protected_roots,
        running,
        name,
        args,
        None,
        scoped,
    )
    .await
}
/// A writing worker's `command_run` that needs the host, run by the runtime through
/// this same consent route (`native_commands::reroute`). `note` says why and is kept
/// on the receipt; the consent card names the reason.
#[allow(clippy::too_many_arguments)]
pub(super) async fn execute_rerouted(
    executor: Arc<StoreExecutor>,
    credential: Credential,
    root: PathBuf,
    permissions: Option<Arc<dyn ClientPermissions>>,
    protected_roots: Vec<PathBuf>,
    running: crate::native_commands::Running,
    args: Value,
    note: Value,
) -> CoreResult<Value> {
    execute_as(
        executor,
        credential,
        root,
        permissions,
        protected_roots,
        running,
        "outside_command",
        args,
        Some(note),
        false,
    )
    .await
}
#[allow(clippy::too_many_arguments)]
async fn execute_as(
    executor: Arc<StoreExecutor>,
    credential: Credential,
    root: PathBuf,
    permissions: Option<Arc<dyn ClientPermissions>>,
    protected_roots: Vec<PathBuf>,
    running: crate::native_commands::Running,
    name: &str,
    args: Value,
    rerouted: Option<Value>,
    scoped: bool,
) -> CoreResult<Value> {
    let write = name == "outside_write";
    let path = target(&root, &args, write)?;
    let name = name.to_owned();
    let key = args["request_key"]
        .as_str()
        .ok_or(CoreError::Refused("invalid_request"))?
        .to_owned();
    let hash = digest(&serde_json::to_vec(&json!([&name, &args]))?);
    let c = credential.clone();
    let k = key.clone();
    let h = hash.clone();
    let n = name.clone();
    let (mode,incoming,saved,who,ru)=recv(executor.write(move|db|{
        let i=writer(db,&c)?;
        // Assignment scope grants project-relative targets only. No mode or
        // outside consent can widen it for a managed Codex channel.
        if scoped {
            return Err(CoreError::Refused(if write {
                "write_outside_assignment_scope"
            } else {
                "managed_codex_host_command_refused"
            }));
        }
        let who=i.profile.clone().unwrap_or_else(||i.role.clone());
        let old:Option<(String,String,String)>=db.query_row("SELECT operation,digest,result_json FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=?",
            (&i.task,&c.channel_id,&k),|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        let mode=access::current(db,&i.task)?;
        if let Some((op,d,result))=old {
            if op!=n || d!=h {return Err(CoreError::Refused("request_key_conflict"));}
            let saved:Value=serde_json::from_str(&result)?;
            if saved["complete"]!=true {return Err(CoreError::Refused("outside_outcome_unknown"));}
            return Ok((mode,None,Some(saved["reply"].clone()),who,false));
        }
        let sessions=db.prepare("SELECT session_id FROM acp_host_sessions WHERE task_id=? AND json_extract(client_info,'$.name')='cortex-rust-acp'")?
            .query_map([&i.task],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
        if sessions.len()>1 {return Err(CoreError::Refused("incoming_session_binding_conflict"));}
        db.execute("INSERT INTO runtime_requests VALUES (?,?,?,?,?,?)",
            (&i.task,&k,&c.channel_id,&n,&h,json!({"complete":false,"mode":mode}).to_string()))?;
        lifecycle::event_tx(db,&i.task,"NATIVE_OUTSIDE_REQUESTED",json!({"channel_id":c.channel_id,"operation":n,"request_key":k,"mode":mode}))?;
        let ru = lifecycle::domain::task_language_tx(db, &i.task)? == "ru";
        Ok((mode,sessions.into_iter().next(),None,who,ru))
    })?).await?;
    if let Some(saved) = saved {
        return Ok(saved);
    }
    let cause = rerouted.as_ref().and_then(|n| n["reason"].as_str());
    let title = match (cause, write, ru) {
        (Some("privilege"), _, false) => "Run on the host: the command uses sudo",
        (Some("privilege"), _, true) => "Запуск на компьютере: команда использует sudo",
        (Some("sandbox_unavailable"), _, false) => "Run on the host: the command sandbox could not start",
        (Some("sandbox_unavailable"), _, true) => "Запуск на компьютере: не удалось запустить песочницу",
        (_, true, false) => "Write outside the project",
        (_, true, true) => "Запись за пределами проекта",
        (_, false, false) => "Run outside the sandbox",
        (_, false, true) => "Запуск вне песочницы",
    };
    let request = json!({"toolCall":{"toolCallId":key,"title":title,
        "kind":if write {"edit"} else {"execute"},"rawInput":args},
        "options":cortex_engine::permission::one_shot_options(ru)});
    let selected_control_plane = protected_roots.iter().any(|root| {
        root.canonicalize()
            .map(|p| path.starts_with(p))
            .unwrap_or(true)
    });
    // Auto decides eligible outside effects, Cortex private state included; all
    // modes refuse writes to a live store file.
    use cortex_engine::permission::PathClass;
    let class = match crate::native_engine_policy::path_class(&root, &path) {
        PathClass::Ordinary
            if selected_control_plane
                || protected(&path)
                || crate::native_command_sandbox::policy::protected_target(&root, &path) =>
        {
            PathClass::Control
        }
        class => class,
    };
    let rule = match class {
        PathClass::LiveStore => "cortex_live_store",
        PathClass::Private => "cortex_private",
        PathClass::Credential => "credential",
        PathClass::Control => "control_target",
        PathClass::Ordinary if write => "outside_write",
        PathClass::Ordinary => "outside_command",
    };
    use crate::native_owner_decisions::Owner;
    let (approved, decider) = if class == PathClass::LiveStore && write {
        (false, "runtime".to_owned())
    } else if mode == "auto" {
        (true, format!("mode:{mode}"))
    } else {
        let mut card = cortex_engine::permission::owner_card(&request, &who, rule, ru);
        // Preserve the actual reason this command was rerouted, even when cwd is present.
        if matches!(cause, Some("privilege" | "sandbox_unavailable")) {
            let prompt = card["toolCall"]["title"].as_str().unwrap_or_default();
            let separator = if ru { " " } else { ". " };
            card["toolCall"]["title"] = json!(format!("{prompt}{separator}{title}."));
        }
        let details = crate::native_owner_decisions::details(&request, &who, rule, &mode, ru);
        match crate::native_owner_decisions::ask(
            &executor,
            &credential,
            permissions,
            incoming,
            &key,
            &card,
            details,
        )
        .await
        {
            Ok(Owner::Allowed) => (true, "owner".to_owned()),
            Ok(Owner::Denied) => (false, "owner".to_owned()),
            Ok(Owner::Withdrawn(reason)) => (false, format!("withdrawn:{reason}")),
            Ok(Owner::Unavailable) => (false, "no_owner_client".to_owned()),
            Err(e) => (false, format!("error:{}", e.code())),
        }
    };
    let result = async {
        if class == PathClass::LiveStore && write {
            return Err(CoreError::Refused("cortex_live_store"));
        }
        if !approved {
            return Err(CoreError::Refused("outside_action_not_confirmed"));
        }
        let c = credential.clone();
        let expected = mode.clone();
        recv(executor.write(move |db| still_confirmed(db, &c, &expected))?)
        .await?;
        if write {
            let root = root.clone();
            let data = args["content"]
                .as_str()
                .ok_or(CoreError::Refused("invalid_content"))?
                .as_bytes()
                .to_vec();
            let args = args.clone();
            let c = credential.clone();
            let expected = mode.clone();
            recv(executor.write(move |db| {
                still_confirmed(db, &c, &expected)?;
                atomic_write(&root, &args, &data)
            })?).await
        } else {
            let (cancel, cancelled) = tokio::sync::watch::channel(false);
            let pending = crate::native_commands::outside_capture(
                executor.clone(), credential.clone(), running, path, &args, &mode, cancelled,
                rerouted,
            );
            tokio::pin!(pending);
            loop {
                tokio::select! {
                    result=&mut pending=>break result,
                    _=tokio::time::sleep(std::time::Duration::from_millis(50))=>{
                        let c=credential.clone();let expected=mode.clone();
                        if recv(executor.write(move|db|still_confirmed(db, &c, &expected))?).await.is_err() {
                            cancel.send_replace(true);
                        }
                    }
                }
            }
        }
    }
    .await;
    // Retain refusals too: denial or an uncertain effect must never silently rerun.
    let reply = match result {
        Ok(value) => {
            json!({"content":[{"type":"text","text":value.to_string()}],"structuredContent":value,"isError":false})
        }
        Err(e) => json!({"content":[{"type":"text","text":e.code()}],"isError":true}),
    };
    let c = credential;
    let v = reply.clone();
    recv(executor.write(move|db|{
        let i=channels::authenticate(db,&c.channel_id,c.secret(),true)?;
        db.execute("UPDATE runtime_requests SET result_json=? WHERE task_id=? AND actor=? AND request_key=? AND operation=? AND digest=?",
            (json!({"complete":true,"reply":v}).to_string(),&i.task,&c.channel_id,&key,&name,&hash))?;
        lifecycle::event_tx(db,&i.task,"NATIVE_OUTSIDE_SETTLED",json!({"channel_id":c.channel_id,"operation":name,"request_key":key,"mode":mode,"allowed":approved,"rule":rule,"decider":decider}))?;
        Ok(())
    })?).await?;
    Ok(reply)
}
fn atomic_write(root: &Path, args: &Value, data: &[u8]) -> CoreResult<Value> {
    use std::{
        ffi::CString,
        fs::File,
        io::Write,
        os::fd::{AsRawFd, FromRawFd},
    };
    let p = target(root, args, true)?;
    if crate::native_engine_policy::path_class(root, &p)
        == cortex_engine::permission::PathClass::LiveStore
    {
        return Err(CoreError::Refused("cortex_live_store"));
    }
    let mut folder = File::open("/")?;
    let parent = p
        .parent()
        .ok_or(CoreError::Refused("outside_parent_unavailable"))?;
    for c in parent.components().skip(1) {
        let part = CString::new(c.as_os_str().as_encoded_bytes())
            .map_err(|_| CoreError::Refused("invalid_path"))?;
        let fd = unsafe {
            libc::openat(
                folder.as_raw_fd(),
                part.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(CoreError::Refused("outside_parent_unavailable"));
        }
        folder = unsafe { File::from_raw_fd(fd) };
    }
    let name = CString::new(p.file_name().unwrap().as_encoded_bytes())
        .map_err(|_| CoreError::Refused("invalid_path"))?;
    let temp = CString::new(format!(
        ".cortex-write-{}",
        crate::native_effects::identity()?
    ))
    .unwrap();
    let fd = unsafe {
        libc::openat(
            folder.as_raw_fd(),
            temp.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        return Err(CoreError::Refused("outside_write_unavailable"));
    }
    let mut file = unsafe { File::from_raw_fd(fd) };
    let effect = (|| -> CoreResult<()> {
        file.write_all(data)?;
        file.sync_all()?;
        if unsafe {
            libc::renameat(
                folder.as_raw_fd(),
                temp.as_ptr(),
                folder.as_raw_fd(),
                name.as_ptr(),
            )
        } < 0
        {
            return Err(CoreError::Refused("outside_write_unavailable"));
        }
        folder.sync_all()?;
        Ok(())
    })();
    if effect.is_err() {
        unsafe { libc::unlinkat(folder.as_raw_fd(), temp.as_ptr(), 0) };
    }
    effect?;
    Ok(json!({"path":p,"byte_count":data.len(),"sha256":digest(data)}))
}

/// A gateway file call (`project_read`, `project_write`, `project_edit`) on a
/// protected project path, decided by the runtime before the call runs: a read of
/// Git control files is an ordinary read; a write of a live store file is refused
/// in every mode (`cortex_live_store`); any other access to Cortex private state
/// and a write of Git control files or of a credential file are allowed on request: Auto allows, Guarded
/// and Development ask the owner (Allow once / Deny). Each decision is one
/// `NATIVE_PROTECTED_PATH_DECIDED` event. `Ok(())` admits exactly this call.
pub(super) async fn admit_protected(
    executor: Arc<StoreExecutor>,
    credential: Credential,
    root: PathBuf,
    permissions: Option<Arc<dyn ClientPermissions>>,
    name: &str,
    args: &Value,
    protection: crate::native_project::Protected,
) -> CoreResult<()> {
    use crate::native_project::Protected;
    let read = name == "project_read";
    let path = args["path"]
        .as_str()
        .ok_or(CoreError::Refused("invalid_arguments"))?
        .to_owned();
    let rule = match (protection, read) {
        // Reading Git control files or a credential file is an ordinary gateway read.
        (Protected::Git | Protected::Credential, true) => return Ok(()),
        (Protected::Credential, false) => "credential",
        (Protected::LiveStore, false) => "cortex_live_store",
        (Protected::Git, false) => "control_target",
        (Protected::Private | Protected::LiveStore, _) => "cortex_private",
    };
    let key = match args["request_key"].as_str() {
        Some(key) => format!("protected:{key}"),
        None => format!("protected:{}", digest(&serde_json::to_vec(&json!([name, args]))?)),
    };
    let c = credential.clone();
    let (mode, incoming, who, ru) = recv(executor.write(move |db| {
        let i = channels::authenticate(db, &c.channel_id, c.secret(), true)?;
        let sessions = db.prepare("SELECT session_id FROM acp_host_sessions WHERE task_id=? AND json_extract(client_info,'$.name')='cortex-rust-acp'")?
            .query_map([&i.task], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
        if sessions.len() > 1 {
            return Err(CoreError::Refused("incoming_session_binding_conflict"));
        }
        let ru = lifecycle::domain::task_language_tx(db, &i.task)? == "ru";
        Ok((access::current(db, &i.task)?, sessions.into_iter().next(),
            i.profile.clone().unwrap_or_else(|| i.role.clone()), ru))
    })?).await?;
    let absolute = root.join(&path);
    let request = json!({"toolCall":{"toolCallId":key,"title":name,
        "kind":if read {"read"} else {"edit"},"locations":[{"path":absolute}],
        "rawInput":{"path":absolute}},
        "options":cortex_engine::permission::one_shot_options(ru)});
    use crate::native_owner_decisions::Owner;
    let (allowed, decider) = if rule == "cortex_live_store" {
        (false, "runtime".to_owned())
    } else if mode == "auto" {
        (true, format!("mode:{mode}"))
    } else {
        let card = cortex_engine::permission::owner_card(&request, &who, rule, ru);
        let details = crate::native_owner_decisions::details(&request, &who, rule, &mode, ru);
        match crate::native_owner_decisions::ask(
            &executor, &credential, permissions, incoming, &key, &card, details,
        )
        .await
        {
            Ok(Owner::Allowed) => (true, "owner".to_owned()),
            Ok(Owner::Denied) => (false, "owner".to_owned()),
            Ok(Owner::Withdrawn(reason)) => (false, format!("withdrawn:{reason}")),
            Ok(Owner::Unavailable) => (false, "no_owner_client".to_owned()),
            Err(e) => (false, format!("error:{}", e.code())),
        }
    };
    let c = credential;
    let operation = name.to_owned();
    let expected = mode.clone();
    recv(executor.write(move |db| {
        let i = channels::authenticate(db, &c.channel_id, c.secret(), true)?;
        // An allowance holds only under the access mode it was decided in.
        if allowed && access::current(db, &i.task)? != expected {
            return Err(CoreError::Refused("access_mode_changed"));
        }
        lifecycle::event_tx(db, &i.task, "NATIVE_PROTECTED_PATH_DECIDED", json!({
            "channel_id": c.channel_id, "operation": operation, "path": path,
            "mode": expected, "allowed": allowed, "rule": rule, "decider": decider}))?;
        Ok(())
    })?).await?;
    if allowed {
        Ok(())
    } else if rule == "cortex_live_store" {
        Err(CoreError::Refused("cortex_live_store"))
    } else {
        Err(CoreError::Refused("protected_path_not_confirmed"))
    }
}
