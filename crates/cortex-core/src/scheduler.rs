//! Dependency selection, native scope/retry policy and durable engine capacity.
//! Adapter policy can restrict native claims, never bypass them. No engine is
//! launched here, and a timeout never releases a reservation.
use crate::{
    error::refuse,
    lifecycle::event_tx,
    ownership::{self, OwnerEpoch},
    CoreResult, Store,
};
use rusqlite::{Connection, OptionalExtension, Transaction};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
pub mod policy;
pub mod retry;
#[derive(Debug, Clone)]
pub struct Node {
    pub id: String,
    pub stage: String,
    pub status: String,
    pub dependencies: Vec<String>,
}
pub fn role_kind(role: &str) -> &'static str {
    match role.trim_start_matches("worker-") {
        "accessibility-auditor" | "security-auditor" | "code-reviewer" => "review",
        "architect" | "database-architect" | "ux-designer" => "design",
        "senior-consultant" => "consultation",
        "accessibility-fixer" | "frontend-dev" | "mobile-dev" => "implementation_frontend",
        "devops-engineer" => "operations",
        "explorer" => "investigation",
        "technical-writer" => "writing",
        "planner" => "planning",
        "qa-engineer" | "build-verification" => "testing",
        "debugger" | "performance-engineer" => "debugging",
        _ => "implementation_backend",
    }
}

/// Classify actual work using the retained plan and stable role defaults.
pub fn node_classification<'a>(role: &str, node: &'a Value) -> (&'a str, &'a str) {
    let kind = match role.trim_start_matches("worker-") {
        "code-reviewer" => "review",
        "senior-consultant" => "consultation",
        _ => node["work_kind"].as_str().unwrap_or(role_kind(role)),
    };
    (kind, node["complexity"].as_str().unwrap_or("normal"))
}

/// Stable longest-remaining-chain first. Cycles/unknown dependencies fail closed.
pub fn dependency_ready(nodes: &[Node], stage: &str) -> CoreResult<Vec<String>> {
    dependency_ready_with_durations(nodes, stage, &BTreeMap::new())
}

fn dependency_ready_with_durations(
    nodes: &[Node],
    stage: &str,
    estimates: &BTreeMap<String, u64>,
) -> CoreResult<Vec<String>> {
    let map: BTreeMap<_, _> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    if map.len() != nodes.len() || nodes.len() > crate::lifecycle::MAX_NODES {
        return refuse("invalid_plan_graph");
    }
    let mut depth = BTreeMap::new();
    fn visit<'a>(
        id: &'a str,
        map: &BTreeMap<&'a str, &'a Node>,
        stack: &mut BTreeSet<&'a str>,
        depth: &mut BTreeMap<&'a str, usize>,
    ) -> CoreResult<usize> {
        if let Some(n) = depth.get(id) {
            return Ok(*n);
        }
        if !stack.insert(id) {
            return refuse("dependency_cycle");
        }
        for dep in &map[id].dependencies {
            if !map.contains_key(dep.as_str()) {
                return refuse("unknown_dependency");
            }
            visit(dep, map, stack, depth)?;
        }
        stack.remove(id);
        // Validate dependencies here; compute downstream priority separately.
        depth.insert(id, 1);
        Ok(1)
    }
    for n in nodes {
        visit(&n.id, &map, &mut BTreeSet::new(), &mut depth)?;
    }
    fn remaining(
        node: &Node,
        nodes: &[Node],
        memo: &mut BTreeMap<String, u64>,
        estimates: &BTreeMap<String, u64>,
        fallback: Option<u64>,
    ) -> u64 {
        if let Some(n) = memo.get(&node.id) {
            return *n;
        }
        let value = if fallback.is_some() && !unfinished(node) {
            0
        } else {
            let own = estimates.get(&node.id).copied().or(fallback).unwrap_or(1);
            let tail = nodes
                .iter()
                .filter(|n| n.dependencies.contains(&node.id))
                .map(|n| remaining(n, nodes, memo, estimates, fallback))
                .max()
                .unwrap_or(0);
            own.saturating_add(tail)
        };
        memo.insert(node.id.clone(), value);
        value
    }
    let fallback = median(estimates.values().copied().collect());
    let mut priority = BTreeMap::new();
    for n in nodes {
        remaining(n, nodes, &mut priority, estimates, fallback);
    }
    let completed: BTreeSet<_> = nodes
        .iter()
        .filter(|n| n.status == "completed")
        .map(|n| &n.id)
        .collect();
    let mut ready: Vec<_> = nodes
        .iter()
        .filter(|n| {
            n.status == "pending"
                && n.stage == stage
                && n.dependencies.iter().all(|d| completed.contains(d))
        })
        .collect();
    ready.sort_by_key(|n| std::cmp::Reverse(priority[&n.id]));
    Ok(ready.iter().map(|n| n.id.clone()).collect())
}
pub(crate) fn revision_nodes(
    db: &rusqlite::Connection,
    task: &str,
    revision: i64,
) -> CoreResult<Vec<(Node, String)>> {
    let rows=db.prepare("SELECT node_id,stage,status,dependencies_json,role FROM runtime_nodes WHERE task_id=? AND revision=? ORDER BY rowid")?
            .query_map((task,revision),|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?)))?.collect::<Result<Vec<_>,_>>()?;
    rows.into_iter()
        .map(|(id, stage, status, deps, role)| {
            Ok((
                Node {
                    id,
                    stage,
                    status,
                    dependencies: serde_json::from_str(&deps)?,
                },
                role,
            ))
        })
        .collect()
}
fn unfinished(node: &Node) -> bool {
    matches!(node.status.as_str(), "pending" | "running")
}

