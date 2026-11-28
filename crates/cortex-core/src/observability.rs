//! Private timing facts. A duration always names the operation or event pair
//! that supplied its boundaries. Concurrent intervals can overlap.
use crate::{error::refuse, CoreResult, Store};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::Serialize;
use serde_json::{json, Value};
use std::{collections::BTreeMap, time::Duration};

#[derive(Clone, Copy)]
pub enum Category {
    EngineLaunch,
    CapacityWait,
    HandoffPreparation,
    PlanningTurn,
    WorkerTurn,
    ReviewTurn,
    ReconciliationTurn,
    VerificationRun,
}

impl Category {
    fn key(self) -> &'static str {
        match self {
            Self::EngineLaunch => "engine_launch",
            Self::CapacityWait => "capacity_wait",
            Self::HandoffPreparation => "handoff_preparation",
            Self::PlanningTurn => "planning_turn",
            Self::WorkerTurn => "worker_turn",
            Self::ReviewTurn => "review_turn",
            Self::ReconciliationTurn => "reconciliation_turn",
            Self::VerificationRun => "verification_run",
        }
    }
}

#[derive(Clone, Copy)]
pub enum Outcome {
    Returned,
    Error,
    Waiting,
}

impl Outcome {
    fn key(self) -> &'static str {
        match self {
            Self::Returned => "returned",
            Self::Error => "error",
            Self::Waiting => "waiting",
        }
    }
}

pub enum Scope {
    Task,
    Assignment(String),
    Channel(String),
}

