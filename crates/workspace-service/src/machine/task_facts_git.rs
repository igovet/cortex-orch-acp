//! Local integration presentation only. Never merge, stage, commit, fetch or push.
use super::task_facts_shadow::cross_branch;
use super::{context, files, git::Git, native::*};
use cortex_core::OpenMode;
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::Path};

pub async fn read(root: &Path, record: &Value, task: &str, git: &mut Git) -> Result<Value> {
    let has_git = git.is_repo(root).await?;
    let target = match record["targetBranch"].as_str().filter(|s| !s.is_empty()) {
        Some(s) => s.to_owned(),
        None if has_git => git.default_branch(root).await?,
        None => String::new(),
    };
    let mut facts = json!({"targetBranch":target,"alreadyInTarget":false,"pushStatus":null});
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
fn answered_without_changes(db: &rusqlite::Connection, task: &str) -> cortex_core::CoreResult<bool> {
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
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=?1 AND type='ANSWER_SUBMITTED')
        AND NOT EXISTS(SELECT 1 FROM runtime_nodes WHERE task_id=?1)
        AND NOT EXISTS(SELECT 1 FROM delegation_provisioning WHERE task_id=?1 AND role_id!='orchestrator')
        AND NOT EXISTS(SELECT 1 FROM runtime_requests WHERE task_id=?1 AND operation IN
            ('project_write','project_edit','project_mkdir','project_delete','project_skill_link',
             'outside_write','outside_command','command_run'))
        AND NOT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=?1 AND (type='NATIVE_PROJECT_WRITTEN'
            OR (type='NATIVE_PERMISSION_REQUESTED' AND json_valid(payload_json)
                AND json_extract(payload_json,'$.kind') IN ('edit','delete','move','execute'))))",
        [&runtime],
        |r| r.get(0),
    )?)
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
    let content=store.read(|db|{
        let runtime:Option<String>=db.query_row("SELECT e.runtime_task_id FROM workspace_tasks t JOIN workspace_executions e
            ON e.execution_id=t.execution_id WHERE t.task_id=?",[task],|r|r.get(0)).optional()?;
        let Some(runtime)=runtime else {return Ok(Some(BTreeMap::new()));};
        // Gateway tool receipts name their channel as the actor; the native runtime's own
        // file tools record NATIVE_PROJECT_WRITTEN with the channel in the payload.
        let rows=db.prepare("SELECT e.type,e.payload_json,cast(strftime('%s',e.created_at) as integer) FROM runtime_events e
            JOIN trusted_channels c ON c.task_id=e.task_id AND c.channel_id=CASE e.type WHEN 'TOOL_COMPLETED' THEN e.actor
                ELSE json_extract(e.payload_json,'$.channel_id') END
            WHERE e.task_id=? AND e.type IN ('TOOL_COMPLETED','NATIVE_PROJECT_WRITTEN') ORDER BY e.id LIMIT 1001")?
            .query_map([&runtime],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<i64>>(2)?)))?
            .collect::<std::result::Result<Vec<_>,_>>()?;
        if rows.len()>1000{return Ok(None);}
        let digest=|s:&str|s.len()==64&&s.bytes().all(|b|b.is_ascii_digit()||(b'a'..=b'f').contains(&b));
        let mut content=BTreeMap::new();
        // The n-th write of one content to one path by one channel is that channel's
        // n-th completed write request recording it (both commit together).
        let mut seen=std::collections::HashMap::<(String,String,String),i64>::new();
        for(kind,raw,at)in rows {
            let v:Value=serde_json::from_str(&raw)?;
            if kind=="NATIVE_PROJECT_WRITTEN" {
                let Some(name)=v["path"].as_str()else{return Ok(None);};
                if relative(name).is_err(){return Ok(None);}
                let channel=v["channel_id"].as_str().unwrap_or_default().to_owned();
                let sha=v["sha256"].as_str().filter(|s|digest(s)).map(str::to_owned);
                // Exact bytes Cortex wrote: the event and its completed request receipt must agree
                // on the path and the full-content digest. Anything else stays unconfirmed.
                let confirmed=match &sha {
                    None=>false,
                    Some(sha)=>{
                        let nth=seen.entry((channel.clone(),name.to_owned(),sha.clone())).or_insert(0);
                        let found:Option<i64>=db.query_row("SELECT 1 FROM runtime_requests WHERE task_id=? AND actor=? AND operation IN ('project_write','project_edit')
                            AND json_valid(result_json) AND json_extract(result_json,'$.complete')=1
                            AND json_extract(result_json,'$.reply.structuredContent.path')=?
                            AND json_extract(result_json,'$.reply.structuredContent.sha256')=? ORDER BY rowid LIMIT 1 OFFSET ?",
                            rusqlite::params![&runtime,&channel,name,sha,*nth],|r|r.get(0)).optional()?;
                        *nth+=1;
                        found.is_some()
                    }
                };
                content.insert(name.to_owned(),(sha.filter(|_|confirmed),at));
                continue;
            }
            if !matches!(v["operation"].as_str(),Some("project_write"|"project_edit"|"project_delete")){continue;}
            let Some(name)=v["path"].as_str()else{return Ok(None);};
            if relative(name).is_err(){return Ok(None);}
            let sha=v["result"]["full_content_sha256"].as_str().filter(|s|digest(s)).map(str::to_owned);
            content.insert(name.to_owned(),(sha,at));
        }
        Ok(if content.len()>100 {None}else{Some(content)})
    }).map_err(context::core)?;
    let Some(content) = content else {
        return Ok(());
    };
    if content.is_empty() {
        // An answer-only result changed nothing: there is nothing to commit, which is
        // not the same as changes nobody recorded. Present only when it applies.
        if store.read(|db| answered_without_changes(db, task)).map_err(context::core)? {
            facts["noChanges"] = json!(true);
            facts["reason"] = json!("This task answered without changing any files, so there is nothing to commit. Close the task when you are done with it.");
        } else {
            facts["reason"] = json!(unrecorded);
        }
        return Ok(());
    }
    if content.values().any(|v| v.0.is_none()) {
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
    let mut pending = Vec::new();
    let mut unknown = Vec::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(4);
    for (name, (wanted, at)) in &content {
        if tokio::time::Instant::now() >= deadline {
            return Ok(());
        }
        if blob(git, path, target, name).await?.as_ref() == wanted.as_ref() {
            continue;
        }
        let history = record["baseRevision"]
            .as_str()
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
        let mut landed = false;
        for line in log.lines() {
            if tokio::time::Instant::now() >= deadline {
                return Ok(());
            }
            let Some((reference, when)) = line.split_once(' ') else {
                continue;
            };
            if record["baseRevision"].as_str().is_none()
                && at.is_none_or(|at| when.parse::<i64>().unwrap_or(0) < at)
            {
                continue;
            }
            if blob(git, path, reference, name).await?.as_ref() == wanted.as_ref() {
                landed = true;
                break;
            }
        }
        if landed {
            continue;
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
        if observed.ok().as_ref() != wanted.as_ref()
            || (!staged.trim().is_empty()
                && blob(git, path, "", name).await?.as_ref() != wanted.as_ref())
            || mode.contains("mode change")
        {
            unknown.push(name.clone());
        } else {
            pending.push(name.clone());
        }
    }
    facts["otherUncommitted"] = json!(dirty
        .iter()
        .any(|p| !pending.contains(p) && !unknown.contains(p)));
    facts["alreadyInTarget"] = json!(pending.is_empty() && unknown.is_empty());
    facts["commitAvailable"] = json!(!pending.is_empty() && unknown.is_empty());
    facts["pendingPaths"] = json!(pending);
    facts["reason"] = if unknown.is_empty() {
        Value::Null
    } else {
        json!("Task files changed again or their recorded content cannot be located. Finish in Git and use Done outside the app, or close as is.")
    };
    Ok(())
}
