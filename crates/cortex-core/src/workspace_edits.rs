//! Host-owned saved draft changes. The write transaction arbitrates against Start.
//! A rename changes only the owner-visible title and is allowed after Start too.
use crate::{error::refuse, store::append_public_tx, CoreError, CoreResult, Store};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};

pub enum Change<'a> {
    Edit {
        expected: &'a str,
        payload: &'a Value,
    },
    Settings(&'a Value),
    Rename(&'a str),
}

/// No scheduler, session, engine or acceptance effects. Caller validates wire payloads.
pub fn change(
    store: &Store,
    project: &str,
    task: &str,
    change: Change<'_>,
) -> CoreResult<Option<String>> {
    store.write(|db| {
        let row: Option<(Option<String>, i64)> = db.query_row(
            "SELECT execution_id,version FROM workspace_tasks WHERE task_id=? AND project_id=?",
            (task, project), |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
        let Some((execution, version)) = row else {
            return match change {
                Change::Rename(_) => Ok(None),
                Change::Settings(_) => refuse("busy"),
                Change::Edit { .. } => refuse("unknown_task"),
            };
        };
        // The title is presentation only: a started task keeps its owner-visible name
        // editable, while its saved request and settings stay fenced by Start.
        if execution.is_some() && !matches!(change, Change::Rename(_)) {
            return refuse(if matches!(change, Change::Settings(_)) {"busy"} else {"task_started"});
        }
        let raw: Option<String> = db.query_row(
            "SELECT options_json FROM workspace_task_options WHERE task_id=?", [task], |r| r.get(0)).optional()?;
        let mut options: Value = match raw { Some(raw) => serde_json::from_str(&raw)?, None => json!({}) };
        let map = options.as_object_mut().ok_or(CoreError::Refused("state_unreadable"))?;
        match change {
            Change::Edit { expected, payload } => {
                let root: String = db.query_row("SELECT project_root FROM workspace_projects WHERE project_id=?",
                    [project], |r| r.get(0))?;
                if root != store.key().root().to_string_lossy() { return refuse("unauthorized"); }
                if version.to_string() != expected { return refuse("stale_version"); }
                let payload = payload.as_object().ok_or(CoreError::Refused("invalid_workspace_request"))?;
                for (key, value) in payload {
                    if key != "description" && key != "title" { map.insert(key.clone(), value.clone()); }
                }
                map.insert("draftTitle".into(), json!(payload["title"].as_str().unwrap_or("").trim()));
                db.execute("UPDATE workspace_tasks SET description=? WHERE task_id=?",
                    (payload["description"].as_str().ok_or(CoreError::Refused("description_required"))?, task))?;
            }
            Change::Settings(settings) => { map.insert("settings".into(), settings.clone()); }
            Change::Rename(title) => { map.insert("draftTitle".into(), json!(title)); }
        }
        db.execute("INSERT INTO workspace_task_options VALUES (?,?) ON CONFLICT(task_id) DO UPDATE SET options_json=excluded.options_json",
            (task, options.to_string()))?;
        db.execute("UPDATE workspace_tasks SET version=version+1 WHERE task_id=?", [task])?;
        db.execute("UPDATE workspace_projects SET version=version+1 WHERE project_id=?", [project])?;
        append_public_tx(db, project, task, "task", version + 1)?;
        Ok(Some((version + 1).to_string()))
    })
}

/// A sprint revision's change of one saved draft, inside the caller's
/// transaction: only a draft with no execution identity at `expected` changes.
/// The title is stored like an owner edit's (`draftTitle`). Returns the new version.
pub fn revise_draft_tx(
    db: &rusqlite::Transaction<'_>,
    project: &str,
    task: &str,
    expected: &str,
    description: Option<&str>,
    title: Option<&str>,
) -> CoreResult<i64> {
    let row: Option<(Option<String>, i64)> = db
        .query_row(
            "SELECT execution_id,version FROM workspace_tasks WHERE task_id=? AND project_id=?",
            (task, project),
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let (execution, version) = row.ok_or(CoreError::Refused("unknown_task"))?;
    let started: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM workspace_starts WHERE task_id=?)",
        [task],
        |r| r.get(0),
    )?;
    if execution.is_some() || started {
        return refuse("task_started");
    }
    if version.to_string() != expected {
        return refuse("stale_version");
    }
    if let Some(description) = description {
        if description.trim().is_empty() {
            return refuse("description_required");
        }
        db.execute(
            "UPDATE workspace_tasks SET description=? WHERE task_id=?",
            (description, task),
        )?;
    }
    if let Some(title) = title {
        let raw: Option<String> = db
            .query_row(
                "SELECT options_json FROM workspace_task_options WHERE task_id=?",
                [task],
                |r| r.get(0),
            )
            .optional()?;
        let mut options: Value = match raw {
            Some(raw) => serde_json::from_str(&raw)?,
            None => json!({}),
        };
        options
            .as_object_mut()
            .ok_or(CoreError::Refused("state_unreadable"))?
            .insert("draftTitle".into(), json!(title.trim()));
        db.execute("INSERT INTO workspace_task_options VALUES (?,?) ON CONFLICT(task_id) DO UPDATE SET options_json=excluded.options_json",
            (task, options.to_string()))?;
    }
    db.execute("UPDATE workspace_tasks SET version=version+1 WHERE task_id=?", [task])?;
    db.execute("UPDATE workspace_projects SET version=version+1 WHERE project_id=?", [project])?;
    append_public_tx(db, project, task, "task", version + 1)?;
    Ok(version + 1)
}
