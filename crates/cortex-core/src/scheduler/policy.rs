//! Native scope reservations, access FIFO, denial and persisted retry policy.
//! Python oracle: orchestration/state.py:1611-1788, access.py:44-309,
//! policy.py:42-58, project_paths.py:11-27. This coordinates cooperative writers;
//! it is NOT an OS filesystem grant or hostile-code containment.
use super::SchedulingPolicy;
use crate::{
    error::refuse,
    lifecycle::{domain, event_tx, request_tx, waits},
    CoreError, CoreResult,
};
use rusqlite::{OptionalExtension, Transaction};
use serde_json::{json, Value};
use std::{collections::BTreeSet, os::unix::fs::MetadataExt, path::Path};

pub fn read_only(role: &str, stage: &str) -> bool {
    stage == "review"
        || [
            "worker-architect",
            "worker-ux-designer",
            "worker-code-reviewer",
            "worker-senior-consultant",
            "senior-consultant",
            "worker-explorer",
            "worker-planner",
            "worker-security-auditor",
            "worker-accessibility-auditor",
            "worker-database-architect",
            "worker-performance-engineer",
            "worker-build-verification",
        ]
        .contains(&role)
}
pub fn canonical(path: &str) -> String {
    let absolute = path.starts_with('/');
    let p = path
        .split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .collect::<Vec<_>>()
        .join("/");
    if absolute {
        format!("/{p}")
    } else if p.is_empty() {
        ".".into()
    } else {
        p
    }
}
pub fn overlaps(left: &str, right: &str) -> bool {
    fn prefix(s: &str) -> (String, bool) {
        let s = canonical(s);
        let idx = s.find(['*', '?', '[']);
        let p = s[..idx.unwrap_or(s.len())].trim_end_matches('/');
        (
            if p == "." { String::new() } else { p.into() },
            idx.is_some(),
        )
    }
    let (a, aw) = prefix(left);
    let (b, bw) = prefix(right);
    a.is_empty()
        || b.is_empty()
        || a == b
        || a.starts_with(&(b.clone() + "/"))
        || b.starts_with(&(a.clone() + "/"))
        || aw && b.starts_with(&a)
        || bw && a.starts_with(&b)
}
fn strings(v: &Value) -> CoreResult<Vec<String>> {
    v.as_array()
        .ok_or(CoreError::Refused("invalid_scope"))?
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .ok_or(CoreError::Refused("invalid_scope"))
        })
        .collect()
}
fn root_tx(db: &Transaction<'_>, task: &str) -> CoreResult<String> {
    Ok(db.query_row("SELECT coalesce((SELECT workspace_root FROM task_workspaces WHERE task_id=t.id),project_root) FROM tasks t WHERE id=?",
        [task],|r|r.get(0))?)
}
fn absolute(root: &str, paths: &[String]) -> Vec<String> {
    paths
        .iter()
        .map(|p| {
            if canonical(p) == "." {
                root.trim_end_matches('/').into()
            } else {
                format!("{}/{}", root.trim_end_matches('/'), canonical(p))
            }
        })
        .collect()
}
pub fn effective_scope_tx(
    db: &Transaction<'_>,
    task: &str,
    assignment: Option<&str>,
    scope: &str,
) -> CoreResult<Vec<String>> {
    let mut scopes = strings(&serde_json::from_str(scope)?)?;
    if let Some(a) = assignment {
        let grants = db
            .prepare(
                "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='ACCESS_GRANTED'
            AND json_extract(payload_json,'$.assignment_ref')=? ORDER BY id",
            )?
            .query_map((task, a), |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        for s in grants {
            scopes.extend(strings(&serde_json::from_str::<Value>(&s)?["added_paths"])?);
        }
    }
    Ok(scopes
        .into_iter()
        .map(|s| canonical(&s))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect())
}
struct Holding {
    task: String,
    assignment: Option<String>,
    paths: Vec<String>,
}
/// The write scopes other work must not overlap. A node holds its files while its
/// task is unfinished and its worker may still write: never for a finished
/// (completed, cancelled or failed) task or one being deleted, whose channels are
/// revoked and whose nodes are never claimed again, and never for an attempt whose
/// every delegation row is terminated with no reservation left unreleased. Rows
/// written by earlier builds are read the same way.
fn holdings_tx(db: &Transaction<'_>) -> CoreResult<Vec<Holding>> {
    let rows=db.prepare("SELECT n.task_id,n.assignment_ref,n.scope_json,n.role,n.stage FROM runtime_nodes n JOIN runtime_tasks t USING(task_id)
        WHERE t.state NOT IN ('COMPLETED','CANCELLED','FAILED')
        AND EXISTS(SELECT 1 FROM tasks b WHERE b.id=n.task_id)
        AND NOT EXISTS(SELECT 1 FROM pending_deletions d WHERE d.task_id=n.task_id)
        AND NOT (n.assignment_ref IS NOT NULL
            AND EXISTS(SELECT 1 FROM delegation_provisioning p WHERE p.task_id=n.task_id AND p.idempotency_key=n.assignment_ref)
            AND NOT EXISTS(SELECT 1 FROM delegation_provisioning p WHERE p.task_id=n.task_id AND p.idempotency_key=n.assignment_ref
                AND p.status!='terminated')
            AND NOT EXISTS(SELECT 1 FROM delegation_provisioning p JOIN capacity_reservations a ON a.attempt_id=p.attempt_id
                WHERE p.task_id=n.task_id AND p.idempotency_key=n.assignment_ref AND a.state IN ('reserved','ambiguous')))
        AND (n.assignment_ref IS NOT NULL OR n.revision=t.revision)
        AND (n.assignment_ref IS NULL OR NOT EXISTS(SELECT 1 FROM runtime_events e WHERE e.task_id=n.task_id
            AND e.type='WORKER_RELEASED' AND json_extract(e.payload_json,'$.assignment_ref')=n.assignment_ref))
        AND NOT EXISTS(SELECT 1 FROM delegation_provisioning p JOIN capacity_reservations r ON r.attempt_id=p.attempt_id
            AND r.kind='worker' AND r.state='released' WHERE p.task_id=n.task_id AND p.idempotency_key=n.assignment_ref
            AND NOT EXISTS(SELECT 1 FROM capacity_reservations a WHERE a.attempt_id=p.attempt_id AND a.state IN ('reserved','ambiguous')))
        AND ((n.status='running' AND t.state IN ('EXECUTING','REVIEWING')) OR EXISTS(
            SELECT 1 FROM delegation_provisioning p WHERE p.task_id=n.task_id AND p.idempotency_key=n.assignment_ref))")?
        .query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?)))?
        .collect::<Result<Vec<_>,_>>()?;
    let mut out = Vec::new();
    for (task, a, scope, role, stage) in rows {
        if !read_only(&role, &stage) {
            out.push(Holding {
                paths: absolute(
                    &root_tx(db, &task)?,
                    &effective_scope_tx(db, &task, a.as_deref(), &scope)?,
                ),
                task,
                assignment: a,
            });
        }
    }
    Ok(out)
}
/// A stable checkout is shared by independent readers through final verification.
fn review_holder_tx(db: &Transaction<'_>, task: &str) -> CoreResult<Option<String>> {
    let root = root_tx(db, task)?;
    Ok(db.query_row("SELECT t.task_id FROM runtime_tasks t JOIN tasks b ON b.id=t.task_id
        LEFT JOIN task_workspaces w ON w.task_id=t.task_id
        WHERE t.task_id!=? AND coalesce(w.workspace_root,b.project_root)=?
        AND (t.state IN ('REVIEWING','VERIFYING') OR (t.state IN ('RECONCILING','ANSWER_READY')
          AND EXISTS(SELECT 1 FROM runtime_events e WHERE e.task_id=t.task_id
            AND e.type='VERIFICATION_PASSED' AND json_extract(e.payload_json,'$.revision')=t.revision)) OR (t.state='EXECUTING'
          AND EXISTS(SELECT 1 FROM runtime_nodes n WHERE n.task_id=t.task_id AND n.revision=t.revision)
          AND NOT EXISTS(SELECT 1 FROM runtime_nodes n WHERE n.task_id=t.task_id AND n.revision=t.revision AND n.status!='completed')))
        AND NOT EXISTS(SELECT 1 FROM pending_deletions d WHERE d.task_id=t.task_id)
        ORDER BY t.task_id LIMIT 1", (task,root), |r|r.get(0)).optional()?)
}

/// Check before entering review, in the same transaction that reserves the checkout.
pub fn review_writer_tx(db: &Transaction<'_>, task: &str) -> CoreResult<Option<Value>> {
    let root = root_tx(db, task)?;
    for h in holdings_tx(db)? {
        if h.task != task && root_tx(db, &h.task)? == root {
            return Ok(Some(
                json!({"node":"review","holder_task":h.task,"paths":h.paths}),
            ));
        }
    }
    Ok(None)
}
fn pending_tx(db: &Transaction<'_>) -> CoreResult<Vec<Value>> {
    let tasks = db
        .prepare("SELECT DISTINCT task_id FROM runtime_events WHERE type='WORKER_WAIT_CHANGED'")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut out = Vec::new();
    for task in tasks {
        for w in waits::projection_tx(db, &task)? {
            if w["kind"] == "access"
                && w["status"] == "open"
                && w["reason"] == "active_writer_conflict"
                && waits::valid_tx(db, &w)?
            {
                out.push(w);
            }
        }
    }
    out.sort_by_key(|w| w["created_at"].as_str().unwrap_or("").to_owned());
    Ok(out)
}
/// The owner's write denials in force (lifted ones excluded).
fn denied_tx(db: &Transaction<'_>, task: &str) -> CoreResult<Vec<String>> {
    crate::lifecycle::steering::denials(db, task)
}
/// Always applied by claim_ready, even when the adapter supplies extra policy.
pub struct NativePolicy {
    pub now_seconds: f64,
}
impl Default for NativePolicy {
    fn default() -> Self {
        Self {
            now_seconds: chrono::Utc::now().timestamp_micros() as f64 / 1_000_000.0,
        }
    }
}
impl SchedulingPolicy for NativePolicy {
    fn may_claim(&self, db: &Transaction<'_>, task: &str, node: &str) -> CoreResult<bool> {
        let (role,stage,scope,assignment):(String,String,String,Option<String>)=db.query_row(
            "SELECT role,stage,scope_json,assignment_ref FROM runtime_nodes WHERE task_id=? AND revision=(SELECT revision FROM runtime_tasks WHERE task_id=?) AND node_id=?",
            (task,task,node),|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
        let held:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_events e JOIN runtime_tasks t USING(task_id)
            WHERE e.task_id=? AND e.type='ASSIGNMENT_RETRIED' AND json_extract(e.payload_json,'$.revision')=t.revision
            AND json_extract(e.payload_json,'$.node')=? AND json_extract(e.payload_json,'$.retry_not_before')>?)",
            (task,node,self.now_seconds),|r|r.get(0))?;
        if held {
            return Ok(false);
        }
        if read_only(&role, &stage) {
            return Ok(true);
        }
        if review_holder_tx(db, task)?.is_some() {
            return Ok(false);
        }
        let scope = effective_scope_tx(db, task, assignment.as_deref(), &scope)?;
        let denied = denied_tx(db, task)?;
        if scope.iter().any(|a| denied.iter().any(|b| overlaps(a, b))) {
            return Ok(false);
        }
        let paths = absolute(&root_tx(db, task)?, &scope);
        for h in holdings_tx(db)? {
            if paths.iter().any(|a| h.paths.iter().any(|b| overlaps(a, b))) {
                return Ok(false);
            }
        }
        for w in pending_tx(db)? {
            let p = absolute(
                &root_tx(db, waits::field(&w, "task_id")?)?,
                &strings(&w["paths"])?,
            );
            if paths.iter().any(|a| p.iter().any(|b| overlaps(a, b))) {
                return Ok(false);
            }
        }
        Ok(true)
    }
}
/// A dependency-ready writing node of the task's current revision that cannot be
/// claimed only because another unfinished task's worker holds overlapping files:
/// `{"node","holder_task","paths"}` for the first such node, in claim order. The
/// node is queued behind that holding; nothing is recorded or changed here.
pub fn write_conflict_tx(db: &Transaction<'_>, task: &str) -> CoreResult<Option<Value>> {
    let Some((state, revision)): Option<(String, i64)> = db
        .query_row(
            "SELECT state,revision FROM runtime_tasks WHERE task_id=?",
            [task],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
    else {
        return Ok(None);
    };
    if matches!(state.as_str(), "REVIEWING" | "VERIFYING") {
        if let Some(conflict) = review_writer_tx(db, task)? {
            return Ok(Some(conflict));
        }
    }
    let stage = match state.as_str() {
        "EXECUTING" => "execution",
        "REVIEWING" => "review",
        _ => return Ok(None),
    };
    let nodes = db
        .prepare("SELECT node_id,stage,status,dependencies_json FROM runtime_nodes WHERE task_id=? AND revision=? ORDER BY rowid")?
        .query_map((task, revision), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?))
        })?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|(id, stage, status, deps)| {
            Ok(super::Node { id, stage, status, dependencies: serde_json::from_str(&deps)? })
        })
        .collect::<CoreResult<Vec<_>>>()?;
    let ready = super::dependency_ready(&nodes, stage)?;
    if ready.is_empty() {
        if state == "EXECUTING"
            && !nodes.is_empty()
            && nodes.iter().all(|n| n.status == "completed")
        {
            return review_writer_tx(db, task);
        }
        return Ok(None);
    }
    let holdings = holdings_tx(db)?;
    let root = root_tx(db, task)?;
    for node in ready {
        let (role, node_stage, scope, assignment): (String, String, String, Option<String>) = db.query_row(
            "SELECT role,stage,scope_json,assignment_ref FROM runtime_nodes WHERE task_id=? AND revision=? AND node_id=?",
            (task, revision, &node),
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
        if read_only(&role, &node_stage) {
            continue;
        }
        if let Some(holder) = review_holder_tx(db, task)? {
            return Ok(Some(
                json!({"node":node,"holder_task":holder,"paths":[root]}),
            ));
        }
        let paths = absolute(
            &root,
            &effective_scope_tx(db, task, assignment.as_deref(), &scope)?,
        );
        for h in holdings.iter().filter(|h| h.task != task) {
            let held: Vec<&String> = h
                .paths
                .iter()
                .filter(|b| paths.iter().any(|a| overlaps(a, b)))
                .collect();
            if !held.is_empty() {
                return Ok(Some(json!({"node":node,"holder_task":h.task,"paths":held})));
            }
        }
    }
    Ok(None)
}
/// Why each dependency-ready node of the task's current revision is not claimed,
/// by the same rules and in the same order as `NativePolicy::may_claim`, so that a
/// stop names what really holds the step. Read-only; nothing is recorded. Each item
/// is `{"node","role","objective","scope","reason",..}` with `reason` one of
/// `retry_backoff`, `denied` (`paths` of the step, owner `rules` they overlap),
/// `held` (`holder_task`, `holder_assignment`, `holder_role`, `paths`; the holder
/// may be an earlier step of this same task), `access_wait` (`holder_task`) or
/// `claimable`. No ready node gives one `{"reason":"no_ready_nodes"}` item.
pub fn claim_obstacles_tx(
    db: &Transaction<'_>,
    task: &str,
    now_seconds: f64,
) -> CoreResult<Vec<Value>> {
    let Some((state, revision)): Option<(String, i64)> = db
        .query_row(
            "SELECT state,revision FROM runtime_tasks WHERE task_id=?",
            [task],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
    else {
        return Ok(Vec::new());
    };
    let stage = if state == "REVIEWING" {
        "review"
    } else {
        "execution"
    };
    let nodes = db
        .prepare("SELECT node_id,stage,status,dependencies_json FROM runtime_nodes WHERE task_id=? AND revision=? ORDER BY rowid")?
        .query_map((task, revision), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?))
        })?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|(id, stage, status, deps)| Ok(super::Node { id, stage, status, dependencies: serde_json::from_str(&deps)? }))
        .collect::<CoreResult<Vec<_>>>()?;
    let ready = super::dependency_ready(&nodes, stage)?;
    if ready.is_empty() {
        return Ok(vec![json!({"reason":"no_ready_nodes"})]);
    }
    let root = root_tx(db, task)?;
    let holdings = holdings_tx(db)?;
    let denied = denied_tx(db, task)?;
    let pending = pending_tx(db)?;
    let mut out = Vec::new();
    for node in ready {
        let (role, node_stage, scope, assignment, objective): (String, String, String, Option<String>, String) = db.query_row(
            "SELECT role,stage,scope_json,assignment_ref,objective FROM runtime_nodes WHERE task_id=? AND revision=? AND node_id=?",
            (task, revision, &node),
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )?;
        let scope = effective_scope_tx(db, task, assignment.as_deref(), &scope)?;
        let mut item = json!({"node":node,"role":role,"objective":objective,"scope":scope});
        let backoff: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='ASSIGNMENT_RETRIED'
             AND json_extract(payload_json,'$.revision')=? AND json_extract(payload_json,'$.node')=?
             AND json_extract(payload_json,'$.retry_not_before')>?)",
            (task, revision, &node, now_seconds),
            |r| r.get(0),
        )?;
        let reason = if backoff {
            json!("retry_backoff")
        } else if read_only(&role, &node_stage) {
            json!("claimable")
        } else if let Some(holder) = review_holder_tx(db, task)? {
            item["holder_task"] = json!(holder);
            item["paths"] = json!([root]);
            json!("held")
        } else if let Some(rules) = Some(
            denied
                .iter()
                .filter(|b| scope.iter().any(|a| overlaps(a, b)))
                .cloned()
                .collect::<Vec<_>>(),
        )
        .filter(|r| !r.is_empty())
        {
            item["rules"] = json!(rules);
            item["paths"] = json!(scope
                .iter()
                .filter(|a| rules.iter().any(|b| overlaps(a, b)))
                .collect::<Vec<_>>());
            json!("denied")
        } else {
            let paths = absolute(&root, &scope);
            let holder = holdings.iter().find_map(|h| {
                let held: Vec<&String> = h
                    .paths
                    .iter()
                    .filter(|b| paths.iter().any(|a| overlaps(a, b)))
                    .collect();
                (!held.is_empty()).then_some((h, held))
            });
            if let Some((h, held)) = holder {
                let holder_role: Option<String> = match &h.assignment {
                    Some(a) => db
                        .query_row(
                            "SELECT role FROM runtime_nodes WHERE task_id=? AND assignment_ref=?",
                            (&h.task, a),
                            |r| r.get(0),
                        )
                        .optional()?,
                    None => None,
                };
                item["holder_task"] = json!(h.task);
                item["holder_assignment"] = json!(h.assignment);
                item["holder_role"] = json!(holder_role);
                item["paths"] = json!(held);
                json!("held")
            } else if let Some(w) = pending.iter().find(|w| {
                waits::field(w, "task_id")
                    .ok()
                    .zip(strings(&w["paths"]).ok())
                    .is_some_and(|(t, p)| {
                        root_tx(db, t).is_ok_and(|r| {
                            absolute(&r, &p)
                                .iter()
                                .any(|b| paths.iter().any(|a| overlaps(a, b)))
                        })
                    })
            }) {
                item["holder_task"] = w["task_id"].clone();
                json!("access_wait")
            } else {
                json!("claimable")
            }
        };
        item["reason"] = reason;
        out.push(item);
    }
    Ok(out)
}
fn safe_path(root: &str, path: &str) -> bool {
    if path.is_empty()
        || path.contains(['*', '?', '\\', '\0'])
        || path.starts_with('/')
        || path.split('/').any(|p| {
            [
                "..",
                ".git",
                ".cortex",
                ".cortex-dev",
                ".cortex-dev-owner.json",
                ".cortex-workspace-dev.json",
            ]
            .contains(&p)
        })
    {
        return false;
    }
    let mut current = Path::new(root).to_path_buf();
    for p in canonical(path).split('/') {
        if p == "." {
            continue;
        }
        current.push(p);
        match std::fs::symlink_metadata(&current) {
            Ok(m) if m.file_type().is_symlink() || m.is_file() && m.nlink() > 1 => return false,
            Ok(m) if !m.is_dir() && !m.is_file() => return false,
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return false,
        }
    }
    true
}
fn predicate_tx(
    db: &Transaction<'_>,
    w: &Value,
    paths: &[String],
) -> CoreResult<Option<&'static str>> {
    let task = waits::field(w, "task_id")?;
    let assignment = waits::field(w, "assignment_ref")?;
    let (role, stage): (String, String) = db.query_row(
        "SELECT role,stage FROM runtime_nodes WHERE task_id=? AND assignment_ref=?",
        (task, assignment),
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if read_only(&role, &stage) {
        return Ok(Some("read_only_role"));
    }
    if paths
        .iter()
        .any(|p| p.starts_with('/') || p.split('/').any(|p| p == ".."))
    {
        return Ok(Some("outside_project"));
    }
    let root = root_tx(db, task)?;
    if paths.is_empty() || paths.iter().any(|p| !safe_path(&root, p)) {
        return Ok(Some("unsafe_path"));
    }
    let denied = denied_tx(db, task)?;
    if paths.iter().any(|p| denied.iter().any(|d| overlaps(p, d))) {
        return Ok(Some("deny_write_policy"));
    }
    let requested = absolute(&root, paths);
    for h in holdings_tx(db)? {
        if h.task == task && h.assignment.as_deref() == Some(assignment) {
            continue;
        }
        if requested
            .iter()
            .any(|p| h.paths.iter().any(|h| overlaps(p, h)))
        {
            return Ok(Some("active_writer_conflict"));
        }
    }
    for other in pending_tx(db)? {
        if other["request_id"] == w["request_id"]
            || other["created_at"].as_str() > w["created_at"].as_str()
        {
            continue;
        }
        let held = absolute(
            &root_tx(db, waits::field(&other, "task_id")?)?,
            &strings(&other["paths"])?,
        );
        if requested
            .iter()
            .any(|p| held.iter().any(|h| overlaps(p, h)))
        {
            return Ok(Some("active_writer_conflict"));
        }
    }
    Ok(None)
}
fn grant_tx(db: &Transaction<'_>, w: &Value, paths: &[String]) -> CoreResult<Value> {
    let task = waits::field(w, "task_id")?;
    let assignment = waits::field(w, "assignment_ref")?;
    let scope: String = db.query_row(
        "SELECT scope_json FROM runtime_nodes WHERE task_id=? AND assignment_ref=?",
        (task, assignment),
        |r| r.get(0),
    )?;
    let effective = effective_scope_tx(db, task, Some(assignment), &scope)?;
    let added: Vec<_> = paths
        .iter()
        .map(|p| canonical(p))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|p| {
            !effective
                .iter()
                .any(|s| s == p || s == "." || p.starts_with(&(s.clone() + "/")))
        })
        .collect();
    let revision: i64 = db.query_row(
        "SELECT count(*) FROM runtime_events WHERE task_id=? AND type='ACCESS_GRANTED'
        AND json_extract(payload_json,'$.assignment_ref')=?",
        (task, assignment),
        |r| r.get(0),
    )?;
    let decision = json!({"request_id":w["request_id"],"status":if added.is_empty(){"already_covered"}else{"granted"},
        "added_paths":added,"declared_scope_revision":revision+i64::from(!added.is_empty()),"enforcement":"already_project_writable"});
    if !added.is_empty() {
        let mut payload = decision.clone();
        payload["assignment_ref"] = json!(assignment);
        event_tx(db, task, "ACCESS_GRANTED", payload)?;
    }
    Ok(decision)
}
fn resolve_tx(db: &Transaction<'_>, w: &mut Value, reason: Option<&str>) -> CoreResult<()> {
    let paths = strings(&w["paths"])?;
    let decision = match reason {
        Some(r) => json!({"status":"denied","reason":r,"request_id":w["request_id"]}),
        None => grant_tx(db, w, &paths)?,
    };
    event_tx(
        db,
        waits::field(w, "task_id")?,
        "ACCESS_DECIDED",
        json!({"wait_id":w["wait_id"],"decision":decision}),
    )?;
    w["status"] = json!("ready");
    w["reason"] = json!(reason);
    w["retry_at"] = Value::Null;
    w["access_decision"] = decision;
    waits::change_tx(db, w)
}
/// No question card and no owner answer can grant an access reservation.
pub fn request_access_tx(
    db: &Transaction<'_>,
    task: &str,
    channel: &str,
    key: &str,
    paths: &[String],
) -> CoreResult<Value> {
    let row: Option<(String, String, String, i64)> = db
        .query_row(
            "SELECT p.idempotency_key,p.attempt_id,p.engine_session_ref,c.owner_epoch
        FROM delegation_provisioning p JOIN trusted_channels c ON p.channel_id=c.channel_id
        WHERE c.task_id=? AND c.channel_id=? AND c.revoked_at IS NULL AND c.role='worker'",
            (task, channel),
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let (assignment, attempt, session, epoch) =
        row.ok_or(CoreError::Refused("stale_worker_authority"))?;
    let mut w = json!({"wait_id":format!("ww_{}",uuid::Uuid::new_v4().simple()),"task_id":task,"assignment_ref":assignment,
        "attempt_id":attempt,"engine_session_ref":session,"channel_id":channel,"owner_epoch":epoch,"revision":domain::task_tx(db,task)?.1,
        "created_at":crate::now(),"kind":"access","paths":paths,"request_id":format!("{channel}:{key}"),"status":"open","question_id":null});
    if !waits::valid_tx(db, &w)? {
        return refuse("stale_worker_authority");
    }
    request_tx(
        db,
        task,
        channel,
        key,
        "access_request",
        &json!({"paths":paths}),
        || {
            waits::check_mutation_tx(db, task, &assignment)?;
            let reason = predicate_tx(db, &w, paths)?;
            if reason == Some("active_writer_conflict") {
                w["reason"] = json!(reason);
                w["retry_attempt"] = json!(0);
                w["retry_at"] = json!((chrono::Utc::now() + chrono::Duration::seconds(1))
                    .to_rfc3339_opts(chrono::SecondsFormat::Micros, false));
                waits::change_tx(db, &mut w)?;
                Ok(
                    json!({"status":"pending","wait_id":w["wait_id"],"reason":reason,"complete":true}),
                )
            } else if let Some(reason) = reason {
                // A denial is delivered through the same retained worker session,
                // never a user question or an OS permission escalation.
                resolve_tx(db, &mut w, Some(reason))?;
                Ok(
                    json!({"status":"pending","request_id":w["request_id"],"wait_id":w["wait_id"],"reason":reason,"complete":true}),
                )
            } else {
                let mut decision = grant_tx(db, &w, paths)?;
                decision["complete"] = json!(true);
                event_tx(
                    db,
                    task,
                    "ACCESS_REQUESTED",
                    json!({"binding":w,"receipt":decision}),
                )?;
                Ok(decision)
            }
        },
    )
}
/// Advance this assignment's open access wait once. The caller arms a timer for
/// the returned duration; owner questions and paused tasks have no timer.
pub fn poll_access_tx(
    db: &Transaction<'_>,
    wait: &mut Value,
    now: &str,
) -> CoreResult<Option<std::time::Duration>> {
    if wait["kind"] != "access" || wait["status"] != "open" {
        return Ok(None);
    }
    if domain::task_tx(db, waits::field(wait, "task_id")?)?.0 == crate::TaskState::Paused {
        return Ok(None);
    }
    if !waits::valid_tx(db, wait)? {
        wait["status"] = json!("cancelled");
        wait["reason"] = json!("authority_lost");
        waits::change_tx(db, wait)?;
        return Ok(None);
    }
    let parse = |s: &str| {
        chrono::DateTime::parse_from_rfc3339(s).map_err(|_| CoreError::Refused("invalid_time"))
    };
    let now = parse(now)?;
    if let Some(at) = wait["retry_at"].as_str() {
        if let Ok(remaining) = (parse(at)? - now).to_std() {
            if !remaining.is_zero() {
                return Ok(Some(remaining));
            }
        }
    }
    let reason = predicate_tx(db, wait, &strings(&wait["paths"])?)?;
    let attempt = wait["retry_attempt"].as_u64().unwrap_or(0);
    if reason != Some("active_writer_conflict") || attempt >= 2 {
        resolve_tx(db, wait, reason)?;
        return Ok(None);
    }
    let next = attempt + 1;
    let seconds = [1, 2, 4][next as usize];
    wait["retry_attempt"] = json!(next);
    wait["retry_at"] = json!((now + chrono::Duration::seconds(seconds))
        .to_rfc3339_opts(chrono::SecondsFormat::Micros, false));
    waits::change_tx(db, wait)?;
    Ok(Some(std::time::Duration::from_secs(seconds as u64)))
}

pub fn reconcile_access_tx(db: &Transaction<'_>, task: &str, now: &str) -> CoreResult<()> {
    for mut wait in waits::projection_tx(db, task)? {
        poll_access_tx(db, &mut wait, now)?;
    }
    Ok(())
}
pub fn revalidate_access_tx(db: &Transaction<'_>, w: &mut Value) -> CoreResult<()> {
    if ["granted", "already_covered"]
        .contains(&w["access_decision"]["status"].as_str().unwrap_or(""))
    {
        if let Some(reason) = predicate_tx(db, w, &strings(&w["paths"])?)? {
            resolve_tx(db, w, Some(reason))?;
        }
    }
    Ok(())
}
#[cfg(test)]
mod reviewer_policy_tests {
    use super::read_only;
    /// The review stage and the reviewer role are read-only for native edit tools,
    /// whichever stage a reviewer role is recorded under.
    #[test]
    fn reviewers_are_read_only_in_every_stage() {
        for stage in ["review", "execution", ""] {
            assert!(read_only("worker-code-reviewer", stage));
        }
        assert!(read_only("worker-backend-dev", "review"));
        assert!(!read_only("worker-backend-dev", "execution"));
    }
}
