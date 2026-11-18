//! Finish or abort retained merges. Intent persistence precedes Git; uncertain effects never delegate.
use super::{
    git::Git,
    ledger,
    native::*,
    retention,
    settings::{self, Lock},
    workers::truthy,
};
use serde_json::{json, Value};
use std::path::Path;

const IN_PROGRESS: &str = "This task's merge is being finished or aborted. Wait for it to finish.";
/// The merge fields of this task's record, plus the Git facts the read added (kept as
/// the retained overlay did), into the saved product as it is now. Never gives up
/// waiting for the machine lock: the Git effect already happened.
fn save(state: &Path, project: &str, task: &str, product: &Value) -> Result<()> {
    settings::save_task(
        state,
        project,
        task,
        &product["tasks"][task],
        &[
            "conflict",
            "merge",
            "acceptance",
            "resolvedBy",
            "status",
            "requiresFreshResult",
        ],
    )?;
    let _lock = Lock::wait(state, "machine.lock")?;
    let mut saved = super::native::product(state, project)?;
    let mut changed = false;
    for key in ["hasGit", "ghAvailable"] {
        if let Some(value) = product.get(key).filter(|v| saved.get(key) != Some(*v)) {
            saved[key] = value.clone();
            changed = true;
        }
    }
    if changed {
        settings::write_json(&product_path(state, project)?, &saved)?;
    }
    Ok(())
}
fn unknown() -> Refused {
    refuse("integration_unknown", "The merge may have changed Git or task state. Inspect Git before requesting a new action; nothing was retried.")
}
pub async fn run(
    root: &Path,
    project: &str,
    task: &str,
    state: &Path,
    product: &mut Value,
    p: &Value,
    git: &mut Git,
    lock: Lock,
) -> Result<Value> {
    let record = &product["tasks"][task];
    if record["conflict"] == "resolving" {
        return Err(refuse(
            "busy",
            "Stop Cortex and wait for the task to stop before resolving manually.",
        ));
    }
    if truthy(&record["checkout"])
        && (!record["checkout"].is_object() || !record["checkout"]["kind"].is_string())
    {
        return Err(refuse("machine_tool_failed", "TypeError"));
    }
    let path = retention::task_path(root, record, git).await?;
    if !truthy(&record["merge"]) {
        return Err(refuse(
            "no_conflict",
            "There is no retained merge conflict for this task.",
        ));
    }
    let abort = p["action"] == "abort_merge";
    if !abort && !record["merge"].is_object() {
        return Err(refuse("machine_tool_failed", "AttributeError"));
    }
    // The Git merge commit (and its hooks) or abort runs after the machine lock is
    // released; this task's other owner actions are refused until it settles.
    let claim = settings::claim(state, project, task, IN_PROGRESS, &lock)?;
    let intent =
        match ledger::Intent::begin_locked(state, "product.task", project, Some(root), p, &lock)? {
            ledger::Begin::Replay(reply) => return super::task_resolution::replay(reply),
            ledger::Begin::Run(intent) => intent,
        };
    drop(lock);
    let mut effects = false;
    let outcome: Result<Value> = async {
        if abort {
            let head = git.out(&path, &["rev-parse", "HEAD"]).await?;
            effects = true;
            git.out(&path, &["merge", "--abort"]).await?;
            if git.out(&path, &["rev-parse", "HEAD"]).await? != head
                || git
                    .success(&path, &["rev-parse", "--verify", "MERGE_HEAD"])
                    .await?
            {
                return Err(unknown());
            }
            let record = &mut product["tasks"][task];
            // An aborted update from the target leaves exactly the reviewed tree, so the
            // result is current again; acceptance is still the owner's to give again.
            if record["merge"]["kind"] == "update" {
                record["requiresFreshResult"] =
                    record["merge"]["before"]["requiresFreshResult"].clone();
            }
            record["conflict"] = json!("none");
            record["merge"] = Value::Null;
            record["acceptance"] = Value::Null;
            save(state, project, task, product)?;
            return Ok(json!({"done":true,"reclamation":product["tasks"][task]["reclamation"]}));
        }
        if !git
            .out(&path, &["diff", "--name-only", "--diff-filter=U"])
            .await?
            .trim()
            .is_empty()
            || !git.success(&path, &["diff", "--cached", "--check"]).await?
        {
            return Err(refuse(
                "unresolved_conflict",
                "Resolve and stage every conflict without markers first.",
            ));
        }
        if git
            .success(&path, &["rev-parse", "--verify", "MERGE_HEAD"])
            .await?
        {
            let head = git.out(&path, &["rev-parse", "HEAD"]).await?;
            let merge_head = git.out(&path, &["rev-parse", "MERGE_HEAD"]).await?;
            effects = true;
            git.out(&path, &["commit", "-m", "Resolve task merge conflicts"])
                .await?;
            // Normal hooks remain active. Success alone cannot settle a hook that replaced HEAD.
            let parents = git
                .out(&path, &["show", "-s", "--format=%P", "HEAD"])
                .await?;
            if parents.split_whitespace().collect::<Vec<_>>()
                != vec![head.trim(), merge_head.trim()]
                || git
                    .success(&path, &["rev-parse", "--verify", "MERGE_HEAD"])
                    .await?
            {
                return Err(unknown());
            }
        }
        effects = true;
        let record = &mut product["tasks"][task];
        record["conflict"] = json!("none");
        record["resolvedBy"] = json!("you");
        record["acceptance"] = Value::Null;
        record.as_object_mut().unwrap().remove("status");
        record["merge"]["finished"] = json!(true);
        record["merge"]["finishedAt"] = json!(now_ms());
        save(state, project, task, product)?;
        Ok(json!({"done":true,"reclamation":product["tasks"][task]["reclamation"]}))
    }
    .await;
    let reply = envelope(outcome.map_err(|e| if effects { unknown() } else { e }));
    intent.finish(state, &reply).map_err(|_| unknown())?;
    drop(claim);
    super::task_resolution::replay(reply)
}
