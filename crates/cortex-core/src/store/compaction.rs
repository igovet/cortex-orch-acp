//! Retention for the streamed worker activity of long-closed tasks.
//!
//! A worker streams thousands of `progress` events (`kind=worker_activity`) per attempt: thought
//! chunks, usage updates and every intermediate state of a tool call. For a task that has been
//! COMPLETED, FAILED or CANCELLED for a while, the chunks the Workspace never displays and the
//! intermediate states of a tool call that has a later terminal state carry no evidence. This pass
//! deletes exactly those rows, in small transactions, and records one marker event per task as the
//! retained summary. Receipts, results, reports, questions, state events and every terminal tool
//! state stay. No schema or format change: it uses `task_events` as it is.
use super::{outbox, Store};
use crate::{error::refuse, CoreResult};
use rusqlite::Transaction;
use serde_json::json;
use std::time::{Duration, Instant};

/// `dedup_key` of the one marker event a compacted task keeps.
pub const COMPACTION_MARKER: &str = "activity_compaction";
/// Newest `progress` events of a task that are never compacted.
pub const KEEP_LAST_EVENTS: i64 = 200;
const POLICY_VERSION: i64 = 1;
/// Tool kinds the worker panel never displays (`HIDDEN` in the Workspace worker history).
const NOISE: &str = "('agent_thought_chunk','thought','thinking','reasoning','wait','wait_agent',\
'task_notification','usage_update','current_mode_update','config_option_update',\
'available_commands_update','session_info_update','turn_started','turn_complete')";

#[derive(Debug, Clone, Copy)]
pub struct CompactionBudget {
    /// Rows deleted per transaction; one transaction holds the write lock for a few milliseconds.
    pub batch_rows: usize,
    pub max_batches: usize,
    pub max_duration: Duration,
    /// Pause between batches so foreground writers always get the lock.
    pub pause: Duration,
    /// Skip the pass when the 1-minute load average per CPU exceeds this (0 disables the check).
    pub max_load_per_cpu: f64,
}
impl Default for CompactionBudget {
    fn default() -> Self {
        Self {
            batch_rows: 2000,
            max_batches: 200,
            max_duration: Duration::from_secs(20),
            pause: Duration::from_millis(100),
            max_load_per_cpu: 0.5,
        }
    }
}
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CompactionReport {
    pub tasks_visited: usize,
    pub rows_deleted: u64,
    pub payload_bytes_deleted: u64,
    /// More eligible rows remain; a later pass continues.
    pub more: bool,
    /// Why the pass did nothing (`busy`, `load`), if it did not run.
    pub skipped: Option<&'static str>,
    pub pages_released: u64,
}

fn overloaded(limit: f64) -> bool {
    if limit <= 0.0 {
        return false;
    }
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get()) as f64;
    std::fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|s| {
            s.split_whitespace()
                .next()
                .and_then(|v| v.parse::<f64>().ok())
        })
        .is_some_and(|load| load / cpus > limit)
}

/// Rows of `task` (`?1`) older than `?2` that may be deleted.
const CANDIDATES: &str = "SELECT e.id, length(e.payload_json) FROM task_events e
 WHERE e.task_id=?1 AND e.event_type='progress' AND e.id<?2 AND json_valid(e.payload_json)
 AND json_extract(e.payload_json,'$.kind')='worker_activity'
 AND (lower(trim(coalesce(json_extract(e.payload_json,'$.tool_kind'),''))) IN NOISE
  OR (json_extract(e.payload_json,'$.status') IN ('pending','in_progress')
      AND json_extract(e.payload_json,'$.activity_ref') IS NOT NULL
      AND EXISTS(SELECT 1 FROM task_events t WHERE t.task_id=?1
          AND json_extract(t.payload_json,'$.activity_ref')=json_extract(e.payload_json,'$.activity_ref')
          AND t.id>e.id AND t.event_type='progress' AND json_valid(t.payload_json)
          AND json_extract(t.payload_json,'$.status') IN ('completed','failed'))))
 ORDER BY e.id LIMIT ?3";

impl Store {
    /// Compact the activity of closed tasks whose last change is older than `older_than_days`.
    /// Runs in small `BEGIN IMMEDIATE` transactions under the ordinary shared access lock; it never
    /// waits for a busy store, never touches a task that is not quiet, and stops at the budget.
    pub fn compact_closed_task_activity(
        &self,
        older_than_days: u64,
        budget: CompactionBudget,
    ) -> CoreResult<CompactionReport> {
        if !(1..=36_500).contains(&older_than_days) || budget.batch_rows == 0 {
            return refuse("invalid_retention");
        }
        let mut report = CompactionReport::default();
        if overloaded(budget.max_load_per_cpu) {
            report.skipped = Some("load");
            report.more = true;
            return Ok(report);
        }
        let cutoff = (chrono::Utc::now() - chrono::Duration::days(older_than_days as i64))
            .to_rfc3339_opts(chrono::SecondsFormat::Micros, false);
        let tasks: Vec<String> = self.read(|db| {
            let mut stmt = db.prepare(
                "SELECT r.task_id FROM runtime_tasks r WHERE r.state IN ('COMPLETED','FAILED','CANCELLED')
                 AND r.updated_at<? AND EXISTS(SELECT 1 FROM task_events e WHERE e.task_id=r.task_id AND e.event_type='progress')
                 ORDER BY r.updated_at",
            )?;
            let rows = stmt.query_map([&cutoff], |r| r.get(0))?.collect::<Result<_, _>>()?;
            Ok(rows)
        })?;
        let started = Instant::now();
        let mut batches = 0;
        for task in tasks {
            let mut counted = false;
            loop {
                if batches >= budget.max_batches || started.elapsed() >= budget.max_duration {
                    report.more = true;
                    return Ok(report);
                }
                if overloaded(budget.max_load_per_cpu) {
                    report.skipped = Some("load");
                    report.more = true;
                    return Ok(report);
                }
                let step =
                    match self.write(|tx| compact_batch(tx, &task, &cutoff, budget.batch_rows)) {
                        Ok(step) => step,
                        // A task that is not quiet or a store another writer holds is left for a later pass.
                        Err(crate::CoreError::Refused("active_execution")) => break,
                        Err(crate::CoreError::Refused("busy")) => {
                            report.skipped = Some("busy");
                            report.more = true;
                            return Ok(report);
                        }
                        Err(e) => return Err(e),
                    };
                batches += 1;
                report.rows_deleted += step.rows;
                report.payload_bytes_deleted += step.bytes;
                if step.visited && !counted {
                    report.tasks_visited += 1;
                    counted = true;
                }
                if step.finished {
                    break;
                }
                std::thread::sleep(budget.pause);
            }
        }
        report.pages_released = self.release_free_pages(report.rows_deleted)?;
        Ok(report)
    }

