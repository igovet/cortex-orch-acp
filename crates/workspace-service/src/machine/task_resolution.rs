//! Manual file choices. Durable intents precede file/index effects; no post-effect fallback.
use super::{
    files,
    git::Git,
    ledger,
    native::*,
    retention,
    settings::{self, Lock},
    workers::truthy,
};
use serde_json::{json, Value};
use std::path::Path;

/// A saved conflict whose JSON shape the current app never writes. It is refused
/// explicitly before any effect rather than interpreted by another runtime.
fn malformed() -> Refused {
    refuse(
        "invalid_request",
        "The saved conflict is malformed. Read the conflict again or finish it in Git.",
    )
}
fn unknown() -> Refused {
    refuse("integration_unknown", "The resolution may have changed files or staging. Inspect Git before requesting a new action; nothing was retried.")
}
pub(super) fn replay(reply: Value) -> Result<Value> {
    if reply["ok"] == true {
        return Ok(reply["result"].clone());
    }
    let code = match reply["code"].as_str() {
        Some("outcome_unknown") => "outcome_unknown",
        Some("unsafe_conflict") => "unsafe_conflict",
        Some("legacy_merge_still_open") => "legacy_merge_still_open",
        Some("unresolved_conflict") => "unresolved_conflict",
        Some("binary_conflict") => "binary_conflict",
        Some("invalid_path") => "invalid_path",
        Some("file_missing") => "file_missing",
        Some("folder_missing") => "folder_missing",
        Some("symlink_refused") => "symlink_refused",
        Some("git_failed") => "git_failed",
        Some("git_timeout") => "git_timeout",
        Some("git_unavailable") => "git_unavailable",
        Some("git_output_too_large") => "git_output_too_large",
        Some("integration_unknown") => "integration_unknown",
        _ => "machine_tool_failed",
    };
    Err(refuse(
        code,
        reply["message"]
            .as_str()
            .unwrap_or("Inspect Git before retrying."),
    ))
}
pub async fn resolve_block(
    root: &Path,
    project: &str,
    task: &str,
    state: &Path,
    product: &mut Value,
    p: &Value,
    git: &mut Git,
    lock: &Lock,
) -> Result<Value> {
    let record = &product["tasks"][task];
    if super::task_merge::manual_blocked(record) {
        return Err(refuse(
            "busy",
            "Stop Cortex and wait for the task to stop before resolving manually.",
        ));
    }
    if truthy(&record["checkout"])
        && (!record["checkout"].is_object() || !record["checkout"]["kind"].is_string())
    {
        return Err(malformed());
    }
    let path = match super::context::owned_checkout(root, project, task)? {
        Some(worktree) => worktree.path,
        None => retention::task_path(root, record, git).await?,
    };
    let merge = &record["merge"];
    if !truthy(merge) || merge["finished"] == true {
        return Err(refuse(
            "no_conflict",
            "There is no retained merge conflict for this task.",
        ));
    }
    super::task_merge::manual_ready(root, project, task, record)?;
    let all = merge["files"].as_array().ok_or_else(malformed)?;
    if all.iter().any(|f| !f.is_object() || !f["path"].is_string()) {
        return Err(malformed());
    }
    let fi = all
        .iter()
        .position(|f| f["path"] == p["path"])
        .ok_or_else(|| refuse("stale_conflict", "This conflict changed. Read it again."))?;
    let file = &all[fi];
    let blocks = file["blocks"].as_array().ok_or_else(malformed)?;
    if blocks
        .iter()
        .any(|b| !b.is_object() || !b["id"].is_string())
    {
        return Err(malformed());
    }
    let bi = blocks.iter().position(|b| b["id"] == p["blockId"]);
    let choice = p["choice"]
        .as_str()
        .filter(|s| ["ours", "theirs", "both"].contains(s));
    let (Some(bi), Some(choice)) = (bi, choice) else {
        return Err(refuse(
            "invalid_choice",
            "Choose one of the displayed file versions.",
        ));
    };
    let block = &blocks[bi];
    if truthy(&block["cortexNote"]) {
        return Err(refuse(
            "unsafe_conflict",
            block["cortexNote"].as_str().ok_or_else(malformed)?,
        ));
    }
    let name = file["path"].as_str().unwrap().to_owned();
    relative(&name)?;
    let lines = block["results"][choice]["lines"]
        .as_array()
        .ok_or_else(malformed)?;
    let lines = lines
        .iter()
        .map(|s| s.as_str().ok_or_else(malformed))
        .collect::<Result<Vec<_>>>()?;
    let text = format!("{}\n", lines.join("\n"));
    if text.len() > files::MAX_FILE {
        return Err(refuse(
            "unsafe_conflict",
            "This resolution is too large. Resolve this file with Git in your editor.",
        ));
    }
    // Replay is checked before inspecting stages: a prior successful add removed them.
    let intent =
        match ledger::Intent::begin_locked(state, "product.task", project, Some(root), p, lock)? {
            ledger::Begin::Replay(reply) => return replay(reply),
            ledger::Begin::Run(intent) => intent,
        };
    let merge = merge.clone();
    let mut effects = false;
    let outcome: Result<Value> = async {
        if merge["sourceRevision"].is_string()
            && super::task_merge::observe(&path, &merge, git).await? != super::task_merge::Observed::Stopped
        {
            return Err(super::task_merge::unsafe_merge());
        }
        // Literal pathspecs cannot expand a displayed file into another conflict.
        let literal = format!(":(literal){name}");
        let stages = git.out(&path, &["ls-files", "-u", "--", &literal]).await?;
        let mut modes = std::collections::BTreeSet::new();
        let mut numbers = std::collections::BTreeSet::new();
        let mut count = 0;
        for line in stages.lines() {
            let fields: Vec<_> = line
                .split_once('\t')
                .map(|(s, _)| s)
                .unwrap_or("")
                .split_whitespace()
                .collect();
            if fields.len() != 3 {
                return Err(refuse(
                    "unsafe_conflict",
                    "Resolve add/delete, symlink or mode conflicts with Git in your editor.",
                ));
            }
            modes.insert(fields[0]);
            numbers.insert(fields[2]);
            count += 1;
        }
        if count != 3
            || numbers != ["1", "2", "3"].into_iter().collect()
            || modes.len() != 1
            || !modes.iter().all(|m| ["100644", "100755"].contains(m))
        {
            return Err(refuse(
                "unsafe_conflict",
                "Resolve add/delete, symlink or mode conflicts with Git in your editor.",
            ));
        }
        if std::fs::symlink_metadata(path.join(&name)).is_ok_and(|m| m.is_symlink()) {
            return Err(refuse(
                "unsafe_conflict",
                "Resolve this symlink conflict with Git yourself.",
            ));
        }
        if text.contains('\0') {
            return Err(refuse(
                "binary_conflict",
                "Resolve this binary conflict with Git yourself.",
            ));
        }
        let current = files::read(&path, &json!({"file":name}))?;
        if !current["readOnly"].is_null() || current["revision"].is_null() {
            return Err(refuse(
                "unsafe_conflict",
                "Resolve this file with Git in your editor.",
            ));
        }
        effects = true;
        let saved = files::save_locked(
            &path,
            &json!({"file":name,"text":text,"revision":current["revision"]}),
            lock,
        )?;
        if saved["outcome"] != "saved" {
            return Err(unknown());
        }
        git.out(&path, &["add", "--", &literal]).await?;
        product["tasks"][task]["merge"]["files"][fi]["blocks"][bi]["choice"] = json!(choice);
        settings::write_json(&product_path(state, project)?, product)?;
        Ok(json!({"done":true,"reclamation":product["tasks"][task]["reclamation"]}))
    }
    .await;
    // Any failure after attempting replacement can have changed files/index or JSON.
    // Leave the intent unknown; do not restore files, force Git, or invoke Python.
    let outcome = outcome.map_err(|e| if effects { unknown() } else { e });
    let reply = envelope(outcome);
    intent.finish_locked(&reply, lock).map_err(|_| unknown())?;
    replay(reply)
}
