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
/// The cause of a store-side refusal before the payload was released.
fn store_cause(e: &CoreError) -> String {
    match e.code() {
        "busy" | "scheduler_owner_busy" => "store_busy".into(),
        "storage_unavailable" => "store_unavailable".into(),
        code => format!("refused:{code}"),
    }
}
/// A supervisor's preparation failure code, accepted only in its fixed vocabulary.
fn launch_cause(failed: &str) -> String {
    let code = |s: &str| {
        !s.is_empty()
            && s.len() <= 64
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b':')
    };
    if code(failed) {
        failed.into()
    } else {
        "sandbox_preparation_failed".into()
    }
}
fn directory(store: &Store, task: &str) -> PathBuf {
    store.key().directory().join(task).join(".native-output")
}

/// Whether a lease-backed command keeps its authority after one check: a definite
/// refusal revokes it; an unanswered check (busy store) holds for a bounded grace.
#[cfg(test)]
fn lease_valid(outcome: &Result<CoreResult<()>, tokio::time::error::Elapsed>, since_confirmed: Duration) -> bool {
    match outcome {
        Ok(result) => result.is_ok(),
        Err(_) => since_confirmed < LEASE_GRACE,
    }
}
/// How long an unanswered (not refused) authority check may keep a lease alive.
#[cfg(test)]
const LEASE_GRACE: Duration = Duration::from_secs(20);
/// Only this evidence permits automatic host routing after a sandbox attempt.
/// It is written after an explicit launcher failure on the supervisor's private
/// control pipe, before the final `go`. Payload stdout/stderr never reach it.
pub(super) const PRESTART_LAUNCHER_FAILURE: &str = "supervisor_launcher_failed_before_release";

/// Automatic launch attempts after the first, for a preparation that failed before the
/// payload was released.
const LAUNCH_RETRIES: u32 = 2;
/// Transient preparation failures that provably precede the payload release: a failed
/// supervisor spawn or a preparation I/O error (a permission refusal is not transient).
fn prestart_retryable(cause: Option<&str>) -> bool {
    cause.is_some_and(|c| {
        c == "supervisor_spawn_failed"
            // A concurrent writer touched a hard-linked file during detachment: the next
            // preparation rescans the checkout and detaches the current versions.
            || matches!(
                c,
                "sandbox_unavailable:command_hardlink_changed"
                    | "sandbox_unavailable:command_hardlink_materialization_timeout"
            )
            || (c.starts_with("preparation_io_") && c != "preparation_io_PermissionDenied")
    })
}
fn launcher_failure(cause: Option<&str>) -> bool {
    matches!(
        cause,
        Some(
            "sandbox_unavailable:landlock_unavailable"
                | "sandbox_unavailable:landlock_launch_failed"
                | "sandbox_unavailable:landlock_probe_failed"
                | "sandbox_unavailable:setup_timeout"
        )
    )
}

/// Output signatures consistent with a refused nested user namespace. Because a
/// payload can print these strings itself, matches are diagnostic only and never
/// establish why a run failed or that it had no effects.
const NESTED_SANDBOX_DENIALS: &[&str] = &[
    "bwrap: No permissions to cre",
    "bwrap: setting up uid map: Permission denied",
    "bwrap: Creating new namespace failed",
    "unshare: unshare failed: Operation not permitted",
    "cannot clone: Operation not permitted",
    "No usable sandbox!",
];

/// Advisory hint for output consistent with a refused nested sandbox.
pub(super) fn nested_sandbox_hint(profile: &str, output: &[&[u8]]) -> Option<Value> {
    let signature = NESTED_SANDBOX_DENIALS.iter().find(|s| {
        output
            .iter()
            .any(|chunk| chunk.windows(s.len()).any(|w| w == s.as_bytes()))
    })?;
    let writer = matches!(profile, "development" | "guarded" | "auto");
    Some(json!({
        "code": "nested_sandbox_denied",
        "signature": signature,
        "explanation": "The output matches a known nested-sandbox denial. It does not prove that the command had no effects.",
        "outside_command": if writer { "offered" } else { "not_offered" },
        "next": if writer {
            "The runtime does not replay this command. Check for prior effects before considering a separate outside_command request under its normal authorization rules."
        } else {
            "This role cannot run commands outside the sandbox: report the check as not run with this blocker, or leave it to a writing specialist."
        },
    }))
}
/// Output signatures of a privileged program (`sudo`, `pkexec`, `doas`) refused root
/// inside the command sandbox, which runs with no_new_privs and nosuid mounts.
const PRIVILEGE_DENIALS: &[&str] = &[
    "\"no new privileges\" flag is set",
    "sudo: effective uid is not 0",
    "pkexec must be setuid root",
    "doas: not installed setuid",
];

