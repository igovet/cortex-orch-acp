//! Local integration presentation only. Never merge, stage, commit, fetch or push.
use super::task_facts_shadow::cross_branch;
use super::{context, files, git::Git, native::*};
use cortex_core::OpenMode;
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::Path};

type RecordedContent = BTreeMap<String, (Option<String>, Option<i64>)>;

fn recorded_content(
    db: &rusqlite::Connection,
    task: &str,
) -> cortex_core::CoreResult<Option<RecordedContent>> {
    recorded_writes(db, task, false)
}

/// The task's recorded content for its own commit: also takes the bytes its final
/// approved and verified review read (`reviewed_content`).
fn landing_content(
    db: &rusqlite::Connection,
    task: &str,
) -> cortex_core::CoreResult<Option<RecordedContent>> {
    recorded_writes(db, task, true)
}

fn recorded_writes(
    db: &rusqlite::Connection,
    task: &str,
    reviewed: bool,
) -> cortex_core::CoreResult<Option<RecordedContent>> {
    let runtime: Option<String> = db
        .query_row(
            "SELECT e.runtime_task_id FROM workspace_tasks t JOIN workspace_executions e
            ON e.execution_id=t.execution_id WHERE t.task_id=?",
            [task],
            |r| r.get(0),
        )
        .optional()?;
    let Some(runtime) = runtime else {
        return Ok(Some(BTreeMap::new()));
    };
    // Gateway tool receipts name their channel as the actor; the native runtime's own
    // file tools record NATIVE_PROJECT_WRITTEN with the channel in the payload.
    let rows=db.prepare("SELECT e.type,e.payload_json,cast(strftime('%s',e.created_at) as integer),e.id FROM runtime_events e
            JOIN trusted_channels c ON c.task_id=e.task_id AND c.channel_id=CASE e.type WHEN 'TOOL_COMPLETED' THEN e.actor
                ELSE json_extract(e.payload_json,'$.channel_id') END
            WHERE e.task_id=? AND e.type IN ('TOOL_COMPLETED','NATIVE_PROJECT_WRITTEN','NATIVE_PROJECT_CHANGED') ORDER BY e.id LIMIT 1001")?
            .query_map([&runtime],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<i64>>(2)?,r.get::<_,i64>(3)?)))?
            .collect::<std::result::Result<Vec<_>,_>>()?;
    if rows.len() > 1000 {
        return Ok(None);
    }
    let digest = |s: &str| {
        s.len() == 64
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    };
    let mut content = BTreeMap::new();
    // The n-th write of one content to one path by one channel is that channel's
    // n-th completed write request recording it (both commit together).
    let mut seen = std::collections::HashMap::<(String, String, String), i64>::new();
    // The event id of each path's latest recorded write.
    let mut written = std::collections::HashMap::<String, i64>::new();
    for (kind, raw, at, id) in rows {
        let v: Value = serde_json::from_str(&raw)?;
        if let Some(name) = v["path"].as_str() {
            written.insert(name.to_owned(), id);
        }
        // A deletion through the native project_delete tool is the path's latest
        // recorded content: absent. Confirmed only by its completed receipt.
        if kind == "NATIVE_PROJECT_CHANGED" {
            if v["operation"] != "project_delete" {
                continue;
            }
            let Some(name) = v["path"].as_str() else {
                return Ok(None);
            };
            if relative(name).is_err() {
                return Ok(None);
            }
            let channel = v["channel_id"].as_str().unwrap_or_default();
            let confirmed:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=?
                    AND operation='project_delete' AND json_valid(result_json) AND json_extract(result_json,'$.complete')=1
                    AND json_extract(result_json,'$.reply.structuredContent.deleted')=1
                    AND json_extract(result_json,'$.reply.structuredContent.path')=?)",
                    rusqlite::params![&runtime,channel,v["request_key"].as_str().unwrap_or_default(),name],|r|r.get(0))?;
            content.insert(name.to_owned(), (confirmed.then(|| DELETED.to_owned()), at));
            continue;
        }
        if kind == "NATIVE_PROJECT_WRITTEN" {
            let Some(name) = v["path"].as_str() else {
                return Ok(None);
            };
            if relative(name).is_err() {
                return Ok(None);
            }
            let channel = v["channel_id"].as_str().unwrap_or_default().to_owned();
            let sha = v["sha256"]
                .as_str()
                .filter(|s| digest(s))
                .map(str::to_owned);
            // Exact bytes Cortex wrote: the event and its completed request receipt must agree
            // on the path and the full-content digest. Anything else stays unconfirmed.
            let confirmed = match &sha {
                None => false,
                Some(sha) => {
                    let nth = seen
                        .entry((channel.clone(), name.to_owned(), sha.clone()))
                        .or_insert(0);
                    let found:Option<i64>=db.query_row("SELECT 1 FROM runtime_requests WHERE task_id=? AND actor=? AND operation IN ('project_write','project_edit')
                            AND json_valid(result_json) AND json_extract(result_json,'$.complete')=1
                            AND json_extract(result_json,'$.reply.structuredContent.path')=?
                            AND json_extract(result_json,'$.reply.structuredContent.sha256')=? ORDER BY rowid LIMIT 1 OFFSET ?",
                            rusqlite::params![&runtime,&channel,name,sha,*nth],|r|r.get(0)).optional()?;
                    *nth += 1;
                    found.is_some()
                }
            };
            content.insert(name.to_owned(), (sha.filter(|_| confirmed), at));
            continue;
        }
        if !matches!(
            v["operation"].as_str(),
            Some("project_write" | "project_edit" | "project_delete")
        ) {
            continue;
        }
        let Some(name) = v["path"].as_str() else {
            return Ok(None);
        };
        if relative(name).is_err() {
            return Ok(None);
        }
        let sha = v["result"]["full_content_sha256"]
            .as_str()
            .filter(|s| digest(s))
            .map(str::to_owned);
        content.insert(name.to_owned(), (sha, at));
    }
    if reviewed {
        reviewed_content(db, &runtime, &written, &mut content)?;
    }
    Ok(if content.len() > 100 {
        None
    } else {
        Some(content)
    })
}