/// Trusted runtime writer. Only fixed categories/outcomes and integer elapsed
/// milliseconds are accepted. An assignment or attempt must belong to the task.
pub fn record_tx(
    db: &Transaction<'_>,
    task: &str,
    scope: &Scope,
    category: Category,
    outcome: Outcome,
    elapsed: Duration,
) -> CoreResult<()> {
    let (assignment, attempt) = match scope {
        Scope::Task => (None, None),
        Scope::Channel(channel) => db.query_row(
            "SELECT assignment_id,attempt_id FROM trusted_channels WHERE task_id=? AND channel_id=? AND attempt_id IS NOT NULL",
            (task, channel),
            |r| Ok((r.get::<_, Option<String>>(0)?, Some(r.get::<_, String>(1)?))),
        )?,
        Scope::Assignment(assignment) => (Some(db.query_row(
            "SELECT assignment_ref FROM runtime_nodes WHERE task_id=?1 AND assignment_ref=?2
             UNION SELECT json_extract(payload_json,'$.assignment_ref') FROM runtime_events
             WHERE task_id=?1 AND type='ASSIGNMENT_CREATED' AND json_extract(payload_json,'$.assignment_ref')=?2",
            (task, assignment), |row| row.get::<_, String>(0),
        )?), None),
    };
    let ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
    db.execute(
        "INSERT INTO observability_events(id,task_id,category,outcome,basis,assignment_ref,attempt_id,detail_json,created_at)
         VALUES (?,?,?,?,'observed',?,?,?,?)",
        (
            uuid::Uuid::new_v4().simple().to_string(),
            task,
            category.key(),
            outcome.key(),
            assignment,
            attempt,
            json!({"duration_ms":ms}).to_string(),
            crate::now(),
        ),
    )?;
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct TaskTimings {
    pub task_id: String,
    pub state: String,
    pub phase_totals_ms: BTreeMap<String, u64>,
    pub review_cycles_by_risk: BTreeMap<String, u64>,
    pub intervals: Vec<Timing>,
}

#[derive(Debug, Serialize)]
pub struct Timing {
    pub category: String,
    pub basis: &'static str,
    pub duration_ms: u64,
    pub outcome: String,
    pub ended_at: String,
    pub assignment_ref: Option<String>,
    pub node_id: Option<String>,
    pub revision: Option<i64>,
    pub attempt_id: Option<String>,
    pub source: Vec<String>,
}

fn elapsed(start: &str, end: &str) -> Option<u64> {
    let start = chrono::DateTime::parse_from_rfc3339(start).ok()?;
    let end = chrono::DateTime::parse_from_rfc3339(end).ok()?;
    (end >= start).then(|| (end - start).num_milliseconds() as u64)
}

fn derived(
    category: &str,
    outcome: &str,
    begin: (String, String),
    end: (String, String),
) -> Option<Timing> {
    Some(Timing {
        category: category.into(),
        basis: "derived",
        duration_ms: elapsed(&begin.0, &end.0)?,
        outcome: outcome.into(),
        ended_at: end.0,
        assignment_ref: None,
        node_id: None,
        revision: None,
        attempt_id: None,
        source: vec![begin.1, end.1],
    })
}

const CLAIMS: &str = "
SELECT DISTINCT json_extract(payload_json,'$.assignment_ref') AS assignment,
       json_extract(payload_json,'$.node') AS node, json_extract(payload_json,'$.revision') AS revision
FROM runtime_events WHERE task_id=?1 AND type='ASSIGNMENT_CREATED'";

const EXACT_RESULT: &str = "json_extract(f.payload_json,'$.assignment_ref')=json_extract(d.payload_json,'$.assignment_ref')
 AND json_extract(f.payload_json,'$.attempt_id')=json_extract(d.payload_json,'$.attempt')";

/// Consistent snapshot of one exact retained execution, including prior revisions.
pub fn task_timings(store: &Store, task: &str) -> CoreResult<TaskTimings> {
    store.read_transaction(|db| task_timings_tx(db, task))
}

fn task_timings_tx(db: &Connection, task: &str) -> CoreResult<TaskTimings> {
    let Some(state) = db
        .query_row(
            "SELECT state FROM runtime_tasks WHERE task_id=?",
            [task],
            |r| r.get(0),
        )
        .optional()?
    else {
        return refuse("timing_task_missing");
    };
    let mut intervals = Vec::new();
    let mut claims = BTreeMap::new();
    let mut q = db.prepare(CLAIMS)?;
    for row in q.query_map([task], |r| {
        Ok((
            r.get::<_, Option<String>>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, Option<i64>>(2)?,
        ))
    })? {
        let (assignment, node, revision) = row?;
        if let (Some(assignment), Some(node), Some(revision)) = (assignment, node, revision) {
            claims
                .entry(assignment)
                .and_modify(|prior| *prior = None)
                .or_insert(Some((node, revision)));
        }
    }
    let mut q = db.prepare(
        "SELECT id, category, outcome, created_at, assignment_ref, attempt_id, detail_json
         FROM observability_events WHERE task_id=? AND basis='observed'
         AND category IN ('engine_launch','capacity_wait','handoff_preparation','planning_turn','worker_turn','review_turn','reconciliation_turn','verification_run')
         AND outcome IN ('returned','error','waiting')",
    )?;
    let mut rows = q.query([task])?;
    while let Some(r) = rows.next()? {
        let raw: String = r.get(6)?;
        let Some(ms) = serde_json::from_str::<Value>(&raw)
            .ok()
            .and_then(|v| v["duration_ms"].as_u64())
        else {
            continue;
        };
        intervals.push(Timing {
            category: r.get(1)?,
            outcome: r.get(2)?,
            ended_at: r.get(3)?,
            assignment_ref: r.get(4)?,
            attempt_id: r.get(5)?,
            source: vec![format!("observation:{}", r.get::<_, String>(0)?)],
            basis: "observed",
            duration_ms: ms,
            node_id: None,
            revision: None,
        });
    }
    phase_intervals(db, task, &mut intervals)?;
    worker_intervals(db, task, &mut intervals)?;
    handoff_intervals(db, task, &mut intervals)?;
    intervals.sort_by(|a, b| (&a.ended_at, &a.source).cmp(&(&b.ended_at, &b.source)));
    let mut totals = BTreeMap::<String, u64>::new();
    for interval in &mut intervals {
        if let Some(Some((node, revision))) =
            interval.assignment_ref.as_ref().and_then(|a| claims.get(a))
        {
            interval.node_id = Some(node.clone());
            interval.revision = Some(*revision);
        }
        let total = totals.entry(interval.category.clone()).or_default();
        *total = total.saturating_add(interval.duration_ms);
    }
    Ok(TaskTimings {
        task_id: task.into(),
        state,
        phase_totals_ms: totals,
        review_cycles_by_risk: review_cycles_by_risk(db, task)?,
        intervals,
    })
}

fn review_cycles_by_risk(db: &Connection, task: &str) -> CoreResult<BTreeMap<String, u64>> {
    let mut query = db.prepare(
        "WITH assignments AS (
            SELECT min(id) AS first_id FROM runtime_events WHERE task_id=?1 AND type='REVIEW_STARTED'
            AND json_type(payload_json,'$.assignment_ref')='text'
            GROUP BY json_extract(payload_json,'$.assignment_ref')
         ), cycles AS (
            SELECT min(e.id) AS first_id FROM assignments a JOIN runtime_events e ON e.id=a.first_id
            GROUP BY CASE WHEN json_type(e.payload_json,'$.revision')='integer' AND json_type(e.payload_json,'$.node')='text'
              THEN json_array('node',json_extract(e.payload_json,'$.revision'),json_extract(e.payload_json,'$.node'))
              ELSE json_array('assignment',json_extract(e.payload_json,'$.assignment_ref')) END
         )
         SELECT json_extract(e.payload_json,'$.risk') FROM cycles c
         JOIN runtime_events e ON e.id=c.first_id",
    )?;
    let mut counts = BTreeMap::new();
    for risk in query.query_map([task], |row| row.get::<_, Option<String>>(0))? {
        let risk = risk?;
        let key = match risk.as_deref() {
            Some(known @ ("low" | "standard" | "high")) => known,
            _ => "unobserved",
        };
        *counts.entry(key.to_owned()).or_default() += 1;
    }
    Ok(counts)
}

