//! Owner acceptance is a private overlay, never a runtime completion or new check receipt.
use super::{context, git::Git, native::*, settings, task_facts};
use cortex_core::{OpenMode, Store};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};
use std::path::Path;

/// Refused or failed automatic settles, by (state, project, task): what the outcome
/// depended on, when, and the outcome. A memo only ever repeats a refusal; an
/// acceptance always runs the full validator. It holds while its inputs (runtime
/// evidence position, saved task record, folder change signal) are unchanged: for
/// `RECENT` with a folder signal, for `UNSIGNALLED` without one (or when the settle
/// failed before its inputs were read). A run-mode switch of the project drops it.
type Key = (std::path::PathBuf, String, String);
struct Memo {
    inputs: Option<String>,
    signalled: bool,
    at: std::time::Instant,
    outcome: Value,
}
static REFUSED: std::sync::LazyLock<std::sync::Mutex<std::collections::BTreeMap<Key, Memo>>> =
    std::sync::LazyLock::new(Default::default);
const UNSIGNALLED: std::time::Duration = std::time::Duration::from_secs(10);
pub(crate) fn forget_refusals(state: &Path, project: &str) {
    if let Ok(mut memo) = REFUSED.lock() {
        memo.retain(|(s, p, _), _| !(s == state && p == project));
    }
}
/// The remembered outcome if it still holds for `inputs` (None: before they are read).
fn remembered(key: &Key, inputs: Option<&str>) -> Option<Value> {
    let memo = REFUSED.lock().ok()?;
    let m = memo.get(key)?;
    let fresh = m.at.elapsed()
        < if m.signalled {
            task_facts::RECENT
        } else {
            UNSIGNALLED
        };
    (fresh && m.inputs.as_deref() == inputs).then(|| m.outcome.clone())
}
fn remember(key: Key, inputs: Option<String>, signalled: bool, outcome: Value) -> Value {
    if let Ok(mut memo) = REFUSED.lock() {
        memo.retain(|_, m| m.at.elapsed() < task_facts::RECENT);
        memo.insert(
            key,
            Memo {
                inputs,
                signalled,
                at: std::time::Instant::now(),
                outcome: outcome.clone(),
            },
        );
    }
    outcome
}

/// Accepts a finished sprint result exactly as manual Accept would, marked
/// automatic, while the sprint's current run mode is queue-driven. The caller holds the
/// machine lock, so the mode checked here is the one in force when the acceptance is
/// written: switching to Manual stops every acceptance not yet written, and switching
/// to a queued mode settles results that are still waiting. Automatic acceptance also
/// needs the runtime's passed verification and no owner resolution of the task.
/// A settle that cannot complete answers `accepted:false` with `failed`/`reason`;
/// it never fails the background pass.
pub async fn automatic(
    root: &Path,
    project: &str,
    task: &str,
    state: &Path,
    product: &mut Value,
    git: &mut Git,
) -> Result<Value> {
    if !product["tasks"][task]["archive"].is_null() || !product["tasks"][task]["archiveHold"].is_null() {
        return Ok(json!({"accepted":false}));
    }
    if product["tasks"][task]["acceptance"].is_object() {
        return Ok(json!({"accepted":true}));
    }
    if product["tasks"][task]["status"] == "done" || product["tasks"][task]["purged"] == true {
        return Ok(json!({"accepted":false}));
    }
    // The cheap mode check comes first: a Manual result never pays for the
    // folder observation.
    let current = product["sprints"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|s| s["id"].is_string() && s["id"] == product["tasks"][task]["sprintId"]);
    if !current.is_some_and(queue_driven_mode) {
        return Ok(json!({"accepted":false}));
    }
    let key = (state.to_owned(), project.to_owned(), task.to_owned());
    if let Some(outcome) = remembered(&key, None) {
        return Ok(outcome);
    }
    let mut inputs = None;
    let mut signalled = false;
    match settle(
        root,
        project,
        task,
        state,
        product,
        git,
        &key,
        &mut inputs,
        &mut signalled,
    )
    .await
    {
        Ok(Some(outcome)) => Ok(outcome),
        Ok(None) => {
            if let Ok(mut memo) = REFUSED.lock() {
                memo.remove(&key);
            }
            Ok(json!({"accepted":true}))
        }
        Err(e) => Ok(remember(
            key,
            inputs,
            signalled,
            json!({"accepted":false,"failed":e.code,"reason":e.message}),
        )),
    }
}
/// None once accepted; otherwise the (remembered) refusal.
#[allow(clippy::too_many_arguments)]
async fn settle(
    root: &Path,
    project: &str,
    task: &str,
    state: &Path,
    product: &mut Value,
    git: &mut Git,
    key: &Key,
    inputs: &mut Option<String>,
    signalled: &mut bool,
) -> Result<Option<Value>> {
    let refusal = |reason: &str| json!({"accepted":false,"reason":reason});
    let store = context::open(root, OpenMode::RuntimeValidate)?.ok_or_else(unavailable)?;
    let record = product["tasks"][task].clone();
    let finished: Option<(String, String, bool)> = store.read(|db| Ok(db.query_row(
        "SELECT e.execution_id, json_array(e.execution_id, r.revision, r.state,
            (SELECT max(id) FROM runtime_events v WHERE v.task_id=r.task_id),
            (SELECT max(rowid) FROM runtime_nodes n WHERE n.task_id=r.task_id)),
            r.task_id, r.revision
         FROM workspace_tasks w JOIN workspace_executions e ON e.execution_id=w.execution_id JOIN runtime_tasks r ON r.task_id=e.runtime_task_id WHERE w.project_id=? AND w.task_id=? AND r.state='COMPLETED'",
        (project,task), |r|Ok((r.get::<_, String>(0)?,r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,r.get::<_, i64>(3)?))).optional()?
        .map(|(execution, evidence, runtime, revision)| {
            let control = cortex_core::workspace::control::owner_rework_event(db, &runtime)?;
            // Without an owner rework the task has one cycle: the runtime passed it.
            // After one, only the verdict of the current cycle, for the current
            // revision, counts: an older cycle's pass never settles the new result.
            let passed: bool = if control.is_none() {
                db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='VERIFICATION_PASSED')",
                    [&runtime], |r| r.get(0),
                )?
            } else { db.query_row(
                "SELECT coalesce((SELECT type='VERIFICATION_PASSED'
                    AND json_extract(payload_json,'$.revision')=?3 FROM runtime_events
                    WHERE task_id=?1 AND id>?2 AND (type IN ('VERIFICATION_PASSED','VERIFICATION_FAILED')
                      OR (type='VERIFICATION_REVISED' AND json_extract(payload_json,'$.accepted_unverified') IS NOT NULL))
                    ORDER BY id DESC LIMIT 1),0)",
                (&runtime, control.unwrap_or(0), revision), |r| r.get(0),
            )? };
            let resolved = cortex_core::workspace::control::owner_resolution_current(db, &runtime)?;
            Ok::<_, cortex_core::CoreError>((execution, evidence, passed && !resolved))
        }).transpose()?)).map_err(context::core)?;
    let Some((execution, evidence, verified)) = finished else {
        return Ok(Some(refusal(
            "There is no completed execution for this result.",
        )));
    };
    // Only a result the runtime verified, with no owner resolution, settles itself.
    if !verified {
        return Ok(Some(refusal("This result has no eligible passing runtime verification. Review the result or request changes.")));
    }
    // An unchanged refusal is not repeated on every background pass.
    let checkout = context::owned_checkout(root, project, task)?;
    let source_root = checkout.as_ref().map_or(root, |w| w.path.as_path());
    let probe = source_root.to_owned();
    let signal = tokio::task::spawn_blocking(move || task_facts::folder_signal(&probe))
        .await
        .ok()
        .flatten();
    *signalled = signal.is_some();
    *inputs = Some(json!([evidence, record, signal]).to_string());
    if let Some(outcome) = remembered(key, inputs.as_deref()) {
        return Ok(Some(outcome));
    }
    let native = task_facts::native_observation(source_root).await;
    let candidate = store
        .read(|db| {
            let (actions, _, warnings, counts) = match task_facts::result_evidence_db(
                db,
                &store,
                root,
                project,
                &execution,
                native.as_deref(),
                task_facts::own_writes(&record),
                true,
                None,
            ) {
                Ok(evidence) => evidence,
                Err(cortex_core::CoreError::Refused("result_unavailable")) => return Ok(Err("The result evidence could not be read. Refresh the task or request a new result.".to_owned())),
                Err(e) => return Err(e),
            };
            if warnings || actions["accept"] != true || counts != [0, 0, 0] {
                let reason = actions["reason"]
                    .as_str()
                    .filter(|text| !text.trim().is_empty())
                    .map(str::to_owned)
                    .unwrap_or_else(|| "The result is not ready to accept.".to_owned());
                return Ok(Err(reason));
            }
            Ok(Ok(json!({"version":actions["version"],"digest":actions["digest"]})))
        })
        .map_err(context::core)?;
    let p = match candidate {
        Ok(p) => p,
        Err(reason) => {
            return Ok(Some(remember(
                key.clone(),
                inputs.clone(),
                *signalled,
                refusal(&reason),
            )))
        }
    };
    // The ordinary validator is repeated under the write transaction. No owner
    // acknowledgement is manufactured for open checks.
    match accept_inner(root, project, task, state, product, &p, git, true).await {
        Ok(_) => Ok(None),
        Err(e)
            if matches!(
                e.code,
                "stale_result"
                    | "checks_required"
                    | "review_required"
                    | "acknowledgement_required"
                    | "result_unavailable"
            ) =>
        {
            Ok(Some(remember(
                key.clone(),
                inputs.clone(),
                *signalled,
                refusal(&e.message),
            )))
        }
        Err(e) => Err(e),
    }
}

fn unavailable() -> Refused {
    refuse(
        "result_unavailable",
        "Only a completed runtime result can be accepted.",
    )
}
fn validate(
    db: &Connection,
    store: &Store,
    root: &Path,
    project: &str,
    task: &str,
    record: &Value,
    p: &Value,
    native: Option<&str>,
) -> Result<(Value, [u64; 3])> {
    let execution = db.query_row(
        "SELECT e.execution_id FROM workspace_tasks t
        JOIN workspace_executions e ON e.execution_id=t.execution_id AND e.task_id=t.task_id
        JOIN runtime_tasks r ON r.task_id=e.runtime_task_id
        JOIN tasks base ON base.id=r.task_id
        JOIN acp_host_sessions s ON s.session_id=e.acp_session_id
        JOIN task_owners o ON o.task_id=r.task_id AND o.epoch=e.owner_epoch
        WHERE t.project_id=? AND t.task_id=? AND r.state='COMPLETED' AND r.revision>=0
        AND (r.blocker IS NULL OR r.blocker='') AND base.project_root=?
        AND s.project_root=base.project_root AND s.task_id=r.task_id
        AND NOT EXISTS(SELECT 1 FROM workspace_starts WHERE execution_id=e.execution_id AND state='unknown')",
        (project, task, root.to_string_lossy().as_ref()), |r| r.get::<_, String>(0))
        .optional().map_err(|e| context::core(e.into()))?.ok_or_else(unavailable)?;
    let (actions, work, warnings, counts) = task_facts::result_evidence_db(
        db,
        store,
        root,
        project,
        &execution,
        native,
        task_facts::own_writes(record),
        false,
        None,
    )
    .map_err(|e| {
        if e.code() == "result_unavailable" {
            refuse("machine_tool_failed", "WorkspaceError")
        } else {
            context::core(e)
        }
    })?;
    if actions["digest"] != p["digest"] || warnings {
        return Err(refuse(
            "stale_result",
            "This result or its workspace changed. Rerun checks and review the new result.",
        ));
    }
    if super::task_merge::fresh_needed(db, project, &execution, record).map_err(context::core)?
        || (!work && record["merge"]["freshChecks"].is_object())
    {
        return Err(refuse("review_required", super::task_merge::FRESH_CHECKS_REASON));
    }
    if actions["accept"] != true {
        return Err(refuse(
            "checks_required",
            actions["reason"]
                .as_str()
                .unwrap_or("Result evidence is unavailable."),
        ));
    }
    if record["requiresFreshResult"]
        == json!({"resultId":actions["resultId"],"version":actions["version"]})
    {
        return Err(refuse(
            "review_required",
            "The source changed after this result. Run fresh checks and review the new result.",
        ));
    }
    if python_text(&p["version"]) != actions["version"].as_str().unwrap_or("") {
        return Err(refuse(
            "stale_result",
            "This result version changed. Refresh and review the current result.",
        ));
    }
    if actions["acknowledgeOpenChecks"] == true && p["acknowledgeOpenChecks"] != true {
        return Err(refuse(
            "acknowledgement_required",
            "Explicitly acknowledge the recorded technical blockers before accepting.",
        ));
    }
    Ok((actions, counts))
}
pub async fn accept(
    root: &Path,
    project: &str,
    task: &str,
    state: &Path,
    product: &mut Value,
    p: &Value,
    git: &mut Git,
) -> Result<Value> {
    accept_inner(root, project, task, state, product, p, git, false).await
}

/// Validate the separate owner consent that lets an Automatic sprint finish this
/// exact result after acceptance.  This is deliberately narrower than ordinary
/// Accept: it is only valid for the currently paused Automatic settlement hold,
/// and the captured target must still be the task and sprint target.
fn automatic_finish_consent(product: &Value, task: &str, p: &Value) -> Result<Option<Value>> {
    let Some(consent) = p.get("automaticFinish") else {
        return Ok(None);
    };
    if consent.is_null() {
        return Ok(None);
    }
    let sprint_id = consent["sprintId"].as_str().filter(|s| !s.is_empty());
    let target = consent["targetBranch"].as_str().filter(|s| !s.is_empty());
    let Some((sprint_id, target)) = sprint_id.zip(target) else {
        return Err(refuse(
            "invalid_request",
            "Automatic finish consent must name the sprint and target branch.",
        ));
    };
    let record = &product["tasks"][task];
    if record["sprintId"] != sprint_id
        || (record["targetBranch"].is_string() && record["targetBranch"] != target)
    {
        return Err(refuse(
            "stale_result",
            "The Automatic target changed. Refresh the task before allowing it to finish.",
        ));
    }
    let sprint = product["sprints"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|s| s["id"] == sprint_id)
        .ok_or_else(|| refuse("stale_result", "The Automatic sprint no longer exists."))?;
    if !super::product::automatic_sprint(sprint)
        || !sprint["taskIds"]
            .as_array()
            .is_some_and(|ids| ids.iter().any(|id| id == task))
        || record["purged"] == true
        || !record["archive"].is_null()
        || (sprint["targetBranch"].is_string() && sprint["targetBranch"] != target)
        || sprint["queueState"] != "paused"
        || !matches!(
            (
                sprint["automaticQueue"]["kind"].as_str(),
                sprint["automaticQueue"]["code"].as_str()
            ),
            (
                Some("settlement_blocked"),
                Some("automatic_acceptance_blocked")
            ) | (Some("owner_integration"), Some("owner_accepted"))
        )
        || sprint["automaticQueue"]["taskId"] != task
        || sprint["automaticQueue"]["needsOwner"] != true
    {
        return Err(refuse(
            "automatic_finish_unavailable",
            "Automatic is no longer waiting for this task. Refresh before approving finish.",
        ));
    }
    if record["checkout"]["kind"] != "main" {
        return Err(refuse(
            "automatic_finish_unavailable",
            "Automatic finish consent is available only for the project-folder checkout.",
        ));
    }
    Ok(Some(json!({"sprintId": sprint_id, "targetBranch": target})))
}

