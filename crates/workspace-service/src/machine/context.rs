//! Core-owned repository context. Read-only discovery never initializes a store.
use super::{files, git::Git, native::*, settings};
use cortex_core::{CoreError, CoreResult, OpenMode, Store};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub(super) fn core(error: CoreError) -> Refused {
    match error.storage_diagnostic() {
        Some(diagnostic) => refuse(diagnostic.reason, diagnostic.message),
        None => refuse(error.code(), "The project store could not confirm this operation."),
    }
}
pub fn key(root: &Path) -> Result<cortex_core::ProjectKey> {
    crate::native_runtime::key(root).map_err(core)
}
pub(super) fn directory(root: &Path) -> Result<PathBuf> {
    crate::store_locations::directory(key(root)?).map_err(core)
}
thread_local! {
    /// Set on the thread that serves one of the owner's screen reads (see `machine::owner_read`).
    static OWNER_READ: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
/// Marks this thread as serving an owner read until the guard drops: its live
/// read-only opens wait for an exclusive migration instead of refusing `busy`.
pub(crate) struct OwnerRead(bool);
pub(crate) fn owner_read() -> OwnerRead {
    OwnerRead(OWNER_READ.with(|r| r.replace(true)))
}
impl Drop for OwnerRead {
    fn drop(&mut self) {
        OWNER_READ.with(|r| r.set(self.0));
    }
}
pub(super) fn open(root: &Path, mode: OpenMode) -> Result<Option<Store>> {
    let key = key(root)?;
    // An owner's screen read waits for an exclusive migration or initialization;
    // actions (even their read-only prechecks) never queue: they refuse `busy` at once.
    let opened = if mode == OpenMode::LiveReadOnly && OWNER_READ.with(|r| r.get()) {
        Store::open_waiting(key.clone(), mode)
    } else {
        Store::open(key.clone(), mode)
    };
    match opened {
        Ok(store) => Ok(Some(store)),
        Err(e) if crate::local::store_missing(&key, &e) => Ok(None),
        Err(e) => Err(core(e)),
    }
}
/// Open without waiting for an exclusive lock: for mutations, which refuse `busy` at once.
pub(super) fn open_now(root: &Path, mode: OpenMode) -> Result<Option<Store>> {
    let key = key(root)?;
    match Store::open(key.clone(), mode) {
        Ok(store) => Ok(Some(store)),
        Err(e) if crate::local::store_missing(&key, &e) => Ok(None),
        Err(e) => Err(core(e)),
    }
}
pub(super) fn checkout_unavailable(error: CoreError) -> Refused {
    refuse(error.code(), "The task's worktree is unavailable. Restore the original checkout or use task history; the project folder is not a replacement.")
}

pub(super) fn owned_checkout(
    root: &Path, project: &str, task: &str,
) -> Result<Option<cortex_core::workspace::worktrees::Worktree>> {
    use cortex_core::workspace::worktrees;
    let Some(store) = open(root, OpenMode::LiveReadOnly)? else { return Ok(None); };
    store.read(|db| {
        let Some(_) = worktrees::owned_on(db, project, task)? else { return Ok(None); };
        let checkout = worktrees::desktop_checkout_on(db, store.key(), project, Some(task), None)?;
        checkout.and_then(|c| c.worktree).map(Some).ok_or(CoreError::Refused("worktree_binding_invalid"))
    }).map_err(checkout_unavailable)
}

pub(super) fn set_worktree(record: &mut Value, w: &cortex_core::workspace::worktrees::Worktree) {
    let shared = record["checkout"]["shared"] == true;
    record["checkout"] = json!({"id":w.id,"kind":"worktree","path":w.path,"branch":w.branch});
    record["baseRevision"] = json!(w.base_sha);
    if shared {
        // A member of a shared sprint worktree finishes onto the sprint branch (a commit of
        // its own files); the sprint's real target is merged once, when the sprint completes.
        if record["sprintTargetBranch"].is_null() {
            record["sprintTargetBranch"] = record["targetBranch"].clone();
        }
        record["checkout"]["shared"] = json!(true);
        record["targetBranch"] = json!(w.branch);
    }
}

pub async fn product(root: &Path, state: &Path, project: &str, git: &mut Git) -> Result<Value> {
    let (mut value, options) = match open(root, OpenMode::LiveReadOnly)? {
        None => (super::native::product(state, project)?, Vec::new()),
        Some(store) => {
            let path = product_path(state, project)?;
            let mut snapshot = None;
            for _ in 0..3 {
                let (mut saved, file) = settings::read_json_snapshot(&path, json!({}))?;
                let observed = store.read(|db| {
                    let mut query = db.prepare("SELECT t.task_id,o.options_json FROM workspace_tasks t
                        JOIN workspace_task_options o USING(task_id) WHERE t.project_id=? ORDER BY t.sequence LIMIT 10001")?;
                    let rows = query.query_map([project], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?
                        .collect::<std::result::Result<Vec<_>,_>>()?;
                    if rows.len() > 10000 { return Err(CoreError::Refused("machine_reply_too_large")); }
                    // A publication can replace the JSON before committing the options.
                    // Read its receipt in this SQLite snapshot and retry if the file was
                    // replaced while this snapshot was being established.
                    if !settings::json_snapshot_is_current(&path, file.as_ref())
                        .map_err(|e| CoreError::Refused(e.code))?
                    {
                        return Ok(None);
                    }
                    super::sprint_materialize::resolve_publication_on(db, project, &mut saved)?;
                    let options = rows.into_iter().map(|(task, options)| {
                        let owned = cortex_core::workspace::worktrees::owned_on(db, project, &task)?;
                        let unavailable = owned.as_ref().and_then(|_| {
                            cortex_core::workspace::worktrees::desktop_checkout_on(
                                db, store.key(), project, Some(&task), None
                            ).err()
                        });
                        let missing = unavailable.is_some();
                        let cleanup_retry_ready = if owned.is_some() {
                            cortex_core::workspace::worktrees::close_cleanup_retry_ready_on(
                                db, store.key(), project, &task,
                                !missing || matches!(unavailable, Some(CoreError::Refused("worktree_closed"))),
                            )?
                        } else { None };
                        Ok((task, options, owned, missing, cleanup_retry_ready))
                    }).collect::<CoreResult<Vec<_>>>()?;
                    Ok(Some(options))
                }).map_err(core)?;
                if let Some(options) = observed {
                    snapshot = Some((saved, options));
                    break;
                }
            }
            let (saved, rows) = snapshot
                .ok_or_else(|| refuse("machine_busy", "Saved project state changed. Refresh the project."))?;
            let mut value = super::native::normalize_product(state, project, saved)?;
            if !value["tasks"].is_object() {
                return Err(refuse("state_unreadable", "Saved tasks are unreadable."));
            }
            super::task_rework::project(&store, project, &mut value, false)?;
            owner_closed(&store, project, &mut value)?;
            // Files attached to answers are kept like message files. An answer recorded
            // before this was kept becomes referenced here; the save happens now when the
            // machine lock is free, otherwise with the caller's own save.
            if super::attachment_files::answer_references(&store, state, project, &mut value)? {
                if let Ok(Some(_lock)) = settings::Lock::try_new(state, "machine.lock") {
                    let path = product_path(state, project)?;
                    let mut fresh = settings::read_json(&path, json!({}))?;
                    super::sprint_materialize::resolve_publication(project, &mut fresh)?;
                    if super::attachment_files::answer_references(
                        &store, state, project, &mut fresh,
                    )? {
                        settings::write_json(&path, &fresh)?;
                    }
                }
            }
            (value, rows)
        }
    };
    if !value["tasks"].is_object() {
        return Err(refuse("state_unreadable", "Saved tasks are unreadable."));
    }
    let has_git = git.is_repo(root).await?;
    let mut branch = None;
    let mut base = None;
    let mut trees = None;
    for (task, raw, owned, missing, cleanup_retry_ready) in options {
        let options: Value = serde_json::from_str(&raw)
            .map_err(|_| refuse("state_unreadable", "Task options are unreadable."))?;
        let record = &mut value["tasks"][&task];
        if !record.is_object() {
            *record = json!({});
        }
        for name in ["checkout", "settings", "baseRevision"] {
            if let Some(v) = options.get(name) {
                record[name] = v.clone();
            }
        }
        if let Some(title) = options.get("draftTitle") {
            record["title"] = title.clone();
            record["titleState"] = json!("user");
        }
        if let Some(w) = owned {
            set_worktree(record, &w);
            if missing { record["checkout"]["unavailable"] = json!(true); }
            if let Some(ready) = cleanup_retry_ready {
                record["checkout"]["cleanupRetryReady"] = json!(ready);
            }
        } else if record["checkout"]["kind"] == "main" {
            if branch.is_none() {
                branch = Some(if has_git {
                    git.out(root, &["branch", "--show-current"])
                        .await?
                        .trim()
                        .to_owned()
                } else {
                    String::new()
                });
            }
            record["checkout"]["branch"] = json!(branch);
        } else if record["checkout"]["kind"] == "worktree"
            && record["checkout"]["path"].is_null()
            && has_git
        {
            if base.is_none() {
                base = Some(worktree_base(root, state, git).await?);
            }
            let legacy = base.as_ref().unwrap().join(&task);
            let suffix = format!("-{}", &files::revision(task.as_bytes())[..12]);
            if trees.is_none() {
                trees = Some(git.trees(root).await?);
            }
            let mut matching = trees.as_ref().unwrap().iter().filter(|tree| {
                tree["path"]
                    .as_str()
                    .is_some_and(|p| Path::new(p) == legacy)
                    || tree["branch"]
                        .as_str()
                        .is_some_and(|b| b.starts_with("cw/") && b.ends_with(&suffix))
            });
            let tree = matching.next();
            if matching.next().is_some() {
                return Err(refuse(
                    "unknown_checkout",
                    "The task checkout identity is ambiguous.",
                ));
            }
            if let Some(tree) = tree {
                record["checkout"]["path"] = tree["path"].clone();
                record["checkout"]["branch"] = tree["branch"].clone();
            } else {
                record["checkout"]["path"] = json!(legacy);
                record["checkout"]["branch"] = json!(format!("cw/{task}"));
            }
        }
    }
    value["hasGit"] = json!(has_git);
    value["ghAvailable"] = json!(settings::available("gh"));
    if serde_json::to_vec(&value)
        .map_err(|_| refuse("state_unreadable", "Invalid state."))?
        .len()
        > FRAME_BUDGET
    {
        return Err(refuse(
            "machine_reply_too_large",
            "Project facts exceed the reply limit.",
        ));
    }
    Ok(value)
}
/// A task the owner finished through the runtime alone (Finish on its stop question, or a
/// Close task whose saved disposition was never written) is the owner's close as is: it
/// reads, archives, settles its sprint and is cleaned up like one. A saved status or
/// archive always wins; nothing here certifies the work.
pub(super) fn owner_closed(store: &Store, project: &str, value: &mut Value) -> Result<()> {
    let key = store.key().root().to_string_lossy().into_owned();
    let closed = store
        .read(|db| cortex_core::projections::owner_closed(db, project, &key))
        .map_err(core)?;
    for (task, at, external) in closed {
        let record = &mut value["tasks"][&task];
        if !record.is_object() {
            *record = json!({});
        }
        if (!external && !record["status"].is_null()) || !record["archive"].is_null() {
            continue;
        }
        record["status"] = json!("done");
        if external {
            record["closedAs"] = json!("external");
            record["acceptance"] = Value::Null;
            record["integration"] = Value::Null;
            record["automaticReady"] = json!(false);
            record["work"] = json!("unmerged");
        } else if record["closedAs"].is_null() {
            record["closedAs"] = json!("as_is");
        }
        record["notVerifiedByCortex"] = json!(true);
        if external || record["doneAt"].is_null() {
            record["doneAt"] = json!(at
                .and_then(|n| u64::try_from(n).ok())
                .unwrap_or_else(now_ms));
        }
    }
    Ok(())
}
async fn worktree_base(root: &Path, state: &Path, git: &mut Git) -> Result<PathBuf> {
    let common = git
        .out(
            root,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .await?;
    let group = files::revision(common.trim().as_bytes());
    let settings = settings::machine(state)?;
    let directory = string(&settings, "worktreeRoot")?;
    let directory = if let Some(tail) = directory.strip_prefix("~/") {
        PathBuf::from(
            std::env::var_os("HOME")
                .ok_or_else(|| refuse("home_unavailable", "Home is unavailable."))?,
        )
        .join(tail)
    } else {
        PathBuf::from(directory)
    };
    Ok(directory.join(&group[..16]))
}

/// The transaction containing this query also contains the editor replacement.
/// Admission of a new Start cannot race the writer check.
fn writer(
    db: &rusqlite::Connection,
    project: &str,
    root: &Path,
    checkout: &Path,
    base: Option<&Path>,
) -> CoreResult<Option<String>> {
    let mut query = db.prepare(
        "SELECT t.task_id,o.options_json FROM workspace_tasks t
        LEFT JOIN workspace_task_options o USING(task_id)
        LEFT JOIN workspace_executions e ON e.execution_id=t.execution_id
        LEFT JOIN runtime_tasks r ON r.task_id=e.runtime_task_id
        WHERE t.project_id=? AND t.execution_id IS NOT NULL
        AND (r.state IS NULL OR r.state NOT IN ('COMPLETED','FAILED','CANCELLED','PAUSED'))
        ORDER BY t.sequence LIMIT 10001",
    )?;
    let rows = query
        .query_map([project], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if rows.len() > 10000 {
        return Err(CoreError::Refused("writer_unconfirmed"));
    }
    for (task, raw) in rows {
        if let Some(binding) = cortex_core::workspace::worktrees::owned_on(db, project, &task)? {
            if binding.path == checkout { return Ok(Some(task)); }
            continue;
        }
        let raw = raw.ok_or(CoreError::Refused("writer_unconfirmed"))?;
        let options: Value = serde_json::from_str(&raw)?;
        if !options.is_object()
            || options.get("checkout").is_some_and(|c| {
                !c.is_object() || !matches!(c["kind"].as_str(), Some("main" | "worktree"))
            })
        {
            return Err(CoreError::Refused("writer_unconfirmed"));
        }
        let target = if options["checkout"]["kind"]
            .as_str()
            .is_none_or(|k| k == "main")
        {
            root.to_path_buf()
        } else if let Some(path) = options["checkout"]["path"].as_str() {
            PathBuf::from(path)
        } else {
            let legacy = base
                .ok_or(CoreError::Refused("writer_unconfirmed"))?
                .join(&task);
            if !legacy.is_dir() {
                return Err(CoreError::Refused("writer_unconfirmed"));
            }
            legacy
        };
        if target == checkout || target.canonicalize().ok().as_deref() == Some(checkout) {
            return Ok(Some(task));
        }
    }
    Ok(None)
}
pub(super) fn require_idle_checkout(root: &Path, project: &str, checkout: &Path) -> Result<()> {
    let Some(store) = open(root, OpenMode::LiveReadOnly)? else { return Ok(()); };
    if store.read(|db| writer(db, project, root, checkout, None)).map_err(core)?.is_some() {
        return Err(refuse("checkout_busy", "A task is still using this checkout. Wait for it to stop before changing it."));
    }
    Ok(())
}

pub(super) async fn file_checkout(
    root: &Path,
    project: &str,
    params: &Value,
    git: &mut Git,
) -> Result<(String, PathBuf)> {
    let wanted = params["checkoutId"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{project}:main"));
    let checkout = git.resolve(root, project, &wanted).await?;
    if let Some(task) = params["taskId"].as_str() {
        if let Some(binding) = owned_checkout(root, project, task)? {
            if binding.id != wanted || binding.path != checkout {
                return Err(refuse("worktree_binding_invalid", "Select the task's original checkout for this file."));
            }
        }
    }
    Ok((wanted, checkout))
}
/// A saved task's files as references: each body is stored privately here first.
pub fn attachments(root: &Path, state: &Path, project: &str, task: &str) -> Result<Value> {
    let store = open(root, OpenMode::LiveReadOnly)?
        .ok_or_else(|| refuse("unknown_task", "Unknown task."))?;
    let files=store.read(|db| {
        let raw: Option<String>=db.query_row("SELECT o.options_json FROM workspace_task_options o JOIN workspace_tasks t USING(task_id)
            WHERE t.project_id=? AND t.task_id=?",(project,task),|r|r.get(0)).optional()?;
        let value:Value=serde_json::from_str(&raw.ok_or(CoreError::Refused("unknown_task"))?)?;
        Ok(value.get("attachments").cloned().unwrap_or(json!([])))
    }).map_err(core)?;
    super::product::attachments(&files)?;
    let mut wrapped = json!({"attachments":files});
    super::attachment_files::externalize(state, &mut wrapped)?;
    Ok(wrapped["attachments"].take())
}

/// Whether a task is a member of a sprint that shares one worktree (saved by the host
/// at materialization; the product overlay need not carry it).
pub(super) fn shared_member(root: &Path, project: &str, task: &str) -> Result<bool> {
    let Some(store) = open(root, OpenMode::LiveReadOnly)? else { return Ok(false); };
    store
        .read(|db| {
            Ok(db
                .query_row(
                    "SELECT json_extract(o.options_json,'$.sprintWorktree.sprintId') IS NOT NULL
                     FROM workspace_tasks t JOIN workspace_task_options o USING(task_id)
                     WHERE t.project_id=? AND t.task_id=?",
                    (project, task),
                    |r| r.get::<_, bool>(0),
                )
                .optional()?
                .unwrap_or(false))
        })
        .map_err(core)
}