/// Bytes the task's final approved review read and its verification then passed
/// on are the task's content, even when someone else changed the file after the
/// task wrote it: the task's own review and checks accepted exactly those bytes
/// (owner store 2026-11-23: an owner merge into task files made every acceptance
/// ask to reconcile again). A path the task wrote again after that review keeps
/// its later write; any change after the review produces other bytes, which stay
/// unknown.
fn reviewed_content(
    db: &rusqlite::Connection,
    runtime: &str,
    written: &std::collections::HashMap<String, i64>,
    content: &mut RecordedContent,
) -> cortex_core::CoreResult<()> {
    let packet: Option<(i64, String)> = db
        .query_row(
            "SELECT p.id,p.payload_json FROM runtime_events p
             JOIN runtime_tasks t ON t.task_id=p.task_id
             JOIN runtime_nodes n ON n.task_id=p.task_id AND n.revision=t.revision
               AND n.assignment_ref=json_extract(p.payload_json,'$.assignment_ref')
             WHERE p.task_id=? AND p.type='REVIEW_PACKET' AND t.state='COMPLETED'
               AND n.stage='review' AND n.status='completed'
               AND upper(coalesce(json_extract(n.result_json,'$.verdict'),''))='APPROVE'
               AND EXISTS(SELECT 1 FROM runtime_events v WHERE v.task_id=p.task_id
                 AND v.type='VERIFICATION_PASSED' AND v.id>p.id
                 AND json_extract(v.payload_json,'$.revision')=t.revision)
             ORDER BY p.id DESC LIMIT 1",
            [runtime],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((id, raw)) = packet else {
        return Ok(());
    };
    let packet: Value = serde_json::from_str(&raw)?;
    for file in packet["packet"]["changed_files"]["files"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let (Some(name), Some(sha)) = (file["path"].as_str(), file["current_sha256"].as_str())
        else {
            continue;
        };
        if written.get(name).is_some_and(|w| *w < id) {
            if let Some(entry) = content.get_mut(name) {
                if entry.0.as_deref() != Some(sha) {
                    entry.0 = Some(sha.to_owned());
                }
            }
        }
    }
    Ok(())
}

/// Compare only the final content covered by this task’s attributable receipts.
/// A mismatch describes the recorded bytes, including when another writer later
/// changed them. It does not establish the availability of equivalent code.
pub(super) fn sprint_availability(
    db: &rusqlite::Connection,
    draft: &str,
    recipient: &Path,
) -> cortex_core::CoreResult<Value> {
    let content = recorded_content(db, draft)?;
    let Some(content) = content.filter(|items| !items.is_empty()) else {
        return Ok(
            json!({"scope":"recorded_paths","status":"unknown","reason":"no_complete_write_evidence"}),
        );
    };
    let mut paths = Vec::with_capacity(content.len());
    for (path, (expected, _)) in content {
        let status = match expected {
            None => "unknown",
            Some(expected) => match files::checkout_digest(recipient, &path) {
                Ok(observed)
                    if observed.as_deref()
                        == (expected != DELETED).then_some(expected.as_str()) =>
                {
                    "confirmed"
                }
                Ok(_) => "missing",
                Err(_) => "unknown",
            },
        };
        paths.push(json!({"path":path,"status":status}));
    }
    let status = if paths.iter().any(|p| p["status"] == "missing") {
        "missing"
    } else if paths.iter().any(|p| p["status"] == "unknown") {
        "unknown"
    } else {
        "confirmed"
    };
    Ok(json!({"scope":"recorded_paths","status":status,"paths":paths}))
}

pub async fn read(root: &Path, record: &Value, task: &str, git: &mut Git) -> Result<Value> {
    let mut facts = read_for_finish(root, record, task, git).await?;
    if let (Some(id), Some(store)) = (record["checkout"]["id"].as_str(), context::open(root, OpenMode::LiveReadOnly)?) {
        if let Some(project) = id.split_once(":wt:").map(|(project,_)| project) {
            use cortex_core::workspace::worktrees;
            if let Err(error) = worktrees::cleanup_preflight(&store, project, task) {
                facts["deleteWorktree"] = json!({"available":false,"default":true,"reason":error.code()});
            } else {
                facts["deleteWorktree"] = json!({"available":true,"default":true});
            }
        }
    }
    // The scheduler uses these paths to request repair. Keep the public facts shape stable.
    if let Some(fields) = facts.as_object_mut() {
        fields.remove("repairPaths");
    }
    Ok(facts)
}

pub(super) async fn read_for_finish(
    root: &Path,
    record: &Value,
    task: &str,
    git: &mut Git,
) -> Result<Value> {
    let has_git = git.is_repo(root).await?;
    let target = match record["targetBranch"].as_str().filter(|s| !s.is_empty()) {
        Some(s) => s.to_owned(),
        None if has_git => git.default_branch(root).await?,
        None => String::new(),
    };
    let mut facts = json!({"targetBranch":target,"alreadyInTarget":false,"pushStatus":null});
    if record["checkout"]["unavailable"] == true {
        facts["reason"] = json!("The original task worktree is unavailable.");
        facts["checkoutUnavailable"] = json!(true);
        return Ok(facts);
    }
    if !has_git {
        return Ok(facts);
    }
    // An unobservable containment stays false, never a successful completion.
    let _ = inspect(root, record, task, &target, git, &mut facts).await;
    Ok(facts)
}
async fn inspect(
    root: &Path,
    record: &Value,
    task: &str,
    target: &str,
    git: &mut Git,
    facts: &mut Value,
) -> Result<()> {
    if target.starts_with('-')
        || !git
            .success(root, &["check-ref-format", "--branch", target])
            .await?
        || !git
            .success(
                root,
                &[
                    "show-ref",
                    "--verify",
                    "--quiet",
                    &format!("refs/heads/{target}"),
                ],
            )
            .await?
    {
        return Ok(());
    }
    if !git.out(root, &["remote"]).await?.trim().is_empty() {
        facts["pushStatus"] = json!("unknown");
        let upstream = git
            .out(
                root,
                &[
                    "for-each-ref",
                    "--format=%(upstream)",
                    &format!("refs/heads/{target}"),
                ],
            )
            .await?;
        let upstream = upstream.trim();
        if !upstream.is_empty()
            && git
                .success(root, &["rev-parse", "--verify", upstream])
                .await?
        {
            facts["pushStatus"] = json!(if git
                .out(
                    root,
                    &["rev-list", &format!("{upstream}..refs/heads/{target}")]
                )
                .await?
                .trim()
                .is_empty()
            {
                "pushed"
            } else {
                "not_pushed"
            });
        }
    }
    let path = if record["checkout"]["kind"].as_str().unwrap_or("main") == "main" {
        root.to_path_buf()
    } else {
        let wanted = record["checkout"]["path"].as_str().ok_or_else(|| {
            refuse(
                "unknown_checkout",
                "The task worktree is missing or its branch changed.",
            )
        })?;
        let known = git
            .trees(root)
            .await?
            .into_iter()
            .any(|v| v["path"] == wanted && v["branch"] == record["checkout"]["branch"]);
        if !known {
            return Err(refuse(
                "unknown_checkout",
                "The task worktree is missing or its branch changed.",
            ));
        }
        wanted.into()
    };
    if git.out(&path, &["branch", "--show-current"]).await?.trim() == target {
        same_branch(root, &path, record, task, target, git, facts).await?;
        return Ok(());
    }
    cross_branch(&path, record, target, git, facts).await
}
async fn blob(git: &mut Git, root: &Path, reference: &str, name: &str) -> Result<Option<String>> {
    let spec = format!("{reference}:{name}");
    if !git.success(root, &["cat-file", "-e", &spec]).await? {
        return Ok(None);
    }
    let n = git.out(root, &["cat-file", "-s", &spec]).await?;
    if n.trim().parse::<usize>().ok().is_none_or(|n| n > 2_000_000) {
        return Ok(None);
    }
    let bytes = git.bytes(root, &["cat-file", "blob", &spec]).await?;
    Ok(Some(files::revision(&bytes)))
}
/// Why a task on its target branch cannot be committed from the app: no file-write
/// receipt at all (its engine changed files with its own tools, or it changed none),
/// or receipts that could not be confirmed. Both name where the task works.
fn same_branch_reasons(record: &Value) -> (String, String) {
    let place = if record["checkout"]["kind"].as_str().unwrap_or("main") == "main" {
        "the project folder"
    } else {
        "its checkout"
    };
    let then = "so they cannot be committed from here. Close as is, or commit in Git and use Done outside the app.";
    (
        format!("There is no record of which files this task changed in {place} (only changes made through the app's own file tools are recorded), {then}"),
        format!("Which files this task changed in {place} could not be confirmed, {then}"),
    )
}
/// The runtime's informational answer: an answer was submitted, no work was ever
/// planned or delegated, and nothing in the task asked to write, run or edit (no
/// write or command request and no engine edit, delete, move or execute permission).
/// Any of those keeps the unrecorded-changes wording instead.
fn answered_without_changes(
    db: &rusqlite::Connection,
    task: &str,
) -> cortex_core::CoreResult<bool> {
    let runtime: Option<String> = db
        .query_row(
            "SELECT e.runtime_task_id FROM workspace_tasks t JOIN workspace_executions e
            ON e.execution_id=t.execution_id WHERE t.task_id=?",
            [task],
            |r| r.get(0),
        )
        .optional()?;
    let Some(runtime) = runtime else {
        return Ok(false);
    };
    // A result with no writing turn changed nothing: an answer, or a plan whose
    // workers only read (an investigation and its review). Read-only commands do
    // not count; any writing role, writer turn, write request or native edit keeps
    // it unknown.
    let roles = db
        .prepare("SELECT role,stage FROM runtime_nodes WHERE task_id=?")?
        .query_map([&runtime], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if !roles
        .iter()
        .all(|(role, stage)| cortex_core::scheduler::policy::read_only(role, stage))
    {
        return Ok(false);
    }
    Ok(db.query_row(
        "SELECT (EXISTS(SELECT 1 FROM runtime_events WHERE task_id=?1 AND type='ANSWER_SUBMITTED')
            OR EXISTS(SELECT 1 FROM runtime_tasks WHERE task_id=?1 AND state='COMPLETED'))
        AND NOT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=?1 AND type='NATIVE_WRITER_TURN_STARTED')
        AND NOT EXISTS(SELECT 1 FROM runtime_requests WHERE task_id=?1 AND operation IN
            ('project_write','project_edit','project_mkdir','project_delete','project_skill_link',
             'outside_write','outside_command'))
        AND (EXISTS(SELECT 1 FROM runtime_events WHERE task_id=?1 AND type='ANSWER_SUBMITTED')
            OR EXISTS(SELECT 1 FROM runtime_events WHERE task_id=?1 AND type IN ('TASK_CREATED','NATIVE_TASK_ADOPTED')
                AND json_extract(payload_json,'$.execution_host')='rust'))
        AND NOT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=?1 AND (type='NATIVE_PROJECT_WRITTEN'
            OR (type IN ('NATIVE_PERMISSION_REQUESTED','NATIVE_PERMISSION_AUTO_ALLOWED') AND json_valid(payload_json)
                AND json_extract(payload_json,'$.kind') IN ('edit','delete','move','execute'))))",
        [&runtime],
        |r| r.get(0),
    )?)
}
/// Recorded file writes of one task checked at once (`same_branch`).
const FILE_CHECKS: usize = 8;
/// Where one recorded file write stands for a same-branch commit.
enum Landing {
    /// The target branch holds it, or it landed in the target's history.
    Landed,
    /// The working folder holds exactly the recorded content, uncommitted.
    Pending,
    /// It changed again, is staged differently, changed mode or cannot be located.
    Unknown,
    /// The inspection deadline passed first.
    Deadline,
}
/// The recorded content of a path the task deleted (never a content digest).
const DELETED: &str = "deleted";
/// One recorded write: its path, the content digest it recorded and when.
struct Recorded<'a> {
    name: &'a str,
    wanted: Option<&'a str>,
    at: Option<i64>,
}
async fn landing(
    git: &mut Git,
    path: &Path,
    target: &str,
    base: Option<&str>,
    file: Recorded<'_>,
    deadline: tokio::time::Instant,
) -> Result<Landing> {
    let Recorded { name, wanted, at } = file;
    if tokio::time::Instant::now() >= deadline {
        return Ok(Landing::Deadline);
    }
    // A recorded deletion: the path must still be absent from the folder. Absent from
    // the target too means nothing to commit (created and removed by the task).
    if wanted == Some(DELETED) {
        if path.join(name).symlink_metadata().is_ok() {
            return Ok(Landing::Unknown);
        }
        let spec = format!("{target}:{name}");
        return Ok(if git.success(path, &["cat-file", "-e", &spec]).await? {
            Landing::Pending
        } else {
            Landing::Landed
        });
    }
    if blob(git, path, target, name).await?.as_deref() == wanted {
        return Ok(Landing::Landed);
    }
    let history = base
        .map(|base| format!("{base}..{target}"))
        .unwrap_or_else(|| target.into());
    let log = git
        .out(
            path,
            &[
                "log",
                "-n",
                "128",
                "--format=%H %ct",
                &history,
                "--",
                &format!(":(literal){name}"),
            ],
        )
        .await?;
    for line in log.lines() {
        if tokio::time::Instant::now() >= deadline {
            return Ok(Landing::Deadline);
        }
        let Some((reference, when)) = line.split_once(' ') else {
            continue;
        };
        if base.is_none() && at.is_none_or(|at| when.parse::<i64>().unwrap_or(0) < at) {
            continue;
        }
        if blob(git, path, reference, name).await?.as_deref() == wanted {
            return Ok(Landing::Landed);
        }
    }
    // Descriptor-relative no-follow reading is shared with the editor path.
    let observed =
        super::task_facts::safe_bytes(path, name, 2_000_000).map(|v| files::revision(&v));
    let staged = git
        .out(
            path,
            &[
                "diff",
                "--cached",
                "--name-only",
                "--",
                &format!(":(literal){name}"),
            ],
        )
        .await?;
    let mode = git
        .out(
            path,
            &[
                "diff",
                "--summary",
                "HEAD",
                "--",
                &format!(":(literal){name}"),
            ],
        )
        .await?;
    Ok(
        if observed.ok().as_deref() != wanted
            || (!staged.trim().is_empty() && blob(git, path, "", name).await?.as_deref() != wanted)
            || mode.contains("mode change")
        {
            Landing::Unknown
        } else {
            Landing::Pending
        },
    )
}
async fn same_branch(
    root: &Path,
    path: &Path,
    record: &Value,
    task: &str,
    target: &str,
    git: &mut Git,
    facts: &mut Value,
) -> Result<()> {
    // Says what is missing: a task whose engine edited files with its own tools left no
    // file-write receipt, which is not the same as receipts Cortex could not confirm.
    let (unrecorded, unconfirmed) = same_branch_reasons(record);
    let initial = json!({"sameBranch":true,"alreadyInTarget":false,"commitAvailable":false,
        "pendingPaths":[],"otherUncommitted":false,"reason":unconfirmed});
    for (k, v) in initial.as_object().unwrap() {
        facts[k] = v.clone();
    }
    let Some(store) = context::open(root, OpenMode::LiveReadOnly)? else {
        facts["reason"] = json!(unrecorded);
        return Ok(());
    };
    let content = store
        .read(|db| landing_content(db, task))
        .map_err(context::core)?;
    let Some(content) = content else {
        return Ok(());
    };
    if content.is_empty() {
        // An answer-only result changed nothing: there is nothing to commit, which is
        // not the same as changes nobody recorded. Present only when it applies.
        if store
            .read(|db| answered_without_changes(db, task))
            .map_err(context::core)?
        {
            facts["noChanges"] = json!(true);
            // Nothing to land: Automatic finishes it as a no-op instead of holding.
            facts["alreadyInTarget"] = json!(true);
            facts["pendingPaths"] = json!([]);
            facts["reason"] = json!("This task answered without changing any files, so there is nothing to commit. Close the task when you are done with it.");
        } else {
            facts["reason"] = json!(unrecorded);
        }
        return Ok(());
    }
    if content.values().any(|v| v.0.is_none()) {
        return Ok(());
    }
    // A Git-ignored, untracked file the task wrote (evidence, scratch output) is not
    // deliverable: it is never committed (`git add` refuses it), so it neither blocks
    // the commit nor joins it.
    let mut content = content;
    let mut ignored = Vec::new();
    for name in content.keys() {
        if git
            .success(path, &["check-ignore", "-q", "--", name])
            .await?
        {
            ignored.push(name.clone());
        }
    }
    for name in &ignored {
        content.remove(name);
    }
    if content.is_empty() {
        facts["noChanges"] = json!(true);
        facts["alreadyInTarget"] = json!(true);
        facts["reason"] = Value::Null;
        return Ok(());
    }
    let mut dirty = std::collections::BTreeSet::new();
    for args in [
        &["diff", "--name-only", "-z", "HEAD"][..],
        &["ls-files", "--others", "--exclude-standard", "-z"][..],
    ] {
        for name in git
            .out(path, args)
            .await?
            .split('\0')
            .filter(|s| !s.is_empty() && !RESERVED.contains(&s.split('/').next().unwrap_or("")))
        {
            dirty.insert(name.to_owned());
        }
    }
    facts["otherUncommitted"] = json!(dirty.iter().any(|n| !content.contains_key(n)));
    // An unchanged checkout reuses its last complete answer (`landing_signal`).
    let probe = (path.to_path_buf(), task.to_owned(), target.to_owned());
    let inputs = json!({"base":record["baseRevision"],"dirty":dirty,
        "content":content.iter().map(|(k, (w, at))| json!([k, w, at])).collect::<Vec<_>>()});
    let names: Vec<String> = content.keys().cloned().collect();
    let signal = tokio::task::spawn_blocking(move || {
        landing_signal(&probe.0, &probe.1, &probe.2, &inputs, &names)
    })
    .await
    .ok()
    .flatten();
    let reused = signal.as_ref().and_then(|signal| {
        let recent = LANDINGS.lock().ok()?;
        let (seen, outcome, at) = recent.get(&(path.to_path_buf(), task.to_owned()))?;
        (seen == signal && at.elapsed() < super::task_facts::RECENT).then(|| outcome.clone())
    });
    let (pending, unknown) = match reused {
        Some(outcome) => outcome,
        None => {
            let at = std::time::Instant::now();
            let Some(outcome) = landings(path, record, target, git, &content).await? else {
                // The inspection outran its deadline: nothing is known yet, which is
                // not unconfirmed content. Automatic checks again instead of asking.
                facts["inspectionIncomplete"] = json!(true);
                facts["reason"] = json!(
                    "Cortex is still checking this task's files. It checks them again shortly."
                );
                return Ok(());
            };
            if let (Some(signal), Ok(mut recent)) = (signal, LANDINGS.lock()) {
                // Bounded: an answer older than its reuse window is forgotten.
                recent.retain(|_, (_, _, seen)| seen.elapsed() < super::task_facts::RECENT);
                recent.insert(
                    (path.to_path_buf(), task.to_owned()),
                    (signal, outcome.clone(), at),
                );
            }
            outcome
        }
    };
    facts["otherUncommitted"] = json!(dirty
        .iter()
        .any(|p| !pending.contains(p) && !unknown.contains(p)));
    facts["alreadyInTarget"] = json!(pending.is_empty() && unknown.is_empty());
    facts["commitAvailable"] = json!(!pending.is_empty() && unknown.is_empty());
    facts["pendingPaths"] = json!(pending);
    facts["reason"] = if unknown.is_empty() {
        Value::Null
    } else {
        facts["repairPaths"] = json!(unknown);
        json!("Task files changed again or their recorded content cannot be located. Finish in Git and use Done outside the app, or close as is.")
    };
    Ok(())
}
/// Pending and unknown recorded paths of one same-branch task.
type Landings = (Vec<String>, Vec<String>);
/// Recent complete `landings` per (checkout, task): the signal taken before each, the
/// outcome and when.
type Recent =
    std::collections::HashMap<(std::path::PathBuf, String), (String, Landings, std::time::Instant)>;
