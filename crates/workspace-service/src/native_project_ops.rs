//! Writing-worker directory creation, confirmed file deletion and guarded
//! project skill links. They use exactly the write authority of
//! `project_write` (`native_project::writable`: writing role and stage, no
//! open question, owner deny-write paths, private storage) and the deletion
//! consent of `command_run`: outside Auto the owner confirms every deletion and
//! every replacement of a different skill link; Auto answers that card itself.
//! Effects use no-follow directory descriptors. A durable started receipt
//! precedes each effect, so an uncertain outcome is never repeated.
use crate::{
    native_project::{digest, lease_denial, missing_parents, open_project_root, openat, safe, writable_target, WorkerLeaseFence},
    native_sessions::ClientPermissions,
};
use cortex_core::{
    access,
    channels::{self, Credential, Identity},
    lifecycle,
    store::executor::StoreExecutor,
    CoreError, CoreResult,
};
use cortex_engine::engine::PermissionDecision;
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use std::{
    ffi::CString,
    fs::File,
    io::{Read, Write},
    os::fd::{AsRawFd, FromRawFd},
    path::PathBuf,
    sync::Arc,
};

pub(super) fn supports(name: &str) -> bool {
    matches!(
        name,
        "project_mkdir" | "project_delete" | "project_skill_link"
    )
}
async fn recv<T>(r: tokio::sync::oneshot::Receiver<CoreResult<T>>) -> CoreResult<T> {
    r.await
        .map_err(|_| CoreError::Refused("native_host_closed"))?
}
fn refused<T>(code: &'static str) -> CoreResult<T> {
    Err(CoreError::Refused(code))
}
fn link_path(name: &str) -> String {
    format!(".claude/skills/{name}")
}
fn link_target(name: &str) -> String {
    format!("../../.agents/skills/{name}")
}
fn check_name(name: &str) -> CoreResult<()> {
    let valid = !name.is_empty()
        && name.len() <= 64
        && name.split('-').all(|p| {
            !p.is_empty()
                && p.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        });
    if valid {
        Ok(())
    } else {
        refused("skill_link_name_invalid")
    }
}
fn cstr(name: &str) -> CoreResult<CString> {
    CString::new(name).map_err(|_| CoreError::Refused("private_or_invalid_path"))
}

