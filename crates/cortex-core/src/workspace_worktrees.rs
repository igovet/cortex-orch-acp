//! Trusted desktop worktree provisioning and checkout authority. No engine effects.
use crate::{error::refuse, ownership::OwnerEpoch, CoreError, CoreResult, ProjectKey, Store};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};
use std::path::Path;

#[path = "workspace_worktree_git.rs"]
mod checkout;
pub use checkout::Worktree;
pub(crate) use checkout::{plan, Witness};

pub(crate) const SCHEMA: &str = r#"
-- Separate additive format-12 authority. No task FK: the exact provision
-- witness and cleanup disposition must survive archived-task purge.
CREATE TABLE IF NOT EXISTS native_worktree_starts (
 project_id TEXT NOT NULL, actor_id TEXT NOT NULL, request_id TEXT NOT NULL,
 draft_id TEXT NOT NULL, draft_version INTEGER NOT NULL, digest TEXT NOT NULL,
 intent_version INTEGER NOT NULL CHECK(intent_version=1),
 state TEXT NOT NULL CHECK(state IN ('pending','bound','failed-known','uncertain')),
 plan_json TEXT, witness_json TEXT, runtime_task_id TEXT UNIQUE, error_code TEXT,
 close_cleanup TEXT, purge_cleanup TEXT,
 PRIMARY KEY(project_id,actor_id,request_id)
);
CREATE UNIQUE INDEX IF NOT EXISTS native_worktree_active_draft ON native_worktree_starts(project_id,draft_id)
 WHERE state IN ('pending','uncertain','bound');
CREATE TRIGGER IF NOT EXISTS native_worktree_bound_immutable
 BEFORE UPDATE OF project_id,actor_id,request_id,draft_id,draft_version,digest,
  intent_version,state,plan_json,witness_json,runtime_task_id,error_code ON native_worktree_starts
 WHEN OLD.state='bound' BEGIN SELECT RAISE(ABORT, 'immutable'); END;
CREATE TRIGGER IF NOT EXISTS native_worktree_bound_keep
 BEFORE DELETE ON native_worktree_starts
 WHEN OLD.state='bound' BEGIN SELECT RAISE(ABORT, 'immutable'); END;
"#;

#[derive(Clone, Debug)]
pub struct ExecutionCheckout {
    pub logical: ProjectKey,
    pub worktree: Option<Worktree>,
}
impl ExecutionCheckout {
    pub fn evidence(&self, epoch: i64) -> Option<Value> {
        self.worktree.as_ref().map(|w| json!({
            "checkout":receipt_binding(w, self.logical.root(), epoch), "cwd":w.path
        }))
    }

    pub fn path(&self) -> &Path {
        self.worktree
            .as_ref()
            .map_or(self.logical.root(), |w| &w.path)
    }
}

pub(crate) enum Reservation {
    New(String),
    Replay(Value),
    Observed(Worktree, Witness),
    Refused(&'static str),
}

fn recorded_error(code: &str) -> &'static str {
    match code {
        "worktree_git_required" => "worktree_git_required",
        "worktree_head_unborn" => "worktree_head_unborn",
        "worktree_git_unavailable" => "worktree_git_unavailable",
        "unsafe_worktree_root" => "unsafe_worktree_root",
        "worktree_collision" => "worktree_collision",
        "worktree_git_failed" => "worktree_git_failed",
        "worktree_git_timeout" => "worktree_git_timeout",
        "worktree_git_metadata_unsafe" => "worktree_git_metadata_unsafe",
        _ => "worktree_uncertain",
    }
}

