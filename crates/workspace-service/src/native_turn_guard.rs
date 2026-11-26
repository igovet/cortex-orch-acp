//! Per-turn bounds on a coordinator or worker turn that keeps acting without
//! changing anything. The runtime sees every MCP `tools/call` of a channel, so it
//! can tell a turn that repeats the same refused call, re-reads the same unchanged
//! result, calls tools without end, or goes silent from a turn that works.
//!
//! A trip is recorded as `NATIVE_TURN_DEGENERATE` (rule `tool_loop`, `tool_calls` or
//! `silence`, with the tool and the count) and ends the turn through the same
//! cancel-and-grace path as every degenerate turn. The turn then counts as one with
//! no progress: the coordinator driver's `coordinator_no_progress_limit` and the
//! strategy ladder take over from there. Further calls of a tripped turn are refused
//! with `turn_tool_loop_limit`. Nothing here reads model text or needs a host API
//! beyond the calls the runtime already receives and the ACP `session/cancel`.
use crate::native_sessions::degenerate::Trip;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::Notify;
use tokio::time::Instant;

/// The same call (tool and arguments, apart from `request_key`) refused with the same
/// code this many times in one turn ends the turn.
pub(crate) const REPEATED_REFUSAL_LIMIT: usize = 4;
/// Refusals that only say the world has not settled yet (a command still running, a
/// checkout another writer keeps changing) are repeated legitimately a few times; they
/// get this multiple of [`REPEATED_REFUSAL_LIMIT`].
pub(crate) const PATIENT_REFUSAL_FACTOR: usize = 4;
/// The same call returning the same result this many times, with no call that was new
/// to the turn in between, ends the turn.
pub(crate) const REPEATED_RESULT_LIMIT: usize = 12;
/// Tool calls of one coordinator turn.
pub(crate) const COORDINATOR_CALL_CEILING: usize = 400;
/// Tool calls of one worker turn (a writing worker legitimately makes many).
pub(crate) const WORKER_CALL_CEILING: usize = 2_000;
/// A turn with no engine update and no tool call in flight for this long is stuck.
pub(crate) const SILENCE_LIMIT: Duration = Duration::from_secs(20 * 60);
/// The refusal code of every call after a trip.
pub(crate) const REFUSED: &str = "turn_tool_loop_limit";

const PATIENT: &[&str] = &[
    "command_not_settled",
    "review_resubmit_required",
    "review_project_revision_changed",
    "busy",
    "storage_unavailable",
];

#[derive(Clone, Copy, Debug)]
pub(crate) struct TurnLimits {
    pub repeated_refusals: usize,
    pub repeated_results: usize,
    pub coordinator_calls: usize,
    pub worker_calls: usize,
    pub silence: Duration,
}

impl Default for TurnLimits {
    fn default() -> Self {
        Self {
            repeated_refusals: REPEATED_REFUSAL_LIMIT,
            repeated_results: REPEATED_RESULT_LIMIT,
            coordinator_calls: COORDINATOR_CALL_CEILING,
            worker_calls: WORKER_CALL_CEILING,
            silence: SILENCE_LIMIT,
        }
    }
}

/// What one finished call came to.
pub(crate) enum Outcome<'a> {
    Refused(&'a str),
    Result(&'a Value),
}

struct State {
    calls: usize,
    in_flight: usize,
    last_activity: Instant,
    seen: HashSet<u64>,
    refusals: HashMap<(u64, String), usize>,
    results: HashMap<(u64, u64), usize>,
    trip: Option<Trip>,
    reported: bool,
}

impl State {
    fn fresh() -> Self {
        Self {
            calls: 0,
            in_flight: 0,
            last_activity: Instant::now(),
            seen: HashSet::new(),
            refusals: HashMap::new(),
            results: HashMap::new(),
            trip: None,
            reported: false,
        }
    }
}

pub(crate) struct TurnGuard {
    limits: TurnLimits,
    worker: bool,
    state: Mutex<State>,
    tripped: Notify,
}

fn hash<T: Hash>(value: &T) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

/// The call's identity: tool and arguments without the per-call `request_key`.
pub(crate) fn call_key(name: &str, arguments: &Value) -> u64 {
    let mut arguments = arguments.clone();
    if let Some(fields) = arguments.as_object_mut() {
        fields.remove("request_key");
    }
    hash(&(name, arguments.to_string()))
}

/// Whether a `command_status` result reports a live command (running or settling).
fn still_running(value: &Value) -> bool {
    let state = value.get("structuredContent").unwrap_or(value);
    state["settled"] != true
        && matches!(state["state"].as_str(), Some("running" | "settling"))
}

/// Marks a call in flight until dropped, also when the call's future is cancelled.
pub(crate) struct InFlight<'a>(&'a TurnGuard);
impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap();
        state.in_flight = state.in_flight.saturating_sub(1);
        state.last_activity = Instant::now();
    }
}

