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
    if crate::native_engine_policy::project_private(&p) {
        return Err(CoreError::Refused("cortex_private"));
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
    let (mode,incoming,saved,who)=recv(executor.write(move|db|{
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
            return Ok((mode,None,Some(saved["reply"].clone()),who));
        }
        let sessions=db.prepare("SELECT session_id FROM acp_host_sessions WHERE task_id=? AND json_extract(client_info,'$.name')='cortex-rust-acp'")?
            .query_map([&i.task],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
        if sessions.len()>1 {return Err(CoreError::Refused("incoming_session_binding_conflict"));}
        db.execute("INSERT INTO runtime_requests VALUES (?,?,?,?,?,?)",
            (&i.task,&k,&c.channel_id,&n,&h,json!({"complete":false,"mode":mode}).to_string()))?;
        lifecycle::event_tx(db,&i.task,"NATIVE_OUTSIDE_REQUESTED",json!({"channel_id":c.channel_id,"operation":n,"request_key":k,"mode":mode}))?;
        Ok((mode,sessions.into_iter().next(),None,who))
    })?).await?;
    if let Some(saved) = saved {
        return Ok(saved);
    }
    let title = match rerouted.as_ref().and_then(|n| n["reason"].as_str()) {
        Some("privilege") => "Run on the host: the command uses sudo",
        Some("sandbox_unavailable") => "Run on the host: the command sandbox could not start",
        _ if write => "Write outside the project",
        _ => "Run outside the sandbox",
    };
    let request = json!({"toolCall":{"toolCallId":key,"title":title,
        "kind":if write {"edit"} else {"execute"},"rawInput":args},
        "options":[{"optionId":"allow","name":"Allow once","kind":"allow_once"},{"optionId":"deny","name":"Deny","kind":"reject_once"}]});
    let selected_control_plane = protected_roots.iter().any(|root| {
        root.canonicalize()
            .map(|p| path.starts_with(p))
            .unwrap_or(true)
    });
    // The access mode's decision table, as for native engine requests: Cortex
    // private state is refused in every mode; Auto allows and records the rest;
    // Guarded and Development ask the owner on the same decision card (a write
    // outside the project asks in Development too). The scope, path-safety and
    // consent checks above stay as they are.
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
        PathClass::Private => "cortex_private",
        PathClass::Credential => "credential",
        PathClass::Control => "control_target",
        PathClass::Ordinary if write => "outside_write",
        PathClass::Ordinary => "outside_command",
    };
    use crate::native_owner_decisions::Owner;
    let (approved, decider) = if class == PathClass::Private {
        (false, "runtime".to_owned())
    } else if mode == "auto" {
        (true, format!("mode:{mode}"))
    } else {
        let card = cortex_engine::permission::owner_card(&request, &who, rule);
        let details = crate::native_owner_decisions::details(&request, &who, rule, &mode);
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
        if class == PathClass::Private {
            return Err(CoreError::Refused("cortex_private"));
        }
        if !approved {
            return Err(CoreError::Refused("outside_action_not_confirmed"));
        }
        let c = credential.clone();
        let expected = mode.clone();
        recv(executor.write(move |db| {
            let i = writer(db, &c)?;
            if access::current(db, &i.task)? != expected {
                return Err(CoreError::Refused("access_mode_changed"));
            }
            Ok(())
        })?)
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
                let i = writer(db, &c)?;
                if access::current(db, &i.task)? != expected {
                    return Err(CoreError::Refused("access_mode_changed"));
                }
                atomic_write(&root, &args, &data)
            })?).await
        } else {
            let (cancel, cancelled) = tokio::sync::watch::channel(false);
            let pending = crate::native_commands::outside_capture(
                executor.clone(),credential.clone(),running,path,&args,cancelled,rerouted.clone(),
            );
            tokio::pin!(pending);
            loop {
                tokio::select! {
                    result=&mut pending=>break result,
                    _=tokio::time::sleep(std::time::Duration::from_millis(50))=>{
                        let c=credential.clone();let expected=mode.clone();
                        if recv(executor.write(move|db|{let i=writer(db,&c)?;if access::current(db,&i.task)?!=expected{return Err(CoreError::Refused("access_mode_changed"));}Ok(())})?).await.is_err() {
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
        == cortex_engine::permission::PathClass::Private
    {
        return Err(CoreError::Refused("cortex_private"));
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