pub(crate) fn reserve(
    store: &Store,
    project: &str,
    actor: &str,
    request: &str,
    draft: &str,
    digest: &str,
    command: &Value,
) -> CoreResult<Reservation> {
    store.write(|db| {
        let actor_ok: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM workspace_projects WHERE project_id=? AND activated_by=? AND project_root=?)",
            (project, actor, store.key().root().to_str()), |r| r.get(0),
        )?;
        if !actor_ok { return refuse("unauthorized"); }
        let prior: Option<(String, String)> = db.query_row(
            "SELECT digest,receipt_json FROM workspace_requests WHERE project_id=? AND actor_id=? AND request_id=?",
            (project, actor, request), |r| Ok((r.get(0)?,r.get(1)?)),
        ).optional()?;
        if let Some((old, receipt)) = prior {
            if old != digest { return refuse("payload_conflict"); }
            return Ok(Reservation::Replay(serde_json::from_str(&receipt)?));
        }
        if !installed(db)? { return refuse("worktree_storage_unavailable"); }
        type Intent = (String, i64, String, Option<String>, Option<String>, Option<String>);
        let intent: Option<Intent> = db.query_row(
            "SELECT digest,intent_version,state,plan_json,witness_json,error_code FROM native_worktree_starts
             WHERE project_id=? AND actor_id=? AND request_id=?",
            (project, actor, request), |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?)),
        ).optional()?;
        if let Some((old, version, state, plan, witness, error)) = intent {
            if old != digest { return refuse("payload_conflict"); }
            if version != 1 { return refuse("worktree_binding_invalid"); }
            return match (state.as_str(), plan, witness) {
                ("pending", Some(plan), Some(witness)) => Ok(Reservation::Observed(
                    serde_json::from_str(&plan)?,serde_json::from_str(&witness)?,
                )),
                ("pending", None, None) => {
                    // No Git tree can have been created without a durably saved plan.
                    db.execute("UPDATE native_worktree_starts SET state='failed-known',error_code='worktree_git_failed'
                        WHERE project_id=? AND actor_id=? AND request_id=?", (project, actor, request))?;
                    Ok(Reservation::Refused("worktree_git_failed"))
                }
                ("pending", _, _) => {
                    // Death before the witness may have left a folder or a branch.
                    // Matching names alone cannot establish authority to adopt it.
                    db.execute("UPDATE native_worktree_starts SET state='uncertain',error_code='worktree_uncertain'
                        WHERE project_id=? AND actor_id=? AND request_id=?", (project, actor, request))?;
                    Ok(Reservation::Refused("worktree_uncertain"))
                }
                ("failed-known" | "uncertain", _, _) =>
                    Ok(Reservation::Refused(recorded_error(error.as_deref().unwrap_or("")))),
                _ => refuse("worktree_binding_invalid"),
            };
        }
        if has_unresolved(db, project, draft)? {
            return refuse("worktree_uncertain");
        }
        let row: Option<(String, i64, Option<String>, String)> = db.query_row(
            "SELECT t.description,t.version,t.execution_id,o.options_json FROM workspace_tasks t
             JOIN workspace_task_options o USING(task_id) WHERE t.project_id=? AND t.task_id=?",
             (project, draft), |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        ).optional()?;
        let (title, version, execution, options) = row.ok_or(CoreError::Refused("task_not_found"))?;
        if command["expectedVersion"] != version.to_string() { return refuse("stale_version"); }
        if execution.is_some() { return refuse("busy"); }
        if !command["ownerEpoch"].is_null() { return refuse("stale_owner_epoch"); }
        let options: Value = serde_json::from_str(&options)?;
        if options["checkout"]["kind"] != "worktree" { return refuse("worktree_not_selected"); }
        if options.get("deletingSprint").is_some() { return refuse("sprint_deleting"); }
        super::saved_settings(&options)?;
        if let Some(files) = options.get("attachments") {
            crate::input::capture_from_wire(files)?;
        }
        db.execute(
            "INSERT INTO native_worktree_starts(project_id,actor_id,request_id,draft_id,draft_version,digest,intent_version,state)
             VALUES (?,?,?,?,?,?,1,'pending')",
            (project, actor, request, draft, version, digest),
        )?;
        Ok(Reservation::New(title))
    })
}