impl TurnGuard {
    pub(crate) fn new(limits: TurnLimits, worker: bool) -> Self {
        Self { limits, worker, state: Mutex::new(State::fresh()), tripped: Notify::new() }
    }

    /// A new prompt of the session starts a new turn: every count starts again.
    pub(crate) fn begin_turn(&self) {
        *self.state.lock().unwrap() = State::fresh();
    }

    /// An engine update arrived: the turn is not silent.
    pub(crate) fn touch(&self) {
        self.state.lock().unwrap().last_activity = Instant::now();
    }

    fn trip(&self, state: &mut State, rule: &'static str, tool: &str, count: usize) {
        if state.trip.is_none() {
            state.trip = Some(Trip {
                rule,
                message_id: None,
                chars: 0,
                repeated_share: None,
                tool: Some(tool.to_owned()),
                count: Some(count),
            });
            self.tripped.notify_one();
        }
    }

    /// Admit one call, or refuse it when the turn already tripped (or this call
    /// exceeds the turn's call ceiling).
    pub(crate) fn enter(&self, name: &str) -> Result<InFlight<'_>, &'static str> {
        let mut state = self.state.lock().unwrap();
        if state.trip.is_some() {
            return Err(REFUSED);
        }
        state.calls += 1;
        let ceiling = if self.worker { self.limits.worker_calls } else { self.limits.coordinator_calls };
        if state.calls > ceiling {
            let calls = state.calls;
            self.trip(&mut state, "tool_calls", name, calls);
            return Err(REFUSED);
        }
        state.in_flight += 1;
        state.last_activity = Instant::now();
        Ok(InFlight(self))
    }

    /// Record how a call ended; trips the turn on a repeated refusal or result.
    pub(crate) fn finish(&self, name: &str, key: u64, outcome: Outcome<'_>) {
        let mut state = self.state.lock().unwrap();
        let new = state.seen.insert(key);
        match outcome {
            Outcome::Refused(code) => {
                let limit = if PATIENT.contains(&code) {
                    self.limits.repeated_refusals * PATIENT_REFUSAL_FACTOR
                } else {
                    self.limits.repeated_refusals
                };
                let count = {
                    let n = state.refusals.entry((key, code.to_owned())).or_default();
                    *n += 1;
                    *n
                };
                if count >= limit {
                    self.trip(&mut state, "tool_loop", name, count);
                }
            }
            Outcome::Result(value) => {
                // Polling a command that is still running (or settling) is waiting, not a
                // loop: it is neither counted nor treated as progress. A finished command
                // re-polled with the same settled result still counts.
                if name == "command_status" && still_running(value) {
                    return;
                }
                // A call new to this turn is progress of a kind (a new read, an edit,
                // a submission): the identical-result counts start over.
                if new {
                    state.results.clear();
                }
                let count = {
                    let n = state.results.entry((key, hash(&value.to_string()))).or_default();
                    *n += 1;
                    *n
                };
                if count >= self.limits.repeated_results {
                    self.trip(&mut state, "tool_loop", name, count);
                }
            }
        }
    }

    /// Whether the turn has been silent (no engine update, no call in flight) for the
    /// silence limit; trips it when so.
    pub(crate) fn check_silence(&self) -> Option<Trip> {
        let mut state = self.state.lock().unwrap();
        if state.trip.is_none()
            && state.in_flight == 0
            && state.last_activity.elapsed() >= self.limits.silence
        {
            let seconds = state.last_activity.elapsed().as_secs() as usize;
            self.trip(&mut state, "silence", "", seconds);
        }
        Self::report(&mut state)
    }

    /// The trip, once, for the turn driver to record and act on.
    pub(crate) fn take_trip(&self) -> Option<Trip> {
        Self::report(&mut self.state.lock().unwrap())
    }

    fn report(state: &mut State) -> Option<Trip> {
        if state.reported {
            return None;
        }
        let trip = state.trip.clone()?;
        state.reported = true;
        Some(trip)
    }

