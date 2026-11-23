//! A task's progress and spend, read from the runtime's own records only.
//!
//! **Progress** is a recorded result the task moved forward with. When the runtime records
//! `TASK_PROGRESS` events for a task, those alone count from the first one on; before that
//! (and for a store that has none) a progress is an accepted plan (`PLAN_ACCEPTED`), a
//! worker result that is not a reviewer's refusal (`WORKER_COMPLETED`), an approving review
//! (`TASK_STATE_CHANGED` from `REVIEWING` to `VERIFYING`), passed verification
//! (`VERIFICATION_PASSED`) and a passed check (`WORKER_CHECK_FINISHED`,
//! `CHECK_EXECUTION_FINISHED`). The no-progress clock starts at the newest progress, or at the
//! newest moment the task started or resumed working (it entered a working state, the owner
//! answered, steered, continued or reopened it, or the service resumed it): time spent paused
//! or waiting for the owner is never counted as spinning.
//!
//! **Spend** is what the engines actually reported: the time the task spent in a working
//! state, the sessions and prompts by confirmed engine and model, and the tokens of
//! prompts whose final engine report is stored in `usage_records`. A prompt without
//! a report is never counted as zero and no figure is estimated.
use cortex_core::CoreResult;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Distinguishes current prompt rows from historical usage records.
pub(crate) const NATIVE_USAGE_PREFIX: &str = "native_usage:";

/// Runtime states in which a task works by itself.
pub(crate) const WORKING: [&str; 6] = [
    "PLANNING",
    "EXECUTING",
    "REVIEWING",
    "VERIFYING",
    "RECONCILING",
    "RECOVERING",
];
/// The ACP `usage` fields kept, as the installed adapters name them.
const TOKEN_FIELDS: [&str; 6] = [
    "inputTokens",
    "outputTokens",
    "cachedReadTokens",
    "cachedWriteTokens",
    "thoughtTokens",
    "totalTokens",
];
/// The event types this module reads; everything else is left in the store.
const READ: &[&str] = &[
    "TASK_PROGRESS",
    "PLAN_ACCEPTED",
    "WORKER_COMPLETED",
    "VERIFICATION_PASSED",
    "TASK_STATE_CHANGED",
    "WORKER_CHECK_FINISHED",
    "CHECK_EXECUTION_FINISHED",
    "TASK_REOPENED",
    "ANSWER_SUBMITTED",
    "USER_STEERING",
    "NATIVE_OWNER_CONTROL",
    "NATIVE_RECOVERY_RESUMED",
    "NATIVE_TASK_CONTINUED",
    "WORKER_FAILED",
    "WORKER_RESULT_REFUSED",
    "VERIFICATION_FAILED",
    "REVIEW_CHANGES_REQUESTED",
    "REVIEW_DEAD_END_ROUTED",
    "REVIEW_NO_PROGRESS",
    "NATIVE_TURN_DEGENERATE",
    "NATIVE_SESSION_START_FAILED",
    "NATIVE_WORKER_ROUTE_BLOCKED",
    "TURN_INTERRUPTED",
    "NATIVE_TURN_SETTLED",
    "NATIVE_SESSION_CONFIGURED",
];

/// One runtime event, its time in Unix milliseconds.
#[derive(Clone, Debug)]
pub(crate) struct Event {
    pub kind: String,
    pub payload: Value,
    pub at: i64,
}

fn passed_checks(p: &Value) -> bool {
    let ok = |c: &Value| c["exit_code"].as_i64() == Some(0) && c["complete"] == true;
    let clear = p["blocker"].is_null() || p["blocker"] == false;
    match p["checks"].as_array() {
        Some(checks) => clear && !checks.is_empty() && checks.iter().all(ok),
        None => clear && ok(p),
    }
}
/// A progress the runtime recorded before it recorded `TASK_PROGRESS`.
fn earlier_progress(e: &Event) -> bool {
    match e.kind.as_str() {
        "PLAN_ACCEPTED" | "VERIFICATION_PASSED" => true,
        // A reviewer's result that asks for changes or stops the review moved nothing forward.
        "WORKER_COMPLETED" => !matches!(
            e.payload["result"]["verdict"].as_str(),
            Some("REQUEST_CHANGES" | "INCOMPLETE" | "BLOCK")
        ),
        "TASK_STATE_CHANGED" => {
            e.payload["previous"] == "REVIEWING" && e.payload["state"] == "VERIFYING"
        }
        "WORKER_CHECK_FINISHED" | "CHECK_EXECUTION_FINISHED" => passed_checks(&e.payload),
        _ => false,
    }
}
fn working(state: &str) -> bool {
    WORKING.contains(&state)
}
/// The task started or resumed working, or the owner acted on it.
fn resumed(e: &Event) -> bool {
    match e.kind.as_str() {
        "TASK_STATE_CHANGED" => {
            e.payload["state"].as_str().is_some_and(working)
                && !e.payload["previous"].as_str().is_some_and(working)
        }
        "TASK_REOPENED"
        | "ANSWER_SUBMITTED"
        | "USER_STEERING"
        | "NATIVE_OWNER_CONTROL"
        | "NATIVE_RECOVERY_RESUMED"
        | "NATIVE_TASK_CONTINUED" => true,
        _ => false,
    }
}