pub(crate) fn has_unresolved(db: &Connection, project: &str, draft: &str) -> CoreResult<bool> {
    if !installed(db)? {
        return Ok(false);
    }
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM native_worktree_starts WHERE project_id=? AND draft_id=?
         AND state IN ('pending','uncertain','bound'))",
        (project, draft),
        |r| r.get(0),
    )?)
}

pub(crate) fn check_request(
    db: &Connection,
    project: &str,
    actor: &str,
    request: &str,
    digest: &str,
    replay_pending: bool,
) -> CoreResult<()> {
    if !installed(db)? {
        return Ok(());
    }
    let prior: Option<(String, String, String)> = db
        .query_row(
            "SELECT digest,state,coalesce(error_code,'') FROM native_worktree_starts
         WHERE project_id=? AND actor_id=? AND request_id=?",
            (project, actor, request),
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    if let Some((old, state, error)) = prior {
        if old != digest {
            return refuse("payload_conflict");
        }
        return match state.as_str() {
            "pending" if replay_pending => Ok(()),
            "failed-known" => refuse(recorded_error(&error)),
            "bound" => refuse("worktree_binding_invalid"),
            _ => refuse("worktree_uncertain"),
        };
    }
    Ok(())
}

pub(crate) fn mark(
    store: &Store,
    project: &str,
    actor: &str,
    request: &str,
    state: &str,
    error: &str,
) -> CoreResult<()> {
    store.write(|db| {
        one(db.execute(
            "UPDATE native_worktree_starts SET state=?,error_code=? WHERE project_id=? AND actor_id=?
             AND request_id=? AND state='pending'",
            (state, error, project,actor,request),
        )?)
    })
}

fn one(n: usize) -> CoreResult<()> {
    if n != 1 {
        return refuse("worktree_binding_invalid");
    }
    Ok(())
}

pub(crate) fn save_plan(
    store: &Store,
    project: &str,
    actor: &str,
    request: &str,
    plan: &Worktree,
) -> CoreResult<()> {
    store.write(|db| one(db.execute(
        "UPDATE native_worktree_starts SET plan_json=? WHERE project_id=? AND actor_id=? AND request_id=?
         AND state='pending' AND plan_json IS NULL",
        (serde_json::to_string(plan)?,project,actor,request),
    )?))
}

pub(crate) fn save_witness(
    store: &Store,
    project: &str,
    actor: &str,
    request: &str,
    witness: &Witness,
) -> CoreResult<()> {
    store.write(|db| one(db.execute(
        "UPDATE native_worktree_starts SET witness_json=? WHERE project_id=? AND actor_id=? AND request_id=?
         AND state='pending' AND plan_json IS NOT NULL AND witness_json IS NULL",
        (serde_json::to_string(witness)?,project,actor,request),
    )?))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn bind_tx(
    db: &Connection,
    project: &str,
    actor: &str,
    request: &str,
    draft: &str,
    version: i64,
    task: &str,
    plan: &Worktree,
    witness: &Witness,
) -> CoreResult<()> {
    one(db.execute(
        "UPDATE native_worktree_starts SET state='bound',runtime_task_id=?
         WHERE project_id=? AND actor_id=? AND request_id=? AND draft_id=? AND draft_version=?
         AND state='pending' AND plan_json=? AND witness_json=?",
        (
            task,
            project,
            actor,
            request,
            draft,
            version,
            serde_json::to_string(plan)?,
            serde_json::to_string(witness)?,
        ),
    )?)?;
    db.execute(
        "INSERT INTO task_workspaces VALUES (?,?,?)",
        (task, plan.path.to_str(), crate::now()),
    )?;
    Ok(())
}

pub(crate) fn installed(db: &Connection) -> CoreResult<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='native_worktree_starts')",
        [], |r| r.get(0),
    )?)
}