    /// Resolves when the turn trips.
    pub(crate) async fn tripped(&self) {
        self.tripped.notified().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn guard(worker: bool) -> TurnGuard {
        TurnGuard::new(TurnLimits::default(), worker)
    }
    fn refuse(g: &TurnGuard, name: &str, args: &Value, code: &str) {
        let _call = g.enter(name).unwrap();
        g.finish(name, call_key(name, args), Outcome::Refused(code));
    }

    #[test]
    fn the_same_refused_call_ends_the_turn_whatever_its_request_key() {
        let g = guard(false);
        for n in 0..REPEATED_REFUSAL_LIMIT {
            assert!(g.take_trip().is_none(), "{n}");
            refuse(&g, "plan_submit", &json!({"plan":{"a":1},"request_key":format!("k{n}")}), "plan_outcome_uncovered");
        }
        let trip = g.take_trip().expect("tripped");
        assert_eq!((trip.rule, trip.tool.as_deref(), trip.count), ("tool_loop", Some("plan_submit"), Some(REPEATED_REFUSAL_LIMIT)));
        assert!(g.take_trip().is_none(), "reported once");
        assert_eq!(g.enter("read_batch").err(), Some(REFUSED));
    }

    #[test]
    fn a_corrected_call_or_another_code_does_not_count_as_a_repeat() {
        let g = guard(false);
        for n in 0..10 {
            refuse(&g, "plan_submit", &json!({"plan":{"a":n}}), "plan_outcome_uncovered");
        }
        for code in ["a", "b", "c", "d", "e"] {
            refuse(&g, "plan_submit", &json!({"plan":"same"}), code);
        }
        assert!(g.take_trip().is_none());
    }

    #[test]
    fn patient_refusals_get_a_wider_bound() {
        let g = guard(true);
        for _ in 0..REPEATED_REFUSAL_LIMIT * PATIENT_REFUSAL_FACTOR - 1 {
            refuse(&g, "result_submit", &json!({"r":1}), "command_not_settled");
        }
        assert!(g.take_trip().is_none());
        refuse(&g, "result_submit", &json!({"r":1}), "command_not_settled");
        assert!(g.take_trip().is_some());
    }

    #[test]
    fn an_unchanged_result_read_over_and_over_ends_the_turn_but_new_calls_reset_it() {
        let g = guard(false);
        let same = json!({"state":"PLANNING"});
        let read = |g: &TurnGuard, args: Value| {
            let Ok(_call) = g.enter("task_get_context") else { return };
            g.finish("task_get_context", call_key("task_get_context", &args), Outcome::Result(&same));
        };
        for n in 0..REPEATED_RESULT_LIMIT - 1 {
            read(&g, json!({}));
            // A different read in between resets the count.
            if n % 5 == 4 {
                read(&g, json!({"at":n}));
            }
        }
        assert!(g.take_trip().is_none());
        for _ in 0..REPEATED_RESULT_LIMIT {
            read(&g, json!({}));
        }
        assert_eq!(g.take_trip().unwrap().rule, "tool_loop");
    }

    fn status(g: &TurnGuard, value: &Value) {
        let args = json!({"tool_call_ref":"native-command:x"});
        let Ok(_call) = g.enter("command_status") else { return };
        g.finish("command_status", call_key("command_status", &args), Outcome::Result(value));
    }

    #[test]
    fn polling_a_running_command_is_waiting_not_a_loop() {
        let g = guard(true);
        let running = json!({"structuredContent":{"state":"running","tool_call_ref":"native-command:x","complete":false,"settled":false},"isError":false});
        for _ in 0..50 {
            status(&g, &running);
        }
        assert!(g.take_trip().is_none());
    }

    #[test]
    fn a_finished_command_polled_again_and_again_ends_the_turn() {
        let g = guard(true);
        let done = json!({"structuredContent":{"state":"finished","tool_call_ref":"native-command:x","complete":true,"settled":true,"exit_code":0},"isError":false});
        for _ in 0..REPEATED_RESULT_LIMIT {
            status(&g, &done);
        }
        assert_eq!(g.take_trip().unwrap().rule, "tool_loop");
    }

    #[test]
    fn the_call_ceiling_differs_by_role_and_a_new_turn_starts_again() {
        let g = guard(false);
        for n in 0..COORDINATOR_CALL_CEILING {
            let _call = g.enter("project_read").unwrap_or_else(|_| panic!("{n}"));
        }
        assert_eq!(g.enter("project_read").err(), Some(REFUSED));
        assert_eq!(g.take_trip().unwrap().rule, "tool_calls");
        g.begin_turn();
        assert!(g.enter("project_read").is_ok());
        let w = guard(true);
        for _ in 0..COORDINATOR_CALL_CEILING + 1 {
            drop(w.enter("project_read").unwrap());
        }
    }

    #[tokio::test(start_paused = true)]
    async fn silence_trips_only_without_updates_and_calls_in_flight() {
        let g = guard(false);
        tokio::time::advance(SILENCE_LIMIT - Duration::from_secs(1)).await;
        assert!(g.check_silence().is_none());
        g.touch();
        tokio::time::advance(SILENCE_LIMIT - Duration::from_secs(1)).await;
        assert!(g.check_silence().is_none());
        // A long command in flight is not silence.
        let call = g.enter("command_run").unwrap();
        tokio::time::advance(SILENCE_LIMIT * 2).await;
        assert!(g.check_silence().is_none());
        drop(call);
        tokio::time::advance(SILENCE_LIMIT).await;
        assert_eq!(g.check_silence().unwrap().rule, "silence");
        assert!(g.check_silence().is_none());
    }

    #[tokio::test]
    async fn a_trip_wakes_the_waiting_turn() {
        let g = guard(false);
        for _ in 0..REPEATED_REFUSAL_LIMIT {
            refuse(&g, "t", &json!({}), "x");
        }
        tokio::time::timeout(Duration::from_secs(1), g.tripped()).await.expect("woken");
    }
}