/// The newest progress: `TASK_PROGRESS` from its first record on, the earlier kinds before it.
pub(crate) fn last_progress(events: &[Event]) -> Option<i64> {
    let switched = events
        .iter()
        .filter(|e| e.kind == "TASK_PROGRESS")
        .map(|e| e.at)
        .min();
    events
        .iter()
        .filter(|e| match switched {
            Some(from) => e.kind == "TASK_PROGRESS" || e.at < from && earlier_progress(e),
            None => earlier_progress(e),
        })
        .map(|e| e.at)
        .max()
}
/// Where the no-progress clock starts: the newest progress or (re)start of work.
pub(crate) fn stalled_since(events: &[Event]) -> Option<i64> {
    let resume = events.iter().filter(|e| resumed(e)).map(|e| e.at).max();
    last_progress(events)
        .max(resume)
        .or_else(|| events.iter().map(|e| e.at).min())
}
/// Milliseconds spent in a working state, and since when it works now (if it does).
pub(crate) fn working_ms(events: &[Event], now: i64) -> (i64, Option<i64>) {
    let mut total = 0;
    let mut since: Option<i64> = None;
    for e in events.iter().filter(|e| e.kind == "TASK_STATE_CHANGED") {
        if let Some(start) = since.take() {
            total += (e.at - start).max(0);
        }
        if e.payload["state"].as_str().is_some_and(working) {
            since = Some(e.at);
        }
    }
    if let Some(start) = since {
        total += (now - start).max(0);
    }
    (total, since)
}

/// A recorded code, spelled as words; anything else is not shown.
fn spelled(code: &Value) -> Option<String> {
    code.as_str()
        .filter(|c| {
            !c.is_empty()
                && c.len() <= 64
                && c.chars()
                    .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
        })
        .map(|c| c.replace('_', " "))
}
/// What one failed or dead-ended attempt hit, in a few plain words. Never model text.
fn reason(e: &Event) -> Option<String> {
    let p = &e.payload;
    Some(match e.kind.as_str() {
        "WORKER_FAILED" => "a worker failed".into(),
        "WORKER_RESULT_REFUSED" => match spelled(&p["code"]) {
            Some(code) => format!("a result was refused ({code})"),
            None => "a result was refused".into(),
        },
        "VERIFICATION_FAILED" => "verification failed".into(),
        "REVIEW_CHANGES_REQUESTED" => "review asked for changes".into(),
        "REVIEW_DEAD_END_ROUTED" => "review hit a dead end".into(),
        "REVIEW_NO_PROGRESS" => "review saw no progress".into(),
        "NATIVE_TURN_DEGENERATE" => match p["rule"].as_str() {
            Some("refusal") => "the model refused a turn".into(),
            _ => match spelled(&p["rule"]) {
                Some(rule) => format!("a model turn was cut off ({rule})"),
                None => "a model turn was cut off".into(),
            },
        },
        "NATIVE_SESSION_START_FAILED" => {
            match spelled(&p["blocker"]).or_else(|| spelled(&p["code"])) {
                Some(code) => format!("an engine session did not start ({code})"),
                None => "an engine session did not start".into(),
            }
        }
        "NATIVE_WORKER_ROUTE_BLOCKED" => "no usable model route".into(),
        "TURN_INTERRUPTED" => "a turn was interrupted".into(),
        "NATIVE_TURN_SETTLED" => match p["stop_reason"].as_str() {
            Some("max_tokens") => "a turn hit the token limit".into(),
            Some("max_turn_requests") => "a turn hit the request limit".into(),
            Some("refusal") => "the model refused a turn".into(),
            _ => return None,
        },
        _ => return None,
    })
}
/// The newest distinct reasons since `since`, newest first, at most three.
pub(crate) fn reasons(events: &[Event], since: Option<i64>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for e in events.iter().rev() {
        if since.is_some_and(|s| e.at < s) {
            break;
        }
        if let Some(r) = reason(e) {
            if !out.contains(&r) {
                out.push(r);
            }
            if out.len() == 3 {
                break;
            }
        }
    }
    out
}