/// Checkout facts from the authenticated Workspace overlay, under its repository
/// and product locks. The original provision witness remains immutable.
pub(super) fn rework_checkout(
    db: &Connection,
    key: &ProjectKey,
    project: &str,
    binding: &super::control::Binding,
    generation: &str,
    machine: &Path,
    target: Option<&str>,
    integrated: bool,
    replacement: &mut Option<(Worktree, Witness)>,
) -> CoreResult<Option<String>> {
    let root = key.root();
    let old = rows(db, &binding.task)?;
    let mut retained = None;
    if !integrated {
        match resolve_on(db, key, &binding.task, &binding.owner) {
            Ok(_) => return Ok(None),
            Err(e) if old.is_some() && e.code() == "worktree_closed" => {
                let (tree, witness) = old.as_ref().unwrap();
                let cleanup: String = db.query_row(
                    "SELECT close_cleanup FROM native_worktree_starts WHERE runtime_task_id=?",
                    [&binding.task], |r| r.get(0),
                )?;
                let cleanup: Value = serde_json::from_str(&cleanup)?;
                if cleanup["status"] == "retained" && tree.verify(root, witness).is_ok() {
                    // A non-forcing cleanup may have left the original checkout
                    // (including uncommitted work) intact. Keep it in place.
                    clear_close_cleanup(db, &binding.task)?;
                    return Ok(None);
                }
                if cleanup["status"] != "removed" || cleanup["path"] != json!(tree.path) {
                    return refuse("worktree_unavailable");
                }
                retained = Some((
                    tree, witness,
                    cleanup["retainedHead"].as_str().map(str::to_owned)
                        .ok_or(CoreError::Refused("worktree_unavailable"))?,
                ));
            }
            Err(e) => return Err(e),
        }
    } else if old.is_none() {
        resolve_on(db, key, &binding.task, &binding.owner)?;
    }
    if integrated && target.is_none() {
        return refuse("rework_target_unavailable");
    }
    let base = match &retained {
        Some((tree, _, _)) => tree.base_sha.clone(),
        None => checkout::rework_base(root, target, old.is_none())?,
    };
    if old.is_some() {
        let mut worktree = checkout::plan_at(
            root, machine, project, generation, &binding.goal, Some(&base),
        )?;
        if let Some((old, witness, head)) = retained {
            worktree = worktree.restore_branch(root, old, witness, &head)?;
        }
        let witness = worktree.provision(root)?;
        *replacement = Some((worktree, witness));
        let (worktree, witness) = replacement.as_ref().unwrap();
        worktree.verify_initial(root, witness)?;
        one(db.execute(
            "UPDATE task_workspaces SET workspace_root=? WHERE task_id=?",
            (worktree.path.to_str(), &binding.task),
        )?)?;
        clear_close_cleanup(db, &binding.task)?;
    }
    // Git may also be used outside Workspace. A moving target is not our base.
    if integrated && checkout::rework_base(root, target, old.is_none())? != base {
        return refuse("rework_target_changed");
    }
    one(db.execute(
        "UPDATE workspace_task_options SET options_json=json_set(options_json,'$.baseRevision',?) WHERE task_id=?",
        (&base, &binding.draft),
    )?)?;
    Ok(Some(base))
}

fn clear_close_cleanup(db: &Connection, task: &str) -> CoreResult<()> {
    one(db.execute(
        "UPDATE native_worktree_starts SET close_cleanup=NULL WHERE runtime_task_id=?",
        [task],
    )?)
}

pub(super) fn abandon_unbound_rework(
    store: &Store,
    task: &str,
    replacement: Option<(Worktree, Witness)>,
) {
    if let Some((worktree, witness)) = replacement {
        // Only compensate when a fresh read proves this exact provision never
        // bound. Never remove a checkout after an ambiguous database outcome.
        let unbound = store.read(|db| Ok(rows(db, task)?.is_none_or(|(w, _)| w.id != worktree.id)));
        if matches!(unbound, Ok(true)) {
            let _ = worktree.abandon_initial(store.key().root(), &witness);
        }
    }
}