fn median(mut values: Vec<u64>) -> Option<u64> {
    values.sort_unstable();
    let middle = values.len() / 2;
    let upper = *values.get(middle)?;
    let lower = values[(values.len() - 1) / 2];
    Some(lower + (upper - lower) / 2)
}

fn measured_duration(
    db: &Connection,
    role: &str,
    kind: &str,
    complexity: &str,
) -> CoreResult<Option<u64>> {
    const SAMPLES: usize = 16;
    Ok(median(crate::observability::recent_node_durations(
        db, role, kind, complexity, SAMPLES,
    )?)
    .map(|ms| ms.max(1)))
}

/// The observed duration estimate of a node in the current retained revision.
pub fn node_duration(db: &Connection, task: &str, node: &str) -> CoreResult<Option<u64>> {
    let row = db
        .query_row(
            "SELECT n.role,t.plan_json FROM runtime_nodes n JOIN runtime_tasks t USING(task_id)
         WHERE n.task_id=? AND n.revision=t.revision AND n.node_id=?",
            (task, node),
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
        )
        .optional()?;
    let Some((role, plan)) = row else {
        return Ok(None);
    };
    let plan: Value = serde_json::from_str(plan.as_deref().unwrap_or("{}"))?;
    let routing = crate::lifecycle::plan::node_routing(&plan, node);
    let (kind, complexity) = node_classification(&role, &routing);
    measured_duration(db, &role, kind, complexity)
}

fn duration_estimates(
    db: &Connection,
    nodes: &[(Node, String)],
    plan: Option<&str>,
) -> CoreResult<BTreeMap<String, u64>> {
    let plan: Value = serde_json::from_str(plan.unwrap_or("{}"))?;
    let mut classes = BTreeMap::new();
    let mut estimates = BTreeMap::new();
    for (node, role) in nodes.iter().filter(|(node, _)| unfinished(node)) {
        let routing = crate::lifecycle::plan::node_routing(&plan, &node.id);
        let (kind, complexity) = node_classification(role, &routing);
        let key = (role, kind.to_owned(), complexity.to_owned());
        let estimate = match classes.get(&key) {
            Some(estimate) => *estimate,
            None => {
                let estimate = measured_duration(db, role, kind, complexity)?;
                classes.insert(key, estimate);
                estimate
            }
        };
        if let Some(ms) = estimate {
            estimates.insert(node.id.clone(), ms);
        }
    }
    Ok(estimates)
}

pub const DEFAULT_WORKER_LIMIT: u16 = 4;
pub const MAX_WORKER_LIMIT: u16 = 4;

fn bounded_worker_limit(setting: Option<&str>) -> u16 {
    setting
        .and_then(|value| value.parse::<u64>().ok())
        .map_or(DEFAULT_WORKER_LIMIT, |value| {
            value.clamp(1, u64::from(MAX_WORKER_LIMIT)) as u16
        })
}