/// The recognized nonnegative integer fields of an ACP `PromptResponse.usage`, or None when
/// the engine reported none (absent, null or no recognized field).
pub(crate) fn reported_tokens(usage: &Value) -> Option<Value> {
    let map = usage.as_object()?;
    let mut out = serde_json::Map::new();
    for field in TOKEN_FIELDS {
        if let Some(n) = map.get(field).and_then(Value::as_u64) {
            out.insert(field.into(), json!(n));
        }
    }
    (!out.is_empty()).then_some(Value::Object(out))
}
/// Accept only an exact, useful report from the final ACP response. Unknown fields
/// (including cost, cumulative data and arbitrary adapter metadata) are never kept.
pub(crate) fn prompt_reported_tokens(usage: &Value) -> Option<Value> {
    let map = usage.as_object()?;
    for field in TOKEN_FIELDS {
        if let Some(value) = map.get(field) {
            // ACP numbers cross JSON interfaces implemented with JS numbers.
            if value.as_u64()? > 9_007_199_254_740_991 {
                return None;
            }
        }
    }
    let tokens = reported_tokens(usage)?;
    (token_total(&tokens)? <= 9_007_199_254_740_991).then_some(tokens)
}

/// A report's total: `totalTokens` when the engine gave it, else its four counters.
pub(crate) fn token_total(tokens: &Value) -> Option<u64> {
    if let Some(total) = tokens["totalTokens"].as_u64() {
        return Some(total);
    }
    let parts = [
        "inputTokens",
        "outputTokens",
        "cachedReadTokens",
        "cachedWriteTokens",
    ];
    let known: Vec<u64> = parts.iter().filter_map(|f| tokens[*f].as_u64()).collect();
    if known.is_empty() {
        return None;
    }
    known.into_iter().try_fold(0_u64, u64::checked_add)
}

/// What one engine and model did for a task.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct EngineSpend {
    pub engine: String,
    pub model: String,
    pub model_label: Option<String>,
    pub sessions: u64,
    pub turns: u64,
    pub reported_turns: u64,
    pub tokens: u128,
}
/// One usage record: the engine session it belongs to, whether the turn ended with a
/// reply, and the tokens it reported.
#[derive(Clone, Debug)]
pub(crate) struct Usage {
    pub call_id: Option<String>,
    pub session: String,
    pub failed: bool,
    pub tokens: Option<Value>,
}
/// A task's progress and spend at `now`.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Spend {
    pub working: bool,
    pub working_ms: i64,
    pub working_since: Option<i64>,
    pub last_progress: Option<i64>,
    /// Since when the working task has made no progress; None while it does not work.
    pub stalled_since: Option<i64>,
    pub engines: Vec<EngineSpend>,
    pub reasons: Vec<String>,
}
impl Spend {
    pub(crate) fn turns(&self) -> u64 {
        self.engines.iter().map(|e| e.turns).sum()
    }
    pub(crate) fn sessions(&self) -> u64 {
        self.engines.iter().map(|e| e.sessions).sum()
    }
    pub(crate) fn reported_turns(&self) -> u64 {
        self.engines.iter().map(|e| e.reported_turns).sum()
    }
    /// The reported tokens; None when no turn reported any.
    pub(crate) fn tokens(&self) -> Option<u64> {
        (self.reported_turns() > 0)
            .then(|| self.engines.iter().map(|e| e.tokens).sum::<u128>())
            .and_then(|n| u64::try_from(n).ok())
    }
    /// The presentation the task page reads (`spend` on the task's facts).
    pub(crate) fn json(&self) -> Value {
        json!({
            "workingMs": self.working_ms,
            "workingSince": self.working_since,
            "lastProgressAt": self.last_progress,
            "stalledSince": self.stalled_since,
            "sessions": self.sessions(),
            "turns": self.turns(),
            "reportedTurns": self.reported_turns(),
            "tokens": self.tokens(),
            "engines": self.engines.iter().map(|e| {
                let mut row = json!({"engine":e.engine,"model":e.model,"sessions":e.sessions,"turns":e.turns,
                    "reportedTurns":e.reported_turns,"tokens":(e.reported_turns > 0).then(|| u64::try_from(e.tokens).ok()).flatten()});
                if let Some(label) = &e.model_label { row["modelLabel"] = json!(label); }
                row
            }).collect::<Vec<_>>(),
            "lastFailures": self.reasons,
        })
    }
}