fn rows(db: &Connection, task: &str) -> CoreResult<Option<(Worktree, Witness)>> {
    if !installed(db)? {
        return Ok(None);
    }
    let data: Option<(String, String, Option<String>)> = db
        .query_row(
            "SELECT p.plan_json,p.witness_json,w.workspace_root FROM native_worktree_starts p
         LEFT JOIN task_workspaces w ON w.task_id=p.runtime_task_id
         WHERE p.runtime_task_id=? AND p.state='bound' AND p.intent_version=1",
            [task],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let rework: Option<(String, String)> = db.query_row(
        "SELECT json_extract(payload_json,'$.worktree'),json_extract(payload_json,'$.worktreeWitness')
         FROM runtime_events WHERE task_id=? AND type='TASK_REOPENED'
         AND json_extract(payload_json,'$.origin')='owner_rework'
         AND json_type(payload_json,'$.worktree')='object' ORDER BY id DESC LIMIT 1",
        [task], |r| Ok((r.get(0)?,r.get(1)?)),
    ).optional()?;
    data.map(|(plan, witness, path)| {
        let (plan, witness) = rework.unwrap_or((plan, witness));
        let plan: Worktree = serde_json::from_str(&plan)?;
        if plan.path.to_str() != path.as_deref() {
            return refuse("worktree_binding_invalid");
        }
        Ok((plan, serde_json::from_str(&witness)?))
    })
    .transpose()
}

/// One committed disposition; destroying a checkout is never implied by a close.
#[derive(Clone, Copy)]
pub enum Cleanup {
    Close,
    Purge,
}
impl Cleanup {
    fn column(self) -> &'static str {
        match self {
            Self::Close => "close_cleanup",
            Self::Purge => "purge_cleanup",
        }
    }
}

fn runtime(db: &Connection, project: &str, draft: &str) -> CoreResult<Option<String>> {
    Ok(db.query_row(
        "SELECT e.runtime_task_id FROM workspace_tasks t JOIN workspace_executions e
         ON e.task_id=t.task_id AND e.execution_id=t.execution_id WHERE t.project_id=? AND t.task_id=?",
        (project, draft), |r| r.get(0),
    ).optional()?)
}

/// Read owned metadata, not a Git liveness claim. Useful to display a missing
/// binding and to apply purge's discard decision to the correct checkout.
pub fn owned(store: &Store, project: &str, draft: &str) -> CoreResult<Option<Worktree>> {
    store.read(|db| owned_on(db, project, draft))
}

pub fn owned_on(db: &Connection, project: &str, draft: &str) -> CoreResult<Option<Worktree>> {
    let Some(task) = runtime(db, project, draft)? else { return Ok(None); };
    Ok(rows(db, &task)?.map(|(plan, _)| plan))
}

/// Resolve a desktop task or exact execution through the current owner, never
/// through the projected options. An absent execution has no binding.
pub fn desktop_checkout_on(
    db: &Connection,
    key: &ProjectKey,
    project: &str,
    draft: Option<&str>,
    execution: Option<&str>,
) -> CoreResult<Option<ExecutionCheckout>> {
    if draft.is_none() && execution.is_none() { return refuse("worktree_binding_invalid"); }
    let row: Option<(String, String, i64)> = db.query_row(
        "SELECT e.runtime_task_id,o.owner_ref,e.owner_epoch FROM workspace_tasks t
         JOIN workspace_executions e ON e.task_id=t.task_id AND e.execution_id=t.execution_id
         JOIN task_owners o ON o.task_id=e.runtime_task_id
         WHERE t.project_id=? AND (? IS NULL OR t.task_id=?) AND (? IS NULL OR e.execution_id=?)",
        (project,draft,draft,execution,execution), |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
    ).optional()?;
    row.map(|(task,owner,epoch)| resolve_on(db,key,&task,&OwnerEpoch { owner,epoch })).transpose()
}