fn phase_intervals(db: &Connection, task: &str, intervals: &mut Vec<Timing>) -> CoreResult<()> {
    let mut q = db.prepare(
        "SELECT id, created_at, json_extract(payload_json,'$.previous'), json_extract(payload_json,'$.state')
         FROM runtime_events WHERE task_id=? AND type='TASK_STATE_CHANGED' ORDER BY id",
    )?;
    let mut rows = q.query([task])?;
    let mut previous: Option<(String, (String, String))> = None;
    while let Some(r) = rows.next()? {
        let boundary = (
            r.get::<_, String>(1)?,
            format!("runtime:{}", r.get::<_, i64>(0)?),
        );
        let from: Option<String> = r.get(2)?;
        let to: Option<String> = r.get(3)?;
        if from.is_some() && from == to {
            continue;
        }
        if let Some((prior, begin)) = previous.take().filter(|(s, _)| Some(s) == from.as_ref()) {
            let category = match prior.as_str() {
                "PLANNING" => Some("planning"),
                "EXECUTING" => Some("execution"),
                "REVIEWING" => Some("reviewing"),
                "VERIFYING" => Some("verification"),
                "RECONCILING" => Some("reconciliation"),
                _ => None,
            };
            if let (Some(category), Some(to)) = (category, &to) {
                intervals.extend(derived(category, to, begin, boundary.clone()));
            }
        }
        previous = to.map(|state| (state, boundary));
    }
    Ok(())
}