/// Advisory hint for output consistent with a privileged program refused in the sandbox.
pub(super) fn privilege_hint(profile: &str, output: &[&[u8]]) -> Option<Value> {
    let signature = PRIVILEGE_DENIALS.iter().find(|s| {
        output
            .iter()
            .any(|chunk| chunk.windows(s.len()).any(|w| w == s.as_bytes()))
    })?;
    let writer = matches!(profile, "development" | "guarded" | "auto");
    Some(json!({
        "code": "privilege_refused_in_sandbox",
        "signature": signature,
        "explanation": "The output matches a known refusal of privileges inside the sandbox. It does not prove that the command had no effects.",
        "next": if writer {
            "The runtime does not replay this command on the host. Check for prior effects before considering a separate outside_command request under its normal authorization rules."
        } else {
            "This role cannot run privileged commands: report the check as not run with this blocker, or leave it to a writing specialist."
        },
    }))
}
/// A write below `.cortex` or `.cortex-dev` that the command view refused. Guarded and
/// Development keep those directories read-only for shell commands: a command cannot
/// pause for a per-write owner prompt. The file tools can, so they are the route.
pub(super) fn cortex_store_hint(mode: &str, argv: &Value, output: &[&[u8]]) -> Option<Value> {
    if mode == "auto" || mode == "read_only" {
        return None;
    }
    let has = |chunk: &[u8], needle: &[u8]| chunk.windows(needle.len()).any(|w| w == needle);
    let read_only = output
        .iter()
        .any(|c| has(c, b"Read-only file system") || has(c, b"Permission denied"));
    let named = argv.to_string().contains(".cortex")
        || output.iter().any(|c| has(c, b".cortex"));
    (read_only && named).then(|| json!({
        "code": "cortex_store_read_only_for_commands",
        "explanation": "In this access mode .cortex and .cortex-dev are read-only inside shell commands; the live store files are read-only in every mode.",
        "next": "Use project_write or project_edit for .cortex files; the runtime will ask the owner. Do not retry the shell write.",
    }))
}
/// A program the command (or a script it ran) could not find: the supervisor's
/// `cortex: execvp NAME: No such file or directory`, or exit 127 with a shell's or `env`'s
/// "not found". The program is named only when it is a plain short name.
pub(super) fn missing_program_hint(exit_code: Option<i64>, output: &[&[u8]]) -> Option<Value> {
    let text: String = output
        .iter()
        .map(|chunk| String::from_utf8_lossy(chunk))
        .collect();
    let plain = |name: &str| {
        let name = name
            .trim()
            .trim_matches(|c| c == '\'' || c == '"' || c == '`');
        (!name.is_empty()
            && name.len() <= 64
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._+-/".contains(&b)))
        .then(|| name.to_owned())
    };
    let execvp = text.lines().find_map(|line| {
        line.trim()
            .strip_prefix("cortex: execvp ")?
            .strip_suffix(": No such file or directory")
            .map(|name| plain(name))
    });
    let program = match execvp {
        Some(name) => name,
        None if exit_code == Some(127) => {
            let found = text.lines().rev().find_map(|line| {
                let line = line.trim();
                let head = line
                    .strip_suffix(": command not found")
                    .or_else(|| line.strip_suffix(": not found"))
                    .or_else(|| {
                        // Only `env` reports a missing program this way; a missing file
                        // named by any other program is not a missing program.
                        let head = line.strip_suffix(": No such file or directory")?;
                        let (tool, _) = head.split_once(": ")?;
                        (tool == "env" || tool.ends_with("/env")).then_some(head)
                    })?;
                Some(head.rsplit(": ").next().and_then(plain))
            })?;
            found
        }
        None => return None,
    };
    Some(json!({
        "code": "program_not_found",
        "program": program,
        "explanation": "The program was not found on the command search path: the project's .venv/bin, venv/bin and node_modules/.bin, then the owner's tool search path (their shell's PATH, folders they added, the existing usual per-user tool folders such as ~/.local/bin, ~/.cargo/bin and the default nvm, fnm or Volta Node, then the system folders). It is not installed where commands look, so repeating the command will not change it.",
        "next": "Use the project's own copy if it has one (for example a node_modules/.bin tool), or report the check as not run with this blocker; the owner can add the program's folder under Diagnostics > Tool search path.",
    }))
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
                    Some("call_ended_before_release"),
                    &Value::Null,
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
#[derive(Clone)]
pub(super) struct ScopedAuthority {
    pub identity: Identity,
    pub root: PathBuf,
    pub scopes: Vec<String>,
}

impl ScopedAuthority {
    pub(super) fn check(
        &self,
        db: &rusqlite::Transaction<'_>,
        current: &Identity,
    ) -> CoreResult<()> {
        if current != &self.identity
            || native_commands::scoped_authority(db, current, &self.root)? != self.scopes
        {
            return Err(CoreError::Refused("channel_or_access_changed"));
        }
        Ok(())
    }
}

/// How often a running command's authority is confirmed (the supervisor lease is 2 s).
const AUTHORITY_POLL: Duration = Duration::from_millis(250);
async fn authorize(
    ex: &StoreExecutor,
    c: &Credential,
    mode: &str,
    denied: &[String],
    readonly: bool,
    reference: &str,
    preparation: bool,
    scoped: Option<&ScopedAuthority>,
) -> CoreResult<()> {
    let c = c.clone();
    let mode = mode.to_owned();
    let denied = denied.to_vec();
    let r = reference.to_owned();
    let scoped = scoped.cloned();
    recv(ex.write(move |db| {
        let i = channels::authenticate(db, &c.channel_id, c.secret(), true)?;
        // A running read-only command cannot write, so owner write denials and
        // access-mode changes made for other work never revoke it; only the
        // channel authentication above and a scoped authority do.
        let unrelated = readonly && !preparation;
        if !unrelated
            && (access::current(db, &i.task)? != mode || native_commands::denials(db, &i.task)? != denied)
        {
            return Err(CoreError::Refused("access_changed"));
        }
        if let Some(authority) = &scoped {
            authority.check(db, &i)?;
        } else if !readonly {
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
    changes: Option<Value>,
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
    (&mut file).take(65536).read_to_end(&mut prefix)?;
    // Errors usually come last: also look at the retained tail for the hint.
    let mut tail = Vec::new();
    if size > 65536 {
        file.seek(SeekFrom::Start(size.saturating_sub(65536).max(65536)))?;
        file.take(65536).read_to_end(&mut tail)?;
    }
    let code = status["exit_code"].as_i64();
    let signal = status["signal"].as_i64();
    let current = match h.get("checkout") {
        Some(binding) => crate::native_project::retained_checkout_matches(store, task, binding)?,
        None => true,
    };
    let revision = revision.filter(|_| current);
    let failure = if current { status["failure"].as_str().map(str::to_owned) }
        else { Some("worktree_binding_invalid".to_owned()) };
    let mut value = json!({"tool_call_ref":h["reference"],"argv":h["argv"],"cwd":h["cwd"],
        "profile":h["profile"],"access_mode":h["mode"],"revision_digest":revision,
        "exit_code":code,"signal":signal,"failure":failure,"settled":true,
        "complete":failure.is_none()&&code.is_some(),"output":String::from_utf8_lossy(&prefix),
        "output_truncated":size>65536,"byte_count":size,"large_output_id":h["id"]});
    if let Some(checkout) = h.get("checkout") {
        value["checkout"] = checkout.clone();
    }
    if let Some(hint) = nested_sandbox_hint(h["profile"].as_str().unwrap_or(""), &[&prefix, &tail])
    {
        value["nested_sandbox"] = hint;
    }
    if let Some(hint) = privilege_hint(h["profile"].as_str().unwrap_or(""), &[&prefix, &tail]) {
        value["sandbox_privilege"] = hint;
    }
    if let Some(hint) = missing_program_hint(code, &[&prefix, &tail]) {
        value["missing_program"] = hint;
    }
    if let Some(hint) = cortex_store_hint(h["mode"].as_str().unwrap_or(""), &h["argv"], &[&prefix, &tail]) {
        value["cortex_store"] = hint;
    }
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
        // Diagnosis only (kept out of the reply): how long the payload ran, in ms.
        if let Some(ms)=status["process_ms"].as_u64() {observation["timings"]=json!({"process_ms":ms});}
        // Which deliverable paths changed while it ran (`native_change_set`). A run
        // settled without its listings (recovered after a restart) has no record
        // unless it was read-only.
        let changes=changes.clone().unwrap_or_else(|| if h["profile"]=="read_only" || h["profile"]==crate::native_command_sandbox::CODEX_SCRATCH {crate::native_change_set::unchanged("read_only")}
            else {crate::native_change_set::incomplete("settled_without_listing")});
        crate::native_change_set::add(&mut observation,&changes);
        lifecycle::event_tx(db,task,"NATIVE_COMMAND_FINISHED",json!({"receipt":{
            "tool_call_ref":h["reference"],"argv":h["argv"],"observation":observation}}))?;
        if let Some(reference) = h["reference"].as_str() {
            crate::owner_command_output::coordinator_marker(db, task, actor, reference,
                if value["complete"] == true && code == Some(0) {"completed"} else {"failed"})?;
        }
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
    scoped: Option<ScopedAuthority>,
    operation: String,
    checkout: Option<Value>,
    launch: crate::native_change_set::Launch,
) -> CoreResult<Value> {
    let status_file = format!("{id}.status");
    let stopping = format!("{id}.stop");
    let lease = scoped.is_some() || checkout.is_some();
    // A read-only or Guarded command hides the checkout's secret files; unless its
    // grant is the whole checkout, the service's index names them (see
    // `native_sandbox::indexed_project_secrets`), so the supervisor never walks.
    let secrets = if matches!(profile.as_str(), "read_only" | "guarded") {
        let (r, w) = (root.clone(), writes.clone());
        tokio::task::spawn_blocking(move || {
            let r = r.canonicalize().ok()?;
            if w.iter().any(|w| w.canonicalize().is_ok_and(|w| r.starts_with(w))) {
                return None;
            }
            crate::native_sandbox::indexed_project_secrets(&r).ok()
        })
        .await
        .ok()
        .flatten()
    } else {
        None
    };
    let spec = Spec {
        root: root.clone(),
        cwd: cwd.clone(),
        argv: argv.clone(),
        profile: profile.clone(),
        auto: mode == "auto",
        checkout: checkout.clone(),
        writes,
        protected,
        secrets,
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
    // Why a launch never released its payload and how long each stage took: fixed
    // reason codes and numbers only, kept on the not-started receipt and the
    // process event for diagnosis.
    let mut cause: Option<String> = None;
    let mut timings = serde_json::Map::new();
    let millis = |since: std::time::Instant| json!(since.elapsed().as_millis() as u64);
    // The "before" digest and the dirty-path listing for the change record run beside
    // the preparation and the payload, never ahead of its release (see
    // `native_change_set::Before`); a read-only command cannot change deliverable
    // files and lists nothing.
    let before = {
        let (digest_root, digest_argv, unscoped) = (root.clone(), argv.clone(), scoped.is_none());
        crate::native_change_set::Before::start(
            launch.clone(),
            root.clone(),
            async move {
                if unscoped { native_commands::check_digest(digest_root, &digest_argv).await } else { None }
            },
            !(readonly || scoped.is_some() || checkout.is_some()),
        )
    };
    let launched = async {
    let mut attempts = 0u32;
    let mut retried: Vec<String> = Vec::new();
    let (child, messages, h, input) = loop {
    let attempt_result = async {
    let mut launcher = Command::from(crate::native_command_scope::launcher(&std::env::current_exe()?));
    // Read now, so a changed tool search path applies from the next command on.
    if let Some(path) = crate::tool_path::for_commands() {
        launcher.env("PATH", path);
    }
    let spawned = std::time::Instant::now();
    let mut child = launcher
        .arg(supervisor::MODE)
        .env_remove("CORTEX_CHANNEL_SECRET")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .inspect_err(|_| cause = Some("supervisor_spawn_failed".into()))?;
    let mut input = child.stdin.take().unwrap();
    input.write_all(&serde_json::to_vec(&spec)?).await?;
    input.write_all(b"\n").await?;
    let mut messages = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut authorizing = 0u64;
    let mut h;
    loop {
        // Each preparation step is normally well under a second. Under heavy host disk
        // load (observed on 2026-11-20) one step took longer than the former 15 seconds,
        // and the command was refused as if the sandbox were unavailable. The bound stays
        // finite: a supervisor that never answers still fails closed, before any launch.
        let line = match tokio::time::timeout(Duration::from_secs(60), messages.next_line()).await {
            Err(_) => {
                cause = Some("sandbox_preparation_timeout".into());
                timings.insert("supervisor_wait_ms".into(), millis(spawned));
                return Err(CoreError::Refused("native_command_sandbox_unavailable"));
            }
            Ok(Err(e)) => {
                cause = Some("supervisor_pipe_failed".into());
                return Err(e.into());
            }
            Ok(Ok(None)) => {
                cause = Some("supervisor_exited".into());
                return Err(CoreError::Refused("native_command_sandbox_unavailable"));
            }
            Ok(Ok(Some(line))) => line,
        };
        let v: Value = serde_json::from_str(&line)?;
        if let Some(object) = v["timings"].as_object() {
            for (stage, value) in object {
                if value.is_u64() {
                    timings.insert(stage.clone(), value.clone());
                }
            }
        }
        if let Some(failed) = v["failed"].as_str() {
            cause = Some(launch_cause(failed));
            return Err(CoreError::Refused("native_command_sandbox_unavailable"));
        }
        let committing = std::time::Instant::now();
        authorize(
            &ex,
            &c,
            &mode,
            &denied,
            readonly,
            &reference,
            v["authorize"] == true,
            scoped.as_ref(),
        )
        .await
        .inspect_err(|e| cause = Some(store_cause(e)))?;
        authorizing += committing.elapsed().as_millis() as u64;
        if v["ready"].is_object() {
            if supervisor::identity(child.id().unwrap()).as_ref() != Some(&v["ready"]) {
                cause = Some("supervisor_identity_mismatch".into());
                return Err(CoreError::Refused("command_handle_identity_mismatch"));
            }
            h = json!({"reference":reference,"identity":v["ready"],"status":status_file,"stop":stopping,
                "output":output,"id":id,"argv":argv,"cwd":cwd,"profile":profile,"mode":mode,"denied":denied});
            if let Some(binding) = &checkout { h["checkout"] = binding.clone(); }
            let handle = h.clone();
            let op = operation.clone();
            let t = i.task.clone();
            let actor = c.channel_id.clone();
            let key = args["request_key"].as_str().unwrap().to_owned();
            let committing = std::time::Instant::now();
            let mut stages = timings.clone();
            stages.insert("authorize_commit_ms".into(), json!(authorizing));
            let launch_authority = scoped.clone();
            let launch_credential = c.clone();
            let launch_mode = mode.clone();
            let launch_denied = denied.clone();
            let launch_checkout = checkout.clone();
            recv(ex.publication(move |store| {
                if let Some(binding) = &launch_checkout {
                    if !crate::native_project::retained_checkout_matches(store, &t, binding)? {
                        return Err(CoreError::Refused("worktree_binding_invalid"));
                    }
                }
                store.write(|db| {
                if let Some(authority) = &launch_authority {
                    let current = channels::authenticate(db, &launch_credential.channel_id, launch_credential.secret(), true)?;
                    if access::current(db, &current.task)? != launch_mode || native_commands::denials(db, &current.task)? != launch_denied {
                        return Err(CoreError::Refused("access_changed"));
                    }
                    authority.check(db, &current)?;
                }
                // A receipt the runtime already settled (its call was lost) never launches.
                let bound = db.execute("UPDATE runtime_requests SET result_json=json_set(result_json,'$.handle',json(?))
                    WHERE task_id=? AND actor=? AND request_key=? AND operation=?
                    AND json_extract(result_json,'$.settled') IS NOT 1",
                    (handle.to_string(),&t,&actor,&key,&op))?;
                if bound != 1 {
                    return Err(CoreError::Refused("command_outcome_unknown"));
                }
                stages.insert("bind_queued_ms".into(), json!(committing.elapsed().as_millis() as u64));
                lifecycle::event_tx(db,&t,"NATIVE_COMMAND_PROCESS",json!({
                    "ref":handle["reference"],"process_identity":handle["identity"],"supervised":true,
                    "timings":stages}))?;
                Ok(())
                })
            })?).await.inspect_err(|e| cause = Some(store_cause(e)))?;
            // The payload may run from here on: a "before" observation still running is
            // checked against this instant.
            launch.released();
            input.write_all(b"go\n").await?;
            break;
        }
        if v["authorize"] != true {
            cause = Some("invalid_supervisor_message".into());
            return Err(CoreError::Refused("invalid_command_supervisor"));
        }
        input.write_all(b"go\n").await?;
    }
    Ok::<_, CoreError>((child, messages, h, input))
    }
    .await;
    match attempt_result {
        Ok(v) => break v,
        Err(e) if attempts < LAUNCH_RETRIES && prestart_retryable(cause.as_deref()) => {
            // The payload was never released: preparing again cannot repeat an effect.
            attempts += 1;
            retried.push(cause.take().unwrap_or_default());
            tokio::time::sleep(Duration::from_millis(250 * u64::from(attempts))).await;
            let _ = e;
        }
        Err(e) => return Err(e),
    }
    };
    let authority_pipe = lease.then_some(input);
    if !retried.is_empty() {
        timings.insert("launch_retries".into(), json!(retried.len()));
    }
    Ok::<_, CoreError>((child, messages, h, authority_pipe))
    }
    .await;
    let (mut child, _messages, h, authority_pipe) = match launched {
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
            let cause = cause.unwrap_or_else(|| store_cause(&error));
            // A timeout, lost pipe or authority failure is not an explicit
            // launcher failure, even when we never released the payload.
            let evidence = if launcher_failure(Some(&cause)) {
                PRESTART_LAUNCHER_FAILURE
            } else {
                "payload_never_released"
            };
            let timings = Value::Object(timings);
            if let Ok(reply) = ex.write(move |db| {
                native_commands::settle_unrecorded_tx(
                    db,
                    &t,
                    &a,
                    &k,
                    Some(&r),
                    "command_not_started",
                    None,
                    evidence,
                    Some(&cause),
                    &timings,
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
        let mut authority_pipe = authority_pipe;
        let mut phase = "process_observation";
        let mut last_confirmed = std::time::Instant::now() - AUTHORITY_POLL;
        let mut confirmed = true;
        let result = async {
            let status = loop {
                let status = observe(&dir, &h)?;
                if status["state"] == "finished" {
                    break status;
                }
                let owner_stopped = *cancelled.borrow();
                // Process exit is observed every 50 ms, but the authority check is a
                // writer-lane transaction: running it at that rate per command starved
                // every owner-facing write. It is confirmed at `AUTHORITY_POLL`, well
                // inside the supervisor's two-second lease; an owner stop never waits.
                let confirm_due = last_confirmed.elapsed() >= AUTHORITY_POLL;
                let valid = if owner_stopped {
                    false
                } else if !confirm_due {
                    confirmed
                } else {
                    let authorization = authorize(
                        &ex,
                        &c,
                        &mode,
                        &denied,
                        readonly,
                        &reference,
                        false,
                        scoped.as_ref(),
                    );
                    let valid = if lease {
                        matches!(
                            tokio::time::timeout(Duration::from_secs(1), authorization).await,
                            Ok(Ok(()))
                        )
                    } else {
                        authorization.await.is_ok()
                    };
                    last_confirmed = std::time::Instant::now();
                    confirmed = valid;
                    valid
                };
                let renewed = if !confirm_due && !owner_stopped && valid {
                    true
                } else if let Some(pipe) = authority_pipe.as_mut().filter(|_| valid) {
                    matches!(
                        tokio::time::timeout(
                            Duration::from_millis(200),
                            pipe.write_all(&[supervisor::AUTHORITY_HEARTBEAT])
                        )
                        .await,
                        Ok(Ok(()))
                    )
                } else {
                    !lease
                };
                if !valid || !renewed {
                    // EOF is a fail-closed stop even if retaining a stop marker
                    // fails. A stuck monitor also stops renewing the lease.
                    if owner_stopped || !lease {
                        stop(&dir, &h)?;
                    }
                    authority_pipe.take();
                }
                tokio::select! { _=tokio::time::sleep(Duration::from_millis(50))=>{},
                _=cancelled.changed()=>{} }
            };
            phase = "settlement";
            let observed = before.finish().await;
            let (after, mut changes) = tokio::join!(
                async {
                    if scoped.is_some() {
                        None
                    } else {
                        native_commands::check_digest(root.clone(), &argv).await
                    }
                },
                async {
                    match &observed.listing {
                        None => crate::native_change_set::unchanged("read_only"),
                        Some(listed) => crate::native_change_set::record_with(
                            listed,
                            &crate::native_change_set::snapshot(root.clone()).await,
                            observed.touched.as_ref(),
                        ),
                    }
                }
            );
            crate::native_change_set::add(&mut changes, &observed.meta);
            let revision = if observed.digest == after { after } else { None };
            let actor = c.channel_id;
            let task = i.task;
            let key = args["request_key"].as_str().unwrap().to_owned();
            let h = h.clone();
            recv(ex.publication(move |s| {
                settle(s, &task, &actor, &key, &h, &status, revision, Some(changes))
            })?)
            .await
        }
        .await;
        drop(authority_pipe);
        // Legacy supervisors survive service death for observation. Scoped
        // ones stop without the live lease. Neither may invent a result.
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
        "SELECT result_json FROM runtime_requests WHERE task_id=? AND actor=? AND
         (operation='command_run' OR (operation='project_git' AND json_type(result_json,'$.checkout')='object'))
         AND json_extract(result_json,'$.tool_call_ref')=?",
        (task,channel,reference), |row| row.get::<_,String>(0)).optional()?))?
        .ok_or(CoreError::Refused("unknown_command_ref"))?;
    public_status(store, task, &serde_json::from_str(&raw)?).map(crate::native_sources::reply)
}

pub(super) async fn recover(runtime: &crate::native_runtime::Runtime) -> CoreResult<()> {
    let ex = runtime.host.executor.clone();
    let running = runtime.host.commands.clone();
    let rows=recv(ex.publication(|s|s.read(|db|Ok(db.prepare(
        "SELECT task_id,actor,request_key,result_json FROM runtime_requests
         WHERE (operation='command_run' OR (operation='project_git' AND json_type(result_json,'$.checkout')='object'))
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
                            let confirmed=settle(s,&t,&a,&k,&handle,&status,None,None).is_ok();
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
///   no member; otherwise it stays unknown (no profile runs in its own PID
///   namespace, so a lost supervisor alone proves nothing);
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
        WHERE task_id=? AND actor=? AND request_key=? AND (operation IN ('command_run','outside_command') OR (operation='project_git' AND json_type(result_json,'$.checkout')='object'))",
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
    let decision: Option<(&str, Option<i64>, &str)> = if matches!(operation.as_str(), "command_run" | "project_git") {
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
                if settle(store, task, actor, key, h, status, None, None).is_ok() {
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
            None,
            &Value::Null,
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
            WHERE (operation IN ('command_run','outside_command') OR (operation='project_git' AND json_type(result_json,'$.checkout')='object')) AND json_extract(result_json,'$.settled') IS NOT 1
            AND json_extract(result_json,'$.complete') IS NOT 1 ORDER BY rowid",
        )?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<Result<Vec<_>, _>>()?)
}

/// A supervisor that reported `finished` this long ago has delivered everything its
/// observer needs; an observer that has still not settled the receipt is hung.
const HUNG_AFTER_FINISHED: Duration = Duration::from_secs(120);
/// Whether this receipt's supervisor finished more than [`HUNG_AFTER_FINISHED`] ago
/// while the receipt is still unsettled (the watchdog case: the observing call is
/// stuck, so nothing else would ever settle it).
fn hung_after_finish(store: &Store, task: &str, actor: &str, key: &str) -> bool {
    let Ok(Some(raw)) = store.read(|db| {
        use rusqlite::OptionalExtension;
        Ok(db
            .query_row(
                "SELECT result_json FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=?",
                (task, actor, key),
                |r| r.get::<_, String>(0),
            )
            .optional()?)
    }) else {
        return false;
    };
    let Ok(v) = serde_json::from_str::<Value>(&raw) else { return false };
    if v["settled"] == true || !v["handle"].is_object() {
        return false;
    }
    let Ok(file) = leaf(&v["handle"], "status") else { return false };
    let path = directory(store, task).join(file);
    supervisor::status(&path).is_ok_and(|s| s["state"] == "finished")
        && std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age >= HUNG_AFTER_FINISHED)
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
            && !hung_after_finish(store, &task, &actor, &key)
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

    #[test]
    fn busy_authority_check_does_not_revoke_a_read_only_lease() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        let timed_out = rt.block_on(async {
            tokio::time::timeout(
                Duration::from_millis(1),
                std::future::pending::<CoreResult<()>>(),
            )
            .await
        });
        assert!(timed_out.is_err());
        assert!(lease_valid(&timed_out, Duration::from_secs(3)));
        assert!(!lease_valid(&timed_out, LEASE_GRACE));
        assert!(lease_valid(&Ok(Ok(())), LEASE_GRACE * 2));
        assert!(!lease_valid(&Ok(Err(CoreError::Refused("access_changed"))), Duration::ZERO));
    }

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
    /// The two observed shapes of a missing program (the supervisor's execvp report,
    /// and a shell's exit 127) name it; other failures and odd names do not.
    #[test]
    fn a_missing_program_is_named_with_the_search_path_it_was_looked_up_on() {
        let execvp = missing_program_hint(
            Some(1),
            &[b"cortex: execvp npm: No such file or directory\n"],
        )
        .unwrap();
        assert_eq!(execvp["code"], "program_not_found");
        assert_eq!(execvp["program"], "npm");
        assert!(execvp["explanation"]
            .as_str()
            .unwrap()
            .contains("~/.cargo/bin"));
        for (output, program) in [
            (&b"sh: 1: cargo: not found\n"[..], "cargo"),
            (b"bash: line 1: pnpm: command not found\n", "pnpm"),
            (b"/usr/bin/env: 'node': No such file or directory\n", "node"),
        ] {
            let hint = missing_program_hint(Some(127), &[output]).unwrap();
            assert_eq!(hint["program"], program, "{hint}");
        }
        assert!(missing_program_hint(Some(1), &[b"sh: 1: cargo: not found\n"]).is_none());
        assert!(missing_program_hint(Some(0), &[b"ok\n"]).is_none());
        assert!(
            missing_program_hint(Some(127), &[b"cat: notes.txt: No such file or directory\n"])
                .is_none()
        );
        let odd = missing_program_hint(Some(127), &[b"sh: 1: a b c $(x): not found\n"]).unwrap();
        assert!(odd["program"].is_null());
    }
    /// A launch refused before its payload was released records why (a fixed code) on
    /// the receipt, and its stage timings (numbers) on the finish event only.
    #[test]
    fn a_not_started_receipt_names_its_cause_and_keeps_timings_out_of_the_reply() {
        let root = std::env::temp_dir().join(format!(
            "command-cause-{}",
            crate::native_effects::identity().unwrap()
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let key = ProjectKey::new(&root, StoreChannel::Dev).unwrap();
        let store = Store::open(key, OpenMode::Initialize).unwrap();
        TaskMachine::new(&store)
            .create("t", "cause", "owner")
            .unwrap();
        let timings = json!({"digest_ms":3,"tree_walk_ms":160,"probe_ms":12});
        store
            .write(|db| {
                row(
                    db,
                    "command_run",
                    "refused",
                    json!({"complete":false,
                    "tool_call_ref":"native-command:refused","argv":["cargo","test"]}),
                )?;
                native_commands::settle_unrecorded_tx(
                    db,
                    "t",
                    "gone-channel",
                    "refused",
                    Some("native-command:refused"),
                    "command_not_started",
                    None,
                    "payload_never_released",
                    Some(&launch_cause("sandbox_unavailable:setup_timeout")),
                    &timings,
                )
            })
            .unwrap();
        let settled = receipt(&store, "refused");
        assert_eq!(settled["failure"], "command_not_started");
        assert_eq!(settled["cause"], "sandbox_unavailable:setup_timeout");
        assert!(settled["timings"].is_null(), "{settled}");
        let observation: Value = store
            .read(|db| {
                let raw: String = db.query_row(
                    "SELECT json_extract(payload_json,'$.receipt.observation') FROM runtime_events
                    WHERE type='NATIVE_COMMAND_FINISHED'",
                    [],
                    |r| r.get(0),
                )?;
                Ok(serde_json::from_str(&raw)?)
            })
            .unwrap();
        assert_eq!(observation["timings"], timings);
        assert_eq!(observation["cause"], "sandbox_unavailable:setup_timeout");
        // A settlement without a caller cause (recovery) still names why.
        store
            .write(|db| {
                row(
                    db,
                    "command_run",
                    "recovered",
                    json!({"complete":false,
                    "tool_call_ref":"native-command:recovered","argv":["cargo","test"]}),
                )?;
                native_commands::settle_unrecorded_tx(
                    db,
                    "t",
                    "gone-channel",
                    "recovered",
                    Some("native-command:recovered"),
                    "command_not_started",
                    None,
                    "not_launched",
                    None,
                    &Value::Null,
                )
            })
            .unwrap();
        assert_eq!(receipt(&store, "recovered")["cause"], "not_launched");
        // Only fixed codes are kept; anything else is reduced to one generic cause.
        assert_eq!(
            launch_cause("bad cause /home/x"),
            "sandbox_preparation_failed"
        );
        assert_eq!(store_cause(&CoreError::Refused("busy")), "store_busy");
        assert_eq!(
            store_cause(&CoreError::Refused("access_changed")),
            "refused:access_changed"
        );
        assert_eq!(
            supervisor::preparation_cause(&std::io::Error::other(
                "native_command_sandbox_unavailable: project_tree_limit"
            )),
            "sandbox_unavailable:project_tree_limit"
        );
        assert_eq!(
            supervisor::preparation_cause(&std::io::Error::from(
                std::io::ErrorKind::PermissionDenied
            )),
            "preparation_io_PermissionDenied"
        );
        drop(store);
        std::fs::remove_dir_all(&root).unwrap();
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

    #[tokio::test]
    async fn the_watchdog_settles_a_hung_observer_only_after_its_supervisor_finished_long_ago() {
        let root = std::env::temp_dir().join(format!(
            "command-resettle-hung-{}",
            crate::native_effects::identity().unwrap()
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let key = ProjectKey::new(&root, StoreChannel::Dev).unwrap();
        let store = Store::open(key, OpenMode::Initialize).unwrap();
        TaskMachine::new(&store).create("t", "resettle", "owner").unwrap();
        let dir = directory(&store, "t");
        cortex_core::store::locks::private_directory(&store.key().directory().join("t"), true).unwrap();
        cortex_core::store::locks::private_directory(&dir, true).unwrap();
        let identity = json!({"pid":1,"boot_id":boot_id().unwrap(),"start_ticks":0});
        let handle = json!({"reference":"native-command:hung","identity":identity,"status":"h.status",
            "stop":"h.stop","output":"h.bin","id":"lo_h","argv":["/bin/false"],"cwd":"/","profile":"read_only",
            "mode":"development","denied":[]});
        let status = json!({"ref":"native-command:hung","supervisor":identity,"state":"finished",
            "exit_code":3,"signal":null,"failure":null,"hash":"0","byte_count":0});
        for (name, body) in [("h.status", status.to_string()), ("h.bin", String::new())] {
            let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600).open(dir.join(name)).unwrap();
            std::io::Write::write_all(&mut file, body.as_bytes()).unwrap();
        }
        store.write(|db| {
            row(db, "command_run", "hung", json!({"complete":false,"tool_call_ref":"native-command:hung","argv":["/bin/false"],"handle":handle}))?;
            Ok(())
        }).unwrap();
        let busy = std::collections::BTreeSet::from(["native-command:hung".to_owned()]);
        // The observer may still be about to settle it: left alone.
        assert_eq!(resettle(&store, &busy, false).unwrap().0, 0);
        assert_ne!(receipt(&store, "hung")["settled"], true);
        let old = std::time::SystemTime::now() - Duration::from_secs(600);
        OpenOptions::new().write(true).open(dir.join("h.status")).unwrap().set_modified(old).unwrap();
        assert_eq!(resettle(&store, &busy, false).unwrap().0, 1);
        let hung = receipt(&store, "hung");
        assert_eq!(hung["settled"], true);
        assert_eq!(hung["failure"], "outcome_not_recorded");
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cortex_store_hint_points_guarded_commands_at_the_file_tools() {
        let argv = json!(["/bin/sh", "-c", "echo x > .cortex/x"]);
        let out: &[&[u8]] = &[b"sh: 1: cannot create .cortex/x: Read-only file system"];
        let hint = cortex_store_hint("guarded", &argv, out).unwrap();
        assert!(hint["next"].as_str().unwrap().contains("project_write"));
        assert!(cortex_store_hint("development", &argv, out).is_some());
        assert!(cortex_store_hint("auto", &argv, out).is_none());
        assert!(cortex_store_hint("guarded", &json!(["ls"]), &[b"Read-only file system"]).is_none());
    }
    #[test]
    fn nested_sandbox_hint_names_the_route_the_profile_actually_has() {
        let bwrap: &[u8] = b"bwrap: No permissions to create a new namespace, likely because the kernel does not allow non-privileged user namespaces.\n";
        let writer = nested_sandbox_hint("development", &[bwrap, b""]).unwrap();
        assert_eq!(writer["code"], "nested_sandbox_denied");
        assert_eq!(writer["outside_command"], "offered");
        assert_eq!(
            nested_sandbox_hint(
                "guarded",
                &[
                    b"",
                    b"[0101/1:FATAL:zygote_host_impl_linux.cc] No usable sandbox!"
                ]
            )
            .unwrap()["outside_command"],
            "offered"
        );
        assert_eq!(
            nested_sandbox_hint(
                "read_only",
                &[b"unshare: unshare failed: Operation not permitted"]
            )
            .unwrap()["outside_command"],
            "not_offered"
        );
        assert!(
            nested_sandbox_hint("git_context", &[bwrap]).unwrap()["outside_command"]
                == "not_offered"
        );
        assert!(nested_sandbox_hint(
            "development",
            &[b"bwrap is mentioned", b"no namespaces here"]
        )
        .is_none());
    }

    #[test]
    fn privilege_hint_names_root_refused_in_the_sandbox() {
        let sudo: &[u8] = b"sudo: The \"no new privileges\" flag is set, which prevents sudo from running as root.\n";
        let writer = privilege_hint("development", &[b"", sudo]).unwrap();
        assert_eq!(writer["code"], "privilege_refused_in_sandbox");
        assert!(writer["next"].as_str().unwrap().contains("on the host"));
        let reader = privilege_hint("read_only", &[sudo]).unwrap();
        assert!(reader["next"]
            .as_str()
            .unwrap()
            .contains("cannot run privileged"));
        assert!(privilege_hint("development", &[b"sudo apt install make"]).is_none());
    }

    #[test]
    fn settled_receipt_carries_nested_sandbox_hint_from_the_output_tail() {
        let root = std::env::temp_dir().join(format!(
            "command-nested-hint-{}",
            crate::native_effects::identity().unwrap()
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let key = ProjectKey::new(&root, StoreChannel::Dev).unwrap();
        let store = Store::open(key, OpenMode::Initialize).unwrap();
        TaskMachine::new(&store)
            .create("t", "nested", "owner")
            .unwrap();
        let dir = directory(&store, "t");
        cortex_core::store::locks::private_directory(&store.key().directory().join("t"), true)
            .unwrap();
        cortex_core::store::locks::private_directory(&dir, true).unwrap();
        // The denial lands after more than 64 KiB of ordinary output.
        let mut body = vec![b'.'; 200_000];
        body.extend_from_slice(b"\nbwrap: No permissions to create a new namespace\n");
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(dir.join("n.bin"))
            .unwrap();
        std::io::Write::write_all(&mut file, &body).unwrap();
        let (hash, size) = native_commands::fingerprint(&mut file).unwrap();
        let handle = json!({"reference":"native-command:nested","identity":{},"status":"n.status",
            "stop":"n.stop","output":"n.bin","id":"lo_n","argv":["bwrap","--unshare-user","--ro-bind","/","/","--","/usr/bin/true"],
            "cwd":"/","profile":"development","mode":"development","denied":[]});
        let status = json!({"state":"finished","exit_code":1,"signal":null,"failure":null,"hash":hash,"byte_count":size});
        store
            .write(|db| row(db, "command_run", "nested", json!({"complete":false,"tool_call_ref":"native-command:nested","handle":handle})))
            .unwrap();
        let value = settle(
            &store,
            "t",
            "gone-channel",
            "nested",
            &handle,
            &status,
            None,
            None,
        )
        .unwrap();
        assert_eq!(value["exit_code"], 1);
        assert_eq!(value["complete"], true);
        assert_eq!(value["output_truncated"], true);
        assert_eq!(value["nested_sandbox"]["outside_command"], "offered");
        assert_eq!(
            receipt(&store, "nested")["nested_sandbox"]["code"],
            "nested_sandbox_denied"
        );
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn only_transient_prestart_causes_are_retried() {
        for c in [
            "supervisor_spawn_failed",
            "preparation_io_NotFound",
            "sandbox_unavailable:command_hardlink_changed",
        ] {
            assert!(prestart_retryable(Some(c)), "{c}");
        }
        for c in [
            "preparation_io_PermissionDenied",
            "sandbox_unavailable:landlock_unavailable",
            "refused:access_changed",
        ] {
            assert!(!prestart_retryable(Some(c)), "{c}");
        }
        assert!(!prestart_retryable(None));
    }
}