async fn accept_inner(
    root: &Path,
    project: &str,
    task: &str,
    state: &Path,
    product: &mut Value,
    p: &Value,
    git: &mut Git,
    automatic: bool,
) -> Result<Value> {
    let store = context::open(root, OpenMode::RuntimeValidate)?.ok_or_else(|| {
        refuse(
            "result_unavailable",
            "Activate Cortex and review its runtime result first.",
        )
    })?;
    let mut record = product["tasks"][task].clone();
    let checkout = context::owned_checkout(root, project, task)?;
    if let Some(w) = &checkout { context::set_worktree(&mut record, w); }
    let source_root = checkout.as_ref().map_or(root, |w| w.path.as_path());
    // Native verification keeps its full-checkout identity. In the project folder,
    // also retain an exact content witness for ordinary Commit when available.
    let source = if checkout.is_some() || record["checkout"]["kind"].as_str().unwrap_or("main") == "main" {
        crate::native_effects::observe_commit(source_root.to_owned()).await
    } else {
        None
    };
    let native = match &source {
        Some(source) => Some(source.digest.clone()),
        None => task_facts::native_observation(source_root).await,
    };
    let native = native.as_deref();
    // Preserve result/version refusal order before resolving the checkout.
    store
        .read(|db| {
            Ok(validate(
                db, &store, root, project, task, &record, p, native,
            ))
        })
        .map_err(context::core)??;
    checkout_shape(&record)?;
    let path = match &checkout { Some(w) => w.path.clone(), None => super::retention::task_path(root, &record, git).await? };
    let finish_consent = automatic_finish_consent(product, task, p)?;
    if let Some(consent) = finish_consent.as_ref() {
        let current_branch = git.out(&path, &["branch", "--show-current"]).await?;
        let effective = match record["targetBranch"].as_str().filter(|s| !s.is_empty()) {
            Some(branch) => branch.to_owned(),
            None => git.default_branch(root).await?,
        };
        if current_branch.trim() != effective || consent["targetBranch"] != effective {
            return Err(refuse(
                "automatic_finish_unavailable",
                "The project folder is no longer on the requested target branch. Refresh before approving finish.",
            ));
        }
    }
    store.write(|db| Ok((|| {
        // Start/owner replacement cannot race evidence validation and overlay replacement.
        let (actions, counts) = validate(db, &store, root, project, task, &record, p, native)?;
        let observed = super::legacy_source::capture(&path).map_err(context::core)?
            .ok_or_else(|| refuse("workspace_unobservable", "The workspace could not be observed; acceptance was not recorded."))?;
        let record = &mut product["tasks"][task];
        record["acceptedRevision"] = json!(observed);
        record["status"] = json!("ready_to_merge");
        record["acceptance"] = json!({"version":p["version"],"digest":p["digest"],"at":now_ms(),
            "open":{"failed":counts[0],"not_run":counts[1],"unverified":counts[2]}});
        if let Some(source) = source.filter(|s| p["digest"] == s.digest) {
            record["acceptance"]["version"] = actions["version"].clone();
            record["acceptance"]["commitSource"] =
                json!({"head":source.head,"branch":source.branch,"content":source.content});
            if let Some(w) = &checkout { record["acceptance"]["commitSource"]["checkoutId"] = json!(w.id); }
        }
        if automatic {
            record["acceptance"]["automatic"] = json!(true);
        } else if let Some(consent) = finish_consent.as_ref() {
            record["acceptance"]["finishConsent"] = consent.clone();
        }
        // No retained-provider error may be returned after this mutation boundary.
        settings::write_json(&product_path(state, project)?, product)?;
        Ok(json!({"done":true,"reclamation":product["tasks"][task]["reclamation"]}))
    })())).map_err(context::core)?
}
/// The owner's explicit consent to delete an archived task's unmerged work when it is
/// deleted. Nothing is deleted here and the task stays archived; archiving itself never asks.
pub async fn confirm_discard(root: &Path, record: &mut Value, git: &mut Git) -> Result<()> {
    if !super::workers::truthy(&record["archive"]) {
        return Err(refuse(
            "not_archived",
            "Only an archived task's work can be marked for deletion.",
        ));
    }
    checkout_shape(record)?;
    let path = super::retention::task_path(root, record, git).await?;
    let observed = super::legacy_source::capture(&path)
        .map_err(context::core)?
        .ok_or_else(|| {
            refuse(
                "workspace_unobservable",
                "The work could not be observed; no discard decision was recorded.",
            )
        })?;
    record["discardRevision"] = json!(observed);
    record["work"] = json!("discard_confirmed");
    record["purgeBlocker"] = Value::Null;
    Ok(())
}
fn checkout_shape(record: &Value) -> Result<()> {
    let checkout = &record["checkout"];
    if super::workers::truthy(checkout)
        && (!checkout.is_object()
            || !checkout["kind"].is_string()
            || checkout["kind"] != "main"
                && (!checkout["path"].is_string() || !checkout["branch"].is_string()))
    {
        // An incomplete checkout record has no observable worktree. Refuse before
        // any effect; nothing is discarded, accepted or recorded.
        return Err(refuse(
            "unknown_checkout",
            "The task worktree is missing or its checkout record is incomplete.",
        ));
    }
    Ok(())
}
fn python_text(value: &Value) -> String {
    match value {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::String(s) => s.clone(),
        _ => value.to_string(),
    }
}
/// Live N1: a finished native task showed "Result evidence is unavailable" and
/// Accept was disabled. Its review actions and identity must come from the
/// runtime evidence bound in the native verifier's digest domain (Python
/// `result_projection`), and Accept must take exactly the presented result.
#[cfg(test)]
mod native_acceptance_tests {
    use super::*;
    use crate::{actor, native_runtime::Runtime, native_runtime::UnqualifiedRoutes};
    use cortex_core::{lifecycle, workspace};
    use sha2::{Digest, Sha256};
    use std::sync::Arc;

    async fn completed() -> (std::path::PathBuf, Arc<Runtime>, String) {
        completed_in_checkout(false).await
    }

