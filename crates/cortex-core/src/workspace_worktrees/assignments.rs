//! Checkout custody for execution assignments. Pending effects survive restart.
use super::{checkout, ExecutionCheckout, Witness, Worktree};
use crate::{
    error::refuse, lifecycle::event_tx, ownership, scheduler::policy, CoreError, CoreResult, Store,
};
use rusqlite::{Connection, OptionalExtension, Transaction};
use serde_json::json;
use std::path::Path;

pub(crate) const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS assignment_worktrees (
 task_id TEXT NOT NULL,
 revision INTEGER NOT NULL,
 node_id TEXT NOT NULL,
 assignment_ref TEXT NOT NULL,
 owner_epoch INTEGER NOT NULL,
 state TEXT NOT NULL CHECK(state IN ('provisioning','bound','integrating','integrated')),
 plan_json TEXT NOT NULL,
 witness_json TEXT,
 integrated_digest TEXT,
 integrated_event INTEGER,
 PRIMARY KEY(task_id,revision,assignment_ref),
 CHECK((state='provisioning')=(witness_json IS NULL)),
 CHECK((state='integrated')=(integrated_digest IS NOT NULL AND integrated_event IS NOT NULL))
);
"#;

/// The journal is additive to format 12. Inspections of old stores can see no
/// assignment worktrees without installing anything.
fn installed(db: &Connection) -> CoreResult<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='assignment_worktrees')",
        [], |r| r.get(0),
    )?)
}

pub fn unsettled(db: &Connection, task: &str) -> CoreResult<bool> {
    if !installed(db)? {
        return Ok(false);
    }
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM assignment_worktrees WHERE task_id=? AND state!='integrated')",
        [task],
        |r| r.get(0),
    )?)
}

pub(crate) fn integration_holder(db: &Connection, root: &str) -> CoreResult<Option<String>> {
    if !installed(db)? {
        return Ok(None);
    }
    Ok(db
        .query_row(
            "SELECT w.task_id FROM assignment_worktrees w JOIN tasks t ON t.id=w.task_id
         WHERE w.state='integrating' AND t.project_root=? ORDER BY w.rowid LIMIT 1",
            [root],
            |r| r.get(0),
        )
        .optional()?)
}

pub(crate) fn require_integrated(db: &Connection, task: &str) -> CoreResult<()> {
    if unsettled(db, task)? {
        return refuse("assignment_worktree_unintegrated");
    }
    Ok(())
}

fn active_execution(
    db: &Connection,
    task: &str,
    owner: &ownership::OwnerEpoch,
    assignment: &str,
) -> CoreResult<i64> {
    let (revision, node, role): (i64, String, String) = db
        .query_row(
            "SELECT t.revision,n.node_id,n.role FROM runtime_tasks t JOIN runtime_nodes n
         ON n.task_id=t.task_id AND n.revision=t.revision
         JOIN task_owners o ON o.task_id=t.task_id
         WHERE t.task_id=? AND t.state='EXECUTING' AND n.assignment_ref=?
           AND o.owner_ref=? AND o.epoch=?
           AND n.stage='execution' AND n.status='running' AND n.result_json IS NULL
           AND NOT EXISTS(SELECT 1 FROM task_stopped WHERE task_id=t.task_id)
           AND NOT EXISTS(SELECT 1 FROM pending_deletions WHERE task_id=t.task_id)",
            (task, assignment, &owner.owner, owner.epoch),
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?
        .ok_or(CoreError::Refused("assignment_worktree_not_executing"))?;
    if policy::node_read_only(db, task, revision, &node, &role, "execution")? {
        return refuse("assignment_worktree_not_writing");
    }
    Ok(revision)
}

fn require_unlaunched(db: &Connection, task: &str, assignment: &str) -> CoreResult<()> {
    let launched: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM delegation_provisioning WHERE task_id=? AND idempotency_key=?)",
        (task, assignment), |r| r.get(0),
    )?;
    if launched {
        return refuse("assignment_worktree_already_launched");
    }
    Ok(())
}

/// Record an exact intent before provisioning, then bind the observed filesystem
/// witness. A refusal after the intent keeps it unresolved; retry does not run
/// Git again or let the same assignment fall back to the shared checkout.
pub fn provision(
    store: &Store,
    task: &str,
    owner: &ownership::OwnerEpoch,
    assignment: &str,
    directory: &Path,
) -> CoreResult<ExecutionCheckout> {
    let logical = store.key().root();
    let (revision, prior) = store.read(|db| {
        let revision = active_execution(db, task, owner, assignment)?;
        Ok((
            revision,
            resolve_on(store.key(), db, task, owner, assignment)?,
        ))
    })?;
    if let Some(result) = prior {
        return Ok(result);
    }
    let id = format!("{task}:{revision}:{assignment}");
    let digest = crate::digest(logical.as_os_str().as_encoded_bytes());
    let plan = checkout::plan(logical, directory, &digest, &id, "assignment")?;
    let encoded_plan = serde_json::to_string(&plan)?;
    store.write(|db| {
        if active_execution(db, task, owner, assignment)? != revision {
            return refuse("assignment_worktree_revision_changed");
        }
        require_unlaunched(db, task, assignment)?;
        let inserted = db.execute(
            "INSERT INTO assignment_worktrees (task_id,revision,node_id,assignment_ref,owner_epoch,state,plan_json)
             SELECT task_id,revision,node_id,assignment_ref,?4,'provisioning',?5
             FROM runtime_nodes WHERE task_id=?1 AND revision=?2 AND assignment_ref=?3
             ON CONFLICT DO NOTHING",
            (task, revision, assignment, owner.epoch, &encoded_plan),
        )?;
        if inserted != 1 {
            return refuse("assignment_worktree_outcome_unknown");
        }
        event_tx(db, task, "ASSIGNMENT_WORKTREE_PROVISIONING", json!({
            "revision":revision, "assignment_ref":assignment, "checkout_id":plan.id
        }))?;
        Ok(())
    })?;
    let witness = plan.provision(logical)?;
    plan.verify_initial(logical, &witness)?;
    let encoded_witness = serde_json::to_string(&witness)?;
    store.write(|db| {
        if active_execution(db, task, owner, assignment)? != revision {
            return refuse("assignment_worktree_revision_changed");
        }
        require_unlaunched(db, task, assignment)?;
        let updated = db.execute(
            "UPDATE assignment_worktrees SET state='bound',witness_json=?
             WHERE task_id=? AND revision=? AND assignment_ref=? AND owner_epoch=?
               AND state='provisioning' AND plan_json=?",
            (
                &encoded_witness,
                task,
                revision,
                assignment,
                owner.epoch,
                &encoded_plan,
            ),
        )?;
        if updated != 1 {
            return refuse("assignment_worktree_outcome_unknown");
        }
        event_tx(
            db,
            task,
            "ASSIGNMENT_WORKTREE_BOUND",
            json!({
                "revision":revision, "assignment_ref":assignment, "checkout_id":plan.id
            }),
        )?;
        Ok(())
    })?;
    Ok(ExecutionCheckout {
        logical: store.key().clone(),
        worktree: Some(plan),
    })
}

