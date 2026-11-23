//! Dependency selection, native scope/retry policy and durable engine capacity.
//! Adapter policy can restrict native claims, never bypass them. No engine is
//! launched here, and a timeout never releases a reservation.
use crate::{
    error::refuse,
    lifecycle::event_tx,
    ownership::{self, OwnerEpoch},
    CoreResult, Store,
};
use rusqlite::Transaction;
use serde_json::json;
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
/// Stable longest-remaining-chain first. Cycles/unknown dependencies fail closed.
pub fn dependency_ready(nodes: &[Node], stage: &str) -> CoreResult<Vec<String>> {
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
    fn remaining(id: &str, nodes: &[Node], memo: &mut BTreeMap<String, usize>) -> usize {
        if let Some(n) = memo.get(id) {
            return *n;
        }
        let value = 1 + nodes
            .iter()
            .filter(|n| n.dependencies.iter().any(|d| d == id))
            .map(|n| remaining(&n.id, nodes, memo))
            .max()
            .unwrap_or(0);
        memo.insert(id.to_owned(), value);
        value
    }
    let mut priority = BTreeMap::new();
    for n in nodes {
        remaining(&n.id, nodes, &mut priority);
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
/// Additional adapter restrictions, checked in the same transaction after the
/// mandatory NativePolicy. Returning true never bypasses native policy.
pub trait SchedulingPolicy {
    fn may_claim(&self, db: &Transaction<'_>, task: &str, node: &str) -> CoreResult<bool>;
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
    ownership::check_tx(db, task, owner)?;
    let stopped: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM task_stopped WHERE task_id=?)",
        [task],
        |r| r.get(0),
    )?;
    if stopped {
        return Ok(Vec::new());
    }
    let (state, revision): (String, i64) = db.query_row(
        "SELECT state,revision FROM runtime_tasks WHERE task_id=?",
        [task],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let stage = match state.as_str() {
        "EXECUTING" => "execution",
        "REVIEWING" => "review",
        _ => return Ok(Vec::new()),
    };
    let rows=db.prepare("SELECT node_id,stage,status,dependencies_json FROM runtime_nodes WHERE task_id=? AND revision=? ORDER BY rowid")?
            .query_map((task,revision),|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?)))?.collect::<Result<Vec<_>,_>>()?;
    let nodes = rows
        .into_iter()
        .map(|(id, stage, status, deps)| {
            Ok(Node {
                id,
                stage,
                status,
                dependencies: serde_json::from_str(&deps)?,
            })
        })
        .collect::<CoreResult<Vec<_>>>()?;
    let used:i64=db.query_row("SELECT COUNT(*) FROM (
            SELECT coalesce(assignment_ref,'node:'||node_id) FROM runtime_nodes WHERE task_id=? AND revision=? AND status='running'
            UNION SELECT coalesce(p.idempotency_key,'attempt:'||r.attempt_id) FROM capacity_reservations r
            LEFT JOIN delegation_provisioning p ON p.attempt_id=r.attempt_id
            WHERE r.task_id=? AND r.kind='worker' AND r.state IN ('reserved','ambiguous'))",(task,revision,task),|r|r.get(0))?;
    let mut slots = (4 - used).max(0);
    let mut claimed = Vec::new();
    for node in dependency_ready(&nodes, stage)? {
        if slots == 0 {
            break;
        }
        if !self::policy::NativePolicy::default().may_claim(db, task, &node)?
            || !policy.may_claim(db, task, &node)?
        {
            continue;
        }
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
        claimed.push(assignment);
        slots -= 1;
    }
    Ok(claimed)
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
    use rusqlite::OptionalExtension;
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
    Ok(total < 4 && on_connection < i64::from(connection_limit))
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