pub fn desktop_draft_for_checkout(
    db: &Connection, project: &str, checkout: &str,
) -> CoreResult<Option<String>> {
    if !installed(db)? { return Ok(None); }
    Ok(db.query_row(
        "SELECT draft_id FROM native_worktree_starts p WHERE project_id=? AND state='bound'
         AND coalesce((SELECT json_extract(e.payload_json,'$.worktree.id')
             FROM runtime_events e WHERE e.task_id=p.runtime_task_id AND e.type='TASK_REOPENED'
             AND json_extract(e.payload_json,'$.origin')='owner_rework'
             AND json_type(e.payload_json,'$.worktree')='object' ORDER BY id DESC LIMIT 1),
             json_extract(p.plan_json,'$.id'))=?",
        (project,checkout), |r| r.get(0),
    ).optional()?)
}

pub fn receipt_binding(worktree: &Worktree, logical: &Path, epoch: i64) -> Value {
    json!({"id":worktree.id,"root":worktree.path,"logical":logical,"owner_epoch":epoch})
}

/// Compare retained evidence to immutable authority. The caller separately binds
/// the task via the request/event row and observes the live checkout.
pub fn receipt_matches(
    db: &Connection, task: &str, receipt: &Value, exact_root: bool,
) -> CoreResult<bool> {
    let Some((worktree, _)) = rows(db, task)? else { return Ok(receipt.get("checkout").is_none()); };
    let (logical, epoch): (String, i64) = db.query_row(
        "SELECT t.project_root,o.epoch FROM tasks t JOIN task_owners o ON o.task_id=t.id WHERE t.id=?",
        [task], |r| Ok((r.get(0)?,r.get(1)?)),
    )?;
    let cwd = receipt["cwd"].as_str().map(Path::new);
    Ok(receipt["checkout"] == receipt_binding(&worktree, Path::new(&logical), epoch)
        && cwd.is_some_and(|cwd| if exact_root { cwd == worktree.path } else {
            cwd.strip_prefix(&worktree.path).is_ok_and(|tail| tail.components().all(|c|
                matches!(c, std::path::Component::Normal(_))))
        }))
}

pub(crate) fn queue_purge_tx(
    db: &Connection,
    project: &str,
    draft: &str,
    task: &str,
) -> CoreResult<()> {
    if !installed(db)? {
        return Ok(());
    }
    // Check the immutable authority matches the scheduler binding before the
    // adapter and runtime rows are deleted. Bad authority never authorizes Git.
    let Some((plan, witness)) = rows(db, task)? else {
        return Ok(());
    };
    let cleanup = match crate::lifecycle::automatic::require_settled_execution(db, task) {
        Ok(()) => json!({"status":"pending","plan":plan,"witness":witness}),
        Err(CoreError::Refused(code)) => {
            json!({"status":"retained","path":plan.path,"reason":code})
        }
        Err(error) => return Err(error),
    };
    one(db.execute(
        "UPDATE native_worktree_starts SET purge_cleanup=?
         WHERE project_id=? AND draft_id=? AND runtime_task_id=? AND purge_cleanup IS NULL",
        (cleanup.to_string(), project, draft, task),
    )?)
}

/// Read-only preview; the actual cleanup always rechecks under the repository lock.
pub fn cleanup_preflight(store: &Store, project: &str, draft: &str) -> CoreResult<()> {
    store.read(|db| {
        let task = runtime(db, project, draft)?.ok_or(CoreError::Refused("worktree_not_owned"))?;
        let (plan, witness) = rows(db, &task)?.ok_or(CoreError::Refused("worktree_not_owned"))?;
        crate::store::require_quiet_workspace_task(db, &task)?;
        crate::lifecycle::automatic::require_settled_execution(db, &task)?;
        plan.check_remove(store.key().root(), &witness)
    })
}

/// Called within the same owner action that persists close, before any Git.
pub fn queue_close_tx(db: &Connection, project: &str, draft: &str) -> CoreResult<()> {
    queue_checkout_cleanup_tx(db, project, draft, false)
}