static LANDINGS: std::sync::LazyLock<std::sync::Mutex<Recent>> =
    std::sync::LazyLock::new(Default::default);
/// Everything `landings` depends on, cheaply: every ref and HEAD (target history and
/// blobs), the index (staged content), the recorded writes, the dirty paths and the
/// identity, size and change times of each recorded file. None, so never reused, when
/// a recorded file changed in the last two seconds or Git cannot answer.
fn landing_signal(
    path: &Path,
    task: &str,
    target: &str,
    inputs: &Value,
    names: &[String],
) -> Option<String> {
    use sha2::Digest;
    use std::os::unix::fs::MetadataExt;
    let refs = super::task_facts::signal_git(
        path,
        &["for-each-ref", "--format=%(refname)%00%(objectname)"],
    )?;
    let head = super::task_facts::signal_git(path, &["rev-parse", "HEAD"])?;
    let index = super::task_facts::signal_git(path, &["ls-files", "--stage", "-z"])?;
    let mut h = sha2::Sha256::new();
    for part in [
        task.as_bytes(),
        target.as_bytes(),
        &refs,
        &head,
        &index,
        inputs.to_string().as_bytes(),
    ] {
        h.update((part.len() as u64).to_be_bytes());
        h.update(part);
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs() as i64;
    for name in names {
        h.update(name.as_bytes());
        match std::fs::symlink_metadata(path.join(name)) {
            Ok(m) => {
                if m.mtime() >= now - 2 || m.ctime() >= now - 2 {
                    return None;
                }
                for v in [m.dev(), m.ino(), m.mode() as u64, m.size()] {
                    h.update(v.to_be_bytes());
                }
                for v in [m.mtime(), m.mtime_nsec(), m.ctime(), m.ctime_nsec()] {
                    h.update(v.to_be_bytes());
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => h.update(b"missing"),
            Err(_) => return None,
        }
    }
    Some(format!("{:x}", h.finalize()))
}
/// Where each recorded write of a same-branch task stands. None when the inspection
/// deadline passed first: that answer is never reused.
async fn landings(
    path: &Path,
    record: &Value,
    target: &str,
    git: &mut Git,
    content: &BTreeMap<String, (Option<String>, Option<i64>)>,
) -> Result<Option<Landings>> {
    let mut pending = Vec::new();
    let mut unknown = Vec::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(4);
    // Each file's checks are independent Git reads: a few files run at once and their
    // outcomes apply in path order, as one pass would. A deadline or an error at any
    // file leaves the facts as set above (pending and unknown are only written below).
    let gate = std::sync::Arc::new(tokio::sync::Semaphore::new(FILE_CHECKS));
    let base = record["baseRevision"].as_str().map(str::to_owned);
    let mut checks = tokio::task::JoinSet::new();
    for (index, (name, (wanted, at))) in content.iter().enumerate() {
        let (gate, mut git, path, target) = (
            gate.clone(),
            git.clone(),
            path.to_path_buf(),
            target.to_owned(),
        );
        let (base, name, wanted, at) = (base.clone(), name.clone(), wanted.clone(), *at);
        checks.spawn(async move {
            let _slot = gate.acquire_owned().await;
            let file = Recorded {
                name: &name,
                wanted: wanted.as_deref(),
                at,
            };
            (
                index,
                landing(&mut git, &path, &target, base.as_deref(), file, deadline).await,
            )
        });
    }
    let mut outcomes: Vec<Option<Landing>> = (0..content.len()).map(|_| None).collect();
    while let Some(joined) = checks.join_next().await {
        let (index, outcome) =
            joined.map_err(|_| refuse("git_failed", "A Git inspection did not finish."))?;
        outcomes[index] = Some(outcome?);
    }
    for (name, outcome) in content.keys().zip(outcomes) {
        match outcome {
            Some(Landing::Landed) => {}
            Some(Landing::Pending) => pending.push(name.clone()),
            Some(Landing::Unknown) => unknown.push(name.clone()),
            Some(Landing::Deadline) | None => return Ok(None),
        }
    }
    Ok(Some((pending, unknown)))
}

#[cfg(test)]
mod sprint_availability_tests {
    use super::*;
    use std::{
        fs,
        process::Command,
        sync::atomic::{AtomicU64, Ordering},
    };

    fn scratch() -> std::path::PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "cortex-sprint-availability-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn git(root: &Path, args: &[&str]) {
        assert!(Command::new("git")
            .current_dir(root)
            .args([
                "-c",
                "user.name=test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false"
            ])
            .args(args)
            .status()
            .unwrap()
            .success());
    }

    fn fixture() -> (rusqlite::Connection, std::path::PathBuf, String) {
        let root = scratch();
        let source = root.join("source");
        let recipient = root.join("recipient");
        fs::create_dir_all(&source).unwrap();
        git(&source, &["init", "-q", "-b", "source"]);
        fs::write(source.join("api.txt"), b"v1\n").unwrap();
        git(&source, &["add", "api.txt"]);
        git(&source, &["commit", "-qm", "base"]);
        git(
            &source,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "consumer",
                recipient.to_str().unwrap(),
                "HEAD",
            ],
        );
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE workspace_tasks(task_id TEXT, execution_id TEXT); CREATE TABLE workspace_executions(execution_id TEXT, runtime_task_id TEXT); CREATE TABLE trusted_channels(task_id TEXT, channel_id TEXT); CREATE TABLE runtime_events(id INTEGER PRIMARY KEY, task_id TEXT, type TEXT, actor TEXT, payload_json TEXT, created_at TEXT DEFAULT CURRENT_TIMESTAMP); CREATE TABLE runtime_requests(task_id TEXT, actor TEXT, request_key TEXT, operation TEXT, result_json TEXT);").unwrap();
        db.execute("INSERT INTO workspace_tasks VALUES ('draft','exec')", [])
            .unwrap();
        db.execute(
            "INSERT INTO workspace_executions VALUES ('exec','runtime')",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO trusted_channels VALUES ('runtime','channel')",
            [],
        )
        .unwrap();
        (db, recipient, source.to_string_lossy().into_owned())
    }

    #[test]
    fn worktree_missing_then_integrated_is_reported_exactly() {
        let (db, recipient, source) = fixture();
        fs::write(std::path::Path::new(&source).join("api.txt"), b"v2\n").unwrap();
        let bytes = fs::read(std::path::Path::new(&source).join("api.txt")).unwrap();
        let sha = files::revision(&bytes);
        db.execute("INSERT INTO runtime_events(id,task_id,type,actor,payload_json) VALUES (1,'runtime','NATIVE_PROJECT_WRITTEN','channel',?)", [json!({"operation":"project_write","path":"api.txt","channel_id":"channel","request_key":"write-1","sha256":sha}).to_string()]).unwrap();
        db.execute("INSERT INTO runtime_requests VALUES ('runtime','channel','write-1','project_write',?)", [json!({"complete":true,"reply":{"structuredContent":{"path":"api.txt","sha256":sha}}}).to_string()]).unwrap();
        let before = sprint_availability(&db, "draft", &recipient).unwrap();
        assert_eq!(before["status"], "missing");
        git(Path::new(&source), &["commit", "-qam", "implement API"]);
        git(&recipient, &["merge", "--ff-only", "-q", "source"]);
        let after = sprint_availability(&db, "draft", &recipient).unwrap();
        assert_eq!(after["status"], "confirmed");
        fs::remove_file(recipient.join("api.txt")).unwrap();
        std::os::unix::fs::symlink(
            Path::new(&source).join("api.txt"),
            recipient.join("api.txt"),
        )
        .unwrap();
        assert_eq!(
            sprint_availability(&db, "draft", &recipient).unwrap()["status"],
            "unknown"
        );
        fs::remove_dir_all(Path::new(&source).parent().unwrap()).unwrap();
    }

    #[test]
    fn unconfirmed_write_and_unreadable_file_stay_unknown() {
        let (db, recipient, source) = fixture();
        let bytes = fs::read(std::path::Path::new(&source).join("api.txt")).unwrap();
        let sha = files::revision(&bytes);
        db.execute("INSERT INTO runtime_events(id,task_id,type,actor,payload_json) VALUES (1,'runtime','NATIVE_PROJECT_WRITTEN','channel',?)", [json!({"operation":"project_write","path":"api.txt","channel_id":"channel","request_key":"write-1","sha256":sha}).to_string()]).unwrap();
        fs::write(recipient.join("api.txt"), bytes).unwrap();
        assert_eq!(
            sprint_availability(&db, "draft", &recipient).unwrap()["status"],
            "unknown"
        );
        let _ = fs::remove_dir_all(std::path::Path::new(&source).parent().unwrap());
    }
}

#[cfg(test)]
mod reviewed_content_tests {
    use super::*;

    fn db() -> rusqlite::Connection {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE runtime_tasks(task_id TEXT, state TEXT, revision INTEGER);
             CREATE TABLE runtime_nodes(task_id TEXT, revision INTEGER, node_id TEXT, stage TEXT,
               status TEXT, assignment_ref TEXT, result_json TEXT);
             CREATE TABLE runtime_events(id INTEGER PRIMARY KEY, task_id TEXT, type TEXT, payload_json TEXT);
             INSERT INTO runtime_tasks VALUES ('t','COMPLETED',2);
             INSERT INTO runtime_nodes VALUES ('t',2,'_review_1','review','completed','ar','{\"verdict\":\"APPROVE\"}');",
        )
        .unwrap();
        db
    }
    fn packet(db: &rusqlite::Connection, id: i64, files: Value) {
        db.execute(
            "INSERT INTO runtime_events VALUES (?,'t','REVIEW_PACKET',?)",
            rusqlite::params![id, json!({"assignment_ref":"ar","packet":{"changed_files":{"files":files}}}).to_string()],
        )
        .unwrap();
    }
    fn verified(db: &rusqlite::Connection, id: i64) {
        db.execute(
            "INSERT INTO runtime_events VALUES (?,'t','VERIFICATION_PASSED','{\"revision\":2}')",
            [id],
        )
        .unwrap();
    }

    /// An owner change merged into a task file after the task wrote it is taken as
    /// the task's content once the task's final approved review read exactly those
    /// bytes and its verification passed; nothing else is adopted.
    #[test]
    fn the_final_approved_reviewed_bytes_are_the_tasks_content() {
        let (a, b, reviewed) = ("a".repeat(64), "b".repeat(64), "c".repeat(64));
        let mut content: RecordedContent = BTreeMap::from([
            ("merged.rs".to_owned(), (Some(a.clone()), None)),
            ("later.rs".to_owned(), (Some(b.clone()), None)),
            ("unlisted.rs".to_owned(), (Some(a.clone()), None)),
        ]);
        let written = std::collections::HashMap::from([
            ("merged.rs".to_owned(), 5),
            ("later.rs".to_owned(), 20),
            ("unlisted.rs".to_owned(), 5),
        ]);
        let db = db();
        packet(&db, 10, json!([{"path":"merged.rs","current_sha256":reviewed},
            {"path":"later.rs","current_sha256":reviewed}]));
        // Without a passed verification after the review nothing changes.
        reviewed_content(&db, "t", &written, &mut content).unwrap();
        assert_eq!(content["merged.rs"].0.as_deref(), Some(a.as_str()));
        verified(&db, 11);
        reviewed_content(&db, "t", &written, &mut content).unwrap();
        assert_eq!(content["merged.rs"].0.as_deref(), Some(reviewed.as_str()));
        // A write after the review keeps the later write; an unlisted path is untouched.
        assert_eq!(content["later.rs"].0.as_deref(), Some(b.as_str()));
        assert_eq!(content["unlisted.rs"].0.as_deref(), Some(a.as_str()));
        // A review that requested changes is never adopted.
        let db = self::db();
        db.execute("UPDATE runtime_nodes SET result_json='{\"verdict\":\"REQUEST_CHANGES\"}'", []).unwrap();
        packet(&db, 10, json!([{"path":"merged.rs","current_sha256":reviewed}]));
        verified(&db, 11);
        let mut fresh: RecordedContent =
            BTreeMap::from([("merged.rs".to_owned(), (Some(a.clone()), None))]);
        reviewed_content(&db, "t", &written, &mut fresh).unwrap();
        assert_eq!(fresh["merged.rs"].0.as_deref(), Some(a.as_str()));
    }
}