fn worker_limit() -> i64 {
    i64::from(bounded_worker_limit(
        std::env::var("CORTEX_WORKER_LIMIT").ok().as_deref(),
    ))
}

/// Additional adapter restrictions, checked in the same transaction after the
/// mandatory NativePolicy. Returning true never bypasses native policy.
pub trait SchedulingPolicy {
    fn may_claim(&self, db: &Transaction<'_>, task: &str, node: &str) -> CoreResult<bool>;

    fn supports_assignment_worktrees(&self) -> bool {
        false
    }
}
pub fn claim_ready(
    store: &Store,
    task: &str,
    owner: &OwnerEpoch,
    channel: &str,
    policy: &dyn SchedulingPolicy,
) -> CoreResult<Vec<String>> {
    store.write(|db| claim_ready_tx(db, task, owner, channel, policy))
}
/// Native claims may be submitted to StoreExecutor::write without reopening
/// a store or running SQLite on an async reactor.
pub fn claim_ready_tx(
    db: &Transaction<'_>,
    task: &str,
    owner: &OwnerEpoch,
    channel: &str,
    policy: &dyn SchedulingPolicy,
) -> CoreResult<Vec<String>> {
    Ok(claim_ready_batch_tx(db, task, owner, channel, policy)?.assignments)
}

#[derive(Default)]
pub struct ClaimBatch {
    pub assignments: Vec<String>,
    pub measured_priority: bool,
}

/// Claim new nodes and tell the host when their dispatch needs the measured
/// priority instead of the legacy plan order.
pub fn claim_ready_batch_tx(
    db: &Transaction<'_>,
    task: &str,
    owner: &OwnerEpoch,
    channel: &str,
    policy: &dyn SchedulingPolicy,
) -> CoreResult<ClaimBatch> {
    ownership::check_tx(db, task, owner)?;
    let stopped: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM task_stopped WHERE task_id=?)",
        [task],
        |r| r.get(0),
    )?;
    if stopped {
        return Ok(ClaimBatch::default());
    }
    let (state, revision, plan): (String, i64, Option<String>) = db.query_row(
        "SELECT state,revision,plan_json FROM runtime_tasks WHERE task_id=?",
        [task],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let stage = match state.as_str() {
        "EXECUTING" => "execution",
        "REVIEWING" => "review",
        _ => return Ok(ClaimBatch::default()),
    };
    let nodes = revision_nodes(db, task, revision)?;
    let used:i64=db.query_row("SELECT COUNT(*) FROM (
            SELECT coalesce(assignment_ref,'node:'||node_id) FROM runtime_nodes WHERE task_id=? AND revision=? AND status='running'
            UNION SELECT coalesce(p.idempotency_key,'attempt:'||r.attempt_id) FROM capacity_reservations r
            LEFT JOIN delegation_provisioning p ON p.attempt_id=r.attempt_id
            WHERE r.task_id=? AND r.kind='worker' AND r.state IN ('reserved','ambiguous'))",(task,revision,task),|r|r.get(0))?;
    let mut slots = worker_limit().saturating_sub(used).max(0);
    if slots == 0 {
        return Ok(ClaimBatch::default());
    }
    let estimates = duration_estimates(db, &nodes, plan.as_deref())?;
    let nodes = nodes.into_iter().map(|(node, _)| node).collect::<Vec<_>>();
    let mut claimed = Vec::new();
    for node in dependency_ready_with_durations(&nodes, stage, &estimates)? {
        if slots == 0 {
            break;
        }
        if !policy.may_claim(db, task, &node)?
            || crate::workspace::worktrees::assignments::pending_dependencies(db, task, revision, &node)? {
            continue;
        }
        let native = self::policy::NativePolicy::default();
        let isolated = if native.may_claim(db, task, &node)? {
            false
        } else if stage == "execution" && policy.supports_assignment_worktrees()
            && native.may_claim_with_checkout(db, task, &node, true)?
            && crate::workspace::worktrees::assignments::beneficial(db, task, revision, &node)?
        {
            true
        } else {
            continue;
        };
        let retry:i64=db.query_row("SELECT count(*) FROM runtime_events WHERE task_id=? AND type='ASSIGNMENT_RETRIED'
                AND json_extract(payload_json,'$.node')=? AND json_extract(payload_json,'$.revision')=?",(task,&node,revision),|r|r.get(0))?;
        let key = format!("runtime:{revision}:{node}:{retry}");
        let assignment = crate::digest(format!("{task}|{channel}|{key}").as_bytes());
        db.execute("UPDATE runtime_nodes SET status='running',assignment_ref=? WHERE task_id=? AND revision=? AND node_id=?",
                (&assignment,task,revision,&node))?;
        event_tx(
            db,
            task,
            "ASSIGNMENT_CREATED",
            json!({"node":node,"assignment_ref":assignment,"revision":revision}),
        )?;
        if isolated {
            event_tx(db, task, "ASSIGNMENT_WORKTREE_SELECTED",
                json!({"node":node,"assignment_ref":assignment,"revision":revision}))?;
        }
        claimed.push(assignment);
        slots -= 1;
    }
    Ok(ClaimBatch {
        assignments: claimed,
        measured_priority: !estimates.is_empty(),
    })
}
/// Replace only the already-running claim whose exact provider-refused channel
/// was revoked and reaped. Keep its slot in the task's DAG; dependencies are
/// neither re-claimed nor released. Ordinary infrastructure retry budgets do
/// not count a provider's pre-work refusal.
pub fn replace_provider_refused_claim_tx(
    db: &Transaction<'_>,
    task: &str,
    owner: &OwnerEpoch,
    assignment: &str,
    channel: &str,
) -> CoreResult<String> {
    ownership::check_tx(db, task, owner)?;
    if crate::workspace::worktrees::assignments::selected(db, task, assignment)?
        || crate::workspace::worktrees::assignments::root_on(db, task, assignment)?.is_some()
    {
        return refuse("assignment_worktree_provider_replacement_unavailable");
    }
    let row: Option<(String, i64)> = db.query_row(
        "SELECT n.node_id,n.revision FROM runtime_nodes n JOIN runtime_tasks t USING(task_id)
         JOIN delegation_provisioning p ON p.task_id=n.task_id AND p.idempotency_key=n.assignment_ref
         JOIN trusted_channels c ON c.channel_id=p.channel_id AND c.attempt_id=p.attempt_id
         JOIN capacity_reservations r ON r.task_id=n.task_id AND r.attempt_id=p.attempt_id
         WHERE n.task_id=? AND n.assignment_ref=? AND c.channel_id=? AND c.owner_epoch=?
         AND c.task_id=n.task_id AND c.assignment_id=n.assignment_ref AND c.revoked_at IS NOT NULL
         AND p.status='terminated' AND r.state='released' AND r.release_observation='process_exited'
         AND n.revision=t.revision AND n.status='running' AND n.result_json IS NULL
         AND ((t.state='EXECUTING' AND n.stage='execution') OR (t.state='REVIEWING' AND n.stage='review'))
         AND NOT EXISTS(SELECT 1 FROM task_stopped s WHERE s.task_id=n.task_id)
         AND NOT EXISTS(SELECT 1 FROM runtime_requests q WHERE q.task_id=n.task_id AND q.actor=c.channel_id)
         AND EXISTS(SELECT 1 FROM runtime_events e WHERE e.task_id=n.task_id AND e.type='NATIVE_PROVIDER_ROUTE_REFUSED'
           AND json_extract(e.payload_json,'$.channel_id')=c.channel_id)",
        (task, assignment, channel, owner.epoch), |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional()?;
    let Some((node, revision)) = row else {
        return refuse("native_provider_retry_claim_changed");
    };
    let replacement = crate::digest(
        format!("{task}|{assignment}|{channel}|provider_retry").as_bytes(),
    );
    db.execute("UPDATE runtime_nodes SET assignment_ref=? WHERE task_id=? AND assignment_ref=?",
        (&replacement, task, assignment))?;
    event_tx(db, task, "NATIVE_PROVIDER_RETRY_CLAIMED", json!({
        "previous_assignment":assignment,"assignment_ref":replacement,"channel_id":channel,
        "node":node,"revision":revision
    }))?;
    event_tx(db, task, "ASSIGNMENT_CREATED",
        json!({"node":node,"assignment_ref":replacement,"revision":revision}))?;
    // The route was already authorized and is not selected again on this retry.
    let route: Option<String> = db.query_row(
        "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='NATIVE_ROUTE_SELECTED'
         AND json_extract(payload_json,'$.assignment_ref')=? ORDER BY id DESC LIMIT 1",
        (task, assignment), |row| row.get(0),
    ).optional()?;
    if let Some(raw) = route {
        let mut route: serde_json::Value = serde_json::from_str(&raw)?;
        route["assignment_ref"] = json!(replacement);
        event_tx(db, task, "NATIVE_ROUTE_SELECTED", route)?;
    }
    Ok(replacement)
}

pub fn submit_claim(
    executor: &crate::store::executor::StoreExecutor,
    task: String,
    owner: OwnerEpoch,
    channel: String,
) -> CoreResult<tokio::sync::oneshot::Receiver<CoreResult<Vec<String>>>> {
    executor.write(move |db| {
        claim_ready_tx(
            db,
            &task,
            &owner,
            &channel,
            &self::policy::NativePolicy::default(),
        )
    })
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReservationKind {
    Worker,
    Coordinator,
}
pub struct Reservation<'a> {
    pub task: &'a str,
    pub connection: &'a str,
    pub assignment: Option<&'a str>,
    pub attempt: &'a str,
    pub kind: ReservationKind,
    pub connection_limit: u16,
}
/// Capacity shared by workers of one task; ambiguous holders still occupy it.
pub fn worker_capacity_available(
    db: &Transaction<'_>,
    task: &str,
    connection: &str,
    connection_limit: u16,
) -> CoreResult<bool> {
    let (total, on_connection): (i64, i64) = db.query_row(
        "SELECT count(*),coalesce(sum(connection_id=?),0) FROM capacity_reservations
         WHERE task_id=? AND kind='worker' AND state IN ('reserved','ambiguous')",
        (connection, task),
        |v| Ok((v.get(0)?, v.get(1)?)),
    )?;
    Ok(total < worker_limit() && on_connection < i64::from(connection_limit))
}
pub fn reserve_tx(db: &Transaction<'_>, r: &Reservation<'_>) -> CoreResult<Option<String>> {
    if r.kind == ReservationKind::Worker
        && !worker_capacity_available(db, r.task, r.connection, r.connection_limit)?
    {
        return Ok(None);
    }
    let id = format!("cap_{}", uuid::Uuid::new_v4().simple());
    db.execute(
        "INSERT INTO capacity_reservations VALUES (?,?,?,?,?,?,'reserved',?,?,NULL)",
        (
            &id,
            r.connection,
            r.task,
            r.assignment,
            r.attempt,
            if r.kind == ReservationKind::Worker {
                "worker"
            } else {
                "coordinator"
            },
            crate::now(),
            crate::now(),
        ),
    )?;
    Ok(Some(id))
}
pub fn settle_tx(
    db: &Transaction<'_>,
    reservation: &str,
    observed: crate::ports::ProcessObservation,
) -> CoreResult<bool> {
    let (state, observation) = match observed {
        crate::ports::ProcessObservation::ConfirmedExited => ("released", "process_exited"),
        crate::ports::ProcessObservation::Unknown => ("ambiguous", "termination_unconfirmed"),
        crate::ports::ProcessObservation::Alive => return Ok(false),
    };
    Ok(db.execute(
        "UPDATE capacity_reservations SET state=?,updated_at=?,release_observation=?
        WHERE reservation_id=? AND state IN ('reserved','ambiguous')",
        (state, crate::now(), observation, reservation),
    )? == 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_worker_limit_stays_within_the_runtime_boundary() {
        assert_eq!(bounded_worker_limit(None), 4);
        assert_eq!(bounded_worker_limit(Some("invalid")), 4);
        assert_eq!(bounded_worker_limit(Some("0")), 1);
        assert_eq!(bounded_worker_limit(Some("2")), 2);
        assert_eq!(bounded_worker_limit(Some("64")), MAX_WORKER_LIMIT);
        assert_eq!(
            bounded_worker_limit(Some("18446744073709551615")),
            MAX_WORKER_LIMIT
        );
    }
}