pub fn queue_merge_cleanup_tx(db: &Connection, project: &str, draft: &str) -> CoreResult<()> {
    queue_checkout_cleanup_tx(db, project, draft, true)
}

fn queue_checkout_cleanup_tx(db: &Connection, project: &str, draft: &str, after_merge: bool) -> CoreResult<()> {
    let task = runtime(db, project, draft)?.ok_or(CoreError::Refused("worktree_not_owned"))?;
    if rows(db, &task)?.is_none() {
        return refuse("worktree_not_owned");
    }
    crate::store::require_quiet_workspace_task(db, &task)?;
    crate::lifecycle::automatic::require_settled_execution(db, &task)?;
    let mut pending = json!({"status":"pending"});
    if after_merge { pending["afterMerge"] = json!(true); }
    // Only a newly confirmed explicit close may retry a known obstruction.
    // Other failures may have occurred during Git removal; do not requeue them.
    one(db.execute(
        "UPDATE native_worktree_starts SET close_cleanup=CASE
           WHEN close_cleanup IS NULL OR (? AND json_extract(close_cleanup,'$.status')='retained'
              AND json_extract(close_cleanup,'$.reason') IN ('worktree_dirty','worktree_occupied'))
             THEN ?
           ELSE close_cleanup END
         WHERE project_id=? AND draft_id=? AND runtime_task_id=?",
        (!after_merge, pending.to_string(), project, draft, &task),
    )?)
}

pub fn cleanup_outcome(
    store: &Store,
    project: &str,
    draft: &str,
    kind: Cleanup,
) -> CoreResult<Option<Value>> {
    store.read(|db| {
        if !installed(db)? {
            return Ok(None);
        }
        let value: Option<String> = db
            .query_row(
                &format!(
                    "SELECT {} FROM native_worktree_starts WHERE project_id=? AND draft_id=?
             AND state='bound' AND {} IS NOT NULL",
                    kind.column(),
                    kind.column()
                ),
                (project, draft),
                |r| r.get(0),
            )
            .optional()?;
        let mut parsed: Option<Value> = value.map(|json| serde_json::from_str(&json)).transpose()?;
        if let Some(fields) = parsed.as_mut().and_then(Value::as_object_mut) {
            fields.remove("plan");
            fields.remove("witness");
            fields.remove("retainedHead");
        }
        Ok(parsed)
    })
}