fn worker_intervals(db: &Connection, task: &str, intervals: &mut Vec<Timing>) -> CoreResult<()> {
    // The delivery is the first for this exact attempt. Route audit is joined by
    // task and attempt, and its recorded decision's source is checked separately.
    let sql = format!(
        "SELECT d.id,d.created_at,f.id,f.created_at,f.type,
                json_extract(d.payload_json,'$.assignment_ref'),json_extract(d.payload_json,'$.attempt')
         FROM runtime_events d JOIN runtime_events f ON f.task_id=d.task_id AND f.id>d.id AND {EXACT_RESULT}
         WHERE d.task_id=?1 AND d.type='NATIVE_ATTEMPT_DELIVERY'
           AND json_extract(d.payload_json,'$.first')=1
           AND f.type IN ('WORKER_COMPLETED','WORKER_FAILED')
         GROUP BY json_extract(d.payload_json,'$.attempt') HAVING count(*)=1"
    );
    let mut q = db.prepare(&sql)?;
    let mut rows = q.query([task])?;
    while let Some(r) = rows.next()? {
        let kind: String = r.get(4)?;
        if let Some(mut timing) = derived("worker", &kind, boundary(r, 0, 1)?, boundary(r, 2, 3)?) {
            timing.assignment_ref = r.get(5)?;
            timing.attempt_id = r.get(6)?;
            intervals.push(timing);
        }
    }
    let mut q = db.prepare(
        "SELECT s.id,s.created_at,f.id,f.created_at,f.type,json_extract(f.payload_json,'$.assignment_ref'),
                json_extract(f.payload_json,'$.attempt_id')
         FROM runtime_events s JOIN runtime_events f ON f.task_id=s.task_id AND f.id>s.id
           AND json_extract(f.payload_json,'$.assignment_ref')=json_extract(s.payload_json,'$.assignment_ref')
         WHERE s.task_id=?1 AND s.type IN ('REVIEW_STARTED','NATIVE_WORKER_CAPACITY_WAITING')
           AND ((s.type='REVIEW_STARTED' AND f.type IN ('WORKER_COMPLETED','WORKER_FAILED'))
             OR (s.type='NATIVE_WORKER_CAPACITY_WAITING' AND f.type='NATIVE_WORKER_CAPACITY_WAIT_ENDED'))
           AND (s.type='NATIVE_WORKER_CAPACITY_WAITING' OR EXISTS(
             SELECT 1 FROM runtime_events d WHERE d.task_id=s.task_id AND d.type='NATIVE_ATTEMPT_DELIVERY'
               AND d.id<f.id AND json_extract(d.payload_json,'$.first')=1
               AND json_extract(d.payload_json,'$.assignment_ref')=json_extract(f.payload_json,'$.assignment_ref')
               AND json_extract(d.payload_json,'$.attempt')=json_extract(f.payload_json,'$.attempt_id')))
           AND s.id=(SELECT max(old.id) FROM runtime_events old WHERE old.task_id=s.task_id
             AND old.type=s.type AND old.id<f.id AND json_extract(old.payload_json,'$.assignment_ref')=json_extract(s.payload_json,'$.assignment_ref'))
           AND f.id=(SELECT min(new.id) FROM runtime_events new WHERE new.task_id=f.task_id
             AND new.type=f.type AND new.id>s.id AND json_extract(new.payload_json,'$.assignment_ref')=json_extract(f.payload_json,'$.assignment_ref'))",
    )?;
    let mut rows = q.query([task])?;
    while let Some(r) = rows.next()? {
        let kind: String = r.get(4)?;
        let category = if kind == "NATIVE_WORKER_CAPACITY_WAIT_ENDED" {
            "slot_wait"
        } else {
            "review_cycle"
        };
        if let Some(mut timing) = derived(category, &kind, boundary(r, 0, 1)?, boundary(r, 2, 3)?) {
            timing.assignment_ref = r.get(5)?;
            timing.attempt_id = r.get(6)?;
            intervals.push(timing);
        }
    }
    let mut q = db.prepare(
        "SELECT e.id,e.created_at,a.id,a.recorded_at,json_extract(d.payload_json,'$.assignment_ref'),a.attempt_id
         FROM attempt_route_audit a JOIN runtime_events d ON d.task_id=a.task_id AND d.type='NATIVE_ATTEMPT_DELIVERY'
           AND json_extract(d.payload_json,'$.attempt')=a.attempt_id AND json_extract(d.payload_json,'$.first')=1
         JOIN runtime_events e ON e.task_id=a.task_id AND e.type='NATIVE_ROUTE_SELECTED' AND e.id<d.id
           AND e.id=json_extract(d.payload_json,'$.route_event') AND e.id=json_extract(a.decision_detail_json,'$.route_event')
           AND json_extract(e.payload_json,'$.assignment_ref')=json_extract(d.payload_json,'$.assignment_ref')
         WHERE a.task_id=? AND json_extract(a.decision_detail_json,'$.source')='NATIVE_ROUTE_SELECTED'
           AND a.recorded_at=json_extract(d.payload_json,'$.delivered_at')",
    )?;
    let mut rows = q.query([task])?;
    while let Some(r) = rows.next()? {
        if let Some(mut timing) = derived(
            "dispatch_to_delivery",
            "recorded",
            boundary(r, 0, 1)?,
            (r.get(3)?, format!("route_audit:{}", r.get::<_, String>(2)?)),
        ) {
            timing.assignment_ref = r.get(4)?;
            timing.attempt_id = r.get(5)?;
            intervals.push(timing);
        }
    }
    Ok(())
}

