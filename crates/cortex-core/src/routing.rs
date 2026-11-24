//! Trusted routing evidence. Neither plans nor tool arguments can manufacture consent.
use crate::{lifecycle::event_tx, CoreResult};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{
    fs::OpenOptions,
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};

fn instruction(root: &Path, name: &str) -> Option<(Value, String)> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(root.join(name))
        .ok()?;
    let m = file.metadata().ok()?;
    if !m.is_file() || m.len() > 131072 || m.nlink() != 1 {
        return None;
    }
    let mut text = String::new();
    file.by_ref().take(131073).read_to_string(&mut text).ok()?;
    if text.len() > 131072 {
        return None;
    }
    let after = file.metadata().ok()?;
    if m.ctime() != after.ctime() || m.ctime_nsec() != after.ctime_nsec() || m.len() != after.len()
    {
        return None;
    }
    Some((
        json!({"digest":crate::digest(text.as_bytes()),"inode":m.ino(),"device":m.dev(),
        "ctime":m.ctime(),"ctime_ns":m.ctime_nsec(),"mtime":m.mtime(),"mtime_ns":m.mtime_nsec()}),
        text,
    ))
}
/// Snapshot identities, not instruction bodies, in the task creation transaction.
pub fn snapshot_tx(db: &Connection, task: &str, root: &Path) -> CoreResult<()> {
    let mut files = json!({});
    for name in ["AGENTS.md", "CLAUDE.md"] {
        files[name] = instruction(root, name)
            .map(|(v, _)| v)
            .unwrap_or(Value::Null);
    }
    event_tx(db, task, "ROUTING_INSTRUCTIONS_SNAPSHOTTED", files)?;
    Ok(())
}
#[derive(Clone)]
pub struct UserInput {
    pub text: String,
    pub source_ref: String,
    /// Only a recorded answer may select a bare model name.
    pub question: Option<String>,
    pub selected_option: bool,
    /// The option label alone for an answer with a retained comment in `text`.
    pub option_with_comment: Option<String>,
}
pub struct Instruction {
    pub path: String,
    pub text: String,
    pub changed: bool,
}
#[derive(Default)]
pub struct Evidence {
    pub inputs: Vec<UserInput>,
    pub instructions: Vec<Instruction>,
    pub unavailable: Vec<Value>,
}
fn choice(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
        .trim_end_matches(['.', '!'])
        .to_string()
}
/// Match the two stored forms: a whole answer or the exact label followed by
/// the blank line that `workspace_control` puts before the owner's comment.
fn selected_choice<'a>(text: &str, options: &'a [Value]) -> (bool, Option<&'a str>) {
    let normalized = choice(text);
    if normalized.is_empty() {
        return (false, None);
    }
    let mut matches = options
        .iter()
        .filter_map(crate::lifecycle::waits::option_label)
        .filter_map(|label| {
            if choice(label) == normalized {
                Some(None)
            } else if !label.trim().is_empty()
                && text
                    .strip_prefix(label)
                    .and_then(|tail| tail.strip_prefix("\n\n"))
                    .is_some_and(|comment| !comment.trim().is_empty())
            {
                Some(Some(label))
            } else {
                None
            }
        });
    match (matches.next(), matches.next()) {
        (Some(label), None) => (true, label),
        _ => (false, None),
    }
}
/// Exact recorded owner input only. Question wording is not appended to answer text.
pub fn evidence(db: &Connection, task: &str, root: &Path) -> CoreResult<Evidence> {
    let mut result = Evidence::default();
    let goal: String = db.query_row(
        "SELECT goal FROM runtime_tasks WHERE task_id=?",
        [task],
        |r| r.get(0),
    )?;
    let mut inputs = vec![(
        String::new(),
        UserInput {
            text: goal,
            source_ref: format!("{task}:goal"),
            question: None,
            selected_option: false,
            option_with_comment: None,
        },
    )];
    let mut stmt = db.prepare("SELECT id,payload_json,created_at FROM runtime_events WHERE task_id=? AND type='USER_STEERING' ORDER BY id")?;
    for row in stmt.query_map([task], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })? {
        let (id, payload, time) = row?;
        let v: Value = serde_json::from_str(&payload)?;
        if let Some(text) = v["text"].as_str() {
            inputs.push((
                time,
                UserInput {
                    text: text.into(),
                    source_ref: format!("event:{id}"),
                    question: None,
                    selected_option: false,
                    option_with_comment: None,
                },
            ));
        }
    }
    let mut stmt = db.prepare("SELECT id,text,options_json,answer_text,answered_at FROM pending_questions
        WHERE task_id=? AND status='answered' AND answer_source IS NOT NULL ORDER BY answered_at,id")?;
    for row in stmt.query_map([task], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
        ))
    })? {
        let (id, question, options, text, time) = row?;
        let options = options.and_then(|s| serde_json::from_str::<Vec<Value>>(&s).ok());
        let (selected_option, option_with_comment) = options
            .as_deref()
            .map_or((false, None), |options| selected_choice(&text, options));
        inputs.push((
            time,
            UserInput {
                text,
                source_ref: format!("{id}:answer"),
                question: Some(question),
                selected_option,
                option_with_comment: option_with_comment.map(str::to_owned),
            },
        ));
    }
    inputs.sort_by(|a, b| a.0.cmp(&b.0));
    result.inputs = inputs.into_iter().map(|(_, i)| i).collect();
    let saved: Option<String> = db
        .query_row(
            "SELECT payload_json FROM runtime_events WHERE task_id=?
        AND type='ROUTING_INSTRUCTIONS_SNAPSHOTTED' ORDER BY id LIMIT 1",
            [task],
            |r| r.get(0),
        )
        .optional()?;
    let saved: Value = saved
        .map(|s| serde_json::from_str(&s))
        .transpose()?
        .unwrap_or(Value::Null);
    for name in ["AGENTS.md", "CLAUDE.md"] {
        let current = instruction(root, name);
        let same = current.as_ref().is_some_and(|(v, _)| saved[name] == *v);
        // Missing old evidence cannot authorize a newly appeared requirement.
        if let Some((_, text)) = current {
            result.instructions.push(Instruction {
                path: name.into(),
                text,
                changed: !same,
            });
        } else if !saved[name].is_null() {
            result.instructions.push(Instruction {
                path: name.into(),
                text: String::new(),
                changed: true,
            });
        }
    }
    let mut stmt = db.prepare(
        "SELECT payload_json FROM runtime_events WHERE type='ROUTE_UNAVAILABLE'
        AND julianday(json_extract(payload_json,'$.expires_at'))>julianday('now') ORDER BY id DESC",
    )?;
    for row in stmt.query_map([], |r| r.get::<_, String>(0))? {
        result.unavailable.push(serde_json::from_str(&row?)?);
    }
    Ok(result)
}

/// Engine of the previous dispatch of `node` within plan `revision`, if any. A node
/// id reused by a later revision (a replan) or a later review cycle (the cycle is
/// part of the `_review_`/`_fix_` id) has no previous dispatch, so it is not routed
/// as an escalation.
pub fn previous_dispatch_engine(
    db: &Connection,
    task: &str,
    node: &str,
    revision: i64,
) -> CoreResult<Option<String>> {
    Ok(db
        .query_row(
            "SELECT json_extract(payload_json,'$.engine') FROM runtime_events WHERE task_id=? AND type='NATIVE_ROUTE_SELECTED'
             AND json_extract(payload_json,'$.node')=? AND json_extract(payload_json,'$.revision')=? ORDER BY id DESC LIMIT 1",
            (task, node, revision),
            |r| r.get(0),
        )
        .optional()?)
}