/// Run only under the same repository lock as provision/integration. The caller
/// has already committed the owner disposition and must not undo it on failure.
pub fn finish_cleanup(
    store: &Store,
    project: &str,
    draft: &str,
    kind: Cleanup,
) -> CoreResult<Option<Value>> {
    let entry: Option<(String, String, String, String)> = store.read(|db| {
        if !installed(db)? {
            return Ok(None);
        }
        Ok(db
            .query_row(
                &format!(
                    "SELECT plan_json,witness_json,runtime_task_id,{} FROM native_worktree_starts
             WHERE project_id=? AND draft_id=? AND state='bound' AND {} IS NOT NULL",
                    kind.column(),
                    kind.column()
                ),
                (project, draft),
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?)
    })?;
    let Some((plan, witness, task, saved)) = entry else {
        return Ok(None);
    };
    let saved: Value = serde_json::from_str(&saved)?;
    if saved["status"] != "pending" {
        return cleanup_outcome(store, project, draft, kind);
    }
    let (plan, witness): (Worktree, Witness) = if saved["plan"].is_object() {
        (serde_json::from_value(saved["plan"].clone())?, serde_json::from_value(saved["witness"].clone())?)
    } else if let Some(binding) = store.read(|db| rows(db, &task))? {
        binding
    } else {
        // Previously queued purge intents already removed the runtime row.
        (serde_json::from_str(&plan)?, serde_json::from_str(&witness)?)
    };
    let settled = store.read(|db| match kind {
        Cleanup::Close => Ok(runtime(db, project, draft)?.as_deref() == Some(&task)
            && crate::store::require_quiet_workspace_task(db, &task).is_ok()
            && crate::lifecycle::automatic::require_settled_execution(db, &task).is_ok()),
        Cleanup::Purge => Ok(runtime(db, project, draft)?.is_none()),
    })?;
    let removed = if settled {
        plan.remove_with_head(
            store.key().root(), &witness, matches!(kind, Cleanup::Close),
        )
    } else {
        refuse("worktree_occupied")
    };
    let (mut outcome, retained_head) = match removed {
        Err(e) => (json!({"status":"retained","path":plan.path,"reason":e.code()}), None),
        Ok(head) => (json!({"status":"removed","path":plan.path}), head),
    };
    if saved["afterMerge"] == true { outcome["afterMerge"] = json!(true); }
    let mut stored = outcome.clone();
    if let Some(head) = retained_head { stored["retainedHead"] = json!(head); }
    store.write(|db| one(db.execute(&format!(
        "UPDATE native_worktree_starts SET {}=? WHERE project_id=? AND draft_id=? AND runtime_task_id=?
         AND json_extract({},'$.status')='pending'", kind.column(),kind.column()),
        (stored.to_string(),project,draft,&task),
    )?))?;
    Ok(Some(outcome))
}

/// Authenticate a retained runtime owner epoch, then resolve only Core authority.
/// A legacy path-only task binding is not evidence of native provision ownership.
pub fn resolve(store: &Store, task: &str, owner: &OwnerEpoch) -> CoreResult<ExecutionCheckout> {
    store.read(|db| resolve_on(db, store.key(), task, owner))
}

/// Resolve on the caller's already authenticated transaction/connection so that
/// a filesystem effect can recheck the same binding without changing stores.
pub fn resolve_on(
    db: &Connection,
    key: &ProjectKey,
    task: &str,
    owner: &OwnerEpoch,
) -> CoreResult<ExecutionCheckout> {
    let root = key.root();
    let data = {
        let valid: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM tasks t JOIN task_owners o ON o.task_id=t.id
             WHERE t.id=? AND t.project_root=? AND o.owner_ref=? AND o.epoch=?
             AND NOT EXISTS(SELECT 1 FROM pending_deletions d WHERE d.task_id=t.id))",
            (task, root.to_str(), &owner.owner, owner.epoch), |r| r.get(0),
        )?;
        if !valid { return refuse("stale_owner"); }
        let data = rows(db, task)?;
        if data.is_some() && db.query_row(
            "SELECT EXISTS(SELECT 1 FROM native_worktree_starts WHERE runtime_task_id=? AND close_cleanup IS NOT NULL
             AND NOT (json_extract(close_cleanup,'$.status')='retained' AND json_extract(close_cleanup,'$.afterMerge') IS 1))",
            [task], |r| r.get::<_,bool>(0),
        )? { return refuse("worktree_closed"); }
        // A task with no per-task worktree start at all keeps its earlier recorded
        // workspace (a legacy checkout); only a task that has a worktree start but no
        // bound, matching row must fail closed instead of falling back to the main folder.
        if data.is_none() && installed(db)? && db.query_row(
            "SELECT EXISTS(SELECT 1 FROM task_workspaces WHERE task_id=? AND workspace_root<>?)
             AND EXISTS(SELECT 1 FROM native_worktree_starts WHERE runtime_task_id=?)",
            (task, root.to_str(), task), |r| r.get::<_,bool>(0),
        )? { return refuse("worktree_binding_invalid"); }
        data
    };
    if let Some((worktree, witness)) = data {
        worktree.verify(root, &witness)?;
        return Ok(ExecutionCheckout {
            logical: key.clone(),
            worktree: Some(worktree),
        });
    }
    Ok(ExecutionCheckout {
        logical: key.clone(),
        worktree: None,
    })
}