/// Resolve only a currently owned execution assignment and authenticate its
/// retained linked checkout. An existing row with any other state fails closed.
pub fn resolve_on(
    key: &crate::ProjectKey,
    db: &Connection,
    task: &str,
    owner: &ownership::OwnerEpoch,
    assignment: &str,
) -> CoreResult<Option<ExecutionCheckout>> {
    if !installed(db)? {
        return Ok(None);
    }
    let row: Option<(i64, i64, String, String, Option<String>)> = db
        .query_row(
            "SELECT revision,owner_epoch,state,plan_json,witness_json FROM assignment_worktrees
         WHERE task_id=? AND assignment_ref=? ORDER BY revision DESC LIMIT 1",
            (task, assignment),
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    let Some((revision, epoch, state, plan, witness)) = row else {
        return Ok(None);
    };
    if state != "bound" {
        return refuse("assignment_worktree_outcome_unknown");
    }
    if epoch != owner.epoch || active_execution(db, task, owner, assignment)? != revision {
        return refuse("assignment_worktree_stale");
    }
    let witness: Witness = serde_json::from_str(
        &witness.ok_or(CoreError::Refused("assignment_worktree_outcome_unknown"))?,
    )?;
    let worktree: Worktree = serde_json::from_str(&plan)?;
    worktree.verify(key.root(), &witness)?;
    Ok(Some(ExecutionCheckout {
        logical: key.clone(),
        worktree: Some(worktree),
    }))
}

/// Existing task-checkout launchers must refuse a custody record they did not
/// resolve. Bound assignments cannot dispatch accidentally on the shared root.
pub fn require_launch(
    db: &Transaction<'_>,
    task: &str,
    owner: &ownership::OwnerEpoch,
    assignment: &str,
    checkout: Option<&str>,
) -> CoreResult<()> {
    if !installed(db)? {
        return Ok(());
    }
    let row: Option<(String, Option<String>, i64)> = db
        .query_row(
            "SELECT state,json_extract(plan_json,'$.id'),owner_epoch FROM assignment_worktrees
         WHERE task_id=? AND assignment_ref=? ORDER BY revision DESC LIMIT 1",
            (task, assignment),
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    if let Some((state, id, epoch)) = row {
        if state == "bound"
            && id.as_deref() == checkout
            && epoch == owner.epoch
            && checkout.is_some()
        {
            active_execution(db, task, owner, assignment)?;
            return Ok(());
        }
        return refuse("assignment_worktree_launch_unavailable");
    }
    if selected(db, task, assignment)? {
        return refuse("assignment_worktree_launch_unavailable");
    }
    Ok(())
}

/// Persisted root for gateway authority checks under the same transaction.
pub fn root_on(
    db: &Connection,
    task: &str,
    assignment: &str,
) -> CoreResult<Option<std::path::PathBuf>> {
    if !installed(db)? {
        return Ok(None);
    }
    let found: Option<(String, String)> = db
        .query_row(
            "SELECT state,json_extract(plan_json,'$.path') FROM assignment_worktrees
         WHERE task_id=? AND assignment_ref=? ORDER BY revision DESC LIMIT 1",
            (task, assignment),
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    match found {
        Some((state, root)) if state == "bound" => Ok(Some(root.into())),
        _ => Ok(None),
    }
}

pub fn assignment_for_receipt(
    db: &Connection,
    task: &str,
    checkout: &serde_json::Value,
) -> CoreResult<Option<String>> {
    if !installed(db)? || checkout["id"].is_null() {
        return Ok(None);
    }
    Ok(db
        .query_row(
            "SELECT assignment_ref FROM assignment_worktrees
         WHERE task_id=? AND json_extract(plan_json,'$.id')=?",
            (task, checkout["id"].as_str()),
            |r| r.get(0),
        )
        .optional()?)
}

/// Receipt identity for a worker result only. Review and verification continue
/// to resolve the task checkout after integration.
pub(crate) fn worker_receipt_matches(
    db: &Connection,
    task: &str,
    channel: &str,
    receipt: &serde_json::Value,
) -> CoreResult<Option<bool>> {
    if !installed(db)? {
        return Ok(None);
    }
    let row: Option<(String, String, i64)> = db
        .query_row(
            "SELECT w.plan_json,t.project_root,w.owner_epoch FROM assignment_worktrees w
         JOIN trusted_channels c ON c.task_id=w.task_id AND c.assignment_id=w.assignment_ref
          AND c.owner_epoch=w.owner_epoch
         JOIN tasks t ON t.id=w.task_id
         WHERE w.task_id=? AND c.channel_id=? AND w.witness_json IS NOT NULL",
            (task, channel),
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((plan, logical, epoch)) = row else {
        return Ok(None);
    };
    let worktree: Worktree = serde_json::from_str(&plan)?;
    let cwd = receipt["cwd"].as_str().map(Path::new);
    Ok(Some(
        receipt["checkout"] == super::receipt_binding(&worktree, Path::new(&logical), epoch)
            && cwd.is_some_and(|cwd| {
                cwd.strip_prefix(&worktree.path).is_ok_and(|tail| {
                    tail.components()
                        .all(|c| matches!(c, std::path::Component::Normal(_)))
                })
            }),
    ))
}

/// A completed node is not a satisfied dependency while its patch lives in its
/// isolated checkout. A replacement cannot start in another checkout while its
/// predecessor attempt or an earlier revision has unsettled checkout effects.
pub(crate) fn pending_dependencies(
    db: &Connection,
    task: &str,
    revision: i64,
    node: &str,
) -> CoreResult<bool> {
    if !installed(db)? {
        return Ok(false);
    }
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM assignment_worktrees w
         WHERE w.task_id=?1 AND w.state!='integrated' AND
           (w.revision!=?2 OR w.node_id=?3 OR w.node_id IN
              (SELECT dep.value FROM runtime_nodes next, json_each(next.dependencies_json) dep
               WHERE next.task_id=?1 AND next.revision=?2 AND next.node_id=?3))
        )",
        (task, revision, node),
        |r| r.get(0),
    )?)
}

fn completed_scope(
    db: &Connection,
    task: &str,
    owner: &ownership::OwnerEpoch,
    assignment: &str,
    expected_state: &str,
) -> CoreResult<(i64, Worktree, Witness, Vec<String>)> {
    let (revision, scope, plan, witness): (i64, String, String, String) = db.query_row(
        "SELECT n.revision,n.scope_json,w.plan_json,w.witness_json
         FROM runtime_nodes n JOIN runtime_tasks t
           ON t.task_id=n.task_id AND t.revision=n.revision
         JOIN assignment_worktrees w
           ON w.task_id=n.task_id AND w.revision=n.revision AND w.assignment_ref=n.assignment_ref
         JOIN task_owners o ON o.task_id=n.task_id
         WHERE n.task_id=?1 AND n.assignment_ref=?2 AND o.owner_ref=?3 AND o.epoch=?4
           AND w.owner_epoch=o.epoch AND w.state=?5 AND t.state='EXECUTING'
           AND n.stage='execution' AND n.status='completed' AND n.result_json IS NOT NULL
           AND NOT EXISTS(SELECT 1 FROM pending_deletions WHERE task_id=t.task_id)
           AND NOT EXISTS(SELECT 1 FROM task_stopped WHERE task_id=t.task_id)
           AND EXISTS(SELECT 1 FROM delegation_provisioning p JOIN capacity_reservations c
               ON c.task_id=p.task_id AND c.attempt_id=p.attempt_id
              WHERE p.task_id=n.task_id AND p.idempotency_key=n.assignment_ref
                AND p.owner_epoch=o.epoch AND c.assignment_ref=n.assignment_ref
                AND p.status='terminated' AND c.state='released' AND c.release_observation='process_exited')
           AND NOT EXISTS(SELECT 1 FROM delegation_provisioning p
              LEFT JOIN capacity_reservations c ON c.task_id=p.task_id AND c.attempt_id=p.attempt_id
              WHERE p.task_id=n.task_id AND p.idempotency_key=n.assignment_ref
                AND (p.status!='terminated' OR c.state IS NOT 'released'
                  OR c.assignment_ref IS NOT n.assignment_ref
                  OR EXISTS(SELECT 1 FROM attempt_process_registry q
                      WHERE q.task_id=p.task_id AND q.attempt_id=p.attempt_id AND q.terminated_at IS NULL)))",
        (task, assignment, &owner.owner, owner.epoch, expected_state),
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    ).optional()?.ok_or(CoreError::Refused("assignment_worktree_not_settled"))?;
    Ok((
        revision,
        serde_json::from_str(&plan)?,
        serde_json::from_str(&witness)?,
        policy::effective_scope_tx(db, task, Some(assignment), &scope)?,
    ))
}

fn require_not_denied(db: &Connection, task: &str, paths: &[String]) -> CoreResult<()> {
    let denials = crate::lifecycle::steering::denials(db, task)?;
    if paths
        .iter()
        .any(|p| denials.iter().any(|deny| policy::overlaps(p, deny)))
    {
        return refuse("assignment_worktree_outside_scope");
    }
    Ok(())
}

fn is_literal_tracked_file_candidate(root: &Path, relative: &str) -> bool {
    use crate::store::owner_file_leases::RelativePath;
    if relative.contains(['*', '?', '[']) || RelativePath::new(relative).is_err() {
        return false;
    }
    let mut path = root.to_path_buf();
    let mut parts = relative.split('/').peekable();
    while let Some(part) = parts.next() {
        path.push(part);
        let Ok(metadata) = path.symlink_metadata() else {
            return false;
        };
        if parts.peek().is_none() {
            return metadata.is_file();
        }
        if !metadata.is_dir() {
            return false;
        }
    }
    false
}

/// Apply a witnessed assignment's ordinary tracked modifications to the task's
/// canonical checkout. Conflicts discovered by preflight leave both checkouts
/// intact. Ambiguous post-intent effects are never replayed.
pub fn integrate(
    store: &Store,
    task: &str,
    owner: &ownership::OwnerEpoch,
    assignment: &str,
) -> CoreResult<()> {
    use checkout::integration::{clean_target_index, Transfer};
    let target = store.key().root();
    let root = target
        .to_str()
        .ok_or(CoreError::Refused("invalid_project"))?;
    let (revision, source, witness, scope) =
        store.read(|db| completed_scope(db, task, owner, assignment, "bound"))?;
    source.verify(target, &witness)?;
    let patch = Transfer::observe(&source.path, &source.base_sha)?;
    let paths: Vec<String> = patch.paths().map(str::to_owned).collect();
    // This transport accepts literal file/directory scopes. An ambiguous scope
    // must be resolved by the ordinary shared-checkout route before isolation.
    if paths.iter().any(|path| {
        !scope.iter().any(|raw| {
            raw == "."
                || crate::store::owner_file_leases::RelativePath::new(raw).is_ok()
                    && !raw.contains(['*', '?', '['])
                    && Path::new(path).starts_with(raw)
        })
    }) {
        return refuse("assignment_worktree_outside_scope");
    }

    let before = store.write(|db| {
        let (current, _, _, _) = completed_scope(db, task, owner, assignment, "bound")?;
        if current != revision || integration_holder(db, root)?.is_some() {
            return refuse("assignment_worktree_target_busy");
        }
        policy::require_assignment_integration(db, task, root, &paths)?;
        require_not_denied(db, task, &paths)?;
        let expected: Option<String> = db.query_row(
            "SELECT integrated_digest FROM assignment_worktrees WHERE task_id=? AND state='integrated'
             ORDER BY integrated_event DESC LIMIT 1",
            [task], |r| r.get(0),
        ).optional()?;
        let before = Transfer::observe(target, &source.base_sha)?;
        if !before.can_combine(&patch) {
            return refuse("assignment_worktree_unsupported_patch");
        }
        if !clean_target_index(target)?
            || paths.iter().any(|path| before.paths().any(|p| p == path))
            || expected.as_ref().map_or(before.count() != 0, |digest| before.digest() != *digest)
        {
            return refuse("assignment_worktree_conflict");
        }
        patch.check(target)?;
        db.execute("UPDATE assignment_worktrees SET state='integrating'
            WHERE task_id=? AND revision=? AND assignment_ref=?", (task, revision, assignment))?;
        event_tx(db, task, "ASSIGNMENT_WORKTREE_INTEGRATING", json!({"revision":revision,
            "assignment_ref":assignment,"checkout_id":source.id,"base":source.base_sha,
            "patch_digest":patch.digest(),"target_before":before.digest()}))?;
        Ok(before)
    })?;
    // No retry after this point. The immediate transaction stops a concurrent
    // runtime owner operation or lease admission between observation and apply.
    store.write(|db| {
        completed_scope(db, task, owner, assignment, "integrating")?;
        policy::require_assignment_integration(db, task, root, &paths)?;
        require_not_denied(db, task, &paths)?;
        if !clean_target_index(target)?
            || Transfer::observe(target, &source.base_sha)?.digest() != before.digest()
        {
            return refuse("assignment_worktree_outcome_unknown");
        }
        source.verify(target, &witness)?;
        if Transfer::observe(&source.path, &source.base_sha)?.digest() != patch.digest() {
            return refuse("assignment_worktree_outcome_unknown");
        }
        patch.apply_to(target)?;
        let after = Transfer::observe(target, &source.base_sha)?;
        if !clean_target_index(target)? || !after.includes(&patch) || !after.includes(&before)
            || after.count() != patch.count() + before.count()
        {
            return refuse("assignment_worktree_outcome_unknown");
        }
        let after_digest = after.digest();
        let event = event_tx(db, task, "ASSIGNMENT_WORKTREE_INTEGRATED", json!({
            "revision":revision,"assignment_ref":assignment,"checkout_id":source.id,
            "base":source.base_sha,"target_after":after_digest,"paths":paths
        }))?;
        let updated = db.execute("UPDATE assignment_worktrees SET state='integrated',integrated_digest=?,integrated_event=?
             WHERE task_id=? AND revision=? AND assignment_ref=? AND state='integrating'",
            (after_digest, event, task, revision, assignment))?;
        if updated != 1 {
            return refuse("assignment_worktree_outcome_unknown");
        }
        Ok(())
    })
}

/// Settle completed isolated nodes before the host observes the next canonical
/// checkout boundary or releases any dependents.
pub fn integrate_ready(store: &Store, task: &str, owner: &ownership::OwnerEpoch) -> CoreResult<()> {
    let ready = store.read(|db| {
        if !installed(db)? {
            return Ok(Vec::new());
        }
        let mut sql = db.prepare(
            "SELECT w.assignment_ref FROM assignment_worktrees w
             JOIN runtime_nodes n ON n.task_id=w.task_id AND n.revision=w.revision
               AND n.assignment_ref=w.assignment_ref
             JOIN runtime_tasks t ON t.task_id=n.task_id AND t.revision=n.revision
             WHERE w.task_id=? AND w.state='bound' AND n.status='completed'
             ORDER BY w.rowid",
        )?;
        let rows = sql
            .query_map([task], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })?;
    for assignment in ready {
        integrate(store, task, owner, &assignment)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{lifecycle::TaskMachine, OpenMode, ProjectKey, StoreChannel};
    use std::{fs, os::unix::fs::DirBuilderExt, path::PathBuf};

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir()
                .join(format!("cw-assignments-{}", uuid::Uuid::new_v4().simple()));
            fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
            Self(path.canonicalize().unwrap())
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn git(root: &Path, args: &[&str]) {
        let output = std::process::Command::new("/usr/bin/git")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .current_dir(root)
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{:?}: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn setup(fixture: &Fixture) -> (Store, PathBuf, ownership::OwnerEpoch) {
        let root = fixture.0.join("project");
        fs::create_dir(&root).unwrap();
        git(&root, &["init", "-q"]);
        fs::write(root.join("alpha.txt"), "base alpha\n").unwrap();
        fs::write(root.join("beta.txt"), "base beta\n").unwrap();
        fs::write(root.join(".gitignore"), "/.cortex-dev/\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-qm", "Base"]);
        let worktrees = fixture.0.join("worktrees");
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&worktrees)
            .unwrap();
        let store = Store::open(
            ProjectKey::new(root, StoreChannel::Dev).unwrap(),
            OpenMode::Initialize,
        )
        .unwrap();
        let owner = TaskMachine::new(&store)
            .create("fixture", "Exercise checkout custody", "owner")
            .unwrap();
        store.write(|db| {
            db.execute("UPDATE runtime_tasks SET state='EXECUTING', revision=1 WHERE task_id='fixture'", [])?;
            for (node, assignment, file) in [("first","a1","alpha.txt"), ("second","a2","beta.txt")] {
                db.execute("INSERT INTO runtime_nodes VALUES ('fixture',1,?,'worker-backend-dev','Work','[]',?,
                    'execution','running',?,NULL)", (node, json!([file]).to_string(), assignment))?;
            }
            db.execute("INSERT INTO runtime_nodes VALUES ('fixture',1,'dependent','worker-backend-dev','Work',
                '[\"first\"]','[\"next.txt\"]','execution','pending',NULL,NULL)", [])?;
            Ok(())
        }).unwrap();
        (store, worktrees, owner)
    }

    /// The Git and custody operations are real. Worker observations use the
    /// settled records of this offline fixture without launching a model.
    fn settle(store: &Store, assignment: &str) {
        let attempt = format!("attempt-{assignment}");
        store.write(|db| {
            db.execute("UPDATE runtime_nodes SET status='completed',result_json='{}' WHERE assignment_ref=?",
                [assignment])?;
            db.execute("INSERT INTO delegation_provisioning
                (idempotency_key,task_id,attempt_id,role_id,step,status,owner_epoch,created_at,updated_at)
                VALUES (?,'fixture',?,'worker-backend-dev','ready','terminated',1,'now','now')",
                (assignment, &attempt))?;
            db.execute("INSERT INTO capacity_reservations VALUES
                (?,'fixture','fixture',?,?,'worker','released','now','now','process_exited')",
                (&attempt, assignment, &attempt))?;
            Ok(())
        }).unwrap();
    }

    #[test]
    fn disjoint_assignment_worktrees_integrate_in_order_and_unblock_dependents() {
        let fixture = Fixture::new();
        let (store, trees, owner) = setup(&fixture);
        let a = provision(&store, "fixture", &owner, "a1", &trees).unwrap();
        let b = provision(&store, "fixture", &owner, "a2", &trees).unwrap();
        assert_ne!(a.path(), b.path());
        assert_eq!(a.logical.database(), b.logical.database());
        let linked = ProjectKey::new(b.path(), StoreChannel::Dev).unwrap();
        assert_eq!(linked.database(), store.key().database());
        let isolated_evidence = a.evidence(owner.epoch).unwrap();
        assert!(!store
            .read(|db| super::super::receipt_matches(db, "fixture", &isolated_evidence, true))
            .unwrap());
        assert_eq!(
            a.path(),
            provision(&store, "fixture", &owner, "a1", &trees)
                .unwrap()
                .path()
        );
        assert_eq!(
            store
                .write(|db| require_launch(db, "fixture", &owner, "a1", None))
                .unwrap_err()
                .code(),
            "assignment_worktree_launch_unavailable"
        );
        fs::write(a.path().join("alpha.txt"), "changed alpha\n").unwrap();
        fs::write(b.path().join("beta.txt"), "changed beta\n").unwrap();
        settle(&store, "a1");
        settle(&store, "a2");
        assert!(store
            .read(|db| pending_dependencies(db, "fixture", 1, "dependent"))
            .unwrap());
        assert_eq!(
            store
                .read(|db| require_integrated(db, "fixture"))
                .unwrap_err()
                .code(),
            "assignment_worktree_unintegrated"
        );
        assert_eq!(
            store
                .write(|db| crate::lifecycle::completion::add_review_tx(
                    db,
                    "fixture",
                    &Default::default()
                ))
                .unwrap_err()
                .code(),
            "assignment_worktree_unintegrated"
        );
        integrate(&store, "fixture", &owner, "a1").unwrap();
        assert!(!store
            .read(|db| pending_dependencies(db, "fixture", 1, "dependent"))
            .unwrap());
        assert!(store.read(|db| unsettled(db, "fixture")).unwrap());
        integrate(&store, "fixture", &owner, "a2").unwrap();
        assert!(!store.read(|db| unsettled(db, "fixture")).unwrap());
        let root = store.key().root();
        assert_eq!(
            fs::read_to_string(root.join("alpha.txt")).unwrap(),
            "changed alpha\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("beta.txt")).unwrap(),
            "changed beta\n"
        );
        assert!(checkout::integration::clean_target_index(root).unwrap());
        let actual = ["alpha.txt", "beta.txt"]
            .into_iter()
            .map(|path| {
                (
                    path.to_owned(),
                    crate::digest(&fs::read(root.join(path)).unwrap()),
                )
            })
            .collect();
        let packet = store
            .read(|db| {
                crate::lifecycle::reconciliation::review_packet_tx(
                    db,
                    "fixture",
                    "_review_1",
                    None,
                    None,
                    &actual,
                )
            })
            .unwrap();
        assert_eq!(
            packet["review_binding"]["paths"],
            json!(["alpha.txt", "beta.txt"])
        );
        let source = a.worktree.as_ref().unwrap();
        let observed = checkout::integration::Transfer::observe(root, &source.base_sha).unwrap();
        let mut verification = crate::lifecycle::completion::EvidenceSnapshot {
            project_digest: Some(observed.digest()),
            ..Default::default()
        };
        assert_eq!(
            store
                .read(|db| crate::lifecycle::completion::current_binding_digest(
                    db,
                    "fixture",
                    &verification
                ))
                .unwrap(),
            observed.digest()
        );
        verification.checkout_evidence = Some(isolated_evidence);
        assert_eq!(
            store
                .read(|db| crate::lifecycle::completion::current_binding_digest(
                    db,
                    "fixture",
                    &verification
                ))
                .unwrap_err()
                .code(),
            "worktree_binding_invalid"
        );
        assert!(integrate(&store, "fixture", &owner, "a1").is_err());
    }

    #[test]
    fn target_conflict_keeps_checkout_bytes_and_unsettled_custody() {
        let fixture = Fixture::new();
        let (store, trees, owner) = setup(&fixture);
        let source = provision(&store, "fixture", &owner, "a1", &trees).unwrap();
        fs::write(source.path().join("alpha.txt"), "isolated\n").unwrap();
        settle(&store, "a1");
        settle(&store, "a2");
        let root = store.key().root().to_owned();
        fs::write(root.join("alpha.txt"), "external\n").unwrap();
        assert_eq!(
            integrate(&store, "fixture", &owner, "a1")
                .unwrap_err()
                .code(),
            "assignment_worktree_conflict"
        );
        assert_eq!(
            fs::read_to_string(root.join("alpha.txt")).unwrap(),
            "external\n"
        );
        assert_eq!(
            fs::read_to_string(source.path().join("alpha.txt")).unwrap(),
            "isolated\n"
        );
        git(&root, &["update-index", "--assume-unchanged", "alpha.txt"]);
        assert_eq!(
            integrate(&store, "fixture", &owner, "a1")
                .unwrap_err()
                .code(),
            "assignment_worktree_conflict"
        );
        assert_eq!(
            fs::read_to_string(root.join("alpha.txt")).unwrap(),
            "external\n"
        );
        let key = store.key().clone();
        drop(store);
        let reopened = Store::open(key, OpenMode::Initialize).unwrap();
        assert!(reopened.read(|db| unsettled(db, "fixture")).unwrap());
        assert!(reopened
            .read(|db| pending_dependencies(db, "fixture", 1, "dependent"))
            .unwrap());
    }

    #[test]
    fn measured_review_pressure_can_select_a_disjoint_writer() {
        let fixture = Fixture::new();
        let (store, trees, owner) = setup(&fixture);
        let lifecycle = TaskMachine::new(&store);
        lifecycle
            .create(
                "holder",
                "Hold the shared checkout for an independent review",
                "owner",
            )
            .unwrap();
        let timed = lifecycle
            .create("timings", "Observe checkout and worker durations", "owner")
            .unwrap();
        store.write(|db| {
            db.execute("UPDATE runtime_nodes SET status='pending',assignment_ref=NULL WHERE task_id='fixture' AND node_id='first'", [])?;
            db.execute("UPDATE runtime_tasks SET state='REVIEWING',revision=1 WHERE task_id='holder'", [])?;
            db.execute("INSERT INTO runtime_nodes VALUES
                ('holder',1,'write','worker-backend-dev','Fixture','[]','[\"beta.txt\"]','execution','completed',NULL,NULL),
                ('holder',1,'review','worker-code-reviewer','Fixture','[\"write\"]','[]','review','running','review-live',NULL)", [])?;
            event_tx(db,"holder","REVIEW_STARTED",json!({"assignment_ref":"review-live","binding":"checkout"}))?;
            db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='timings'", [])?;
            db.execute("INSERT INTO runtime_nodes VALUES
                ('timings',1,'checkout','worker-backend-dev','Fixture','[]','[\"alpha.txt\"]',
                'execution','running','time-checkout',NULL)", [])?;
            Ok(())
        }).unwrap();
        let start = std::time::Instant::now();
        let measured = provision(&store, "timings", &timed, "time-checkout", &trees).unwrap();
        let checkout_ms = start.elapsed().as_millis().max(1) as u64;
        let verify = std::time::Instant::now();
        git(measured.path(), &["diff", "--exit-code", "HEAD"]);
        let verify_ms = verify.elapsed().as_millis().max(1) as u64;
        let checkout = measured.worktree.unwrap().id;
        store
            .write(|db| {
                db.execute(
                    "UPDATE runtime_nodes SET status='completed' WHERE task_id='timings'",
                    [],
                )?;
                db.execute(
                    "UPDATE runtime_tasks SET state='COMPLETED' WHERE task_id='timings'",
                    [],
                )?;
                event_tx(
                    db,
                    "timings",
                    "NATIVE_WORKTREE_PROVISION_TIME",
                    json!({"checkout_id":checkout,"elapsed_ms":checkout_ms}),
                )?;
                event_tx(
                    db,
                    "timings",
                    "NATIVE_WORKTREE_VERIFICATION_TIME",
                    json!({"checkout_id":checkout,"run":"git-check","elapsed_ms":verify_ms}),
                )?;
                Ok(())
            })
            .unwrap();
        let trial_ms = (checkout_ms * 2 + verify_ms + 200).max(300);
        for (index, role) in ["worker-backend-dev", "worker-code-reviewer"]
            .into_iter()
            .enumerate()
        {
            let assignment = format!("timed-{index}");
            let attempt = format!("attempt-{index}");
            let routing = json!({});
            let (kind, complexity) = crate::scheduler::node_classification(role, &routing);
            let route = store
                .write(|db| {
                    event_tx(
                        db,
                        "timings",
                        "NATIVE_ROUTE_SELECTED",
                        json!({"assignment_ref":assignment,
                "timing_work_kind":kind,"timing_complexity":complexity}),
                    )
                })
                .unwrap();
            let delivered = crate::now();
            store.write(|db| {
                db.execute("INSERT INTO attempt_route_audit
                    (id,attempt_id,task_id,role,engine,connection_id,model,params_fingerprint,decision,decision_detail_json,recorded_at)
                    VALUES (?,?,'timings',?,'engine','connection','model','digest','authorized',?,?)",
                    (&assignment,&attempt,role,json!({"route_event":route}).to_string(),&delivered))?;
                event_tx(db,"timings","NATIVE_ATTEMPT_DELIVERY",json!({"assignment_ref":assignment,"attempt":attempt,
                    "first":true,"delivered_at":delivered,"route_event":route}))?;
                Ok(())
            }).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(trial_ms));
            store
                .write(|db| {
                    event_tx(
                        db,
                        "timings",
                        "WORKER_COMPLETED",
                        json!({"assignment_ref":assignment,"attempt_id":attempt}),
                    )
                })
                .unwrap();
        }
        store
            .write(|db| {
                event_tx(
                    db,
                    "holder",
                    "ASSIGNMENT_CREATED",
                    json!({"assignment_ref":"review-live","node":"review","revision":1}),
                )
            })
            .unwrap();

        struct TestHost;
        impl crate::scheduler::SchedulingPolicy for TestHost {
            fn may_claim(&self, _: &Transaction<'_>, _: &str, _: &str) -> CoreResult<bool> {
                Ok(true)
            }
            fn supports_assignment_worktrees(&self) -> bool {
                true
            }
        }
        let claims =
            crate::scheduler::claim_ready(&store, "fixture", &owner, "adaptive-fixture", &TestHost)
                .unwrap();
        assert_eq!(claims.len(), 1);
        assert!(store
            .read(|db| selected(db, "fixture", &claims[0]))
            .unwrap());
        assert_eq!(
            store
                .write(|db| require_launch(db, "fixture", &owner, &claims[0], None))
                .unwrap_err()
                .code(),
            "assignment_worktree_launch_unavailable"
        );
    }
}

/// The ordinary desktop host is opt-in to measured isolation. Its provider
/// compatibility and live clean checkout are rechecked before provisioning.
pub(crate) fn beneficial(
    db: &Connection,
    task: &str,
    revision: i64,
    node: &str,
) -> CoreResult<bool> {
    use crate::scheduler::node_duration;
    let logical: String =
        db.query_row("SELECT project_root FROM tasks WHERE id=?", [task], |r| {
            r.get(0)
        })?;
    if policy::root_tx(db, task)? != logical {
        return Ok(false);
    }
    let Some(holder) = policy::review_holder_tx(db, task)? else {
        return Ok(false);
    };
    let scope: String = db.query_row(
        "SELECT scope_json FROM runtime_nodes WHERE task_id=? AND revision=? AND node_id=?",
        (task, revision, node),
        |r| r.get(0),
    )?;
    let scope: Vec<String> = serde_json::from_str(&scope)?;
    let root = Path::new(&logical);
    if scope.is_empty()
        || scope.len() > checkout::integration::MAX_FILES
        || scope
            .iter()
            .any(|path| !is_literal_tracked_file_candidate(root, path))
    {
        return Ok(false);
    }
    let declined: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='ASSIGNMENT_WORKTREE_SELECTION_DEFERRED'
         AND json_extract(payload_json,'$.revision')=? AND json_extract(payload_json,'$.node')=?)",
        (task, revision, node), |r| r.get(0),
    )?;
    if declined {
        return Ok(false);
    }
    // A clean canonical checkout under review may be read in parallel only
    // when all of the holder's writing scopes are disjoint.
    let mut held = db.prepare(
        "SELECT n.revision,n.node_id,n.role,n.scope_json FROM runtime_nodes n
         JOIN runtime_tasks t ON t.task_id=n.task_id AND t.revision=n.revision
         WHERE n.task_id=? AND n.stage='execution'",
    )?;
    let held = held
        .query_map([&holder], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (revision, node, role, paths) in held {
        if policy::node_read_only(db, &holder, revision, &node, &role, "execution")? {
            continue;
        }
        let paths: Vec<String> = serde_json::from_str(&paths)?;
        if scope
            .iter()
            .any(|path| paths.iter().any(|other| policy::overlaps(path, other)))
        {
            return Ok(false);
        }
    }
    let reviewer: Option<(String, String)> = db
        .query_row(
            "SELECT n.node_id,e.created_at FROM runtime_tasks t JOIN runtime_nodes n
          ON n.task_id=t.task_id AND n.revision=t.revision
         JOIN runtime_events e ON e.task_id=n.task_id AND e.type='ASSIGNMENT_CREATED'
          AND json_extract(e.payload_json,'$.assignment_ref')=n.assignment_ref
         WHERE t.task_id=? AND t.state='REVIEWING' AND n.stage='review' AND n.status='running'
         ORDER BY e.id DESC LIMIT 1",
            [&holder],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((reviewer, started)) = reviewer else {
        return Ok(false);
    };
    let (Some(worker_ms), Some(review_ms)) = (
        node_duration(db, task, node)?,
        node_duration(db, &holder, &reviewer)?,
    ) else {
        return Ok(false);
    };
    let Ok(started) = chrono::DateTime::parse_from_rfc3339(&started) else {
        return Ok(false);
    };
    let age = chrono::Utc::now()
        .signed_duration_since(started)
        .num_milliseconds();
    let Ok(age) = u64::try_from(age) else {
        return Ok(false);
    };
    // Charge the worst recent verified checkout preparation plus a complete
    // cold verifier run, and allow another preparation interval for integration.
    // No timing sample can stand in for a missing preparation or verifier run.
    let cost: Option<i64> = db.query_row(
        "SELECT max(cost) FROM (
           SELECT 2*json_extract(p.payload_json,'$.elapsed_ms')+json_extract(v.payload_json,'$.elapsed_ms') AS cost
           FROM runtime_events v JOIN runtime_events p ON p.task_id=v.task_id AND p.id<v.id
             AND p.type='NATIVE_WORKTREE_PROVISION_TIME'
             AND json_extract(p.payload_json,'$.checkout_id')=json_extract(v.payload_json,'$.checkout_id')
           WHERE v.type='NATIVE_WORKTREE_VERIFICATION_TIME'
             AND json_extract(v.payload_json,'$.elapsed_ms') BETWEEN 1 AND 86400000
             AND json_extract(p.payload_json,'$.elapsed_ms') BETWEEN 1 AND 3600000
           ORDER BY v.id DESC LIMIT 8)", [], |r| r.get(0),
    )?;
    Ok(cost
        .and_then(|ms| u64::try_from(ms).ok())
        .is_some_and(|ms| worker_ms.min(review_ms.saturating_sub(age)) > ms))
}

pub fn selected(db: &Connection, task: &str, assignment: &str) -> CoreResult<bool> {
    Ok(db.query_row(
        "SELECT coalesce((SELECT type='ASSIGNMENT_WORKTREE_SELECTED' FROM runtime_events
           WHERE task_id=? AND type IN ('ASSIGNMENT_WORKTREE_SELECTED','ASSIGNMENT_WORKTREE_SELECTION_DEFERRED','ASSIGNMENT_CREATED')
           AND json_extract(payload_json,'$.assignment_ref')=? ORDER BY id DESC LIMIT 1),0)",
        (task, assignment), |r| r.get(0),
    )?)
}

/// Skip an unusable selection before any filesystem effect. A later normal
/// claim may use the project folder once the native checkout reservation permits.
pub fn defer(
    tx: &Transaction<'_>,
    task: &str,
    owner: &ownership::OwnerEpoch,
    assignment: &str,
    reason: &str,
) -> CoreResult<()> {
    let revision = active_execution(tx, task, owner, assignment)?;
    require_unlaunched(tx, task, assignment)?;
    if !selected(tx, task, assignment)? || tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM assignment_worktrees WHERE task_id=? AND assignment_ref=?)",
        (task, assignment), |r| r.get::<_,bool>(0),
    )? {
        return refuse("assignment_worktree_outcome_unknown");
    }
    let node: String = tx.query_row(
        "UPDATE runtime_nodes SET status='pending',assignment_ref=NULL
         WHERE task_id=? AND revision=? AND assignment_ref=? RETURNING node_id",
        (task, revision, assignment),
        |r| r.get(0),
    )?;
    event_tx(
        tx,
        task,
        "ASSIGNMENT_WORKTREE_SELECTION_DEFERRED",
        json!({"revision":revision,"node":node,"assignment_ref":assignment,"reason":reason}),
    )?;
    Ok(())
}

/// Check for a matching clean committed canonical checkout before provisioning.
pub fn eligible_target(
    store: &Store,
    task: &str,
    owner: &ownership::OwnerEpoch,
    assignment: &str,
) -> CoreResult<bool> {
    let root = store.key().root();
    let scope: String = store.read(|db| {
        let revision = active_execution(db, task, owner, assignment)?;
        Ok(db.query_row(
            "SELECT scope_json FROM runtime_nodes WHERE task_id=? AND revision=? AND assignment_ref=?",
            (task, revision, assignment), |r| r.get(0),
        )?)
    })?;
    let scope: Vec<String> = serde_json::from_str(&scope)?;
    if scope.is_empty()
        || scope.len() > checkout::integration::MAX_FILES
        || scope
            .iter()
            .any(|path| !is_literal_tracked_file_candidate(root, path))
        || !checkout::integration::index_tracks(root, &scope)?
    {
        return Ok(false);
    }
    let Ok(base) = checkout::rework_base(root, None, true) else {
        return Ok(false);
    };
    match checkout::integration::Transfer::observe(root, &base) {
        Ok(observation) => {
            Ok(observation.count() == 0 && checkout::integration::clean_target_index(root)?)
        }
        Err(_) => Ok(false),
    }
}
