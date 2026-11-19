//! Core-owned repository context. Read-only discovery never initializes a store.
use super::{files, git::Git, native::*, settings};
use cortex_core::{CoreError, CoreResult, OpenMode, Store};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub(super) fn core(error: CoreError) -> Refused {
    refuse(
        error.code(),
        "The project store could not confirm this operation.",
    )
}
pub fn key(root: &Path) -> Result<cortex_core::ProjectKey> {
    crate::native_runtime::key(root).map_err(core)
}
pub(super) fn open(root: &Path, mode: OpenMode) -> Result<Option<Store>> {
    let key = key(root)?;
    match key.database().symlink_metadata() {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
        Ok(_) => Store::open(key, mode).map(Some).map_err(core),
    }
}
pub async fn product(root: &Path, state: &Path, project: &str, git: &mut Git) -> Result<Value> {
    let mut value = super::native::product(state, project)?;
    if !value["tasks"].is_object() {
        return Err(refuse("state_unreadable", "Saved tasks are unreadable."));
    }
    let options = match open(root, OpenMode::LiveReadOnly)? {
        None => Vec::new(),
        Some(store) => {
            let rows = store.read(|db| {
            let mut query = db.prepare("SELECT t.task_id,o.options_json FROM workspace_tasks t
                JOIN workspace_task_options o USING(task_id) WHERE t.project_id=? ORDER BY t.sequence LIMIT 10001")?;
            let rows = query.query_map([project], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?
                .collect::<std::result::Result<Vec<_>,_>>()?;
            if rows.len() > 10000 { return Err(CoreError::Refused("machine_reply_too_large")); }
            Ok(rows)
        }).map_err(core)?;
            owner_closed(&store, project, &mut value)?;
            // Files attached to answers are kept like message files. An answer recorded
            // before this was kept becomes referenced here; the save happens now when the
            // machine lock is free, otherwise with the caller's own save.
            if super::attachment_files::answer_references(&store, state, project, &mut value)? {
                if let Ok(Some(_lock)) = settings::Lock::try_new(state, "machine.lock") {
                    let path = product_path(state, project)?;
                    let mut fresh = settings::read_json(&path, json!({}))?;
                    if super::attachment_files::answer_references(
                        &store, state, project, &mut fresh,
                    )? {
                        settings::write_json(&path, &fresh)?;
                    }
                }
            }
            rows
        }
    };
    let has_git = git.is_repo(root).await?;
    let mut branch = None;
    let mut base = None;
    let mut trees = None;
    for (task, raw) in options {
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
        if record["checkout"]["kind"] == "main" {
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
fn owner_closed(store: &Store, project: &str, value: &mut Value) -> Result<()> {
    let key = store.key().root().to_string_lossy().into_owned();
    let closed = store
        .read(|db| cortex_core::projections::owner_closed(db, project, &key))
        .map_err(core)?;
    for (task, at) in closed {
        let record = &mut value["tasks"][&task];
        if !record.is_object() {
            *record = json!({});
        }
        if !record["status"].is_null() || !record["archive"].is_null() {
            continue;
        }
        record["status"] = json!("done");
        if record["closedAs"].is_null() {
            record["closedAs"] = json!("as_is");
        }
        record["notVerifiedByCortex"] = json!(true);
        if record["doneAt"].is_null() {
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
pub async fn file(
    root: &Path,
    state: &Path,
    project: &str,
    params: &Value,
    save: bool,
    git: &mut Git,
) -> Result<Value> {
    let wanted = params["checkoutId"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{project}:main"));
    let checkout = git.resolve(root, project, &wanted).await?;
    let name = string(params, "file")?;
    relative(name)?;
    if git.is_repo(&checkout).await? {
        if let Some(driver) = git.filters(&checkout, &[name.to_owned()]).await?.get(name) {
            if save {
                return Err(refuse(
                    "filtered_file",
                    "Filtered files cannot be edited here.",
                ));
            }
            return Ok(json!({"file":name,"text":null,"revision":null,
                "reason":format!("Git filter '{driver}'-managed file. Editing is unavailable; no filter is run.")}));
        }
    }
    let Some(store) = open(
        root,
        if save {
            OpenMode::RuntimeValidate
        } else {
            OpenMode::LiveReadOnly
        },
    )?
    else {
        return if save {
            files::save(&checkout, state, params)
        } else {
            files::read(&checkout, params)
        };
    };
    let base = if git.is_repo(root).await? {
        Some(worktree_base(root, state, git).await?)
    } else {
        None
    };
    if save {
        store
            .write(|db| {
                if writer(db, project, root, &checkout, base.as_deref())?.is_some() {
                    return Err(CoreError::Refused("checkout_busy"));
                }
                files::save(&checkout, state, params).map_err(|e| CoreError::Refused(e.code))
            })
            .map_err(core)
    } else {
        let held = store
            .read(|db| writer(db, project, root, &checkout, base.as_deref()))
            .map_err(core)?;
        let mut reply = files::read(&checkout, params)?;
        if held.is_some() {
            reply["writer"] = json!(held);
            reply["readOnly"] =
                json!("Cortex is still writing in this checkout. Stop it before editing.");
        }
        Ok(reply)
    }
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