/// The wire session of a usage record's `native:<attempt>:<session>` reference.
fn wire_session(reference: &str) -> Option<&str> {
    reference
        .strip_prefix("native:")?
        .split_once(':')
        .map(|(_, s)| s)
}

/// Spend from the events and usage records of one task. Pure: the tests feed it directly.
pub(crate) fn compute(state: &str, events: &[Event], usage: &[Usage], now: i64) -> Spend {
    let (working_ms, working_since) = working_ms(events, now);
    let working = working(state);
    // The engine and model each channel's session was configured with.
    let mut by_channel: BTreeMap<String, (String, String)> = BTreeMap::new();
    let mut by_session: BTreeMap<String, (String, String)> = BTreeMap::new();
    let mut by_session_ref = BTreeMap::new();
    let mut engines: BTreeMap<(String, String), EngineSpend> = BTreeMap::new();
    for e in events
        .iter()
        .filter(|e| e.kind == "NATIVE_SESSION_CONFIGURED")
    {
        let engine = e.payload["engine"].as_str().unwrap_or("").to_owned();
        let model = e.payload["model"].as_str().unwrap_or("").to_owned();
        let key = (engine.clone(), model.clone());
        if let Some(c) = e.payload["channel_id"].as_str() {
            by_channel.insert(c.to_owned(), key.clone());
        }
        if let Some(s) = e.payload["session"].as_str() {
            by_session.insert(s.to_owned(), key.clone());
        }
        if let Some(reference) = e.payload["engine_session_ref"].as_str() {
            by_session_ref.insert(reference.to_owned(), key.clone());
        }
        let row = engines.entry(key).or_insert_with(|| EngineSpend {
            engine,
            model,
            ..Default::default()
        });
        row.sessions += 1;
        if let Some(label) = e.payload["model_label"].as_str().filter(|l| !l.is_empty()) {
            row.model_label = Some(label.to_owned());
        }
    }
    let unknown = || (String::new(), String::new());
    fn bucket(
        engines: &mut BTreeMap<(String, String), EngineSpend>,
        key: (String, String),
    ) -> &mut EngineSpend {
        engines.entry(key.clone()).or_insert_with(|| EngineSpend {
            engine: key.0,
            model: key.1,
            ..Default::default()
        })
    }
    let mut settled_calls = BTreeSet::new();
    for e in events.iter().filter(|e| e.kind == "NATIVE_TURN_SETTLED") {
        if let Some(id) = e.payload["usage_id"].as_str() {
            settled_calls.insert(id);
        }
        let key = e.payload["channel_id"]
            .as_str()
            .and_then(|c| by_channel.get(c).cloned())
            .unwrap_or_else(unknown);
        bucket(&mut engines, key).turns += 1;
    }
    for u in usage {
        let (key, count_turn) = match &u.call_id {
            Some(id) => (
                by_session_ref.get(&u.session).cloned(),
                !settled_calls.contains(id.as_str()),
            ),
            None => (
                wire_session(&u.session).and_then(|s| by_session.get(s).cloned()),
                u.failed,
            ),
        };
        let row = bucket(&mut engines, key.unwrap_or_else(unknown));
        // New rows identify their exact settle event, including post-settle failures.
        // Historical rows keep the old failed/settled counting convention.
        if count_turn {
            row.turns += 1;
        }
        if let Some(total) = u.tokens.as_ref().and_then(token_total) {
            row.reported_turns += 1;
            row.tokens += u128::from(total);
        }
    }
    let stalled_since = if working { stalled_since(events) } else { None };
    Spend {
        working,
        working_ms,
        working_since,
        last_progress: last_progress(events),
        reasons: reasons(events, stalled_since),
        stalled_since,
        engines: engines.into_values().collect(),
    }
}