#[derive(Debug, PartialEq)]
enum Entry {
    Absent,
    Directory,
    File,
    Link(String),
    Other,
}
/// The entry `name` in `dir`, never following a link.
fn entry(dir: &File, name: &str) -> CoreResult<Entry> {
    let c = cstr(name)?;
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    let rc = unsafe {
        libc::fstatat(
            dir.as_raw_fd(),
            c.as_ptr(),
            &mut st,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if rc < 0 {
        let e = std::io::Error::last_os_error();
        if e.kind() == std::io::ErrorKind::NotFound {
            return Ok(Entry::Absent);
        }
        return Err(e.into());
    }
    Ok(match st.st_mode & libc::S_IFMT {
        libc::S_IFDIR => Entry::Directory,
        libc::S_IFREG => Entry::File,
        libc::S_IFLNK => {
            let mut buf = vec![0u8; 4097];
            let n = unsafe {
                libc::readlinkat(
                    dir.as_raw_fd(),
                    c.as_ptr(),
                    buf.as_mut_ptr().cast(),
                    buf.len(),
                )
            };
            if n < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            buf.truncate(n as usize);
            Entry::Link(String::from_utf8_lossy(&buf).into_owned())
        }
        _ => Entry::Other,
    })
}
fn open_dir(dir: &File, name: &str) -> CoreResult<File> {
    openat(dir, name, libc::O_RDONLY | libc::O_DIRECTORY)
}
/// A real child directory; created when absent and `create`. Any other entry
/// (a link, file or device) refuses with `unsafe_code`.
fn child_dir(
    dir: &File,
    name: &str,
    create: bool,
    unsafe_code: &'static str,
    possible: &mut bool,
) -> CoreResult<Option<(File, bool)>> {
    let mut created = false;
    match entry(dir, name)? {
        Entry::Directory => {}
        Entry::Absent if create => {
            *possible = true;
            let c = cstr(name)?;
            if unsafe { libc::mkdirat(dir.as_raw_fd(), c.as_ptr(), 0o755) } < 0 {
                let e = std::io::Error::last_os_error();
                if e.kind() != std::io::ErrorKind::AlreadyExists {
                    return Err(e.into());
                }
            } else {
                created = true;
                dir.sync_all()?;
            }
            if entry(dir, name)? != Entry::Directory {
                return refused(unsafe_code);
            }
        }
        Entry::Absent => return Ok(None),
        Entry::Link(_) if unsafe_code == "gateway_path_exists" => {
            return refused("symlink_refused")
        }
        _ => return refused(unsafe_code),
    }
    Ok(Some((open_dir(dir, name)?, created)))
}
/// The directory holding `path`'s last component, by no-follow descriptors.
fn parent_of(root: &PathBuf, path: &str) -> CoreResult<Option<(File, String)>> {
    let mut parts = path.split('/').collect::<Vec<_>>();
    let leaf = parts.pop().unwrap().to_owned();
    let mut dir = open_project_root(root)?;
    let mut never = false;
    for part in parts {
        match child_dir(&dir, part, false, "symlink_refused", &mut never)? {
            Some((next, _)) => dir = next,
            None => return Ok(None),
        }
    }
    Ok(Some((dir, leaf)))
}
/// Current raw-byte SHA-256 of one regular, project file (a hard link is fine: only the directory entry is acted on).
fn file_digest(dir: &File, name: &str) -> CoreResult<String> {
    match entry(dir, name)? {
        Entry::File => {}
        Entry::Absent => return refused("file_missing"),
        Entry::Directory => return refused("directory_required"),
        Entry::Link(_) => return refused("symlink_refused"),
        Entry::Other => return refused("regular_file_required"),
    }
    let mut file = openat(dir, name, libc::O_RDONLY)?;
    let m = file.metadata()?;
    if !m.is_file() {
        return refused("regular_unlinked_file_required");
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(digest(&bytes))
}

/// What a confirmed effect will do, decided before any consent is asked.
#[derive(Debug)]
enum Plan {
    Mkdir,
    Delete,
    Link { replace: Option<String> },
    Unlink,
    Unchanged(Value),
}
fn skills_dir(root: &PathBuf, missing: &[String], possible: &mut bool) -> CoreResult<Option<File>> {
    let root = open_project_root(root)?;
    let Some((claude, _)) = child_dir(
        &root,
        ".claude",
        missing.iter().any(|p| p == ".claude"),
        "skill_link_parent_unsafe",
        possible,
    )?
    else {
        return Ok(None);
    };
    Ok(child_dir(
        &claude,
        "skills",
        missing.iter().any(|p| p == ".claude/skills"),
        "skill_link_parent_unsafe",
        possible,
    )?
    .map(|(d, _)| d))
}
fn verify_source(root: &PathBuf, name: &str) -> CoreResult<()> {
    let mut dir = open_project_root(root)?;
    let mut never = false;
    for part in [".agents", "skills", name] {
        dir = match child_dir(&dir, part, false, "skill_source_missing", &mut never)? {
            Some((d, _)) => d,
            None => return refused("skill_source_missing"),
        };
    }
    if entry(&dir, "SKILL.md")? != Entry::File {
        return refused("skill_source_missing");
    }
    Ok(())
}
fn link_reply(name: &str, changed: bool, previous: Option<&str>, action: &str) -> Value {
    json!({"action":action,"path":link_path(name),"target":link_target(name),"changed":changed,
        "previous":previous.map(|t| json!({"kind":"symlink","target":t}))})
}
fn plan(root: &PathBuf, name: &str, args: &Value) -> CoreResult<Plan> {
    match name {
        "project_mkdir" => Ok(Plan::Mkdir),
        "project_delete" => {
            let path = args["path"].as_str().unwrap_or_default();
            if path == "." {
                return refused("directory_required");
            }
            let (dir, leaf) = parent_of(root, path)?.ok_or(CoreError::Refused("file_missing"))?;
            if file_digest(&dir, &leaf)? != args["expected_sha256"].as_str().unwrap_or_default() {
                return refused("gateway_write_conflict");
            }
            Ok(Plan::Delete)
        }
        _ => {
            let skill = args["name"].as_str().unwrap_or_default();
            let mut never = false;
            let target = link_target(skill);
            if args["action"] == "unlink" {
                let current = match skills_dir(root, &[], &mut never)? {
                    Some(dir) => entry(&dir, skill)?,
                    None => Entry::Absent,
                };
                return match current {
                    Entry::Absent => Ok(Plan::Unchanged(link_reply(skill, false, None, "unlink"))),
                    Entry::Link(text) if text == target => Ok(Plan::Unlink),
                    _ => refused("skill_link_not_managed"),
                };
            }
            verify_source(root, skill)?;
            let current = match skills_dir(root, &[], &mut never)? {
                Some(dir) => entry(&dir, skill)?,
                None => Entry::Absent,
            };
            let replace = args["replace_symlink_target"].as_str();
            match current {
                Entry::Link(text) if text == target => {
                    Ok(Plan::Unchanged(link_reply(skill, false, None, "link")))
                }
                Entry::Absent => Ok(Plan::Link { replace: None }),
                Entry::Link(text) if replace == Some(text.as_str()) => Ok(Plan::Link {
                    replace: Some(text),
                }),
                Entry::Link(_) if replace.is_some() => refused("skill_link_conflict"),
                Entry::Link(_) => refused("skill_link_occupied"),
                _ => refused("skill_link_collision"),
            }
        }
    }
}
/// The effect itself. `possible` is set before the first mutating syscall.
#[cfg(test)]
fn apply(
    root: &PathBuf,
    rule: &str,
    args: &Value,
    plan: &Plan,
    missing: &[String],
    possible: &mut bool,
) -> CoreResult<Value> {
    apply_fenced(root, rule, args, plan, missing, possible, || Ok(()))
}
fn apply_fenced(
    root: &PathBuf,
    rule: &str,
    args: &Value,
    plan: &Plan,
    missing: &[String],
    possible: &mut bool,
    fence: impl Fn() -> CoreResult<()>,
) -> CoreResult<Value> {
    match plan {
        Plan::Unchanged(v) => Ok(v.clone()),
        Plan::Mkdir => {
            let path = args["path"].as_str().unwrap_or_default();
            if path == "." {
                return Ok(json!({"path":path,"created":false}));
            }
            let mut dir = open_project_root(root)?;
            let mut created = false;
            let mut end = 0;
            for part in path.split('/') {
                end += part.len();
                let prefix = &path[..end];
                let create = prefix == path || missing.iter().any(|p| p == prefix);
                let (next, made) = child_dir(&dir, part, create, "gateway_path_exists", possible)?
                    .ok_or(CoreError::Refused("gateway_path_exists"))?;
                dir = next;
                created = made;
                end += 1;
            }
            Ok(json!({"path":path,"created":created}))
        }
        Plan::Delete => {
            let path = args["path"].as_str().unwrap_or_default();
            let (dir, leaf) = parent_of(root, path)?.ok_or(CoreError::Refused("file_missing"))?;
            // Observed again after the owner's confirmation, immediately before the effect.
            if file_digest(&dir, &leaf)? != args["expected_sha256"].as_str().unwrap_or_default() {
                return refused("gateway_write_conflict");
            }
            if path == ".gitignore" {
                // The root ignore file keeps only the channel-store rule.
                let body = format!("{rule}\n");
                let temp = format!(".cortex-edit-{}", &crate::native_effects::identity()?[..16]);
                let t = cstr(&temp)?;
                let fd = unsafe {
                    libc::openat(
                        dir.as_raw_fd(),
                        t.as_ptr(),
                        libc::O_WRONLY
                            | libc::O_CREAT
                            | libc::O_EXCL
                            | libc::O_NOFOLLOW
                            | libc::O_CLOEXEC,
                        0o644,
                    )
                };
                if fd < 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
                let mut file = unsafe { File::from_raw_fd(fd) };
                let written = (|| -> CoreResult<()> {
                    file.write_all(body.as_bytes())?;
                    file.sync_all()?;
                    fence()?;
                    *possible = true;
                    let dest = cstr(&leaf)?;
                    if unsafe {
                        libc::renameat(dir.as_raw_fd(), t.as_ptr(), dir.as_raw_fd(), dest.as_ptr())
                    } < 0
                    {
                        return Err(std::io::Error::last_os_error().into());
                    }
                    dir.sync_all()?;
                    Ok(())
                })();
                unsafe { libc::unlinkat(dir.as_raw_fd(), t.as_ptr(), 0) };
                written?;
                return Ok(
                    json!({"path":path,"deleted":false,"gitignore_guard":true,"sha256":digest(body.as_bytes())}),
                );
            }
            *possible = true;
            let c = cstr(&leaf)?;
            if unsafe { libc::unlinkat(dir.as_raw_fd(), c.as_ptr(), 0) } < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            dir.sync_all()?;
            Ok(json!({"path":path,"deleted":true}))
        }
        Plan::Unlink => {
            let skill = args["name"].as_str().unwrap_or_default();
            let mut never = false;
            let dir = skills_dir(root, &[], &mut never)?
                .ok_or(CoreError::Refused("skill_link_not_managed"))?;
            let target = link_target(skill);
            if entry(&dir, skill)? != Entry::Link(target.clone()) {
                return refused("skill_link_not_managed");
            }
            *possible = true;
            let c = cstr(skill)?;
            if unsafe { libc::unlinkat(dir.as_raw_fd(), c.as_ptr(), 0) } < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            dir.sync_all()?;
            Ok(link_reply(skill, true, Some(&target), "unlink"))
        }
        Plan::Link { replace } => {
            let skill = args["name"].as_str().unwrap_or_default();
            verify_source(root, skill)?;
            let dir = skills_dir(root, missing, possible)?
                .ok_or(CoreError::Refused("skill_link_parent_unsafe"))?;
            let target = link_target(skill);
            let t = cstr(&target)?;
            let leaf = cstr(skill)?;
            let current = entry(&dir, skill)?;
            match (replace, &current) {
                (_, Entry::Link(text)) if *text == target => {
                    return Ok(link_reply(skill, false, None, "link"))
                }
                (None, Entry::Absent) => {
                    *possible = true;
                    if unsafe { libc::symlinkat(t.as_ptr(), dir.as_raw_fd(), leaf.as_ptr()) } < 0 {
                        return Err(std::io::Error::last_os_error().into());
                    }
                }
                // Only the exact link the owner confirmed is replaced, atomically.
                (Some(expected), Entry::Link(text)) if text == expected => {
                    let temp = cstr(&format!(
                        ".cortex-link-{}",
                        &crate::native_effects::identity()?[..16]
                    ))?;
                    *possible = true;
                    if unsafe { libc::symlinkat(t.as_ptr(), dir.as_raw_fd(), temp.as_ptr()) } < 0 {
                        return Err(std::io::Error::last_os_error().into());
                    }
                    if let Err(error) = fence() {
                        unsafe { libc::unlinkat(dir.as_raw_fd(), temp.as_ptr(), 0) };
                        return Err(error);
                    }
                    let rc = unsafe {
                        libc::renameat(
                            dir.as_raw_fd(),
                            temp.as_ptr(),
                            dir.as_raw_fd(),
                            leaf.as_ptr(),
                        )
                    };
                    if rc < 0 {
                        let e = std::io::Error::last_os_error();
                        unsafe { libc::unlinkat(dir.as_raw_fd(), temp.as_ptr(), 0) };
                        return Err(e.into());
                    }
                }
                (Some(_), Entry::Link(_)) => return refused("skill_link_conflict"),
                (_, Entry::Link(_)) => return refused("skill_link_occupied"),
                _ => return refused("skill_link_collision"),
            }
            dir.sync_all()?;
            Ok(link_reply(skill, true, replace.as_deref(), "link"))
        }
    }
}
/// Whether the project's Git would ignore the link or its skill source
/// (root and nested .gitignore and info/exclude; the owner's global
/// configuration is not read). `null` outside a checkout or when Git cannot run.
async fn git_retention(root: &std::path::Path, skill: &str) -> Value {
    let paths = [link_path(skill), format!(".agents/skills/{skill}/SKILL.md")];
    let mut value = json!({"link_ignored":null,"source_ignored":null,
        "scope":"root and nested .gitignore and .git/info/exclude"});
    let Ok(out) = crate::native_effects::git_ref(
        root,
        &[
            "check-ignore",
            "--no-index",
            "-v",
            "-n",
            "--",
            &paths[0],
            &paths[1],
        ],
    )
    .await
    else {
        return value;
    };
    if !out.complete || out.truncated {
        return value;
    }
    let mut ignored = std::collections::BTreeMap::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let Some((source, path)) = line.split_once('\t') else {
            continue;
        };
        let pattern = source.splitn(3, ':').nth(2).unwrap_or("");
        ignored.insert(
            path.to_owned(),
            !pattern.is_empty() && !pattern.starts_with('!'),
        );
    }
    if let (Some(link), Some(source)) = (ignored.get(&paths[0]), ignored.get(&paths[1])) {
        value["link_ignored"] = json!(link);
        value["source_ignored"] = json!(source);
    }
    value
}
fn authorized(
    db: &rusqlite::Transaction<'_>,
    c: &Credential,
    expected: Option<&Identity>,
    root: &std::path::Path,
    path: &str,
    scoped: bool,
) -> CoreResult<Identity> {
    let i = channels::authenticate(db, &c.channel_id, c.secret(), true)?;
    if expected.is_some_and(|e| e != &i) {
        return refused("channel_identity_changed");
    }
    writable_target(db, &i, root, path, scoped)?;
    Ok(i)
}
fn consent_code(name: &str) -> &'static str {
    if name == "project_delete" {
        "deletion_not_confirmed"
    } else {
        "skill_link_replace_not_confirmed"
    }
}
pub(super) async fn execute(
    executor: Arc<StoreExecutor>,
    credential: Credential,
    root: PathBuf,
    permissions: Option<Arc<dyn ClientPermissions>>,
    name: &str,
    args: Value,
    scoped: bool,
) -> CoreResult<Value> {
    let name = name.to_owned();
    // A private or malformed path is refused before anything else, so a
    // deletion there never asks the owner first.
    let path = if name == "project_skill_link" {
        let skill = args["name"]
            .as_str()
            .ok_or(CoreError::Refused("invalid_arguments"))?;
        check_name(skill)?;
        if args["action"] == "unlink" && args.get("replace_symlink_target").is_some() {
            return refused("invalid_arguments");
        }
        link_path(skill)
    } else {
        let path = args["path"]
            .as_str()
            .ok_or(CoreError::Refused("invalid_arguments"))?;
        safe(path)?;
        if name == "project_delete"
            && crate::native_engine_policy::live_store_file(&root.join(path))
        {
            return refused("cortex_live_store");
        }
        path.to_owned()
    };
    let hash = digest(&serde_json::to_vec(&json!([&name, &args]))?);
    let explicit = args["request_key"].as_str().map(str::to_owned);
    let key = explicit.clone().unwrap_or_else(|| format!("file:{hash}"));
    let (c, p, n, k, h) = (
        credential.clone(),
        path.clone(),
        name.clone(),
        key.clone(),
        hash.clone(),
    );
    let initial_root = root.clone();
    let (identity, saved, mode, session, rule, binding, ru) = recv(executor.publication(move |store| {
        let binding = if scoped && initial_root != store.key().root() {
            let i = store.read(|db| channels::authenticate(db, &c.channel_id, c.secret(), true))?;
            let binding = super::native_project::checkout_binding(store, &i)?
                .ok_or(CoreError::Refused("worktree_binding_invalid"))?;
            if binding["root"].as_str().map(std::path::Path::new) != Some(initial_root.as_path()) {
                return refused("worktree_binding_invalid");
            }
            Some(binding)
        } else { None };
        let rule = format!("/{}/", store.key().channel().directory());
        store.write(|db| {
            let i = authorized(db, &c, None, &initial_root, &p, scoped)?;
            let old: Option<(String, String, String)> = db.query_row(
                "SELECT operation,digest,result_json FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=?",
                (&i.task, &c.channel_id, &k), |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional()?;
            if let Some((op, d, result)) = old {
                if op != n || d != h {
                    return refused("request_key_conflict");
                }
                let v: Value = serde_json::from_str(&result)?;
                if binding.is_some() && v.get("checkout") != binding.as_ref() {
                    return refused("worktree_receipt_binding_mismatch");
                }
                if v["complete"] != true {
                    return refused("project_effect_outcome_unknown");
                }
                return Ok((i, Some(v), String::new(), None, rule.clone(), binding, false));
            }
            match WorkerLeaseFence::new(&initial_root, &p, &[])?.check(db) {
                Ok(()) => {}
                Err(CoreError::OwnerFileLeaseHeld { .. }) => {
                    let denial = lease_denial(binding.as_ref());
                    db.execute("INSERT INTO runtime_requests VALUES (?,?,?,?,?,?)",
                        (&i.task, &k, &c.channel_id, &n, &h, denial.to_string()))?;
                    return Ok((i, Some(denial), String::new(), None, rule.clone(), binding, false));
                }
                Err(error) => return Err(error),
            }
            let mode = access::current(db, &i.task)?;
            let sessions = db.prepare("SELECT session_id FROM acp_host_sessions WHERE task_id=? AND json_extract(client_info,'$.name')='cortex-rust-acp'")?
                .query_map([&i.task], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
            if sessions.len() > 1 {
                return refused("incoming_session_binding_conflict");
            }
            let ru = cortex_core::lifecycle::domain::task_language_tx(db, &i.task)? == "ru";
            Ok((i, None, mode, sessions.into_iter().next(), rule.clone(), binding, ru))
        })
    })?)
    .await?;
    if let Some(saved) = saved {
        // A retained owner refusal is final for this identical request: the
        // confirmation is never asked again.
        if saved["refused"] == consent_code(&name) {
            return refused(consent_code(&name));
        }
        return Ok(saved["reply"].clone());
    }
    let mut possible = false;
    let mut started = false;
    let mut denied_by_lease = false;
    let outcome: CoreResult<Value> = async {
        let planned = plan(&root, &name, &args)?;
        let confirm = matches!(planned, Plan::Delete | Plan::Link { replace: Some(_) });
        if confirm && mode != "auto" {
            let (title, kind) = match (name.as_str(), ru) {
                ("project_delete", false) => (format!("Delete {path}"), "delete"),
                ("project_delete", true) => (format!("Удалить {path}"), "delete"),
                (_, false) => (format!("Replace the skill link {path}"), "edit"),
                (_, true) => (format!("Заменить ссылку на навык {path}"), "edit"),
            };
            let request = json!({"toolCall":{"toolCallId":key,"title":title,"kind":kind,"rawInput":args},
                "options":cortex_engine::permission::one_shot_options(ru)});
            let allowed = match (&permissions, &session) {
                (Some(p), Some(s)) => {
                    matches!(p.decide(s, &request).await, Ok(PermissionDecision::Selected(id)) if id == "allow")
                }
                _ => false,
            };
            if !allowed {
                return refused(consent_code(&name));
            }
        }
        // Authority, owner policy and access mode are checked again, then the
        // started receipt is committed before the effect.
        let (c, p, n, k, h, m) = (credential.clone(), path.clone(), name.clone(), key.clone(), hash.clone(), mode.clone());
        let expected = identity.clone();
        let intent_root = root.clone();
        let started_binding = binding.clone();
        recv(executor.write(move |db| {
            let i = authorized(db, &c, Some(&expected), &intent_root, &p, scoped)?;
            if access::current(db, &i.task)? != m {
                return refused("access_mode_changed");
            }
            let mut receipt = json!({"complete":false,"path":p,"revision":lifecycle::domain::task_tx(db, &i.task)?.1});
            if let Some(binding) = started_binding { receipt["checkout"] = binding; }
            db.execute("INSERT INTO runtime_requests VALUES (?,?,?,?,?,?)",
                (&i.task, &k, &c.channel_id, &n, &h, receipt.to_string()))?;
            Ok(())
        })?)
        .await?;
        started = true;
        let (c, p, m, effect_root, effect_args, expected) = (
            credential.clone(), path.clone(), mode.clone(), root.clone(), args.clone(), identity.clone(),
        );
        let effect_binding = binding.clone();
        let effect_key = key.clone();
        let (effect, maybe_changed, denied) = recv(executor.publication(move |store| {
            if let Some(binding) = &effect_binding {
                if !super::native_project::retained_checkout_matches(store, &expected.task, binding)? {
                    return refused("worktree_binding_invalid");
                }
            }
            store.write(|db| {
            let i = authorized(db, &c, Some(&expected), &effect_root, &p, scoped)?;
            if access::current(db, &i.task)? != m {
                return refused("access_mode_changed");
            }
            let revision: i64 = db.query_row(
                "SELECT json_extract(result_json,'$.revision') FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=?",
                (&i.task, &c.channel_id, &effect_key), |r| r.get(0))?;
            if revision != lifecycle::domain::task_tx(db, &i.task)?.1 {
                return refused("stale_project_write");
            }
            let missing = if matches!(planned, Plan::Mkdir | Plan::Link { .. }) {
                missing_parents(&effect_root, &p, |parent| {
                    writable_target(db, &i, &effect_root, parent, scoped)
                })?
            } else {
                Vec::new()
            };
            let lease_fence = WorkerLeaseFence::new(&effect_root, &p, &missing)?;
            let fence = || lease_fence.check(db);
            match fence() {
                Ok(()) => {}
                Err(CoreError::OwnerFileLeaseHeld { .. }) => {
                    let denial = lease_denial(effect_binding.as_ref());
                    db.execute("UPDATE runtime_requests SET result_json=? WHERE task_id=? AND actor=? AND request_key=?",
                        (denial.to_string(), &i.task, &c.channel_id, &effect_key))?;
                    return Ok((Ok(denial["reply"].clone()), false, true));
                }
                Err(error) => return Err(error),
            }
            let mut maybe_changed = false;
            let value = apply_fenced(&effect_root, &rule, &effect_args, &planned, &missing, &mut maybe_changed, fence);
            Ok((value, maybe_changed, false))
            })
        })?).await?;
        possible = maybe_changed;
        denied_by_lease = denied;
        let mut value = effect?;
        if denied {
            return Ok(value);
        }
        if name == "project_skill_link" && args["action"] == "link" {
            // Observed after the effect, outside every store transaction.
            value["git"] = git_retention(&root, args["name"].as_str().unwrap_or_default()).await;
        }
        Ok(value)
    }
    .await;
    let c = credential.clone();
    let (n, k, h, p) = (name.clone(), key.clone(), hash.clone(), path.clone());
    let task = identity.task.clone();
    match outcome {
        Ok(reply) if denied_by_lease => Ok(reply),
        Ok(value) => {
            let reply = crate::native_sources::reply(value.clone());
            let r = reply.clone();
            let changed = value["created"] == true
                || value["deleted"] == true
                || value["changed"] == true
                || value["gitignore_guard"] == true;
            recv(executor.write(move |db| {
                let mut receipt = json!({"complete":true,"reply":r});
                let mut event = json!({"channel_id":c.channel_id,"operation":n,"path":p,"request_key":k});
                let revision: i64 = db.query_row(
                    "SELECT json_extract(result_json,'$.revision') FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=? AND operation=?",
                    (&task, &c.channel_id, &k, &n), |r| r.get(0))?;
                receipt["revision"] = json!(revision);
                event["revision"] = json!(revision);
                if let Some(binding) = binding {
                    receipt["checkout"] = binding.clone();
                    event["checkout"] = binding;
                }
                db.execute("UPDATE runtime_requests SET result_json=? WHERE task_id=? AND actor=? AND request_key=? AND operation=?",
                    (receipt.to_string(), &task, &c.channel_id, &k, &n))?;
                if changed {
                    lifecycle::event_tx(db, &task, "NATIVE_PROJECT_CHANGED", event)?;
                }
                Ok(())
            })?)
            .await?;
            Ok(reply)
        }
        // A possible effect keeps its started receipt: it is never repeated.
        Err(error) if possible => Err(error),
        Err(error) => {
            let code = error.code();
            let consent = code == consent_code(&name);
            let retained = if consent {
                Some(json!({"complete":true,"refused":code}))
            } else if explicit.is_some() && !crate::native_tool_errors::authority(code) {
                Some(json!({"complete":true,"reply":crate::native_tool_errors::reply(code)}))
            } else {
                None
            };
            let _ = recv(executor.write(move |db| {
                if started {
                    db.execute("DELETE FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=? AND operation=?",
                        (&task, &c.channel_id, &k, &n))?;
                }
                if let Some(mut retained) = retained {
                    if let Some(binding) = binding { retained["checkout"] = binding; }
                    db.execute("INSERT OR IGNORE INTO runtime_requests VALUES (?,?,?,?,?,?)",
                        (&task, &k, &c.channel_id, &n, &h, retained.to_string()))?;
                }
                Ok(())
            })?)
            .await;
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn skill_names_are_single_directory_names() {
        for good in ["release-notes", "a", "x1-y2"] {
            check_name(good).unwrap();
        }
        for bad in ["", "-a", "a-", "a--b", "A", "a/b", "..", &"a".repeat(65)] {
            assert_eq!(
                check_name(bad).unwrap_err().code(),
                "skill_link_name_invalid"
            );
        }
    }
    #[test]
    fn effects_never_follow_links_or_replace_foreign_entries() {
        let root = std::env::temp_dir().join(format!(
            "project-ops-{}",
            crate::native_effects::identity().unwrap()
        ));
        std::fs::create_dir_all(root.join(".agents/skills/s")).unwrap();
        std::fs::write(root.join(".agents/skills/s/SKILL.md"), "# s\n").unwrap();
        let mut possible = false;
        // mkdir creates parents; an existing directory is created: false; a file refuses.
        let mk = |p: &str, possible: &mut bool| {
            apply(
                &root,
                "/.cortex/",
                &json!({"path":p}),
                &Plan::Mkdir,
                &missing_parents(&root, p, |_| Ok(()))?,
                possible,
            )
        };
        assert_eq!(mk("a/b/c", &mut possible).unwrap()["created"], true);
        assert_eq!(mk("a/b", &mut possible).unwrap()["created"], false);
        std::fs::write(root.join("a/file"), "x").unwrap();
        assert_eq!(
            mk("a/file", &mut possible).unwrap_err().code(),
            "gateway_path_exists"
        );
        std::os::unix::fs::symlink("a", root.join("alias")).unwrap();
        assert_eq!(
            mk("alias/new", &mut possible).unwrap_err().code(),
            "symlink_refused"
        );
        assert!(!root.join("a/new").exists());
        // delete checks the exact current sha256 and refuses directories and links.
        let sha = digest(b"x");
        assert_eq!(
            plan(
                &root,
                "project_delete",
                &json!({"path":"a/file","expected_sha256":"0"})
            )
            .unwrap_err()
            .code(),
            "gateway_write_conflict"
        );
        assert_eq!(
            plan(
                &root,
                "project_delete",
                &json!({"path":"a","expected_sha256":sha})
            )
            .unwrap_err()
            .code(),
            "directory_required"
        );
        assert_eq!(
            plan(
                &root,
                "project_delete",
                &json!({"path":"alias/file","expected_sha256":sha})
            )
            .unwrap_err()
            .code(),
            "symlink_refused"
        );
        let args = json!({"path":"a/file","expected_sha256":sha});
        let p = plan(&root, "project_delete", &args).unwrap();
        assert_eq!(
            apply(&root, "/.cortex/", &args, &p, &[], &mut possible).unwrap()["deleted"],
            true
        );
        assert!(!root.join("a/file").exists());
        // Root .gitignore keeps only the store rule.
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
        let args = json!({"path":".gitignore","expected_sha256":digest(b"target/\n")});
        let p = plan(&root, "project_delete", &args).unwrap();
        assert_eq!(
            apply(&root, "/.cortex/", &args, &p, &[], &mut possible).unwrap()["deleted"],
            false
        );
        assert_eq!(
            std::fs::read_to_string(root.join(".gitignore")).unwrap(),
            "/.cortex/\n"
        );
        // Links: create, keep, refuse a foreign link, replace only the named text, unlink.
        let link = json!({"action":"link","name":"s","request_key":"k"});
        let p = plan(&root, "project_skill_link", &link).unwrap();
        assert_eq!(
            apply(
                &root,
                "",
                &link,
                &p,
                &[".claude".into(), ".claude/skills".into()],
                &mut possible
            )
            .unwrap()["changed"],
            true
        );
        assert_eq!(
            std::fs::read_link(root.join(".claude/skills/s")).unwrap(),
            PathBuf::from("../../.agents/skills/s")
        );
        assert!(matches!(
            plan(&root, "project_skill_link", &link).unwrap(),
            Plan::Unchanged(_)
        ));
        std::fs::remove_file(root.join(".claude/skills/s")).unwrap();
        std::os::unix::fs::symlink("elsewhere", root.join(".claude/skills/s")).unwrap();
        assert_eq!(
            plan(&root, "project_skill_link", &link)
                .err()
                .unwrap()
                .code(),
            "skill_link_occupied"
        );
        let wrong =
            json!({"action":"link","name":"s","request_key":"k","replace_symlink_target":"other"});
        assert_eq!(
            plan(&root, "project_skill_link", &wrong)
                .err()
                .unwrap()
                .code(),
            "skill_link_conflict"
        );
        let replace = json!({"action":"link","name":"s","request_key":"k","replace_symlink_target":"elsewhere"});
        let p = plan(&root, "project_skill_link", &replace).unwrap();
        assert!(matches!(p, Plan::Link { replace: Some(_) }));
        assert_eq!(
            apply(&root, "", &replace, &p, &[], &mut possible).unwrap()["previous"]["target"],
            "elsewhere"
        );
        let unlink = json!({"action":"unlink","name":"s","request_key":"u"});
        let p = plan(&root, "project_skill_link", &unlink).unwrap();
        assert_eq!(
            apply(&root, "", &unlink, &p, &[], &mut possible).unwrap()["changed"],
            true
        );
        assert!(root.join(".agents/skills/s/SKILL.md").is_file());
        std::fs::create_dir_all(root.join(".claude/skills/d")).unwrap();
        std::fs::create_dir_all(root.join(".agents/skills/d")).unwrap();
        std::fs::write(root.join(".agents/skills/d/SKILL.md"), "# d\n").unwrap();
        assert_eq!(
            plan(
                &root,
                "project_skill_link",
                &json!({"action":"link","name":"d"})
            )
            .err()
            .unwrap()
            .code(),
            "skill_link_collision"
        );
        assert_eq!(
            plan(
                &root,
                "project_skill_link",
                &json!({"action":"link","name":"none"})
            )
            .err()
            .unwrap()
            .code(),
            "skill_source_missing"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }
}