    /// Returns free pages to the file when the database was created with incremental auto-vacuum.
    /// With `auto_vacuum=NONE` (every store written so far) the pages stay on the freelist for
    /// reuse and the file shrinks only at an explicit `VACUUM`.
    fn release_free_pages(&self, deleted: u64) -> CoreResult<u64> {
        if deleted == 0 {
            return Ok(0);
        }
        self.write(|tx| {
            let mode: i64 = tx.query_row("PRAGMA auto_vacuum", [], |r| r.get(0))?;
            if mode != 2 {
                return Ok(0);
            }
            let before: i64 = tx.query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
            tx.execute_batch("PRAGMA incremental_vacuum(4000)")?;
            let after: i64 = tx.query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
            Ok((before - after).max(0) as u64)
        })
    }
}

struct Step {
    rows: u64,
    bytes: u64,
    visited: bool,
    finished: bool,
}

fn compact_batch(tx: &Transaction<'_>, task: &str, cutoff: &str, batch: usize) -> CoreResult<Step> {
    // Re-check inside the write transaction: state, age and quietness may have changed.
    let eligible: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_tasks WHERE task_id=? AND state IN ('COMPLETED','FAILED','CANCELLED') AND updated_at<?)",
        (task, cutoff),
        |r| r.get(0),
    )?;
    if !eligible {
        return refuse("active_execution");
    }
    super::require_quiet_workspace_task(tx, task)?;
    let marker: Option<(i64, i64)> = tx
        .query_row(
            "SELECT id, coalesce(json_extract(payload_json,'$.through_event_id'),0) FROM task_events WHERE task_id=? AND dedup_key=?",
            (task, COMPACTION_MARKER),
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    let newest: i64 = tx.query_row(
        "SELECT coalesce(max(id),0) FROM task_events WHERE task_id=? AND dedup_key!=?",
        (task, COMPACTION_MARKER),
        |r| r.get(0),
    )?;
    if marker.is_some_and(|(_, through)| through >= newest) {
        return Ok(Step {
            rows: 0,
            bytes: 0,
            visited: false,
            finished: true,
        });
    }
    // The newest KEEP_LAST_EVENTS progress events are never touched.
    let keep_from: i64 = tx
        .query_row(
            "SELECT id FROM task_events WHERE task_id=? AND event_type='progress' AND dedup_key!=? ORDER BY id DESC LIMIT 1 OFFSET ?",
            (task, COMPACTION_MARKER, KEEP_LAST_EVENTS - 1),
            |r| r.get(0),
        )
        .unwrap_or(0);
    let sql = CANDIDATES.replace("IN NOISE", &format!("IN {NOISE}"));
    let doomed: Vec<(i64, i64)> = tx
        .prepare(&sql)?
        .query_map((task, keep_from, batch as i64), |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?
        .collect::<Result<_, _>>()?;
    // The deleted rows publish nothing: no consumer can see a closed task's hidden chunks.
    outbox::suppress_for_transaction(tx, "runtime", task)?;
    for (id, _) in &doomed {
        tx.execute("DELETE FROM task_events WHERE id=?", [id])?;
    }
    let rows = doomed.len() as u64;
    let bytes = doomed.iter().map(|(_, n)| *n as u64).sum::<u64>();
    let finished = doomed.len() < batch;
    if finished || rows > 0 {
        let (total_rows, total_bytes) = marker
            .and_then(|(id, _)| {
                tx.query_row(
                    "SELECT coalesce(json_extract(payload_json,'$.rows_removed'),0), coalesce(json_extract(payload_json,'$.payload_bytes_removed'),0) FROM task_events WHERE id=?",
                    [id],
                    |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
                )
                .ok()
            })
            .unwrap_or((0, 0));
        let payload = json!({
            "kind":"activity_compacted","policy":POLICY_VERSION,"kept_last_events":KEEP_LAST_EVENTS,
            "rows_removed":total_rows + rows as i64,"payload_bytes_removed":total_bytes + bytes as i64,
            "through_event_id":if finished { newest } else { 0 },"compacted_at":crate::now(),
        });
        tx.execute(
            "INSERT INTO task_events(task_id,event_type,assignment_ref,dedup_key,payload_json,origin,created_at,consumed_by,superseded)
             VALUES (?1,'progress',NULL,?2,?3,'runtime',?4,'retention',0)
             ON CONFLICT(task_id,dedup_key) DO UPDATE SET payload_json=excluded.payload_json",
            (task, COMPACTION_MARKER, payload.to_string(), crate::now()),
        )?;
    }
    Ok(Step {
        rows,
        bytes,
        visited: true,
        finished,
    })
}