const AT_MS: &str = "CAST(round((julianday(created_at)-2440587.5)*86400000) AS INTEGER)";
/// One runtime task's spend, read from its store.
pub(crate) fn read(db: &Connection, task: &str, now: i64) -> CoreResult<Spend> {
    let state: String = db
        .query_row(
            "SELECT state FROM runtime_tasks WHERE task_id=?",
            [task],
            |r| r.get(0),
        )
        .unwrap_or_default();
    let kinds = READ
        .iter()
        .map(|k| format!("'{k}'"))
        .collect::<Vec<_>>()
        .join(",");
    let mut statement = db.prepare(&format!(
        "SELECT type,payload_json,{AT_MS} FROM runtime_events WHERE task_id=? AND type IN ({kinds}) ORDER BY id LIMIT 20000"
    ))?;
    let events = statement
        .query_map([task], |r| {
            Ok(Event {
                kind: r.get(0)?,
                payload: serde_json::from_str(&r.get::<_, String>(1)?).unwrap_or(Value::Null),
                at: r.get::<_, Option<i64>>(2)?.unwrap_or(0),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut statement = db.prepare(
        "SELECT engine_session_ref,failed,basis,tokens_json,id FROM usage_records WHERE task_id=? ORDER BY created_at LIMIT 20000",
    )?;
    let usage = statement
        .query_map([task], |r| {
            let basis: String = r.get(2)?;
            let tokens: String = r.get(3)?;
            let id: String = r.get(4)?;
            Ok(Usage {
                call_id: id.starts_with(NATIVE_USAGE_PREFIX).then_some(id),
                session: r.get(0)?,
                failed: r.get::<_, i64>(1)? != 0,
                tokens: (basis == "engine_reported")
                    .then(|| serde_json::from_str::<Value>(&tokens).ok())
                    .flatten()
                    .as_ref()
                    .and_then(reported_tokens),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(compute(&state, &events, &usage, now))
}

/// A working task of the project: its Workspace id, runtime id and spend.
pub(crate) struct Working {
    pub task: String,
    pub runtime: String,
    pub spend: Spend,
}
/// The project's working tasks with their spend.
pub(crate) fn working_tasks(db: &Connection, now: i64) -> CoreResult<Vec<Working>> {
    let states = WORKING
        .iter()
        .map(|s| format!("'{s}'"))
        .collect::<Vec<_>>()
        .join(",");
    let mut statement = db.prepare(&format!(
        "SELECT t.task_id,e.runtime_task_id FROM workspace_tasks t JOIN workspace_executions e ON e.execution_id=t.execution_id
         JOIN runtime_tasks r ON r.task_id=e.runtime_task_id WHERE r.state IN ({states}) ORDER BY t.sequence LIMIT 200"
    ))?;
    let rows = statement
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(task, runtime)| {
            let spend = read(db, &runtime, now)?;
            Ok(Working {
                task,
                runtime,
                spend,
            })
        })
        .collect()
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}
/// "3 h 12 min", "45 min", "2 d 4 h".
pub(crate) fn duration(ms: i64) -> String {
    let minutes = ms.max(0) / 60_000;
    let (d, h, m) = (minutes / 1440, minutes / 60 % 24, minutes % 60);
    match (d, h) {
        (0, 0) => format!("{m} min"),
        (0, _) if m == 0 => format!("{h} h"),
        (0, _) => format!("{h} h {m} min"),
        _ if h == 0 => format!("{d} d"),
        _ => format!("{d} d {h} h"),
    }
}
/// "950", "34k", "1.2M".
pub(crate) fn count(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1000..=999_999 => format!("{}k", (n + 500) / 1000),
        _ => {
            let tenths = (n + 50_000) / 100_000;
            if tenths % 10 == 0 {
                format!("{}M", tenths / 10)
            } else {
                format!("{}.{}M", tenths / 10, tenths % 10)
            }
        }
    }
}
/// The notification's second line: what the task spent and what its last attempts hit.
pub(crate) fn summary(spend: &Spend) -> String {
    let turns = spend.turns();
    let tokens = match spend.tokens() {
        None => "tokens not reported".to_owned(),
        Some(n) if spend.reported_turns() < turns => {
            format!(
                "{} tokens reported for {} of {turns} turns",
                count(n),
                spend.reported_turns()
            )
        }
        Some(n) => format!("{} tokens", count(n)),
    };
    let mut text = format!(
        "Spent: {} working, {turns} turn{}, {tokens}.",
        duration(spend.working_ms),
        if turns == 1 { "" } else { "s" }
    );
    if !spend.reasons.is_empty() {
        text.push_str(&format!(
            " Last attempts hit: {}.",
            spend.reasons.join("; ")
        ));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: i64 = 3_600_000;
    fn ev(kind: &str, payload: Value, at: i64) -> Event {
        Event {
            kind: kind.into(),
            payload,
            at,
        }
    }
    fn state(previous: &str, state: &str, at: i64) -> Event {
        ev(
            "TASK_STATE_CHANGED",
            json!({"previous":previous,"state":state}),
            at,
        )
    }

    #[test]
    fn progress_comes_from_the_earlier_events_until_task_progress_is_recorded() {
        let mut events = vec![
            state("CREATED", "PLANNING", 0),
            ev("PLAN_ACCEPTED", json!({}), H),
            ev(
                "WORKER_COMPLETED",
                json!({"result":{"verdict":"REQUEST_CHANGES"}}),
                2 * H,
            ),
            ev("WORKER_COMPLETED", json!({"result":{"summary":"x"}}), 3 * H),
            ev("VERIFICATION_FAILED", json!({}), 4 * H),
        ];
        assert_eq!(last_progress(&events), Some(3 * H));
        events.push(state("REVIEWING", "VERIFYING", 5 * H));
        assert_eq!(last_progress(&events), Some(5 * H));
        events.push(ev(
            "WORKER_CHECK_FINISHED",
            json!({"checks":[{"exit_code":0,"complete":true}]}),
            6 * H,
        ));
        events.push(ev(
            "WORKER_CHECK_FINISHED",
            json!({"checks":[{"exit_code":1,"complete":true}]}),
            7 * H,
        ));
        assert_eq!(last_progress(&events), Some(6 * H));
        // From the first TASK_PROGRESS on, only TASK_PROGRESS counts.
        events.push(ev("TASK_PROGRESS", json!({"kind":"plan_accepted"}), 8 * H));
        events.push(ev("PLAN_ACCEPTED", json!({}), 9 * H));
        events.push(ev("VERIFICATION_PASSED", json!({}), 10 * H));
        assert_eq!(last_progress(&events), Some(8 * H));
        events.push(ev("TASK_PROGRESS", json!({"kind":"check_passed"}), 11 * H));
        assert_eq!(last_progress(&events), Some(11 * H));
    }

    #[test]
    fn the_clock_restarts_when_work_resumes_and_paused_time_is_not_working_time() {
        let events = vec![
            state("CREATED", "PLANNING", 0),
            state("PLANNING", "EXECUTING", H),
            state("EXECUTING", "WAITING_USER", 2 * H),
            state("WAITING_USER", "EXECUTING", 6 * H),
        ];
        assert_eq!(stalled_since(&events), Some(6 * H));
        let (ms, since) = working_ms(&events, 7 * H);
        assert_eq!((ms, since), (3 * H, Some(6 * H)));
        let spend = compute("EXECUTING", &events, &[], 7 * H);
        assert_eq!(spend.stalled_since, Some(6 * H));
        let paused = compute("PAUSED", &events, &[], 7 * H);
        assert_eq!(paused.stalled_since, None);
    }

    #[test]
    fn spend_aggregates_reported_usage_by_engine_and_model_and_never_invents_tokens() {
        let events = vec![
            state("CREATED", "PLANNING", 0),
            ev(
                "NATIVE_SESSION_CONFIGURED",
                json!({"channel_id":"c1","session":"s1","engine":"claude-agent-acp","model":"opus","model_label":"Opus"}),
                1,
            ),
            ev(
                "NATIVE_SESSION_CONFIGURED",
                json!({"channel_id":"c2","session":"s2","engine":"codex-acp","model":"gpt"}),
                2,
            ),
            ev(
                "NATIVE_TURN_SETTLED",
                json!({"channel_id":"c1","stop_reason":"end_turn"}),
                3,
            ),
            ev(
                "NATIVE_TURN_SETTLED",
                json!({"channel_id":"c1","stop_reason":"max_tokens"}),
                4,
            ),
            ev(
                "NATIVE_TURN_SETTLED",
                json!({"channel_id":"c2","stop_reason":"end_turn"}),
                5,
            ),
        ];
        let usage = vec![
            Usage {
                call_id: None,
                session: "native:co_1:s1".into(),
                failed: false,
                tokens: Some(json!({"inputTokens":10,"outputTokens":5,"totalTokens":1200})),
            },
            Usage {
                call_id: None,
                session: "native:co_1:s1".into(),
                failed: false,
                tokens: Some(json!({"inputTokens":100,"outputTokens":50})),
            },
            // Codex reported nothing for its turn.
            Usage {
                call_id: None,
                session: "native:wa_2:s2".into(),
                failed: false,
                tokens: None,
            },
            // A turn that ended without a reply.
            Usage {
                call_id: None,
                session: "native:wa_2:s2".into(),
                failed: true,
                tokens: None,
            },
        ];
        let spend = compute("EXECUTING", &events, &usage, 10);
        let claude = spend
            .engines
            .iter()
            .find(|e| e.engine == "claude-agent-acp")
            .unwrap();
        assert_eq!(
            (
                claude.sessions,
                claude.turns,
                claude.reported_turns,
                claude.tokens
            ),
            (1, 2, 2, 1350)
        );
        assert_eq!(claude.model_label.as_deref(), Some("Opus"));
        let codex = spend
            .engines
            .iter()
            .find(|e| e.engine == "codex-acp")
            .unwrap();
        assert_eq!(
            (codex.sessions, codex.turns, codex.reported_turns),
            (1, 2, 0)
        );
        assert_eq!(spend.tokens(), Some(1350));
        let shown = spend.json();
        assert_eq!(shown["engines"][1]["tokens"], Value::Null);
        assert_eq!(shown["turns"], 4);
        assert_eq!(spend.reasons, vec!["a turn hit the token limit".to_owned()]);
        assert!(summary(&spend).contains("1k tokens reported for 2 of 4 turns"));
        let silent = compute("EXECUTING", &events, &[], 10);
        assert_eq!(silent.tokens(), None);
        assert!(summary(&silent).contains("tokens not reported"));
    }

    #[test]
    fn spend_reads_exact_prompt_rows_and_legacy_rows_from_sqlite() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(include_str!("../../cortex-core/src/store/schema.sql"))
            .unwrap();
        db.execute_batch(
            "PRAGMA foreign_keys=ON;
             INSERT INTO tasks VALUES ('t','/fixture','2026-01-01'), ('other','/fixture','2026-01-01');
             INSERT INTO runtime_tasks(task_id,state,goal,created_at,updated_at)
             VALUES ('t','EXECUTING','Fixture','2026-01-01','2026-01-01'),
                    ('other','EXECUTING','Fixture','2026-01-01','2026-01-01');",
        )
        .unwrap();
        let event = |kind, payload: Value| {
            db.execute(
                "INSERT INTO runtime_events(task_id,type,actor,payload_json,created_at)
                 VALUES ('t',?,'runtime',?,'2026-01-01')",
                (kind, payload.to_string()),
            )
            .unwrap();
        };
        let row = |task, id, attempt, session, seq, basis, tokens: Value, failed| {
            db.execute(
                "INSERT INTO usage_records(id,task_id,attempt_id,engine_session_ref,
                 call_sequence,basis,tokens_json,failed,created_at)
                 VALUES (?,?,?,?,?,?,?,?,'2026-01-01')",
                (
                    id,
                    task,
                    attempt,
                    session,
                    seq,
                    basis,
                    tokens.to_string(),
                    failed,
                ),
            )
            .unwrap();
        };
        // The same wire session ID can be returned by independent engine processes.
        event(
            "NATIVE_SESSION_CONFIGURED",
            json!({"session":"same","channel_id":"c","engine_session_ref":"native:co:same",
                "engine":"codex-acp","model":"gpt"}),
        );
        event(
            "NATIVE_SESSION_CONFIGURED",
            json!({"session":"same","channel_id":"w","engine_session_ref":"native:worker:same",
                "engine":"claude-agent-acp","model":"opus"}),
        );
        event(
            "NATIVE_SESSION_CONFIGURED",
            json!({"session":"legacy","channel_id":"old","engine":"opencode","model":"legacy"}),
        );
        event(
            "NATIVE_TURN_SETTLED",
            json!({"channel_id":"c","usage_id":"native_usage:native:co:same:1","stop_reason":"refusal"}),
        );
        row(
            "t",
            "native_usage:native:co:same:1",
            "co",
            "native:co:same",
            1,
            "unknown",
            json!({}),
            1,
        );
        row(
            "t",
            "native_usage:native:worker:same:1",
            "worker",
            "native:worker:same",
            1,
            "unknown",
            json!({}),
            1,
        );
        let unknown = read(&db, "t", 0).unwrap().json();
        assert_eq!(unknown["turns"], 2);
        assert_eq!(unknown["reportedTurns"], 0);
        assert_eq!(unknown["tokens"], Value::Null);

        event(
            "NATIVE_TURN_SETTLED",
            json!({"channel_id":"c","usage_id":"native_usage:native:co:same:2"}),
        );
        row(
            "t",
            "native_usage:native:co:same:2",
            "co",
            "native:co:same",
            2,
            "engine_reported",
            json!({"totalTokens":0}),
            0,
        );
        let zero = read(&db, "t", 0).unwrap().json();
        assert_eq!(zero["tokens"], 0);
        assert_eq!(zero["reportedTurns"], 1);
        assert_eq!(zero["turns"], 3);

        event("NATIVE_TURN_SETTLED", json!({"channel_id":"old"}));
        row(
            "t",
            "legacy-id",
            "old",
            "native:old:legacy",
            1,
            "engine_reported",
            json!({"inputTokens":12}),
            0,
        );
        row(
            "other",
            "native_usage:native:other:same:1",
            "other",
            "native:other:same",
            1,
            "engine_reported",
            json!({"totalTokens":3000}),
            0,
        );
        // A committed settle still contributes a turn if its separate write failed.
        event(
            "NATIVE_TURN_SETTLED",
            json!({"channel_id":"w","usage_id":"native_usage:native:worker:same:2"}),
        );
        let shown = read(&db, "t", 0).unwrap().json();
        assert_eq!(shown["tokens"], 12);
        assert_eq!(shown["reportedTurns"], 2);
        assert_eq!(shown["turns"], 5);
        let engines = shown["engines"].as_array().unwrap();
        assert_eq!(engines[0]["engine"], "claude-agent-acp");
        assert_eq!(engines[0]["model"], "opus");
        assert_eq!(engines[0]["turns"], 2);
        assert_eq!(engines[0]["tokens"], Value::Null);
        assert_eq!(engines[1]["engine"], "codex-acp");
        assert_eq!(engines[1]["model"], "gpt");
        assert_eq!(engines[1]["turns"], 2);
        assert_eq!(engines[1]["tokens"], 0);
        assert_eq!(engines[2]["model"], "legacy");
        assert_eq!(engines[2]["tokens"], 12);
        assert!(db
            .execute("UPDATE usage_records SET failed=0 WHERE id='legacy-id'", [])
            .is_err());
    }

    #[test]
    fn reasons_are_plain_short_and_newest_first() {
        let events = vec![
            ev("VERIFICATION_FAILED", json!({}), 1),
            ev(
                "WORKER_RESULT_REFUSED",
                json!({"code":"command_not_settled"}),
                2,
            ),
            ev("WORKER_RESULT_REFUSED", json!({"code":"Free text <b>"}), 3),
            ev("NATIVE_TURN_DEGENERATE", json!({"rule":"repetition"}), 4),
            ev("VERIFICATION_FAILED", json!({}), 5),
        ];
        assert_eq!(
            reasons(&events, None),
            vec![
                "verification failed",
                "a model turn was cut off (repetition)",
                "a result was refused"
            ]
        );
        assert_eq!(reasons(&events, Some(5)), vec!["verification failed"]);
    }

    #[test]
    fn usage_keeps_only_reported_integer_fields() {
        assert_eq!(reported_tokens(&Value::Null), None);
        assert_eq!(reported_tokens(&json!({"cost":1.5})), None);
        for invalid in [
            json!({"totalTokens":-1,"outputTokens":2}),
            json!({"totalTokens":1.5}),
            json!({"totalTokens":"0"}),
            json!({"totalTokens":9_007_199_254_740_992_u64}),
            json!({"thoughtTokens":5}),
            json!({"used":25,"size":100,"cumulative":{"totalTokens":25}}),
        ] {
            assert_eq!(prompt_reported_tokens(&invalid), None, "{invalid}");
        }
        assert_eq!(
            prompt_reported_tokens(&json!({"totalTokens":0,"cost":8})),
            Some(json!({"totalTokens":0}))
        );
        assert_eq!(
            reported_tokens(&json!({"inputTokens":3,"outputTokens":-1,"totalTokens":9,"x":1})),
            Some(json!({"inputTokens":3,"totalTokens":9}))
        );
        assert_eq!(
            token_total(&json!({"inputTokens":3,"cachedReadTokens":4})),
            Some(7)
        );
        assert_eq!(duration(3 * H + 12 * 60_000), "3 h 12 min");
        assert_eq!(duration(26 * H), "1 d 2 h");
        assert_eq!(count(1_250_000), "1.3M");
        assert_eq!(count(34_400), "34k");
    }
}