    async fn completed_in_checkout(tracked: bool) -> (std::path::PathBuf, Arc<Runtime>, String) {
        let root =
            std::env::temp_dir().join(format!("native-accept-{}", crate::identity().unwrap()));
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("work.txt"), "after").unwrap();
        assert!(std::process::Command::new("/usr/bin/git")
            .args(["init", "-q"])
            .current_dir(&root)
            .status()
            .unwrap()
            .success());
        if tracked {
            for (key, value) in [
                ("user.name", "Fixture"),
                ("user.email", "fixture@example.invalid"),
                ("commit.gpgsign", "false"),
                ("core.hooksPath", "/dev/null"),
                ("core.excludesFile", "/dev/null"),
            ] {
                git_fixture(&root, &["config", "--local", key, value]);
            }
            std::fs::write(root.join(".git/info/exclude"), "/.cortex/\n/.cortex-dev/\n").unwrap();
            git_fixture(&root, &["symbolic-ref", "HEAD", "refs/heads/main"]);
            std::fs::write(root.join("work.txt"), "before\n").unwrap();
            git_fixture(&root, &["add", "--", "work.txt"]);
            git_fixture(&root, &["commit", "-qm", "Base"]);
            git_fixture(&root, &["switch", "-qc", "qa/task"]);
            std::fs::write(root.join("work.txt"), "after").unwrap();
        }
        let path = root.clone();
        let runtime = Arc::new(
            tokio::task::spawn_blocking(move || {
                Runtime::open(&path, "project", Arc::new(UnqualifiedRoutes))
            })
            .await
            .unwrap()
            .unwrap(),
        );
        // The digest the native verifier records for this folder.
        let digest = crate::native_effects::observe(root.clone())
            .await
            .unwrap()
            .project_digest
            .unwrap();
        let draft = runtime.host.executor.publication(move |store| {
            let command = json!({"protocol":{"major":1,"minor":8},"requestId":"draft",
                "projectId":"project","actorId":actor(),"operation":"tasks.create","target":"project",
                "expectedVersion":"0","ownerEpoch":null,
                "payload":{"description":"Accept a finished result","checkout":{"kind":"main"}}});
            let draft = workspace::create_draft(store, "project", &actor(), &command)?.receipt;
            let start = json!({"protocol":{"major":1,"minor":8},"requestId":"start",
                "projectId":"project","actorId":actor(),"operation":"tasks.start",
                "target":draft["target"],"expectedVersion":draft["entityVersion"],
                "ownerEpoch":null,"payload":{}});
            let started = workspace::start_native(store, "project", &actor(), &start)?;
            let task = started["task"].as_str().unwrap().to_owned();
            // Exactly what a native completion retains: published worker and
            // approving reviewer reports, and bound runtime verification.
            let folder = store.key().directory().join(&task);
            std::fs::create_dir_all(&folder)?;
            let mut digests = Vec::new();
            for name in ["work.md", "review.md"] {
                let body = format!("# {name}\n\nObserved disposable fixture.\n");
                std::fs::write(folder.join(name), &body)?;
                digests.push(format!("{:x}", Sha256::digest(body.as_bytes())));
            }
            store.write(|db| {
                for (id, name) in [("r_work", "work.md"), ("r_review", "review.md")] {
                    db.execute("INSERT INTO reports VALUES (?,?,'report',?,'now')", (id, &task, name))?;
                }
                db.execute("INSERT INTO runtime_nodes VALUES (?,0,'work','worker-backend-dev','Work','[]','[\"work.txt\"]','execution','completed','a-work',?)",
                    (&task, json!({"report_ref":"r_work","report_digest":digests[0],"summary":"Wrote work.txt.","checks":[{"name":"content check","outcome":"passed","tool_call_ref":"native-command:w"}]}).to_string()))?;
                db.execute("INSERT INTO runtime_nodes VALUES (?,0,'review','worker-code-reviewer','Review','[\"work\"]','[\"work.txt\"]','review','completed','a-review',?)",
                    (&task, json!({"report_ref":"r_review","report_digest":digests[1],"verdict":"APPROVE","checks":[],"review_independence":{"status":"warning","same_engine_as_reviewed":true,"warning":"The reviewer runs on the same model as the work it reviews."}}).to_string()))?;
                for (key, channel) in [("a-work", "ch-work"), ("a-review", "ch-review")] {
                    db.execute("INSERT INTO delegation_provisioning(idempotency_key,task_id,assignment_id,attempt_id,role_id,step,status,channel_id,owner_epoch,created_at,updated_at) VALUES (?,?,?,?,'role','ready','ready',?,1,'now','now')",
                        (key, &task, key, format!("att-{key}"), channel))?;
                    db.execute("INSERT INTO trusted_channels(channel_id,secret_hash,task_id,principal_thread_id,role,assignment_id,attempt_id,owner_epoch,issued_at) VALUES (?,'h',?,'p','role',?,?,1,'now')",
                        (channel, &task, key, format!("att-{key}")))?;
                }
                lifecycle::event_tx(db, &task, "NATIVE_COMMAND_FINISHED", json!({"receipt":{"revision":0,"argv":["/bin/true"],"revision_digest":digest,
                    "tool_call_ref":"native-command:v","observation":{"complete":true,"exit_code":0}}}))?;
                lifecycle::event_tx(db, &task, "NATIVE_PROJECT_WRITTEN", json!({"channel_id":"ch-work","path":"work.txt","sha256":"s","add":3,"del":1}))?;
                lifecycle::event_tx(db, &task, "NATIVE_PROJECT_WRITTEN", json!({"channel_id":"ch-work","path":"work.txt","sha256":"t","add":2,"del":0}))?;
                // What those writes' requests recorded: an unfinished attempt, the first
                // write's whole-text diff, and a clipped diff for the second.
                let reply = |sha: &str, diff: &str, truncated: bool| json!({"complete":true,"reply":{"content":[],"isError":false,
                    "structuredContent":{"path":"work.txt","sha256":sha,"diff":diff,"diff_truncated":truncated,"outcome":"written"}}}).to_string();
                for (key, result) in [
                    ("k0", json!({"complete":false,"path":"work.txt","reply":{"structuredContent":{"path":"work.txt","sha256":"s","diff":"--- work.txt\n+++ work.txt\n@@ -1,0 +1,1 @@\n+WRONG\n","diff_truncated":false}}}).to_string()),
                    ("k1", reply("s", "--- work.txt\n+++ work.txt\n@@ -1,2 +1,4 @@\n-a\n-b\n+a\n+B\n+c\n+d\n", false)),
                    ("k2", reply("t", "--- work.txt\n+++ work.txt\n@@ -1,4 +1,6 @@\n-a\n", true)),
                ] {
                    db.execute("INSERT INTO runtime_requests VALUES (?,?,'ch-work','project_edit','d',?)", (&task, key, result))?;
                }
                db.execute("UPDATE runtime_tasks SET state='COMPLETED',synthesis='Fixture result.',plan_json=? WHERE task_id=?",
                    (json!({"verification":[["/bin/true"]]}).to_string(), &task))?;
                lifecycle::event_tx(db, &task, "VERIFICATION_PASSED", json!({"revision":0,"revision_digest":digest,
                    "checks":[{"argv":["/bin/true"],"complete":true,"exit_code":0,"revision_digest":digest}]}))?;
                Ok(())
            })?;
            Ok(draft["target"].as_str().unwrap().to_owned())
        }).unwrap().await.unwrap().unwrap();
        (root, runtime, draft)
    }

    fn sprint_product(draft: &str, mode: &str) -> Value {
        let mut product = json!({"tasks":{},"messages":[],"sprints":[{"id":"sp","runMode":mode,"archivedAt":null}]});
        product["tasks"][draft] = json!({"checkout":{"kind":"main"},"sprintId":"sp"});
        product
    }
    fn automatic_state(root: &Path) -> std::path::PathBuf {
        let state = root.join(".cortex-dev/automatic-state");
        std::fs::create_dir(&state).unwrap();
        state
    }
    #[tokio::test]
    async fn automatic_acceptance_binds_the_result_and_follows_the_current_mode() {
        // (mode while the task ran and finished, mode when the result is settled)
        for (at_finish, at_settle, accepted) in [
            ("hybrid", "hybrid", true),
            ("manual", "hybrid", true),
            ("hybrid", "manual", false),
            ("automatic", "automatic", true),
            ("manual", "manual", false),
        ] {
            let (root, runtime, draft) = completed().await;
            let state = automatic_state(&root);
            let mut product = sprint_product(&draft, at_finish);
            settings::write_json(&product_path(&state, "project").unwrap(), &product).unwrap();
            product["sprints"][0]["runMode"] = json!(at_settle);
            let result = automatic(
                &root,
                "project",
                &draft,
                &state,
                &mut product,
                &mut Git::new(),
            )
            .await
            .unwrap();
            assert_eq!(result["accepted"], accepted, "{at_finish} -> {at_settle}");
            if accepted {
                let acceptance = product["tasks"][&draft]["acceptance"].clone();
                assert_eq!(acceptance["automatic"], true);
                assert_eq!(
                    acceptance["open"],
                    json!({"failed":0,"not_run":0,"unverified":0})
                );
                assert_eq!(
                    acceptance["digest"],
                    presented(&runtime, &draft).await["result"]["digest"]
                );
                assert_eq!(product["tasks"][&draft]["status"], "ready_to_merge");
                let projected = runtime
                    .owner_facts("project", product.clone(), json!({}))
                    .await
                    .unwrap();
                assert_eq!(
                    projected["product"]["tasks"][&draft]["acceptance"]["automatic"],
                    true
                );
                assert!(product["tasks"][&draft]["integration"].is_null());
                automatic(
                    &root,
                    "project",
                    &draft,
                    &state,
                    &mut product,
                    &mut Git::new(),
                )
                .await
                .unwrap();
                assert_eq!(product["tasks"][&draft]["acceptance"], acceptance);
            } else {
                assert!(product["tasks"][&draft]["acceptance"].is_null());
                let task = presented(&runtime, &draft).await;
                let p = json!({"version":task["result"]["sourceVersion"],"digest":task["result"]["digest"]});
                accept(
                    &root,
                    "project",
                    &draft,
                    &state,
                    &mut product,
                    &p,
                    &mut Git::new(),
                )
                .await
                .unwrap();
                assert!(product["tasks"][&draft]["acceptance"]["automatic"].is_null());
            }
            runtime.shutdown().await;
            drop(runtime);
            std::fs::remove_dir_all(root).unwrap();
        }
    }
    #[tokio::test]
    async fn automatic_acceptance_uses_the_reopened_cycles_verification_and_resolution() {
        for (fresh_pass, current_resolution, expected) in [
            (false, false, false),
            (true, false, true),
            (true, true, false),
        ] {
            let (root, runtime, draft) = completed().await;
            let state = automatic_state(&root);
            let mut product = sprint_product(&draft, "automatic");
            with_task(&runtime, move |db, task| {
                let pass: String = db.query_row(
                    "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='VERIFICATION_PASSED'",
                    [task], |row| row.get(0),
                )?;
                lifecycle::event_tx(db, task, "OWNER_TASK_RESOLVED", json!({}))?;
                lifecycle::event_tx(db, task, "TASK_REOPENED", json!({"origin":"owner_rework"}))?;
                if fresh_pass {
                    lifecycle::event_tx(db, task, "VERIFICATION_PASSED", serde_json::from_str(&pass)?)?;
                }
                if current_resolution {
                    lifecycle::event_tx(db, task, "OWNER_TASK_RESOLVED", json!({}))?;
                }
                Ok(())
            }).await;
            settings::write_json(&product_path(&state, "project").unwrap(), &product).unwrap();
            let result = automatic(&root, "project", &draft, &state, &mut product, &mut Git::new())
                .await
                .unwrap();
            assert_eq!(result["accepted"], expected, "{fresh_pass}/{current_resolution}: {result}");
            assert_eq!(product["tasks"][&draft]["acceptance"].is_object(), expected);
            runtime.shutdown().await;
            drop(runtime);
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[tokio::test]
    async fn a_switch_to_manual_before_settling_leaves_the_result_for_the_owner() {
        let (root, runtime, draft) = completed().await;
        let state = automatic_state(&root);
        let mut product = sprint_product(&draft, "automatic");
        settings::write_json(&product_path(&state, "project").unwrap(), &product).unwrap();
        product["sprints"][0]["runMode"] = json!("manual");
        let result = automatic(
            &root,
            "project",
            &draft,
            &state,
            &mut product,
            &mut Git::new(),
        )
        .await
        .unwrap();
        assert_eq!(result["accepted"], false);
        assert!(product["tasks"][&draft]["acceptance"].is_null());
        assert!(product["tasks"][&draft]["status"].is_null());
        let shown = runtime
            .owner_facts("project", product.clone(), json!({}))
            .await
            .unwrap();
        let task = &shown["product"]["tasks"][&draft];
        assert_eq!(task["reviewActions"]["accept"], true, "{task}");
        assert_eq!(task["result"]["acceptanceAvailable"], true, "{task}");
        // Switching back to Automatic settles the same waiting result.
        product["sprints"][0]["runMode"] = json!("automatic");
        let result = automatic(
            &root,
            "project",
            &draft,
            &state,
            &mut product,
            &mut Git::new(),
        )
        .await
        .unwrap();
        assert_eq!(result["accepted"], true);
        assert_eq!(product["tasks"][&draft]["acceptance"]["automatic"], true);
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn a_held_machine_lock_defers_a_settle_without_waiting_or_failing() {
        let (root, runtime, draft) = completed().await;
        let state = automatic_state(&root);
        let product = sprint_product(&draft, "automatic");
        settings::write_json(&product_path(&state, "project").unwrap(), &product).unwrap();
        let held = settings::Lock::new(&state, "machine.lock").unwrap();
        let settle = json!({"taskId":draft,"action":"auto_ready"});
        let started = std::time::Instant::now();
        let deferred =
            super::super::task_actions::run(&root, "project", &state, &settle, &mut Git::new())
                .await
                .unwrap();
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(deferred, json!({"accepted":false,"deferred":true}));
        // The runtime writer is never behind the machine lock.
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            with_task(&runtime, |db, task| {
                lifecycle::event_tx(db, task, "NATIVE_ROUTE_SELECTED", json!({"fixture":true}))?;
                Ok(())
            }),
        )
        .await
        .expect("runtime writer waited for the machine lock");
        drop(held);
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn automatic_acceptance_refuses_stale_or_open_checks_and_unsprinted_results() {
        for outcome in [
            "standalone",
            "closed",
            "missing",
            "stale",
            "failed",
            "not_run",
            "unverified",
            "owner_resolved",
            "no_runtime_verification",
        ] {
            let (root, runtime, draft) = completed().await;
            let state = automatic_state(&root);
            let mut product = sprint_product(&draft, "automatic");
            if outcome == "standalone" {
                product["tasks"][&draft]["sprintId"] = Value::Null;
            }
            if outcome == "closed" {
                product["sprints"][0]["archivedAt"] = json!(1);
            }
            settings::write_json(&product_path(&state, "project").unwrap(), &product).unwrap();
            if outcome == "missing" {
                with_task(&runtime, |db, task| {
                    db.execute(
                        "UPDATE runtime_tasks SET synthesis=NULL WHERE task_id=?",
                        [task],
                    )?;
                    Ok(())
                })
                .await;
            }
            if outcome == "stale" {
                std::fs::write(root.join("work.txt"), "drift").unwrap();
            }
            // Automatic acceptance needs the runtime's passed verification and no owner
            // resolution of the task.
            if outcome == "owner_resolved" {
                with_task(&runtime, |db, task| {
                    lifecycle::event_tx(db, task, "OWNER_TASK_RESOLVED", json!({"fixture":true}))?;
                    Ok(())
                })
                .await;
            }
            if outcome == "no_runtime_verification" {
                with_task(&runtime, |db, task| {
                    db.execute(
                        "DELETE FROM runtime_events WHERE task_id=? AND type='VERIFICATION_PASSED'",
                        [task],
                    )?;
                    Ok(())
                })
                .await;
            }
            if ["failed", "not_run", "unverified"].contains(&outcome) {
                report_checks(&runtime,json!([{"name":"open","outcome":outcome,"command":["/bin/false"],"blocker":"Unavailable in fixture"}]),json!([])).await;
                // A later successful runtime verification supersedes self-reported
                // not-run checks. Keep this fixture genuinely unverified instead.
                with_task(&runtime, |db, task| {
                    lifecycle::event_tx(
                        db,
                        task,
                        "VERIFICATION_FAILED",
                        json!({"revision":0,"checks":[]}),
                    )?;
                    Ok(())
                })
                .await;
            }
            assert_eq!(
                automatic(
                    &root,
                    "project",
                    &draft,
                    &state,
                    &mut product,
                    &mut Git::new()
                )
                .await
                .unwrap()["accepted"],
                false,
                "{outcome}"
            );
            assert!(product["tasks"][&draft]["acceptance"].is_null());
            runtime.shutdown().await;
            drop(runtime);
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[tokio::test]
    async fn automatic_acceptance_keeps_unrequested_policy_checks_as_notes() {
        let (root, runtime, draft) = completed().await;
        let state = automatic_state(&root);
        let mut product = sprint_product(&draft, "automatic");
        report_checks(
            &runtime,
            json!([{"name":"Paid live qualification","outcome":"not_run",
                "blocker":"paid live check not requested by the owner"}]),
            json!([{"name":"Paseo CLI qualification","outcome":"not_run",
                "command":["scripts/cortex-paseo-tools.sh","probe-codex"],
                "blocker":"paid live check not requested by the owner"}]),
        )
        .await;
        let result = automatic(
            &root,
            "project",
            &draft,
            &state,
            &mut product,
            &mut Git::new(),
        )
        .await
        .unwrap();
        assert_eq!(result["accepted"], true, "{result}");
        assert_eq!(
            product["tasks"][&draft]["acceptance"]["open"],
            json!({
                "failed": 0, "not_run": 0, "unverified": 0
            })
        );
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Owner store 2026-11-23: a self-reported check whose blocker only mentions
    /// refusal tests and a missing host capability is an environment gap, a note once
    /// the runtime verification passed; an explicit restriction still asks the owner.
    #[tokio::test]
    async fn automatic_acceptance_takes_an_environment_gap_mentioning_refusals_as_a_note() {
        for (blocker, accepted) in [
            ("The harness reported native command namespaces unavailable on this host. Its refusal cases ran; positive journeys did not. Permission tests passed.", true),
            ("Running it requires approval from the owner.", false),
        ] {
            let (root, runtime, draft) = completed().await;
            let state = automatic_state(&root);
            let mut product = sprint_product(&draft, "automatic");
            report_checks(
                &runtime,
                json!([{"name":"Native session positive scenarios","outcome":"unverified","blocker":blocker}]),
                json!([]),
            )
            .await;
            let result = automatic(&root, "project", &draft, &state, &mut product, &mut Git::new())
                .await
                .unwrap();
            assert_eq!(result["accepted"], accepted, "{blocker}: {result}");
            runtime.shutdown().await;
            drop(runtime);
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    /// Owner store 2026-11-22: the final review was INCOMPLETE with its unreachable
    /// parts disclosed as limits, the runtime verified and completed the task, and the
    /// Automatic acceptance still held the sprint ("Independent review is not
    /// approved"). Such a review is approved with its limits as notes; an INCOMPLETE
    /// review without that disclosure is not.
    #[tokio::test]
    async fn automatic_acceptance_takes_a_review_with_disclosed_limits_as_approved() {
        for disclosed in [true, false] {
            let (root, runtime, draft) = completed().await;
            let state = automatic_state(&root);
            let mut product = sprint_product(&draft, "automatic");
            report_checks(
                &runtime,
                json!([]),
                json!([{"name":"Review could not read src/a.rs","outcome":"unverified","source":"runtime",
                    "blocker":"The reviewer's tools could not reach src/a.rs; it is not covered by the review."}]),
            )
            .await;
            with_task(&runtime, move |db, task| {
                db.execute(
                    "UPDATE runtime_nodes SET result_json=json_set(result_json,'$.verdict','INCOMPLETE','$.review_limit_disclosed',json(?)) WHERE task_id=? AND stage='review'",
                    (if disclosed { "true" } else { "false" }, task),
                )?;
                Ok(())
            })
            .await;
            let result = automatic(
                &root,
                "project",
                &draft,
                &state,
                &mut product,
                &mut Git::new(),
            )
            .await
            .unwrap();
            assert_eq!(result["accepted"], disclosed, "{disclosed}: {result}");
            runtime.shutdown().await;
            drop(runtime);
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    /// Owner store 2026-11-22: "Paid live check not requested by the owner for this
    /// recovery; ..." held the Automatic sprint for the owner although it is the
    /// documented no-paid convention with extra detail. It is an optional note.
    #[tokio::test]
    async fn automatic_acceptance_takes_an_unrequested_paid_check_with_detail_as_a_note() {
        for blocker in [
            "Paid live check not requested by the owner for this recovery; remote effects are outside assignment scope.",
            "Платная живая проверка не запрошена владельцем.",
        ] {
            let (root, runtime, draft) = completed().await;
            let state = automatic_state(&root);
            let mut product = sprint_product(&draft, "automatic");
            report_checks(
                &runtime,
                json!([{"name":"New paid desktop and remote GitHub qualification","outcome":"not_run",
                    "evidence_strength":"none","blocker":blocker}]),
                json!([]),
            )
            .await;
            let result = automatic(
                &root,
                "project",
                &draft,
                &state,
                &mut product,
                &mut Git::new(),
            )
            .await
            .unwrap();
            assert_eq!(result["accepted"], true, "{blocker}: {result}");
            runtime.shutdown().await;
            drop(runtime);
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    /// Owner store 2026-11-22: a completed task whose review disclosed unread files
    /// ("... after repeated coverage refusals ...") held the Automatic sprint for the
    /// owner because "refus" read as a policy restriction. A limitation the runtime
    /// itself disclosed is a manual note: the result is accepted, never as passed.
    #[tokio::test]
    async fn automatic_acceptance_takes_runtime_disclosed_limits_as_notes() {
        let (root, runtime, draft) = completed().await;
        let state = automatic_state(&root);
        let mut product = sprint_product(&draft, "automatic");
        report_checks(
            &runtime,
            json!([]),
            json!([{"name":"Changed files the reviewer did not read","outcome":"unverified","source":"runtime",
                "blocker":"70 changed files were still unread after repeated coverage refusals and are not reviewed: a.rs; inspect them directly."}]),
        )
        .await;
        let result = automatic(
            &root,
            "project",
            &draft,
            &state,
            &mut product,
            &mut Git::new(),
        )
        .await
        .unwrap();
        assert_eq!(result["accepted"], true, "{result}");
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn automatic_acceptance_does_not_treat_policy_variants_as_optional_notes() {
        // A reported receipt, a plan command, or a different refusal requires review.
        let cases = [
            json!({"name":"reported receipt","outcome":"not_run",
                "tool_call_ref":"native-command:v","blocker":"paid live check not requested by the owner"}),
            json!({"name":"required command","outcome":"not_run",
                "command":["/bin/true"],
                "blocker":"paid live check not requested by the owner"}),
            json!({"name":"similar policy wording","outcome":"not_run",
                "blocker":"paid live check was not requested by the owner"}),
        ];
        for check in cases {
            let (root, runtime, draft) = completed().await;
            let state = automatic_state(&root);
            let mut product = sprint_product(&draft, "automatic");
            report_checks(&runtime, json!([check]), json!([])).await;
            let result = automatic(
                &root,
                "project",
                &draft,
                &state,
                &mut product,
                &mut Git::new(),
            )
            .await
            .unwrap();
            assert_eq!(result["accepted"], false, "{result}");
            assert!(result["reason"].is_string(), "{result}");
            assert!(product["tasks"][&draft]["acceptance"].is_null());
            runtime.shutdown().await;
            drop(runtime);
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[tokio::test]
    async fn automatic_acceptance_refuses_optional_note_when_runtime_verification_failed() {
        let (root, runtime, draft) = completed().await;
        let state = automatic_state(&root);
        let mut product = sprint_product(&draft, "automatic");
        report_checks(
            &runtime,
            json!([{"name":"Paid live qualification","outcome":"not_run",
                "blocker":"paid live check not requested by the owner"}]),
            json!([]),
        )
        .await;
        with_task(&runtime, |db, task| {
            lifecycle::event_tx(
                db,
                task,
                "VERIFICATION_FAILED",
                json!({"revision":0,"checks":[]}),
            )?;
            Ok(())
        })
        .await;
        let result = automatic(
            &root,
            "project",
            &draft,
            &state,
            &mut product,
            &mut Git::new(),
        )
        .await
        .unwrap();
        assert_eq!(result["accepted"], false, "{result}");
        assert!(result["reason"].is_string(), "{result}");
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn a_settle_that_cannot_finish_reports_why_and_is_not_retried_every_pass() {
        let (root, runtime, draft) = completed().await;
        let state = automatic_state(&root);
        let mut product = sprint_product(&draft, "automatic");
        // The acceptance cannot be written: its saved product is a directory.
        let path = product_path(&state, "project").unwrap();
        std::fs::create_dir_all(&path).unwrap();
        let first = automatic(
            &root,
            "project",
            &draft,
            &state,
            &mut product,
            &mut Git::new(),
        )
        .await
        .unwrap();
        assert_eq!(first["accepted"], false, "{first}");
        assert!(
            first["failed"].is_string() && first["reason"].is_string(),
            "{first}"
        );
        // Remembered: the next pass gets the same answer without evaluating again.
        let key = (state.clone(), "project".to_owned(), draft.clone());
        let at = REFUSED.lock().unwrap().get(&key).map(|m| m.at).unwrap();
        // Each call reads the saved product again; nothing was saved.
        let mut product = sprint_product(&draft, "automatic");
        let again = automatic(
            &root,
            "project",
            &draft,
            &state,
            &mut product,
            &mut Git::new(),
        )
        .await
        .unwrap();
        assert_eq!(again, first);
        assert_eq!(REFUSED.lock().unwrap().get(&key).map(|m| m.at), Some(at));
        // Once writable and after a mode switch, it settles.
        std::fs::remove_dir(&path).unwrap();
        forget_refusals(&state, "project");
        let mut product = sprint_product(&draft, "automatic");
        assert_eq!(
            automatic(
                &root,
                "project",
                &draft,
                &state,
                &mut product,
                &mut Git::new()
            )
            .await
            .unwrap()["accepted"],
            true
        );
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn an_unchanged_refusal_is_not_reevaluated_until_its_inputs_change() {
        let (root, runtime, draft) = completed().await;
        let state = automatic_state(&root);
        let mut product = sprint_product(&draft, "automatic");
        settings::write_json(&product_path(&state, "project").unwrap(), &product).unwrap();
        // The folder signal needs a commit to read.
        assert!(std::process::Command::new("/usr/bin/git")
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "fixture"
            ])
            .current_dir(&root)
            .status()
            .unwrap()
            .success());
        report_checks(&runtime,json!([{"name":"open","outcome":"failed","command":["/bin/false"],"blocker":"Unavailable in fixture"}]),json!([])).await;
        with_task(&runtime, |db, task| {
            lifecycle::event_tx(
                db,
                task,
                "VERIFICATION_FAILED",
                json!({"revision":0,"checks":[]}),
            )?;
            Ok(())
        })
        .await;
        // The folder signal trusts no file written in the last two seconds.
        tokio::time::sleep(std::time::Duration::from_millis(3100)).await;
        let key = (state.clone(), "project".to_owned(), draft.clone());
        let memo = || {
            REFUSED
                .lock()
                .unwrap()
                .get(&key)
                .map(|m| (m.inputs.clone().unwrap_or_default(), m.at))
        };
        assert_eq!(
            automatic(
                &root,
                "project",
                &draft,
                &state,
                &mut product,
                &mut Git::new()
            )
            .await
            .unwrap()["accepted"],
            false
        );
        let (inputs, at) = memo().expect("a refusal with a folder signal is remembered");
        // Unchanged inputs: the refusal is returned without evaluating again.
        assert_eq!(
            automatic(
                &root,
                "project",
                &draft,
                &state,
                &mut product,
                &mut Git::new()
            )
            .await
            .unwrap()["accepted"],
            false
        );
        assert_eq!(memo(), Some((inputs.clone(), at)));
        // New runtime evidence re-evaluates it.
        with_task(&runtime, |db, task| {
            lifecycle::event_tx(db, task, "NATIVE_ROUTE_SELECTED", json!({"fixture":true}))?;
            Ok(())
        })
        .await;
        assert_eq!(
            automatic(
                &root,
                "project",
                &draft,
                &state,
                &mut product,
                &mut Git::new()
            )
            .await
            .unwrap()["accepted"],
            false
        );
        let (changed, again) = memo().unwrap();
        assert_ne!(changed, inputs);
        assert!(again > at);
        // So does a change of the saved task record.
        product["tasks"][&draft]["targetBranch"] = json!("main");
        assert_eq!(
            automatic(
                &root,
                "project",
                &draft,
                &state,
                &mut product,
                &mut Git::new()
            )
            .await
            .unwrap()["accepted"],
            false
        );
        assert_ne!(memo().unwrap().0, changed);
        // A run-mode switch forgets every refusal of the project.
        forget_refusals(&state, "project");
        assert!(memo().is_none());
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn a_merging_task_refuses_its_other_actions_at_once_and_defers_its_settle() {
        let (root, runtime, draft) = completed().await;
        let state = automatic_state(&root);
        let product = sprint_product(&draft, "automatic");
        settings::write_json(&product_path(&state, "project").unwrap(), &product).unwrap();
        let lock = settings::Lock::new(&state, "machine.lock").unwrap();
        let claim = settings::claim(
            &state,
            "project",
            &draft,
            "This task is being merged. Wait for it to finish.",
            &lock,
        )
        .unwrap();
        drop(lock);
        let started = std::time::Instant::now();
        let refused = super::super::task_actions::run(
            &root,
            "project",
            &state,
            &json!({"taskId":draft,"action":"target","branch":"main"}),
            &mut Git::new(),
        )
        .await
        .err()
        .unwrap();
        assert_eq!(
            (refused.code, refused.message.as_str()),
            (
                "operation_in_progress",
                "This task is being merged. Wait for it to finish."
            )
        );
        let deferred = super::super::task_actions::run(
            &root,
            "project",
            &state,
            &json!({"taskId":draft,"action":"auto_ready"}),
            &mut Git::new(),
        )
        .await
        .unwrap();
        assert_eq!(deferred, json!({"accepted":false,"deferred":true}));
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        drop(claim);
        let accepted = super::super::task_actions::run(
            &root,
            "project",
            &state,
            &json!({"taskId":draft,"action":"auto_ready"}),
            &mut Git::new(),
        )
        .await
        .unwrap();
        assert_eq!(accepted["accepted"], true);
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    async fn presented(runtime: &Runtime, draft: &str) -> Value {
        let facts = runtime
            .owner_facts(
                "project",
                json!({"tasks":{},"sprints":[],"messages":[]}),
                json!({}),
            )
            .await
            .unwrap();
        facts["product"]["tasks"][draft].clone()
    }
    #[tokio::test]
    async fn finished_native_result_is_presented_acceptable_and_accept_records_it() {
        let (root, runtime, draft) = completed().await;
        let task = presented(&runtime, &draft).await;
        assert_eq!(task["reviewActions"]["accept"], true, "{task}");
        assert_eq!(task["result"]["acceptanceAvailable"], true, "{task}");
        assert_eq!(task["result"]["kind"], "work");
        assert_eq!(task["result"]["digest"], task["workspaceDigest"]);
        // The same-model review is a visible note, not unavailable evidence.
        assert_eq!(
            task["result"]["warnings"][0],
            "The reviewer runs on the same model as the work it reviews."
        );
        assert!(task["reviewActions"]["workspaceChanged"].is_null());
        assert_eq!(task["result"]["checks"][0]["source"], "runtime");
        // Private store area: an empty directory elsewhere in the project is a
        // project change and would make the recorded digest stale.
        let state = root.join(".cortex-dev/acceptance-state");
        std::fs::create_dir(&state).unwrap();
        let saved = |draft: &str| {
            let mut product = json!({"tasks":{},"sprints":[]});
            product["tasks"][draft] = json!({"checkout":{"kind":"main"}});
            product
        };
        let mut product = saved(&draft);
        let p = json!({"action":"accept","taskId":draft,"digest":task["result"]["digest"],
            "version":task["result"]["sourceVersion"],"acknowledgeOpenChecks":false});
        let accepted = accept(
            &root,
            "project",
            &draft,
            &state,
            &mut product,
            &p,
            &mut Git::new(),
        )
        .await
        .unwrap_or_else(|e| panic!("{}: {}", e.code, e.message));
        assert_eq!(accepted["done"], true);
        assert_eq!(product["tasks"][&draft]["status"], "ready_to_merge");
        assert_eq!(
            product["tasks"][&draft]["acceptance"]["digest"],
            p["digest"]
        );
        // Files changed after the result: no longer acceptable, and Accept of the
        // old identity is refused without recording anything.
        std::fs::write(root.join("work.txt"), "owner change").unwrap();
        let drifted = presented(&runtime, &draft).await;
        assert_eq!(drifted["result"]["acceptanceAvailable"], false);
        assert_eq!(drifted["reviewActions"]["workspaceChanged"], true);
        assert_ne!(drifted["result"]["digest"], drifted["workspaceDigest"]);
        assert_eq!(drifted["reviewActions"]["unblockAction"], "request_changes");
        let mut fresh = saved(&draft);
        let refused = accept(
            &root,
            "project",
            &draft,
            &state,
            &mut fresh,
            &p,
            &mut Git::new(),
        )
        .await
        .unwrap_err();
        assert_eq!(refused.code, "stale_result");
        assert!(fresh["tasks"][&draft]["acceptance"].is_null());
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn native_workers_report_their_edits_result_and_checks() {
        let (root, runtime, draft) = completed().await;
        let facts = runtime
            .owner_facts(
                "project",
                json!({"tasks":{},"sprints":[],"messages":[]}),
                json!({}),
            )
            .await
            .unwrap();
        let workers = facts["workers"].as_array().unwrap();
        let work = workers.iter().find(|w| w["role"] == "backend-dev").unwrap();
        assert_eq!(work["taskId"], draft);
        assert_eq!(work["result"]["summary"], "Wrote work.txt.");
        assert_eq!(work["result"]["reportId"], "r_work");
        assert_eq!(work["result"]["checks"][0]["source"], "worker");
        assert_eq!(
            work["result"]["edits"],
            json!([{"path":"work.txt","add":5,"del":1}])
        );
        let edits: Vec<_> = work["activity"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|a| a["kind"] == "edit")
            .collect();
        assert_eq!(edits.len(), 2);
        assert_eq!(edits[0]["code"], "work.txt");
        assert_eq!(edits[0]["diff"]["add"], 3);
        // The recorded write opens on its changed lines; a clipped record shows its counts only.
        assert_eq!(
            edits[0]["diff"]["lines"],
            json!(["@@ -1,2 +1,4 @@", " a", "-b", "+B", "+c", "+d"])
        );
        assert_eq!(edits[1]["diff"]["lines"], json!([]));
        assert_eq!(edits[1]["diff"]["add"], 2);
        // The reviewer made no edits.
        let review = workers
            .iter()
            .find(|w| w["role"] == "code-reviewer")
            .unwrap();
        assert_eq!(review["result"]["edits"], json!([]));
        assert!(review["activity"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["kind"] != "edit"));
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Owner store 2026-11-17: a worker card read "This worker reported no result" with
    /// no reason after its result submissions were refused. Its recorded refusals (each
    /// distinct code and up to three items the latest named) are on the card; a worker
    /// with none keeps its recorded shape.
    #[tokio::test]
    async fn a_workers_refused_results_are_on_its_card() {
        let (root, runtime, _) = completed().await;
        runtime
            .host
            .executor
            .publication(|store| {
                store.write(|db| {
                    let (task, attempt): (String, String) = db.query_row(
                        "SELECT task_id,attempt_id FROM delegation_provisioning WHERE idempotency_key='a-work'",
                        [],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )?;
                    for code in ["open_failed_check", "omitted_failed_check", "open_failed_check"] {
                        lifecycle::event_tx(db, &task, "WORKER_RESULT_REFUSED", json!({"channel_id":"c","attempt":attempt,"assignment_ref":"a","code":code}))?;
                    }
                    // The latest refusal names what was outstanding; up to three items ride with it.
                    lifecycle::event_tx(db, &task, "WORKER_RESULT_REFUSED", json!({"channel_id":"c","attempt":attempt,"assignment_ref":"a",
                        "code":"review_file_coverage_required","missing":["README.md (current complete read is missing or stale)","a.rs","b.rs","c.rs"]}))?;
                    Ok(())
                })?;
                Ok(())
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let facts = runtime
            .owner_facts(
                "project",
                json!({"tasks":{},"sprints":[],"messages":[]}),
                json!({}),
            )
            .await
            .unwrap();
        let workers = facts["workers"].as_array().unwrap();
        let work = workers.iter().find(|w| w["role"] == "backend-dev").unwrap();
        assert_eq!(
            work["resultRefusals"],
            json!({"count":4,"code":"review_file_coverage_required",
                "codes":["open_failed_check","omitted_failed_check","review_file_coverage_required"],
                "missing":["README.md (current complete read is missing or stale)","a.rs","b.rs"]})
        );
        let review = workers
            .iter()
            .find(|w| w["role"] == "code-reviewer")
            .unwrap();
        assert!(review.get("resultRefusals").is_none());
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Runs one write against the fixture's runtime task and its session.
    async fn mutate(
        runtime: &Runtime,
        f: impl FnOnce(&Connection, &str, &str) -> cortex_core::CoreResult<()> + Send + 'static,
    ) {
        runtime
            .host
            .executor
            .publication(move |store| {
                store.write(|db| {
                    let (task, session): (String, String) = db.query_row(
                        "SELECT runtime_task_id,acp_session_id FROM workspace_executions LIMIT 1",
                        [],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )?;
                    f(db, &task, &session)
                })?;
                Ok(())
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
    }
    fn page_sequences(worker: &Value) -> Vec<i64> {
        worker["activity"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["sequence"].as_i64().unwrap())
            .collect()
    }

    /// The worker of the fixture reports, beside its passed check, two claims the runtime's
    /// passing verification supersedes (one run before the last edit, one not run for a
    /// recorded technical blocker) and a failed probe it withdrew. The withdrawn probe's
    /// command ran on the worker's channel, whose output the native host keeps under the
    /// channel's scope (`output`: the stored bytes, or None for a file that is gone).
    async fn with_claims(runtime: &Runtime, output: Option<&'static [u8]>) {
        runtime.host.executor.publication(move |store| {
            let directory = store.key().directory();
            store.write(|db| {
                let task: String = db.query_row("SELECT runtime_task_id FROM workspace_executions LIMIT 1", [], |r| r.get(0))?;
                let bytes: &[u8] = b"boom";
                let folder = directory.join(&task).join(".native-output");
                std::fs::create_dir_all(&folder)?;
                if let Some(stored) = output {
                    // Private, as the native host keeps it.
                    std::fs::write(folder.join("lo_p.bin"), stored)?;
                    std::fs::set_permissions(folder.join("lo_p.bin"), std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
                }
                let sha = format!("{:x}", Sha256::digest(bytes));
                db.execute("INSERT INTO runtime_requests VALUES (?,'kp','ch-work','command_run','d',?)", (&task,
                    json!({"tool_call_ref":"native-command:p","argv":["/bin/false"],"cwd":"/fixture","profile":"development",
                        "complete":true,"exit_code":1,"failure":null,"large_output_id":"lo_p","byte_count":4}).to_string()))?;
                db.execute("INSERT INTO large_outputs VALUES ('lo_p',?,'ch-work','terminal',?,'lo_p.bin',4,0,1,NULL,NULL,'now')", (&task, &sha))?;
                let raw: String = db.query_row("SELECT result_json FROM runtime_nodes WHERE task_id=? AND stage='execution'", [&task], |r| r.get(0))?;
                let mut result: Value = serde_json::from_str(&raw)?;
                for claim in [
                    json!({"name":"early","outcome":"unverified","evidence_gap":"revision_mismatch","command":["/bin/true"]}),
                    json!({"name":"deployed","outcome":"not_run","blocker":"No deployed environment is available."}),
                    json!({"name":"probe","outcome":"failed","tool_call_ref":"native-command:p","probe_withdrawn":"The probe was withdrawn.","retracted_by":"probe_withdrawn"}),
                ] {
                    result["checks"].as_array_mut().unwrap().push(claim);
                }
                db.execute("UPDATE runtime_nodes SET result_json=? WHERE task_id=? AND stage='execution'", (result.to_string(), &task))?;
                Ok(())
            })?;
            Ok(())
        }).unwrap().await.unwrap().unwrap();
    }
    /// Owner report 2026-11-17: runtime verification passed and the review approved, the card
    /// said the self-reported checks do not block acceptance, and Accept stayed off ("Run the
    /// unpassed checks first"). The worker's withdrawn probe stayed a failed check because its
    /// output, kept under the channel that ran it, was only looked for under the assignment.
    #[tokio::test]
    async fn a_withdrawn_probe_and_superseded_claims_leave_accept_available() {
        let (root, runtime, draft) = completed().await;
        with_claims(&runtime, Some(b"boom")).await;
        let task = presented(&runtime, &draft).await;
        assert_eq!(task["reviewActions"]["accept"], true, "{task}");
        assert_eq!(
            task["reviewActions"]["acknowledgeOpenChecks"], false,
            "{task}"
        );
        let notes = task["result"]["warnings"].to_string();
        assert!(
            notes.contains("2 self-reported checks it does not cover are shown for information"),
            "{notes}"
        );
        assert!(
            notes.contains(
                "\u{201c}deployed\u{201d}: not run: No deployed environment is available."
            ),
            "{notes}"
        );
        assert!(!notes.contains("probe"), "{notes}");
        // Accept takes exactly this result; the validator observes the folder fresh.
        let state = root.join(".cortex-dev/acceptance-state");
        std::fs::create_dir(&state).unwrap();
        let mut product = json!({"tasks":{},"sprints":[]});
        product["tasks"][&draft] = json!({"checkout":{"kind":"main"}});
        let p = json!({"action":"accept","taskId":draft,"digest":task["result"]["digest"],
            "version":task["result"]["sourceVersion"],"acknowledgeOpenChecks":false});
        accept(
            &root,
            "project",
            &draft,
            &state,
            &mut product,
            &p,
            &mut Git::new(),
        )
        .await
        .unwrap_or_else(|e| panic!("{}: {}", e.code, e.message));
        assert_eq!(product["tasks"][&draft]["status"], "ready_to_merge");
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }
    /// A verification the native runtime recorded is in the native digest domain only. When
    /// the folder changed after it, the result is stale, and no legacy walk of the whole
    /// folder runs to look for a match that cannot exist (it made every product read of a
    /// project with stale results slow).
    #[tokio::test]
    async fn a_native_result_on_a_changed_folder_takes_no_legacy_capture() {
        let (root, runtime, draft) = completed().await;
        std::fs::write(root.join("work.txt"), "owner change").unwrap();
        let task = presented(&runtime, &draft).await;
        assert_eq!(task["reviewActions"]["workspaceChanged"], true, "{task}");
        assert_eq!(task["reviewActions"]["accept"], false);
        assert_eq!(crate::machine::legacy_source::scans(&root), 0);
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }
    /// A withdrawal that cannot be re-derived (the output the probe's run was bound to is gone)
    /// keeps the probe a failed check. Accept stays off, and the reason names that check.
    #[tokio::test]
    async fn an_unconfirmed_withdrawal_blocks_accept_and_the_reason_names_the_check() {
        let (root, runtime, draft) = completed().await;
        with_claims(&runtime, None).await;
        let task = presented(&runtime, &draft).await;
        assert_eq!(task["reviewActions"]["accept"], false, "{task}");
        assert_eq!(task["reviewActions"]["unblockAction"], "rerun_checks");
        let reason = task["reviewActions"]["reason"].as_str().unwrap();
        assert!(
            reason.starts_with(
                "Worker check \u{201c}probe\u{201d} failed. Rerun that check or request changes."
            ),
            "{reason}"
        );
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }
    /// A changed stored output (its hash no longer matches what completion recorded) does not
    /// count either: the withdrawal is re-derived only from the bytes completion saw.
    #[tokio::test]
    async fn a_changed_probe_output_does_not_confirm_its_withdrawal() {
        let (root, runtime, draft) = completed().await;
        with_claims(&runtime, Some(b"BOOM")).await;
        let task = presented(&runtime, &draft).await;
        assert_eq!(task["reviewActions"]["accept"], false, "{task}");
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }
    /// The task screen's first read (`defer`) does not wait for a slow folder observation: the
    /// result's own record and the task's workers come at once, Accept is off with a pending
    /// reason, and the read that follows has the settled review. Accept still observes fresh.
    #[tokio::test]
    async fn a_deferring_read_shows_the_result_while_the_folder_observation_runs() {
        let (root, runtime, draft) = completed().await;
        let observed = root.clone();
        let slow = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            crate::machine::task_facts::recent_observation(observed).await
        });
        let began = std::time::Instant::now();
        let facts = runtime
            .owner_facts_observed(
                "project",
                json!({"tasks":{},"sprints":[],"messages":[]}),
                json!({"defer":true,"taskIds":[draft]}),
                slow,
            )
            .await
            .unwrap();
        assert!(
            began.elapsed() < std::time::Duration::from_secs(2),
            "{:?}",
            began.elapsed()
        );
        let task = &facts["product"]["tasks"][&draft];
        assert_eq!(task["result"]["summary"], "Fixture result.", "{task}");
        assert_eq!(task["result"]["version"], 1);
        assert_eq!(task["result"]["checks"][0]["source"], "runtime");
        assert_eq!(task["reviewActions"]["pending"], true);
        assert_eq!(task["reviewActions"]["accept"], false);
        assert_eq!(
            task["reviewActions"]["reason"],
            crate::native_owner_controls::CHECKING_REASON
        );
        assert!(task["reviewActions"]["unblockAction"].is_null());
        assert!(
            facts["workers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|w| w["taskId"] == draft),
            "{}",
            facts["workers"]
        );
        // The next read settles: the same result, now acceptable.
        let observing = tokio::spawn(crate::machine::task_facts::recent_observation(root.clone()));
        let settled = runtime
            .owner_facts_observed(
                "project",
                json!({"tasks":{},"sprints":[],"messages":[]}),
                json!({"taskIds":[draft]}),
                observing,
            )
            .await
            .unwrap();
        let task = &settled["product"]["tasks"][&draft];
        assert_eq!(task["reviewActions"]["accept"], true, "{task}");
        assert!(task["reviewActions"]["pending"].is_null());
        assert_eq!(task["result"]["summary"], "Fixture result.");
        // A file changed after that read: Accept of the presented identity is refused.
        std::fs::write(root.join("work.txt"), "owner change").unwrap();
        let state = root.join(".cortex-dev/acceptance-state");
        std::fs::create_dir(&state).unwrap();
        let mut product = json!({"tasks":{},"sprints":[]});
        product["tasks"][&draft] = json!({"checkout":{"kind":"main"}});
        let p = json!({"action":"accept","taskId":draft,"digest":task["result"]["digest"],
            "version":task["result"]["sourceVersion"],"acknowledgeOpenChecks":false});
        let refused = accept(
            &root,
            "project",
            &draft,
            &state,
            &mut product,
            &p,
            &mut Git::new(),
        )
        .await
        .unwrap_err();
        assert_eq!(refused.code, "stale_result");
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Owner store 2026-11-17: a long native task (many commands, guidance messages, workers and
    /// check receipts) failed every read with `native_facts_limit`. Nothing is refused for size:
    /// a worker's page is a window with a cursor to its older rows, every row is reachable, and a
    /// result whose receipts exceed the read bound is marked and never offered for Accept.
    #[tokio::test]
    async fn a_large_task_reads_in_pages_and_is_never_refused() {
        let (root, runtime, draft) = completed().await;
        let digest = crate::native_effects::observe(root.clone())
            .await
            .unwrap()
            .project_digest
            .unwrap();
        mutate(&runtime, move |db, task, _| {
            for i in 0..250 {
                let reference = format!("native-command:l{i}");
                lifecycle::event_tx(db, task, "NATIVE_COMMAND_STARTED", json!({"ref":reference,"argv":["/bin/echo",i.to_string()],"channel_id":"ch-work"}))?;
                lifecycle::event_tx(db, task, "NATIVE_COMMAND_FINISHED", json!({"receipt":{"revision":0,"argv":["/bin/echo",i.to_string()],"revision_digest":digest,
                    "tool_call_ref":reference,"observation":{"complete":true,"exit_code":0}}}))?;
            }
            for i in 0..120 {
                lifecycle::event_tx(db, task, "NATIVE_GUIDANCE_QUEUED", json!({"request":format!("g{i}"),"worker":"a-work","text":"keep going"}))?;
            }
            for i in 0..120 {
                let (key, channel) = (format!("a-extra{i}"), format!("ch-extra{i}"));
                db.execute("INSERT INTO runtime_nodes VALUES (?,0,?,'worker-backend-dev','Extra','[]','[]','execution','completed',?,NULL)", (task, format!("extra{i}"), &key))?;
                db.execute("INSERT INTO delegation_provisioning(idempotency_key,task_id,assignment_id,attempt_id,role_id,step,status,channel_id,owner_epoch,created_at,updated_at) VALUES (?,?,?,?,'role','ready','ready',?,1,'now','now')",
                    (&key, task, &key, format!("att-{key}"), &channel))?;
            }
            Ok(())
        })
        .await;
        let facts = runtime
            .owner_facts(
                "project",
                json!({"tasks":{},"sprints":[],"messages":[]}),
                json!({}),
            )
            .await
            .unwrap();
        // Every guidance message is present; the roster holds all 122 workers.
        assert_eq!(facts["product"]["messages"].as_array().unwrap().len(), 120);
        assert_eq!(facts["workers"].as_array().unwrap().len(), 122);
        assert!(facts["partial"]["messages"].is_null() && facts["partial"]["workers"].is_null());
        let work = facts["workers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["id"] == "a-work")
            .unwrap()
            .clone();
        assert_eq!(work["activity"].as_array().unwrap().len(), 200);
        assert_eq!(work["activityPartial"], true);
        // The pages together hold every command and both edits once, newest page first.
        let mut all = page_sequences(&work);
        let mut cursor = work["activityCursor"].as_str().map(str::to_owned);
        let mut pages = 1;
        while let Some(next) = cursor {
            let page = runtime
                .owner_facts(
                    "project",
                    json!({"tasks":{},"sprints":[],"messages":[]}),
                    json!({"taskIds":[draft],"workerId":"a-work","cursor":next}),
                )
                .await
                .unwrap();
            let worker = page["workers"].as_array().unwrap()[0].clone();
            assert!(page_sequences(&worker).iter().all(|s| !all.contains(s)));
            all.extend(page_sequences(&worker));
            cursor = worker["activityCursor"].as_str().map(str::to_owned);
            pages += 1;
            assert!(pages < 10);
        }
        let mut unique = all.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), all.len());
        assert_eq!(all.len(), 250 + 2);
        assert_eq!(pages, 2);
        // The result's receipts are within the bound here, so Accept stays decided by the review.
        assert!(facts["product"]["tasks"][&draft]["result"]["checksPartial"].is_null());
        // More receipts than one read presents: marked, and Accept is unavailable with a reason.
        let digest = crate::native_effects::observe(root.clone())
            .await
            .unwrap()
            .project_digest
            .unwrap();
        mutate(&runtime, move |db, task, _| {
            for i in 0..1005 {
                lifecycle::event_tx(db, task, "NATIVE_COMMAND_FINISHED", json!({"receipt":{"revision":0,"argv":["/bin/echo",format!("c{i}")],"revision_digest":digest,
                    "tool_call_ref":format!("native-command:r{i}"),"observation":{"complete":true,"exit_code":0}}}))?;
            }
            Ok(())
        })
        .await;
        let facts = runtime
            .owner_facts(
                "project",
                json!({"tasks":{},"sprints":[],"messages":[]}),
                json!({}),
            )
            .await
            .unwrap();
        let task = &facts["product"]["tasks"][&draft];
        assert_eq!(task["result"]["checksPartial"], true);
        assert_eq!(task["result"]["acceptanceAvailable"], false);
        assert_eq!(task["reviewActions"]["accept"], false);
        assert!(task["reviewActions"]["reason"]
            .as_str()
            .unwrap()
            .contains("check receipts"));
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Nothing a worker did is out of reach: tool steps, narration and queued messages past one page are read behind the cursor.
    #[tokio::test]
    async fn older_steps_narration_and_messages_are_reachable_by_cursor() {
        let (root, runtime, draft) = completed().await;
        mutate(&runtime, |db, task, session| {
            for i in 0..450 {
                lifecycle::event_tx(db, task, "NATIVE_GUIDANCE_QUEUED", json!({"request":format!("g{i}"),"worker":"a-work","text":"keep going"}))?;
                let update = json!({"sessionUpdate":"tool_call","toolCallId":format!("att-a-work:c{i}"),"title":"Read a.ts","kind":"read","rawInput":{"path":"a.ts"},"status":"completed"});
                db.execute("INSERT INTO host_updates VALUES (?,?,?,'2026-11-17T10:00:01.000000+00:00')", (session, 10_000 + i as i64, update.to_string()))?;
                db.execute("INSERT INTO host_transcript(session_id,seq,kind,text,message_id,created_at) VALUES (?,?,'agent_message',?,?,'2026-11-17T10:00:00.000000+00:00')",
                    (session, i as i64, format!("note {i}"), format!("att-a-work:n{i}")))?;
            }
            Ok(())
        })
        .await;
        let read = |params: Value| {
            let runtime = runtime.clone();
            async move {
                runtime
                    .owner_facts(
                        "project",
                        json!({"tasks":{},"sprints":[],"messages":[]}),
                        params,
                    )
                    .await
                    .unwrap()
            }
        };
        let first = read(json!({})).await;
        let mut messages: Vec<String> = first["product"]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["id"].as_str().unwrap().to_owned())
            .collect();
        let mut worker = first["workers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["id"] == "a-work")
            .unwrap()
            .clone();
        let (mut tools, mut notes) = (0, 0);
        let mut pages = 0;
        loop {
            for a in worker["activity"].as_array().unwrap() {
                match a["kind"].as_str().unwrap() {
                    "tool" => tools += 1,
                    "note" => notes += 1,
                    _ => {}
                }
            }
            messages.extend(
                worker["messages"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|m| m["id"].as_str().unwrap().to_owned()),
            );
            let Some(cursor) = worker["activityCursor"].as_str().map(str::to_owned) else {
                break;
            };
            pages += 1;
            assert!(pages < 30, "paging must end");
            let page = read(json!({"taskIds":[draft],"workerId":"a-work","cursor":cursor})).await;
            worker = page["workers"].as_array().unwrap()[0].clone();
        }
        assert_eq!((tools, notes), (450, 450));
        messages.sort();
        let before = messages.len();
        messages.dedup();
        assert_eq!((before, messages.len()), (450, 450));
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Accept itself (not only its presentation) refuses a result whose check receipts exceed what one read presents.
    #[tokio::test]
    async fn accept_refuses_a_partial_receipt_set_with_a_readable_reason() {
        let (root, runtime, draft) = completed().await;
        let digest = crate::native_effects::observe(root.clone())
            .await
            .unwrap()
            .project_digest
            .unwrap();
        mutate(&runtime, move |db, task, _| {
            for i in 0..1005 {
                lifecycle::event_tx(db, task, "NATIVE_COMMAND_FINISHED", json!({"receipt":{"revision":0,"argv":["/bin/echo",format!("c{i}")],"revision_digest":digest,
                    "tool_call_ref":format!("native-command:r{i}"),"observation":{"complete":true,"exit_code":0}}}))?;
            }
            Ok(())
        })
        .await;
        let task = presented(&runtime, &draft).await;
        assert_eq!(task["reviewActions"]["accept"], false);
        let state = root.join(".cortex-dev/acceptance-state");
        std::fs::create_dir(&state).unwrap();
        let mut product = json!({"tasks":{},"sprints":[]});
        product["tasks"][&draft] = json!({"checkout":{"kind":"main"}});
        let p = json!({"action":"accept","taskId":draft,"digest":task["result"]["digest"],
            "version":task["result"]["sourceVersion"],"acknowledgeOpenChecks":false});
        let refused = accept(
            &root,
            "project",
            &draft,
            &state,
            &mut product,
            &p,
            &mut Git::new(),
        )
        .await
        .unwrap_err();
        assert!(
            refused.message.contains("check receipts"),
            "{}: {}",
            refused.code,
            refused.message
        );
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Coordinator, workers and the Conversation each hold only their own rows. One session carries a
    /// coordinator and two workers; the coordinator page pages across more than 200 rows.
    #[tokio::test]
    async fn coordinator_and_worker_pages_hold_only_their_own_rows() {
        let (root, runtime, draft) = completed().await;
        mutate(&runtime, |db, task, session| {
            db.execute("INSERT INTO delegation_provisioning(idempotency_key,task_id,assignment_id,attempt_id,role_id,step,status,owner_epoch,created_at,updated_at) VALUES ('a-coord',?,'a-coord','att-coord','orchestrator','ready','ready',1,'now','now')", [task])?;
            let mut seq = 0;
            let mut say = |db: &Connection, id: &str, text: &str| -> cortex_core::CoreResult<()> {
                seq += 1;
                db.execute("INSERT INTO host_transcript(session_id,seq,kind,text,message_id,created_at) VALUES (?,?,'agent_message',?,?,'2026-11-17T10:00:00.000000+00:00')", (session, seq, text, id))?;
                Ok(())
            };
            for i in 0..250 {
                say(db, "att-coord:m", &format!("coordinator note {i}"))?;
                if i % 50 == 0 {
                    say(db, "att-a-work:m", &format!("work note {i}"))?;
                    say(db, "att-a-review:m", &format!("review note {i}"))?;
                }
            }
            for (n, call) in ["att-coord:c1", "att-a-work:c2", "att-a-review:c3"].into_iter().enumerate() {
                let update = json!({"sessionUpdate":"tool_call","toolCallId":call,"title":"Read a.ts","kind":"read","rawInput":{"path":"a.ts"},"status":"completed"});
                db.execute("INSERT INTO host_updates VALUES (?,?,?,'2026-11-17T10:00:01.000000+00:00')", (session, 10_000 + n as i64, update.to_string()))?;
            }
            Ok(())
        })
        .await;
        // The coordinator page: only coordinator rows, every page reachable by cursor.
        let mut texts = Vec::new();
        let mut cursor = Value::Null;
        let mut pages = 0;
        loop {
            let (d, c) = (draft.clone(), cursor.clone());
            let rows = runtime
                .host
                .executor
                .read(move |db| {
                    crate::machine::coordinator::read(
                        db,
                        "project",
                        &json!({"taskIds":[d],"workerId":format!("coordinator:{d}"),"cursor":c}),
                    )
                })
                .unwrap()
                .await
                .unwrap()
                .unwrap();
            let page = &rows[0];
            for a in page["activity"].as_array().unwrap() {
                texts.push(format!(
                    "{}|{}",
                    a["text"].as_str().unwrap_or(""),
                    a["toolCallRef"].as_str().unwrap_or("")
                ));
            }
            pages += 1;
            assert!(pages < 10);
            match page["activityCursor"].as_str() {
                Some(next) => cursor = json!(next),
                None => break,
            }
        }
        assert!(
            texts
                .iter()
                .filter(|t| t.contains("coordinator note"))
                .count()
                == 250
        );
        assert!(texts.iter().all(|t| !t.contains("work note")
            && !t.contains("review note")
            && !t.contains("att-a-")));
        assert!(texts.iter().any(|t| t.ends_with("att-coord:c1")));
        assert!(pages >= 3);
        // Each worker page: its own narration and calls, nothing of the coordinator or the other worker.
        let facts = runtime
            .owner_facts(
                "project",
                json!({"tasks":{},"sprints":[],"messages":[]}),
                json!({}),
            )
            .await
            .unwrap();
        for (id, mine, other) in [
            ("a-work", "work note", "review note"),
            ("a-review", "review note", "work note"),
        ] {
            let worker = facts["workers"]
                .as_array()
                .unwrap()
                .iter()
                .find(|w| w["id"] == id)
                .unwrap();
            let body = worker["activity"].to_string();
            assert!(body.contains(mine), "{id}");
            assert!(
                !body.contains(other)
                    && !body.contains("coordinator note")
                    && !body.contains("att-coord:"),
                "{id}"
            );
            let other_attempt = if id == "a-work" {
                "att-a-review:"
            } else {
                "att-a-work:"
            };
            assert!(!body.contains(other_attempt), "{id}");
        }
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Owner store 2026-11-17: a worker that submitted a failed outcome (WORKER_FAILED with a
    /// summary and report) showed "reported no result". Its failed result, summary and report
    /// are on its card; workers without a result say why they ended, from recorded facts.
    #[tokio::test]
    async fn a_failed_outcome_and_other_endings_are_on_the_worker_card() {
        let (root, runtime, _) = completed().await;
        let set = |sql: &'static str, kind: Option<(&'static str, Value)>| {
            runtime
                .host
                .executor
                .publication(move |store| {
                    store.write(|db| {
                        let task: String = db.query_row("SELECT task_id FROM delegation_provisioning WHERE idempotency_key='a-work'", [], |r| r.get(0))?;
                        db.execute(sql, [])?;
                        if let Some((kind, payload)) = &kind {
                            lifecycle::event_tx(db, &task, kind, payload.clone())?;
                        }
                        Ok(())
                    })?;
                    Ok(())
                })
                .unwrap()
        };
        set(
            "UPDATE runtime_nodes SET status='failed', result_json='{\"assignment_outcome\":\"failed\",\"blocker\":{\"kind\":\"tool_unavailable\",\"detail\":\"No browser here.\"},\"checks\":[],\"report_ref\":\"r_a408f1d4976c\",\"summary\":\"The documentation update remains incomplete.\"}' WHERE assignment_ref='a-work'",
            None,
        )
        .await
        .unwrap()
        .unwrap();
        set(
            "UPDATE runtime_nodes SET status='cancelled' WHERE assignment_ref='a-review'",
            Some((
                "WORKER_STOPPED_BY_OWNER",
                json!({"assignment_ref":"a-review","reason":"stop"}),
            )),
        )
        .await
        .unwrap()
        .unwrap();
        let facts = || {
            runtime.owner_facts(
                "project",
                json!({"tasks":{},"sprints":[],"messages":[]}),
                json!({}),
            )
        };
        let all = facts().await.unwrap();
        let workers = all["workers"].as_array().unwrap();
        let work = workers.iter().find(|w| w["role"] == "backend-dev").unwrap();
        assert_eq!(work["status"], "failed");
        assert_eq!(work["result"]["outcome"], "failed");
        assert_eq!(work["result"]["reportId"], "r_a408f1d4976c");
        assert_eq!(
            work["result"]["summary"],
            "The documentation update remains incomplete."
        );
        assert_eq!(work["result"]["checks"], json!([]));
        assert!(work.get("endedReason").is_none());
        // The dead-end kind it named, so the card can say it could not run here rather than "Failed".
        assert_eq!(work["blockerKind"], "tool_unavailable");
        let review = workers
            .iter()
            .find(|w| w["role"] == "code-reviewer")
            .unwrap();
        assert!(review["result"].is_null());
        assert_eq!(review["endedReason"], "stopped_by_owner");
        // Without the owner-stop event the same cancelled node is the coordinator's doing.
        set(
            "DELETE FROM runtime_events WHERE type='WORKER_STOPPED_BY_OWNER'",
            None,
        )
        .await
        .unwrap()
        .unwrap();
        let all = facts().await.unwrap();
        let review = all["workers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["role"] == "code-reviewer")
            .unwrap()
            .clone();
        assert_eq!(review["endedReason"], "cancelled_by_coordinator");
        // Its session is not confirmed as stopped yet: the card says it is stopping.
        assert_eq!(review["stopping"], true);
        set(
            "UPDATE delegation_provisioning SET status='terminated' WHERE idempotency_key='a-review'",
            None,
        )
        .await
        .unwrap()
        .unwrap();
        let all = facts().await.unwrap();
        let review = all["workers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["role"] == "code-reviewer")
            .unwrap()
            .clone();
        assert!(review.get("stopping").is_none());
        // A failed worker whose retry records a lost session.
        set(
            "UPDATE runtime_nodes SET status='failed' WHERE assignment_ref='a-review'",
            Some(("ASSIGNMENT_RETRIED", json!({"previous_assignment":"a-review","failure_class":"worker_lost","observed_session":"absent","retry":1}))),
        )
        .await
        .unwrap()
        .unwrap();
        let all = facts().await.unwrap();
        let review = all["workers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["role"] == "code-reviewer")
            .unwrap()
            .clone();
        assert_eq!(review["endedReason"], "worker_lost");
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// A result with no recorded workspace digest is not a claim about the files:
    /// it must not read as stale, and Accept still asks for verification first.
    #[tokio::test]
    async fn result_without_verified_digest_is_not_called_stale() {
        let (root, runtime, draft) = completed().await;
        runtime
            .host
            .executor
            .publication(|store| {
                store.write(|db| {
                    db.execute("DROP TRIGGER IF EXISTS runtime_events_no_delete", [])?;
                    db.execute(
                        "DELETE FROM runtime_events WHERE type='VERIFICATION_PASSED'",
                        [],
                    )?;
                    Ok(())
                })?;
                Ok(())
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let task = presented(&runtime, &draft).await;
        assert!(
            task["result"]["digest"]
                .as_str()
                .unwrap()
                .starts_with("result:"),
            "{task}"
        );
        assert_eq!(task["workspaceDigest"], task["result"]["digest"], "{task}");
        assert_eq!(task["reviewActions"]["accept"], false);
        assert_eq!(task["reviewActions"]["unblockAction"], "rerun_checks");
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Live run: right after Request changes the Changes tab showed "Result v2" with the
    /// accepted result's check and an Accept v2 button while the task was working. A
    /// reopened task presents no result (and so no Accept) until it publishes a new one.
    #[tokio::test]
    async fn a_reopened_working_task_presents_no_result_until_a_new_one_exists() {
        let (root, runtime, draft) = completed().await;
        assert!(presented(&runtime, &draft).await["result"].is_object());
        runtime
            .host
            .executor
            .publication(|store| {
                store.write(|db| {
                    let task: String =
                        db.query_row("SELECT task_id FROM runtime_tasks LIMIT 1", [], |r| {
                            r.get(0)
                        })?;
                    lifecycle::event_tx(
                        db,
                        &task,
                        "TASK_REOPENED",
                        json!({"previous":"COMPLETED","revision":0}),
                    )?;
                    db.execute(
                        "UPDATE runtime_tasks SET state='PLANNING' WHERE task_id=?",
                        [&task],
                    )?;
                    Ok(())
                })?;
                Ok(())
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let task = presented(&runtime, &draft).await;
        assert!(task["result"].is_null(), "{task}");
        assert!(task["reviewActions"].is_null(), "{task}");
        // The next run's own host check is that run's, presented again.
        runtime
            .host
            .executor
            .publication(|store| {
                store.write(|db| {
                    let task: String = db.query_row("SELECT task_id FROM runtime_tasks LIMIT 1", [], |r| r.get(0))?;
                    lifecycle::event_tx(db, &task, "NATIVE_COMMAND_FINISHED", json!({"receipt":{"revision":0,"argv":["/bin/false"],"revision_digest":"next",
                        "tool_call_ref":"native-command:next","observation":{"complete":true,"exit_code":1}}}))?;
                    db.execute("UPDATE runtime_tasks SET state='VERIFYING' WHERE task_id=?", [&task])?;
                    Ok(())
                })?;
                Ok(())
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let next = presented(&runtime, &draft).await;
        assert_eq!(
            next["result"]["checks"].as_array().unwrap().len(),
            1,
            "{next}"
        );
        assert_eq!(next["result"]["checks"][0]["name"], "/bin/false");
        assert_eq!(next["result"]["acceptanceAvailable"], false);
        // This check-only snapshot can ride alongside a newer board that already
        // shows the task in Review. It must not imply that the result failed review.
        assert_eq!(next["reviewActions"]["pending"], true, "{next}");
        assert_eq!(next["reviewActions"]["accept"], false);
        assert_eq!(next["reviewActions"]["unblockAction"], "wait");
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Runs `body` on the fixture's one runtime task inside a store write.
    async fn with_task(
        runtime: &Runtime,
        body: impl FnOnce(&rusqlite::Connection, &str) -> cortex_core::CoreResult<()> + Send + 'static,
    ) {
        runtime
            .host
            .executor
            .publication(move |store| {
                store.write(|db| {
                    let task: String =
                        db.query_row("SELECT task_id FROM runtime_tasks LIMIT 1", [], |r| {
                            r.get(0)
                        })?;
                    body(db, &task)
                })
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
    }
    async fn presented_with(runtime: &Runtime, draft: &str, record: Value) -> Value {
        let mut product = json!({"tasks":{},"sprints":[],"messages":[]});
        product["tasks"][draft] = record;
        let facts = runtime
            .owner_facts("project", product, json!({}))
            .await
            .unwrap();
        facts["product"]["tasks"][draft].clone()
    }
    /// Replaces the worker's and reviewer's reported checks of the fixture result.
    async fn report_checks(runtime: &Runtime, work: Value, review: Value) {
        with_task(runtime, move |db, task| {
            for (stage, checks) in [("execution", work), ("review", review)] {
                db.execute("UPDATE runtime_nodes SET result_json=json_set(result_json,'$.checks',json(?)) WHERE task_id=? AND stage=?",
                    (checks.to_string(), task, stage))?;
            }
            Ok(())
        })
        .await;
    }

    #[tokio::test]
    async fn result_checks_include_not_run_worker_and_reviewer_rows() {
        let (root, runtime, draft) = completed().await;
        let paid = "paid live check not requested by the owner";
        let forbidden = "Deployment is prohibited by policy.";
        report_checks(
            &runtime,
            json!([{"name":"Live desktop","outcome":"not_run","blocker":paid}]),
            json!([{"name":"Deployment","outcome":"not_run","blocker":forbidden}]),
        )
        .await;
        let task = presented(&runtime, &draft).await;
        assert_eq!(task["reviewActions"]["accept"], true, "{task}");
        assert_eq!(task["reviewActions"]["acknowledgeOpenChecks"], true);
        assert_eq!(task["result"]["checks"][0]["outcome"], "passed");
        assert_eq!(
            &task["result"]["checks"].as_array().unwrap()[1..],
            &[
                json!({"name":"Live desktop","outcome":"not_run","detail":paid,
                    "technicalReason":paid,"source":"worker","kind":"policy","rerunnable":false}),
                json!({"name":"Deployment","outcome":"not_run","detail":forbidden,
                    "technicalReason":forbidden,"source":"worker","kind":"policy","rerunnable":false}),
            ]
        );
        assert!(task["reviewActions"].get("notRunChecks").is_none());
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn result_checks_include_not_run_runtime_attempt_with_blocker() {
        let (root, runtime, draft) = completed().await;
        with_task(&runtime, |db, task| {
            let (revision, epoch): (i64, i64) = db.query_row(
                "SELECT revision,epoch FROM runtime_tasks JOIN task_owners USING(task_id) WHERE task_id=?",
                [task],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            db.execute(
                "UPDATE runtime_tasks SET plan_json=json_insert(plan_json,'$.verification[#]',json(?)) WHERE task_id=?",
                (r#"["missing-tool","test"]"#, task),
            )?;
            let payload = json!({
                "binding":{"revision":revision,"owner_epoch":epoch,
                    "target":{"assignment":"runtime-verification","index":1}},
                "result":{"blocker":"verification_executable_unavailable"}
            });
            db.execute(
                "INSERT INTO runtime_events(task_id,type,actor,payload_json,created_at)
                 VALUES (?,'WORKER_CHECK_FINISHED','runtime',?,'2026-11-19T00:00:00Z')",
                (task, payload.to_string()),
            )?;
            Ok(())
        })
        .await;
        let task = presented(&runtime, &draft).await;
        assert_eq!(task["reviewActions"]["accept"], false, "{task}");
        assert!(
            task["result"]["checks"]
                .as_array()
                .unwrap()
                .contains(&json!({
                    "name":"missing-tool test","outcome":"not_run",
                    "detail":"verification_executable_unavailable",
                    "technicalReason":"verification_executable_unavailable",
                    "source":"runtime","kind":"automated","rerunnable":false,
                    "command":["missing-tool","test"]
                })),
            "{task}"
        );
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn result_checks_distinguish_verifier_launch_skip_from_timeout() {
        let (root, runtime, draft) = completed().await;
        with_task(&runtime, |db, task| {
            let (revision, digest): (i64, String) = db.query_row(
                "SELECT json_extract(payload_json,'$.revision'),json_extract(payload_json,'$.revision_digest')
                 FROM runtime_events WHERE task_id=? AND type='VERIFICATION_PASSED' ORDER BY id DESC LIMIT 1",
                [task],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            for (command, reason) in [
                ("sandboxed", "native_verification_sandbox_unavailable"),
                ("timed-out", "timeout"),
            ] {
                let payload = json!({"receipt":{"argv":[command],"revision":revision,
                    "revision_digest":digest,"tool_call_ref":format!("native-command:{command}"),
                    "observation":{"complete":false,"exit_code":null,"failure":reason}}});
                db.execute(
                    "INSERT INTO runtime_events(task_id,type,actor,payload_json,created_at)
                     VALUES (?,'NATIVE_COMMAND_FINISHED','runtime',?,'2026-11-19T00:00:00Z')",
                    (task, payload.to_string()),
                )?;
            }
            Ok(())
        })
        .await;
        let task = presented(&runtime, &draft).await;
        let checks = task["result"]["checks"].as_array().unwrap();
        assert!(
            checks.contains(&json!({
                "name":"sandboxed","outcome":"not_run","source":"runtime","kind":"automated",
                "rerunnable":false,"command":["sandboxed"],"toolCallRef":"native-command:sandboxed",
                "detail":"native_verification_sandbox_unavailable",
                "technicalReason":"native_verification_sandbox_unavailable"
            })),
            "{task}"
        );
        let timeout = checks
            .iter()
            .find(|check| check["name"] == "timed-out")
            .unwrap();
        assert_eq!(timeout["outcome"], "failed");
        assert!(timeout.get("technicalReason").is_none());
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Live run (MCP task): runtime verification of the current version passed and the
    /// reviewer approved, yet Accept stayed off ("Run the unpassed checks first") because
    /// of the worker's own superseded claims (a run before its last edit, a trivial
    /// command, a check its sandbox could not run) and the reviewer's matching gaps. They
    /// are notes now; nothing needs acknowledging and Accept takes the result.
    #[tokio::test]
    async fn superseded_self_reported_checks_are_notes_once_runtime_verification_passed() {
        let (root, runtime, draft) = completed().await;
        report_checks(&runtime,
            json!([
                {"name":"test exits 0","outcome":"unverified","claimed_outcome":"passed","evidence_gap":"revision_mismatch",
                 "blocker":"Runtime kept this passed claim unverified: revision_mismatch.","tool_call_ref":"native-command:early"},
                {"name":"line count","outcome":"unverified","claimed_outcome":"passed","evidence_gap":"trivial_command",
                 "blocker":"Runtime kept this passed claim unverified: trivial_command.","tool_call_ref":"native-command:wc"},
                {"name":"npm test","outcome":"not_run","command":["npm","test"],"blocker":"npm is not installed in the guarded sandbox."}
            ]),
            json!([
                {"name":"/bin/true","outcome":"unverified","command":["/bin/true"],
                 "blocker":"The implementation report's native command receipt was rejected by the runtime for revision_mismatch."}
            ])).await;
        let task = presented(&runtime, &draft).await;
        let actions = &task["reviewActions"];
        assert_eq!(actions["accept"], true, "{task}");
        assert_eq!(actions["acknowledgeOpenChecks"], false, "{task}");
        assert!(actions["unblockAction"].is_null(), "{task}");
        assert_eq!(task["result"]["acceptanceAvailable"], true);
        assert!(
            task["result"]["checks"]
                .as_array()
                .unwrap()
                .contains(&json!({
                    "name":"npm test","outcome":"not_run","source":"worker","kind":"automated",
                    "rerunnable":false,"command":["npm","test"],
                    "detail":"npm is not installed in the guarded sandbox.",
                    "technicalReason":"npm is not installed in the guarded sandbox."
                })),
            "{task}"
        );
        let notes: Vec<&str> = task["result"]["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert!(
            notes.iter().any(|n| n.starts_with(
                "The runtime's own verification of this version passed. 4 self-reported checks"
            )),
            "{notes:?}"
        );
        assert!(notes.contains(&"Worker check \u{201c}test exits 0\u{201d}: it ran before the final edits, so that run does not count."), "{notes:?}");
        assert!(notes.contains(&"Worker check \u{201c}npm test\u{201d}: not run: npm is not installed in the guarded sandbox."), "{notes:?}");
        assert!(
            notes
                .iter()
                .any(|n| n.starts_with("Reviewer check \u{201c}/bin/true\u{201d}")),
            "{notes:?}"
        );
        // The same-model review note is still there.
        assert!(
            notes.contains(&"The reviewer runs on the same model as the work it reviews."),
            "{notes:?}"
        );
        let state = root.join(".cortex-dev/acceptance-state");
        std::fs::create_dir(&state).unwrap();
        let mut product = json!({"tasks":{},"sprints":[]});
        product["tasks"][&draft] = json!({"checkout":{"kind":"main"}});
        let p = json!({"action":"accept","taskId":draft,"digest":task["result"]["digest"],
            "version":task["result"]["sourceVersion"],"acknowledgeOpenChecks":false});
        let accepted = accept(
            &root,
            "project",
            &draft,
            &state,
            &mut product,
            &p,
            &mut Git::new(),
        )
        .await
        .unwrap_or_else(|e| panic!("{}: {}", e.code, e.message));
        assert_eq!(accepted["done"], true);
        assert_eq!(
            product["tasks"][&draft]["acceptance"]["open"],
            json!({"failed":0,"not_run":0,"unverified":0})
        );
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// A limitation the runtime recorded with an approved review (changes it could not list)
    /// is a manual check: never called a self-reported claim, never an open check.
    #[tokio::test]
    async fn a_runtime_recorded_review_limitation_is_a_manual_check_not_a_claim() {
        let (root, runtime, draft) = completed().await;
        report_checks(
            &runtime,
            json!([]),
            json!([
                {"name":"Changes outside the recorded writes","outcome":"unverified","source":"runtime",
                 "blocker":"Changes made by commands outside the recorded writes could not be listed; inspect the folder."}
            ]),
        )
        .await;
        let task = presented(&runtime, &draft).await;
        let actions = &task["reviewActions"];
        assert_eq!(actions["accept"], true, "{task}");
        assert_eq!(actions["acknowledgeOpenChecks"], false, "{task}");
        let notes: Vec<&str> = task["result"]["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert!(
            !notes.iter().any(|n| n.contains("self-reported")
                || n.contains("Changes outside the recorded writes")),
            "{notes:?}"
        );
        assert!(
            task["result"]["checks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| {
                    c["name"] == "Changes outside the recorded writes" && c["kind"] == "manual"
                }),
            "{task}"
        );
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// A failed self-reported check is never softened into a note, and without a passing
    /// runtime verification of this version an unconfirmed claim still blocks Accept.
    #[tokio::test]
    async fn failed_claims_and_unverified_runtime_checks_still_block_acceptance() {
        let (root, runtime, draft) = completed().await;
        report_checks(&runtime,
            json!([{"name":"unit tests","outcome":"failed","command":["/bin/false"],"tool_call_ref":"native-command:f"}]),
            json!([])).await;
        let task = presented(&runtime, &draft).await;
        assert_eq!(task["reviewActions"]["accept"], false, "{task}");
        assert_eq!(
            task["reviewActions"]["acknowledgeOpenChecks"], true,
            "{task}"
        );
        assert!(
            task["result"]["warnings"]
                .as_array()
                .unwrap()
                .iter()
                .all(|n| !n.as_str().unwrap().contains("self-reported")),
            "{task}"
        );
        // The runtime's own verification failed: the unconfirmed claim stays an open check.
        report_checks(&runtime,
            json!([{"name":"npm test","outcome":"not_run","command":["npm","test"],"blocker":"npm is missing."}]),
            json!([])).await;
        with_task(&runtime, |db, task| {
            let digest: String = db.query_row("SELECT json_extract(payload_json,'$.revision_digest') FROM runtime_events WHERE task_id=? AND type='VERIFICATION_PASSED'", [task], |r| r.get(0))?;
            lifecycle::event_tx(db, task, "VERIFICATION_FAILED", json!({"revision":0,"revision_digest":digest,
                "checks":[{"argv":["/bin/true"],"complete":true,"exit_code":1,"revision_digest":digest}]}))?;
            Ok(())
        }).await;
        let failed = presented(&runtime, &draft).await;
        assert_eq!(failed["reviewActions"]["accept"], false, "{failed}");
        assert!(
            failed["result"]["warnings"].as_array().map_or(true, |a| a
                .iter()
                .all(|n| !n.as_str().unwrap().contains("self-reported"))),
            "{failed}"
        );
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Live run (Automatic sprint on the project folder): every earlier task's result went
    /// stale as soon as a later task wrote the folder. A project-folder sprint task is
    /// compared on the files it recorded writing; a change to one of them is still stale.
    #[tokio::test]
    async fn a_project_folder_sprint_result_is_stale_only_when_its_own_files_change() {
        let (root, runtime, draft) = completed().await;
        let written = format!("{:x}", Sha256::digest(b"after"));
        with_task(&runtime, move |db, task| {
            // The worker's last recorded write holds what is in work.txt; the runtime then verified.
            lifecycle::event_tx(db, task, "NATIVE_PROJECT_WRITTEN", json!({"channel_id":"ch-work","path":"work.txt","sha256":written,"add":1,"del":1}))?;
            let payload: String = db.query_row("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='VERIFICATION_PASSED'", [task], |r| r.get(0))?;
            lifecycle::event_tx(db, task, "VERIFICATION_PASSED", serde_json::from_str(&payload)?)?;
            Ok(())
        }).await;
        let sprint = json!({"checkout":{"kind":"main"},"sprintId":"sp_1"});
        assert_eq!(
            presented_with(&runtime, &draft, sprint.clone()).await["reviewActions"]["accept"],
            true
        );
        // A later task of the sprint writes another file of the same folder.
        std::fs::write(root.join("later.txt"), "a later task's file").unwrap();
        let task = presented_with(&runtime, &draft, sprint.clone()).await;
        assert_eq!(task["reviewActions"]["accept"], true, "{task}");
        assert_eq!(task["workspaceDigest"], task["result"]["digest"], "{task}");
        assert!(
            task["reviewActions"]["workspaceChanged"].is_null()
                && task["reviewActions"]["ownWritesOnly"].is_null(),
            "{task}"
        );
        // Accept binds the same result identity.
        let state = root.join(".cortex-dev/acceptance-state");
        std::fs::create_dir(&state).unwrap();
        let mut product = json!({"tasks":{},"sprints":[]});
        product["tasks"][&draft] = sprint.clone();
        let p = json!({"action":"accept","taskId":draft,"digest":task["result"]["digest"],
            "version":task["result"]["sourceVersion"],"acknowledgeOpenChecks":false});
        accept(
            &root,
            "project",
            &draft,
            &state,
            &mut product,
            &p,
            &mut Git::new(),
        )
        .await
        .unwrap_or_else(|e| panic!("{}: {}", e.code, e.message));
        // Outside a sprint the whole folder still counts.
        let single = presented_with(&runtime, &draft, json!({"checkout":{"kind":"main"}})).await;
        assert_eq!(single["reviewActions"]["accept"], false, "{single}");
        assert_eq!(single["reviewActions"]["workspaceChanged"], true);
        // A change to a file the task wrote makes it stale again.
        std::fs::write(root.join("work.txt"), "changed by a later task").unwrap();
        let changed = presented_with(&runtime, &draft, sprint).await;
        assert_eq!(changed["reviewActions"]["accept"], false, "{changed}");
        assert_eq!(changed["reviewActions"]["workspaceChanged"], true);
        assert_ne!(changed["workspaceDigest"], changed["result"]["digest"]);
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn git_fixture(root: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("/usr/bin/git")
            .args(args)
            .current_dir(root)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    #[tokio::test]
    async fn ordinary_tracked_commit_keeps_the_native_accepted_result_current() {
        let (root, runtime, draft) = completed_in_checkout(true).await;
        let state = automatic_state(&root);
        let mut product = json!({"tasks":{},"sprints":[],"messages":[],"drafts":{}});
        product["tasks"][&draft] = json!({"checkout":{"kind":"main"},"targetBranch":"main"});
        let shown = presented_with(&runtime, &draft, product["tasks"][&draft].clone()).await;
        let result = &shown["result"];
        assert_eq!(shown["workspaceDigest"], result["digest"]);
        let mut git = Git::new();
        accept(
            &root,
            "project",
            &draft,
            &state,
            &mut product,
            &json!({"digest":result["digest"],"version":result["sourceVersion"]}),
            &mut git,
        )
        .await
        .unwrap();
        let parent = git_fixture(&root, &["rev-parse", "HEAD"]);
        git.action(
            &root,
            "project",
            &state,
            &json!({"action":"commit","checkoutId":"project:main","message":"Commit accepted edit"}),
        )
        .await
        .unwrap();
        assert_eq!(git_fixture(&root, &["rev-parse", "HEAD^"]), parent);
        assert_eq!(
            git_fixture(
                &root,
                &["diff-tree", "--no-commit-id", "--name-status", "-r", "HEAD"]
            ),
            "M\twork.txt"
        );
        assert_eq!(git_fixture(&root, &["show", "HEAD:work.txt"]), "after");
        assert_eq!(git_fixture(&root, &["status", "--porcelain"]), "");
        let record =
            super::super::native::product(&state, "project").unwrap()["tasks"][&draft].clone();
        assert!(record["acceptance"]["committedDigest"].is_string());
        let after = presented_with(&runtime, &draft, record.clone()).await;
        std::fs::write(root.join("work.txt"), "changed after commit").unwrap();
        let changed = presented_with(&runtime, &draft, record).await;
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
        assert_eq!(after["result"]["version"], result["version"]);
        assert_eq!(after["result"]["sourceVersion"], result["sourceVersion"]);
        assert_eq!(after["acceptance"]["version"], result["version"]);
        assert_eq!(after["status"], "ready_to_merge");
        assert_eq!(after["result"]["checks"], result["checks"]);
        assert_eq!(after["result"]["digest"], result["digest"]);
        assert_ne!(after["reviewActions"]["workspaceChanged"], true, "{after}");
        assert_eq!(after["workspaceDigest"], result["digest"], "{after}");
        assert!(after["reviewActions"]["acceptedCommitCurrent"].is_null());
        assert_eq!(changed["reviewActions"]["workspaceChanged"], true);
        assert_ne!(changed["workspaceDigest"], changed["result"]["digest"]);
    }

    /// Live run: after Accept and the app's own "Commit to main" of exactly that result,
    /// its card still said "Stale result" (the commit moved HEAD, which the folder digest
    /// covers). The committed result is not stale.
    #[tokio::test]
    async fn the_apps_own_commit_of_a_result_does_not_make_it_stale() {
        let (root, runtime, draft) = completed().await;
        let digest = presented(&runtime, &draft).await["result"]["digest"].clone();
        std::fs::write(root.join("moved-head.txt"), "stands in for the commit").unwrap();
        let committed = json!({"checkout":{"kind":"main"},"status":"done","work":"merged",
            "integration":{"method":"commit","digest":digest,"targetBranch":"main"}});
        let task = presented_with(&runtime, &draft, committed.clone()).await;
        assert_eq!(task["workspaceDigest"], digest, "{task}");
        // Another integration identity is not this result's commit.
        let mut other = committed;
        other["integration"]["digest"] = json!("f".repeat(64));
        assert_ne!(
            presented_with(&runtime, &draft, other).await["workspaceDigest"],
            digest
        );
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Live run: after Request changes the Inspector counted the previous version's
    /// worker and reviewer claims as the current checks. Those workers are superseded.
    #[tokio::test]
    async fn workers_of_a_reopened_version_are_marked_superseded() {
        let (root, runtime, _draft) = completed().await;
        let workers = |facts: &Value| facts["workers"].as_array().unwrap().clone();
        let facts = runtime
            .owner_facts(
                "project",
                json!({"tasks":{},"sprints":[],"messages":[]}),
                json!({}),
            )
            .await
            .unwrap();
        assert!(
            workers(&facts)
                .iter()
                .all(|w| w.get("superseded").is_none()),
            "{facts}"
        );
        with_task(&runtime, |db, task| {
            lifecycle::event_tx(
                db,
                task,
                "TASK_REOPENED",
                json!({"previous":"COMPLETED","revision":0}),
            )?;
            db.execute(
                "UPDATE runtime_tasks SET state='PLANNING' WHERE task_id=?",
                [task],
            )?;
            Ok(())
        })
        .await;
        let facts = runtime
            .owner_facts(
                "project",
                json!({"tasks":{},"sprints":[],"messages":[]}),
                json!({}),
            )
            .await
            .unwrap();
        assert_eq!(workers(&facts).len(), 2);
        assert!(
            workers(&facts).iter().all(|w| w["superseded"] == true),
            "{facts}"
        );
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Live run: a worker that only read files showed "No activity recorded yet" on its
    /// page while the task timeline showed its reads. Its page lists the same retained
    /// tool rows: reads, skill reads and searches, but not its Cortex writes and commands
    /// twice, and never another worker's steps.
    #[tokio::test]
    async fn a_workers_page_lists_its_reads_and_other_tool_steps_from_the_timeline_rows() {
        let (root, runtime, _draft) = completed().await;
        with_task(&runtime, |db, _task| {
            let session: String = db.query_row("SELECT acp_session_id FROM workspace_executions LIMIT 1", [], |r| r.get(0))?;
            let rows = [
                json!({"sessionUpdate":"tool_call","toolCallId":"att-a-work:read","status":"in_progress","title":"worker-backend-dev: mcp.cortex.project_read","kind":"execute",
                    "rawInput":{"server":"cortex","tool":"project_read","arguments":{"path":"src/counter.js"}}}),
                json!({"sessionUpdate":"tool_call_update","toolCallId":"att-a-work:read","status":"completed"}),
                json!({"sessionUpdate":"tool_call","toolCallId":"att-a-work:skill","status":"in_progress","title":"worker-backend-dev: mcp.cortex.read_skill","kind":"execute",
                    "rawInput":{"server":"cortex","tool":"read_skill","arguments":{"skill":"documentation-sync"}}}),
                json!({"sessionUpdate":"tool_call","toolCallId":"att-a-work:grep","status":"completed","title":"worker-backend-dev: grep TODO","kind":"search","rawInput":{"pattern":"TODO","path":"src"}}),
                json!({"sessionUpdate":"tool_call","toolCallId":"att-a-work:edit","status":"completed","title":"worker-backend-dev: mcp__cortex__project_edit","kind":"other","rawInput":{"path":"work.txt"}}),
                json!({"sessionUpdate":"tool_call","toolCallId":"att-a-review:read","status":"completed","title":"worker-code-reviewer: mcp__cortex__project_read","kind":"other","rawInput":{"path":"work.txt"}}),
            ];
            for (seq, row) in rows.iter().enumerate() {
                db.execute("INSERT INTO host_updates VALUES (?,?,?,?)", rusqlite::params![&session, 1000 + seq as i64, row.to_string(), cortex_core::now()])?;
            }
            Ok(())
        }).await;
        let facts = runtime
            .owner_facts(
                "project",
                json!({"tasks":{},"sprints":[],"messages":[]}),
                json!({}),
            )
            .await
            .unwrap();
        let workers = facts["workers"].as_array().unwrap();
        let work = workers.iter().find(|w| w["role"] == "backend-dev").unwrap();
        let steps: Vec<&Value> = work["activity"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|a| a["kind"] == "tool")
            .collect();
        let step = |call: &str| {
            steps
                .iter()
                .find(|a| a["toolCallRef"] == format!("att-a-work:{call}"))
                .map(|a| serde_json::from_str::<Value>(a["args"].as_str().unwrap()).unwrap())
        };
        let read = step("read").expect("the read is on the worker's page");
        assert_eq!(
            (read["path"].as_str(), read["status"].as_str()),
            (Some("src/counter.js"), Some("completed"))
        );
        assert_eq!(step("skill").unwrap()["ref"], "documentation-sync");
        assert_eq!(step("grep").unwrap()["query"], "TODO");
        assert!(
            step("edit").is_none(),
            "a Cortex write is the edit row above, not a second step"
        );
        assert!(steps.iter().all(|a| a["text"]
            .as_str()
            .unwrap()
            .starts_with("worker-backend-dev:")));
        // Every row has a time and a distinct sequence, and they are in time order.
        let all = work["activity"].as_array().unwrap();
        assert!(all.iter().all(|a| a["at"].is_i64()), "{all:?}");
        let mut sequences: Vec<i64> = all
            .iter()
            .map(|a| a["sequence"].as_i64().unwrap())
            .collect();
        assert!(all
            .windows(2)
            .all(|w| w[0]["at"].as_i64() <= w[1]["at"].as_i64()));
        sequences.sort();
        sequences.dedup();
        assert_eq!(sequences.len(), all.len());
        let review = workers
            .iter()
            .find(|w| w["role"] == "code-reviewer")
            .unwrap();
        let reviewer: Vec<&Value> = review["activity"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|a| a["kind"] == "tool")
            .collect();
        assert_eq!(reviewer.len(), 1, "{review}");
        assert_eq!(reviewer[0]["toolCallRef"], "att-a-review:read");
        runtime.shutdown().await;
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }
}