// Handoff latency follows an explicit graph edge. The latest matching result
// must predate the consumer's claim, so a later retry cannot become its source.
fn handoff_intervals(db: &Connection, task: &str, intervals: &mut Vec<Timing>) -> CoreResult<()> {
    let mut query = db.prepare(
        "WITH claims AS MATERIALIZED (
            SELECT id,json_extract(payload_json,'$.assignment_ref') AS assignment,
                   json_extract(payload_json,'$.node') AS node,
                   json_extract(payload_json,'$.revision') AS revision
            FROM runtime_events WHERE task_id=?1 AND type='ASSIGNMENT_CREATED'
         )
         SELECT f.id,f.created_at,d.id,d.created_at,c.assignment,json_extract(d.payload_json,'$.attempt')
         FROM claims c JOIN runtime_nodes n ON n.task_id=?1 AND n.revision=c.revision AND n.node_id=c.node
         JOIN json_each(n.dependencies_json) dep
         JOIN runtime_events d ON d.task_id=n.task_id AND d.type='NATIVE_ATTEMPT_DELIVERY'
           AND json_extract(d.payload_json,'$.first')=1 AND d.id>c.id
           AND json_extract(d.payload_json,'$.assignment_ref')=c.assignment
         JOIN runtime_events f ON f.id=(
           SELECT max(e.id) FROM claims p JOIN runtime_events e ON e.task_id=n.task_id
             AND e.type='WORKER_COMPLETED' AND e.id>p.id AND e.id<c.id
             AND json_extract(e.payload_json,'$.assignment_ref')=p.assignment
             AND EXISTS(SELECT 1 FROM runtime_events sent WHERE sent.task_id=e.task_id
               AND sent.type='NATIVE_ATTEMPT_DELIVERY' AND sent.id<e.id
               AND json_extract(sent.payload_json,'$.first')=1
               AND json_extract(sent.payload_json,'$.attempt')=json_extract(e.payload_json,'$.attempt_id')
               AND json_extract(sent.payload_json,'$.assignment_ref')=p.assignment)
           WHERE p.revision=c.revision AND p.node=dep.value
         )",
    )?;
    let mut rows = query.query([task])?;
    while let Some(r) = rows.next()? {
        if let Some(mut timing) = derived(
            "dependency_handoff",
            "recorded",
            boundary(r, 0, 1)?,
            boundary(r, 2, 3)?,
        ) {
            timing.assignment_ref = r.get(4)?;
            timing.attempt_id = r.get(5)?;
            intervals.push(timing);
        }
    }
    Ok(())
}

fn boundary(r: &rusqlite::Row<'_>, id: usize, at: usize) -> rusqlite::Result<(String, String)> {
    Ok((r.get(at)?, format!("runtime:{}", r.get::<_, i64>(id)?)))
}

/// Recent exact worker start-to-result samples for this route classification.
/// Inspect at most the last 512 retained successful worker results globally.
// SQLite must drive this join from the bounded result window. CROSS JOIN keeps
// query planning from choosing a scan of all historical deliveries.
pub fn recent_node_durations(
    db: &Connection,
    role: &str,
    work_kind: &str,
    complexity: &str,
    limit: usize,
) -> CoreResult<Vec<u64>> {
    let sql = format!(
        "WITH recent AS MATERIALIZED (
            SELECT task_id,id,created_at,payload_json FROM runtime_events
            WHERE type='WORKER_COMPLETED' ORDER BY id DESC LIMIT 512
         )
         SELECT d.created_at,f.created_at FROM recent f
         CROSS JOIN runtime_events d ON d.task_id=f.task_id AND d.type='NATIVE_ATTEMPT_DELIVERY'
           AND d.id<f.id AND json_extract(d.payload_json,'$.first')=1 AND {EXACT_RESULT}
         CROSS JOIN attempt_route_audit a ON a.task_id=f.task_id AND a.attempt_id=json_extract(d.payload_json,'$.attempt')
           AND a.recorded_at=json_extract(d.payload_json,'$.delivered_at')
         CROSS JOIN runtime_events e ON e.id=json_extract(d.payload_json,'$.route_event')
           AND e.id=json_extract(a.decision_detail_json,'$.route_event')
           AND e.task_id=f.task_id AND e.type='NATIVE_ROUTE_SELECTED' AND e.id<d.id
           AND json_extract(e.payload_json,'$.assignment_ref')=json_extract(d.payload_json,'$.assignment_ref')
         WHERE a.role=?1 AND json_extract(e.payload_json,'$.timing_work_kind')=?2
           AND json_extract(e.payload_json,'$.timing_complexity')=?3
         GROUP BY f.task_id,json_extract(f.payload_json,'$.attempt_id') HAVING count(*)=1
         ORDER BY f.id DESC"
    );
    let mut q = db.prepare(&sql)?;
    let mut rows = q.query(params![role, work_kind, complexity])?;
    let mut out = Vec::new();
    while out.len() < limit.min(128) {
        let Some(r) = rows.next()? else { break };
        let start: String = r.get(0)?;
        let end: String = r.get(1)?;
        out.extend(elapsed(&start, &end).filter(|ms| *ms > 0));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{lifecycle::TaskMachine, OpenMode, ProjectKey, StoreChannel};

    struct Fixture(std::path::PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn timing_requires_exact_attempts_and_preserves_retained_node_history() {
        let fixture = Fixture(std::env::temp_dir().join(uuid::Uuid::new_v4().to_string()));
        std::fs::create_dir(&fixture.0).unwrap();
        let store = Store::open(
            ProjectKey::new(&fixture.0, StoreChannel::Dev).unwrap(),
            OpenMode::Initialize,
        )
        .unwrap();
        TaskMachine::new(&store)
            .create("timed", "private objective", "owner")
            .unwrap();
        store.write(|db| {
            db.execute("UPDATE runtime_tasks SET state='COMPLETED' WHERE task_id='timed'", [])?;
            let event = |kind: &str, at: &str, payload: Value| -> CoreResult<i64> {
                db.execute("INSERT INTO runtime_events(task_id,type,actor,payload_json,created_at) VALUES
                    ('timed',?,'runtime',?,?)", (kind, payload.to_string(), format!("2026-11-20T10:00:{at}+00:00")))?;
                Ok(db.last_insert_rowid())
            };
            event("TASK_STATE_CHANGED", "00", json!({"previous":"CREATED","state":"PLANNING"}))?;
            event("TASK_STATE_CHANGED", "01", json!({"previous":"PLANNING","state":"EXECUTING"}))?;
            event("ASSIGNMENT_CREATED", "02", json!({"assignment_ref":"a","node":"n","revision":1}))?;
            event("NATIVE_WORKER_CAPACITY_WAITING", "03", json!({"assignment_ref":"a"}))?;
            event("NATIVE_WORKER_CAPACITY_WAIT_ENDED", "04", json!({"assignment_ref":"a"}))?;
            let route = event("NATIVE_ROUTE_SELECTED", "05", json!({"assignment_ref":"a",
                "timing_work_kind":"implementation_backend","timing_complexity":"normal"}))?;
            let delivered = "2026-11-20T10:00:06+00:00";
            event("NATIVE_ATTEMPT_DELIVERY", "06", json!({"assignment_ref":"a","attempt":"x","first":true,
                "delivered_at":delivered,"route_event":route}))?;
            event("WORKER_COMPLETED", "07", json!({"assignment_ref":"a","attempt_id":"wrong","result":"private"}))?;
            event("WORKER_COMPLETED", "08", json!({"assignment_ref":"a","attempt_id":"x","result":"private"}))?;
            db.execute("INSERT INTO runtime_nodes VALUES
                ('timed',1,'downstream','worker-code-reviewer','private','[\"n\"]','[]','review','completed','b',NULL)", [])?;
            event("ASSIGNMENT_CREATED", "09", json!({"assignment_ref":"b","node":"downstream","revision":1}))?;
            event("REVIEW_STARTED", "10", json!({"assignment_ref":"b","node":"downstream","revision":1,"risk":"high"}))?;
            event("NATIVE_ATTEMPT_DELIVERY", "11", json!({"assignment_ref":"b","attempt":"review","first":true}))?;
            event("REVIEW_STARTED", "12", json!({"assignment_ref":"b","risk":"low"}))?;
            event("WORKER_COMPLETED", "13", json!({"assignment_ref":"b","attempt_id":"review","result":"private"}))?;
            event("REVIEW_STARTED", "14", json!({"assignment_ref":"legacy"}))?;
            event("REVIEW_STARTED", "15", json!({"assignment_ref":"b-retry","node":"downstream","revision":1,"risk":"high"}))?;
            db.execute("INSERT INTO attempt_route_audit
                (id,attempt_id,task_id,role,engine,connection_id,model,params_fingerprint,decision,decision_detail_json,recorded_at)
                VALUES ('route','x','timed','worker-backend-dev','engine','connection','model','digest','authorized',?,?)",
                (json!({"source":"NATIVE_ROUTE_SELECTED","route_event":route}).to_string(), delivered))?;
            event("TASK_STATE_CHANGED", "09", json!({"previous":"EXECUTING","state":"COMPLETED"}))?;
            record_tx(db, "timed", &Scope::Task, Category::VerificationRun, Outcome::Returned, Duration::from_millis(75))?;
            Ok(())
        }).unwrap();
        let timings = task_timings(&store, "timed").unwrap();
        assert_eq!(timings.state, "COMPLETED");
        assert_eq!(timings.phase_totals_ms["planning"], 1_000);
        assert_eq!(timings.phase_totals_ms["execution"], 8_000);
        assert_eq!(timings.phase_totals_ms["slot_wait"], 1_000);
        assert_eq!(timings.phase_totals_ms["dispatch_to_delivery"], 1_000);
        assert_eq!(timings.phase_totals_ms["dependency_handoff"], 3_000);
        assert_eq!(timings.phase_totals_ms["review_cycle"], 1_000);
        assert_eq!(
            timings.review_cycles_by_risk,
            BTreeMap::from([("high".into(), 1), ("unobserved".into(), 1)])
        );
        assert_eq!(timings.phase_totals_ms["verification_run"], 75);
        let workers: Vec<_> = timings
            .intervals
            .iter()
            .filter(|i| i.category == "worker" && i.assignment_ref.as_deref() == Some("a"))
            .collect();
        assert_eq!(workers.len(), 1);
        assert_eq!(workers[0].node_id.as_deref(), Some("n"));
        assert_eq!(workers[0].attempt_id.as_deref(), Some("x"));
        assert_eq!(workers[0].duration_ms, 2_000);
        assert!(!serde_json::to_string(&timings).unwrap().contains("private"));
        store
            .read(|db| {
                let read = |role, complexity| {
                    recent_node_durations(db, role, "implementation_backend", complexity, 10)
                };
                assert_eq!(read("worker-backend-dev", "normal")?, [2_000]);
                assert!(read("worker-backend-dev", "high")?.is_empty());
                Ok(())
            })
            .unwrap();
        assert_eq!(
            task_timings(&store, "missing").unwrap_err().code(),
            "timing_task_missing"
        );
    }
}
