//! Native sprint planning and queue execution.
use crate::{actor, machine::sprint_live::{self, Snapshot}, native_runtime::Runtime, Service};
use serde_json::{json, Value};
use std::{
    collections::{BTreeSet, HashMap},
    io,
    sync::Arc,
    time::Duration,
};

const PASS_EVERY: Duration = Duration::from_secs(2);
mod planning;
pub(crate) use planning::{
    check_deliverables, check_tasks, planning_fact, planning_request, planning_step, PlanError, PlanningFailure,
    PlanningStep, MAX_PART_BYTES,
};
use planning::{planning_files, planning_start_reason};
#[cfg(test)]
pub(super) use planning::validate_tasks;
#[cfg(test)]
use planning::{finalize_items, PlanningFact};

/// Background jobs of a pass; dropping the pass aborts them. Planning itself is
/// an ordinary runtime task, so no planner job runs here.
#[derive(Default)]
struct Jobs(
    HashMap<(String, String), tokio::task::JoinHandle<()>>,
    HashMap<String, HashMap<String, u8>>,
);
impl Drop for Jobs {
    fn drop(&mut self) {
        for job in self.0.values() {
            job.abort();
        }
    }
}
#[cfg(test)]
mod budget_tests;

/// Legacy manual-queue pause wording, retained for saved holds.
#[cfg(test)]
pub(super) fn with_settle_failure<'a>(
    pause: Option<&'a str>,
    failed: Option<&'a str>,
) -> Option<&'a str> {
    match (pause, failed) {
        (Some(UNSETTLED), Some(why)) => Some(why),
        (pause, _) => pause,
    }
}
const UNSETTLED: &str = "A result has missing or unsuccessful checks.";
/// The disposition a sprint member is settled by: its saved one, or, for a task the
/// owner finished through the runtime alone (Finish on its stop question) with none
/// saved, the owner's close as is, read from the same projection rule the board uses.
pub(crate) fn member_disposition(
    record: &Value,
    task: Option<&Value>,
) -> (String, Option<String>) {
    if task.is_some_and(|t| warned(t, "owner_closed_unconfirmed")) {
        return ("done".into(), Some("unconfirmed".into()));
    }
    if record["archive"].is_null() && task.is_some_and(|t| {
        t["phase"] == "stopped" && t["runtimeState"] == "CANCELLED"
            && warned(t, "owner_completed_externally") && !warned(t, "contradictory_evidence")
    }) {
        return ("done".into(), Some("external".into()));
    }
    let status = record["status"].as_str().unwrap_or("").to_owned();
    let closed = record["closedAs"].as_str().map(str::to_owned);
    let resolved = task.is_some_and(|t| {
        t["phase"] == "stopped"
            && t["runtimeState"] == "CANCELLED"
            && t["warnings"].as_array().is_some_and(|w| {
                w.iter()
                    .any(|w| w == "owner_resolved_without_certification")
                    && !w.iter().any(|w| w == "contradictory_evidence")
            })
    });
    if status.is_empty() && record["archive"].is_null() && resolved {
        return (
            "done".into(),
            Some(closed.unwrap_or_else(|| "as_is".into())),
        );
    }
    (status, closed)
}
#[cfg(test)]
mod settle_tests {
    use super::*;

    #[test]
    fn a_closed_unconfirmed_member_is_skipped_but_does_not_finish_its_dependency() {
        let records = json!({"first":{"title":"First"},"second":{"dependencies":["first"]}});
        let tasks = HashMap::from([
            (
                "first".into(),
                json!({"phase":"unknown","warnings":["contradictory_evidence","owner_closed_unconfirmed"]}),
            ),
            ("second".into(), json!({"phase":"not_started"})),
        ]);
        let members = vec!["first".into(), "second".into()];
        let disposition = members
            .iter()
            .map(|id: &String| (id.clone(), member_disposition(&records[id], tasks.get(id))))
            .collect();
        for hybrid in [false, true] {
            assert!(matches!(
                automatic_queue_step(&members[..1], &tasks, &records, &disposition, hybrid),
                QueueStep::Complete
            ));
            assert!(matches!(
                automatic_queue_step(&members, &tasks, &records, &disposition, hybrid),
                QueueStep::Hold(_)
            ));
        }
    }
    #[test]
    fn a_failed_settle_names_itself_and_other_pauses_keep_their_reason() {
        let why = "Build could not be accepted automatically: The result could not be read. Open the task to accept it yourself.";
        assert_eq!(with_settle_failure(Some(UNSETTLED), Some(why)), Some(why));
        assert_eq!(with_settle_failure(Some(UNSETTLED), None), Some(UNSETTLED));
        assert_eq!(
            with_settle_failure(Some("This sprint is in manual mode."), Some(why)),
            Some("This sprint is in manual mode.")
        );
        assert_eq!(with_settle_failure(None, Some(why)), None);
    }
    #[test]
    fn a_member_finished_through_the_runtime_settles_as_closed_as_is() {
        let resolved = json!({"phase":"stopped","runtimeState":"CANCELLED","warnings":["owner_resolved_without_certification"]});
        let none = (String::new(), None);
        let as_is = ("done".to_owned(), Some("as_is".to_owned()));
        assert_eq!(member_disposition(&json!({}), Some(&resolved)), as_is);
        // A run the runtime cancelled by itself, or contradictory evidence, settles nothing.
        let plain = json!({"phase":"stopped","runtimeState":"CANCELLED","warnings":[]});
        assert_eq!(member_disposition(&json!({}), Some(&plain)), none);
        let mut odd = resolved.clone();
        odd["warnings"] = json!([
            "owner_resolved_without_certification",
            "contradictory_evidence"
        ]);
        assert_eq!(member_disposition(&json!({}), Some(&odd)), none);
        assert_eq!(member_disposition(&json!({}), None), none);
        // A saved disposition always wins.
        let saved = json!({"status":"done","closedAs":"cancel"});
        assert_eq!(
            member_disposition(&saved, Some(&resolved)),
            ("done".to_owned(), Some("cancel".to_owned()))
        );
    }
}
#[cfg(test)]
mod hold_tests {
    use super::*;
    /// H1: only a recorded hold (or an exact legacy text) is a system hold. A failed
    /// settle text proves nothing: earlier versions wrote it over the owner's pause.
    #[test]
    fn an_owner_pause_is_never_a_system_hold_whatever_its_text() {
        let paused = |reason: &str| json!({"runMode":"hybrid","queueState":"paused","pauseReason":reason});
        for reason in [
            "Paused by you",
            "Build could not be accepted automatically: the check stopped before it finished.",
            "This sprint is in manual mode.",
        ] {
            assert!(!system_hold(&paused(reason)), "{reason}");
            assert!(!resumable_automatic_queue(&paused(reason)), "{reason}");
        }
        assert!(system_hold(&paused(
            "No task can run; review dependencies."
        )));
        let mut held = paused("Paused by you");
        held["automaticQueue"] = json!({"kind":"settlement_blocked"});
        assert!(system_hold(&held));
    }
    /// L1: an acceptance the finish cannot use never "continues by itself".
    #[test]
    fn an_unusable_acceptance_is_the_owners_decision_with_an_action() {
        let code = "automatic_acceptance_required";
        assert_eq!(first_actor("settlement_blocked", code), BY_OWNER);
        let plain = automatic_hold(
            "settlement_blocked",
            Some("t"),
            code,
            "r",
            &HoldFacts::default(),
        );
        assert_eq!(plain["nextActor"], BY_OWNER);
        assert_eq!(plain["ownerActions"], json!(["close_as_is"]));
        let repair = HoldFacts {
            repair: true,
            ..HoldFacts::default()
        };
        let offered = automatic_hold("settlement_blocked", Some("t"), code, "r", &repair);
        assert_eq!(
            offered["ownerActions"],
            json!(["allow_repair", "close_as_is"])
        );
    }
    /// M5: Hybrid offers its acceptance for a worktree result; Automatic's own
    /// finish still needs the project folder.
    #[test]
    fn hybrid_offers_acceptance_in_a_worktree_and_automatic_finish_needs_the_folder() {
        let record = |checkout: &str| {
            json!({"checkout":{"kind":checkout},"conflict":"none",
                "reviewActions":{"acknowledgeOpenChecks":true}})
        };
        let hybrid = json!({"runMode":"hybrid"});
        let automatic = json!({"runMode":"automatic"});
        let code = "automatic_acceptance_blocked";
        assert!(finish_offered(&hybrid, &record("worktree"), code, false));
        assert!(finish_offered(&hybrid, &record("main"), code, false));
        assert!(!finish_offered(
            &automatic,
            &record("worktree"),
            code,
            false
        ));
        assert!(finish_offered(&automatic, &record("main"), code, false));
        assert!(!finish_offered(&hybrid, &record("worktree"), code, true));
        assert!(!finish_offered(
            &hybrid,
            &record("worktree"),
            "stale_result",
            false
        ));
        let mut facts = HoldFacts {
            finish: true,
            accept_only: true,
            ..HoldFacts::default()
        };
        facts.asks = vec!["Accept it.".into()];
        let hold = automatic_hold("settlement_blocked", Some("t"), code, "r", &facts);
        assert_eq!(
            hold["ownerActions"],
            json!(["accept_result", "close_as_is"])
        );
    }
    /// L6: the hold names the dependent the cancelled prerequisite actually blocks,
    /// not the first task waiting behind it.
    #[test]
    fn a_cancelled_prerequisite_holds_the_task_that_depends_on_it() {
        let members = ["schema", "api", "ui"].map(str::to_owned);
        // "ui" waits for "api"; "api" waits for the cancelled "schema".
        let records = json!({
            "schema":{"title":"Schema","closedAs":"cancel"},
            "api":{"title":"API","dependencies":["schema"]},
            "ui":{"title":"UI","dependencies":["api"]}
        });
        let tasks = ["api", "ui"]
            .into_iter()
            .map(|id| (id.to_owned(), json!({"phase":"not_started"})))
            .collect();
        let disposition: HashMap<String, (String, Option<String>)> = [(
            "schema".to_owned(),
            ("done".to_owned(), Some("cancel".to_owned())),
        )]
        .into_iter()
        .collect();
        // The waiting task is listed first: it must not be the one named.
        let order = ["ui", "api", "schema"].map(str::to_owned);
        for members in [&members[..], &order[..]] {
            let QueueStep::Hold(hold) =
                automatic_queue_step(members, &tasks, &records, &disposition, false)
            else {
                panic!("a cancelled prerequisite must hold the queue");
            };
            assert_eq!(hold["code"], "dependencies_blocked");
            assert_eq!(hold["taskId"], "api", "{hold}");
            let reason = hold["reason"].as_str().unwrap();
            assert!(
                reason.contains("\u{201c}Schema\u{201d}") && reason.contains("cancelled"),
                "{reason}"
            );
        }
    }
}
/// The outcome of one attempt to send a task back to its agent.
enum Repair {
    /// The agent is working on it; no hold is persisted.
    Started,
    /// Not decidable in this pass (busy, unreadable, review still running).
    Later,
    /// No agent repair now; the facts for the owner's decision.
    Declined(HoldFacts),
}
/// The outcome of one automatic settle for one task.
enum Settle {
    Accepted,
    Waiting(String),
    Deferred,
    Failed(String),
    Unobservable(String),
}
const WORKTREE_UNAVAILABLE: &str =
    "Automatic cannot start in a separate worktree. It runs only in the project folder.";
const NOT_SENT_BACK_ASK: &str = "The agent could not be sent back to this task. Close it as is, or check its work details first.";
const UNKNOWN_EFFECT_ASK: &str = "An action of this task may or may not have finished, so no agent is sent back to repeat it. Check the project folder, then close the task as is.";
/// A failed commit, in words that match the actions the owner is offered.
const INTEGRATION_FAILED: &str = "Git could not commit this task's changes. Nothing was committed and the files are unchanged. Check the Git identity and commit hooks of the project folder. A failed commit is never repeated as it is.";

/// Why Automatic could not start a task, in plain words (never the refusal code).
fn start_reason(code: &str) -> String {
    match code {
        code if code.starts_with("deliverable_") => "A required sprint output could not be confirmed. Open its producer and restore the reviewed, verified result.",
        "native_worktree_unavailable" => WORKTREE_UNAVAILABLE,
        "stale_version" | "stale_result" => "The task changed while the queue was starting it.",
        "maintenance_busy" => {
            "Maintenance was running when the queue tried to start this task."
        }
        "service_stopping" | "native_host_closed" => {
            "The service was stopping when the queue tried to start this task."
        }
        _ => "The queue could not start this task.",
    }
    .to_owned()
}

fn resumable_automatic_queue(sprint: &Value) -> bool {
    matches!(sprint["runMode"].as_str(), Some("automatic" | "hybrid"))
        && sprint["queueState"] == "paused"
        && system_hold(sprint)
}

/// A pause the scheduler made, rechecked on every pass. The owner's own pause
/// ("Paused by you") and a mode switch are never lifted by the scheduler.
fn system_hold(sprint: &Value) -> bool {
    if matches!(
        sprint["automaticQueue"]["kind"].as_str(),
        Some("continuation" | "owner_integration" | "settlement_blocked" | "start_refused")
    ) {
        return true;
    }
    // Hybrid pauses saved as free text by earlier versions, matched exactly. Nothing
    // writes these any more. A failed-settle text is not one of them: earlier
    // versions also wrote it over the owner's own pause, so it proves nothing.
    sprint["runMode"] == "hybrid"
        && sprint["automaticQueue"].is_null()
        && sprint["pauseReason"].as_str().is_some_and(|reason| {
            matches!(
                reason,
                "A sprint task is missing; review the queue."
                    | "A task was cancelled. Review the task before continuing the queue."
                    | "A task needs attention or has an unknown outcome."
                    | "A result has missing or unsuccessful checks."
                    | "Resolve the task conflict first."
                    | "No task can run; review dependencies."
            ) || reason.starts_with("Start was refused: ")
        })
}

/// The runtime itself rechecks the condition on every pass and continues on its own.
pub(super) const BY_RUNTIME: &str = "runtime";
/// The assigned agents fix it within their granted permissions; the runtime sends
/// the work back by itself. No hold is persisted while an agent repair runs.
pub(super) const BY_AGENT: &str = "agent";
/// Only the owner can decide. The hold names what is asked and the actions.
pub(super) const BY_OWNER: &str = "owner";

/// What a hold's decision depends on beyond its kind and code.
#[derive(Default, Clone, Debug)]
pub(super) struct HoldFacts {
    /// A fresh agent repair is possible if the owner allows it.
    pub repair: bool,
    /// The owner can allow Automatic to finish the recorded result as it is.
    pub finish: bool,
    /// Hybrid: the owner's acceptance is all that is asked; merging stays theirs.
    pub accept_only: bool,
    /// Concrete items the owner is asked to allow, in plain language.
    pub asks: Vec<String>,
}

/// Who acts next on an Automatic queue hold, and which owner actions exist.
/// This is the one place the decision is made: every hold the scheduler
/// persists is built by `automatic_hold`, which calls it. An owner hold always
/// has at least one action; the last one is always the separate Close as is.
pub(super) fn hold_decision(
    kind: &str,
    code: &str,
    task: Option<&str>,
    facts: &HoldFacts,
) -> (&'static str, Vec<&'static str>) {
    let close = |mut actions: Vec<&'static str>| {
        if task.is_some() {
            actions.push("close_as_is");
        } else if actions.is_empty() {
            // A hold that names no task can still be checked again on request.
            actions.push("retry");
        }
        actions
    };
    let repair = || {
        if facts.repair {
            vec!["allow_repair"]
        } else {
            vec![]
        }
    };
    match (kind, code) {
        // No-stop policy: every continuation hold is the runtime's own. A route,
        // login or quota blocker resumes once the route qualifies (or another one
        // does); a pause an earlier version recorded for the owner (an attempt cap,
        // an owner blocker, an unknown outcome) is resumed by the runtime with a
        // changed strategy and the uncertainty recorded, never replayed blindly.
        ("continuation", _) => (BY_RUNTIME, vec![]),
        ("owner_integration", "owner_accepted") => (BY_OWNER, close(vec!["allow_finish"])),
        ("owner_integration", _) => (BY_OWNER, close(vec!["confirm_integration"])),
        ("start_refused", "native_worktree_unavailable") => {
            (BY_OWNER, close(vec!["change_run_mode"]))
        }
        ("start_refused", _) => (BY_OWNER, close(vec!["retry"])),
        // A missing record or a conflict being resolved: rechecked on every pass.
        ("settlement_blocked", "task_missing" | "conflict_resolving" | "queue_blocked")
            if code != "queue_blocked" || task.is_none() =>
        {
            (BY_RUNTIME, vec![])
        }
        // An acceptance the finish cannot use (saved without its accepted revision,
        // or made by hand) is never re-accepted by the runtime: the owner decides.
        ("settlement_blocked", "automatic_acceptance_required") => (BY_OWNER, close(repair())),
        ("settlement_blocked", "task_conflict") => (BY_OWNER, close(vec!["resolve_conflict"])),
        ("settlement_blocked", "task_paused") => (BY_OWNER, close(vec!["resume_task"])),
        ("settlement_blocked", "task_blocked" | "task_recovery") => {
            (BY_OWNER, close(vec!["confirm_and_continue"]))
        }
        // Agent work first; the owner is asked only after the bounded repairs.
        ("settlement_blocked", "task_failed") => (BY_OWNER, close(repair())),
        ("settlement_blocked", "automatic_acceptance_blocked") => {
            let mut actions = match (facts.finish, facts.accept_only) {
                (true, true) => vec!["accept_result"],
                (true, false) => vec!["allow_finish"],
                _ => vec![],
            };
            actions.extend(repair());
            (BY_OWNER, close(actions))
        }
        (
            "settlement_blocked",
            "stale_result"
            | "workspace_drift"
            | "task_files_changed"
            | "task_content_unknown"
            | "review_required"
            | "integration_failed",
        ) => (BY_OWNER, close(repair())),
        // Stopped by the owner, unknown state, unknown finish outcome, or a
        // dependency that cannot finish: only the owner can settle it.
        (
            "settlement_blocked",
            "task_stopped"
            | "task_state_unknown"
            | "outcome_unknown"
            | "integration_outcome_unknown"
            | "dependencies_blocked"
            | "queue_blocked",
        ) => (BY_OWNER, close(vec![])),
        // A finish precondition outside the task (a branch, the target checkout,
        // Git): rechecked on every pass, and the owner can ask for it now.
        ("settlement_blocked", _) => (BY_OWNER, close(vec!["retry"])),
        _ => (BY_OWNER, close(vec!["retry"])),
    }
}

/// Who acts first on a hold condition. For agent-first conditions the scheduler
/// sends the work back by itself (bounded, deduplicated) and persists a hold,
/// decided by `hold_decision`, only when no agent repair can start.
pub(super) fn first_actor(kind: &str, code: &str) -> &'static str {
    match (kind, code) {
        (
            "settlement_blocked",
            "task_failed"
            | "automatic_acceptance_blocked"
            | "stale_result"
            | "workspace_drift"
            | "task_files_changed"
            | "review_required",
        ) => BY_AGENT,
        _ => hold_decision(kind, code, None, &HoldFacts::default()).0,
    }
}

/// Whether the owner can let the recorded result finish (Automatic) or accept it
/// (Hybrid) as it is. Only Automatic's own finish needs the project folder; a Hybrid
/// acceptance works in a separate worktree too, and merging stays the owner's.
pub(super) fn finish_offered(sprint: &Value, record: &Value, code: &str, unsure: bool) -> bool {
    let actions = &record["reviewActions"];
    code == "automatic_acceptance_blocked"
        && !unsure
        && (sprint["runMode"] == "hybrid" || record["checkout"]["kind"] == "main" || record["checkout"]["shared"] == true)
        && record["conflict"].as_str().is_none_or(|c| c == "none")
        && record["integration"].is_null()
        && (actions["accept"] == true || actions["acknowledgeOpenChecks"] == true)
}

/// The owner's plain-language request for the primary action, when the hold
/// carries no more specific items.
fn default_ask(action: &str) -> &'static str {
    match action {
        "allow_finish" => "Allow Automatic to finish this result with its recorded checks.",
        "accept_result" => "Accept this result with its recorded checks. The queue then continues; merging stays your decision.",
        "allow_repair" => "Let the agent fix this and publish a fresh result. Automatic checks it again and finishes the task when it passes.",
        "allow_retries" => "Allow the agent more automatic attempts. It continues from the recorded state.",
        "confirm_and_continue" => "Check the recorded work, then let the agent continue from the current state. Actions whose outcome is unknown are not repeated.",
        "resume_task" => "Resume the task you paused. Automatic then continues the queue.",
        "retry" => "Fix the cause named above, then let Automatic try again.",
        "confirm_integration" => "Confirm how to bring this result into the target branch.",
        "resolve_conflict" => "Resolve the conflict with the target branch.",
        "change_run_mode" => "Automatic runs only in the project folder. Switch this sprint to Hybrid or Manual to use a separate worktree.",
        _ => "Close the task as is. The queue then continues with the next task.",
    }
}

pub(super) fn automatic_hold(
    kind: &str,
    task: Option<&str>,
    code: &str,
    reason: &str,
    facts: &HoldFacts,
) -> Value {
    let (actor, actions) = hold_decision(kind, code, task, facts);
    let asks: Vec<String> = if actor != BY_OWNER {
        vec![]
    } else if facts.asks.is_empty() {
        vec![default_ask(actions.first().copied().unwrap_or("close_as_is")).to_owned()]
    } else {
        facts.asks.iter().take(10).cloned().collect()
    };
    json!({"kind":kind,"taskId":task,"code":code,"reason":reason,
        "needsOwner":actor == BY_OWNER,"nextActor":actor,"ownerActions":actions,"asks":asks})
}

/// Dispatched actions of a held task whose outcome is still unknown, the same
/// receipts `require_known_effects` refuses on: supervised commands only, which
/// the runtime settles by itself, or any other effect it can never confirm.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Unsettled {
    Commands,
    Effects,
}
pub(super) const SETTLING_REASON: &str = "Waiting for an unconfirmed command of this task to settle. Automatic continues by itself once it has.";
pub(super) const UNSETTLED_REASON: &str = "An action of this task never confirmed whether it finished, and Cortex cannot find out by itself.";
pub(super) const UNSETTLED_CLOSE_ASK: &str = "Check the project folder, then close the task as is. Nothing is repeated, and the unconfirmed action stays recorded as unknown, never as done.";
/// The owner actions whose handlers accept a task with an unknown effect. Allow a
/// repair, more attempts, and a finish or acceptance are refused or unsafe until
/// it is known; Close as is records the owner's acknowledgement (see `task_actions`).
pub(super) const ACCEPTED_UNSETTLED: &[&str] = &[
    "confirm_and_continue",
    "resume_task",
    "retry",
    "confirm_integration",
    "resolve_conflict",
    "change_run_mode",
    "close_as_is",
];

/// The hold persisted while a task has an unknown effect. While the runtime can
/// still settle it (supervised commands, with no time cap) the runtime
/// acts next and the owner is asked nothing; otherwise the owner is offered only
/// actions the service accepts. The decision it replaces is kept in `settling.hold`
/// and comes back once the effect is known. A hold without a task, a refused start,
/// and a task whose effects are known are unchanged.
pub(super) fn settling_hold(
    hold: Value,
    prior: &Value,
    unsettled: Option<Unsettled>,
    now: i64,
) -> Value {
    let original = if hold["settling"]["hold"].is_object() {
        hold["settling"]["hold"].clone()
    } else {
        hold
    };
    let Some(unsettled) = unsettled else {
        return original;
    };
    if original["taskId"].as_str().is_none() || original["kind"] == "start_refused" {
        return original;
    }
    let since = prior["settling"]["since"]
        .as_i64()
        .filter(|_| prior["taskId"] == original["taskId"])
        .unwrap_or(now);
    // No-stop policy: the runtime keeps waiting for its own command evidence (no
    // time cap); only an effect it can never confirm reaches the owner here.
    let waiting = unsettled == Unsettled::Commands;
    let (actor, reason, actions, asks) = if waiting {
        (BY_RUNTIME, SETTLING_REASON, vec![], vec![])
    } else {
        let mut actions: Vec<&str> = original["ownerActions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|a| *a != "close_as_is")
            .filter_map(|a| ACCEPTED_UNSETTLED.iter().copied().find(|k| *k == a))
            .collect();
        let mut asks: Vec<String> = actions.iter().map(|a| default_ask(a).to_owned()).collect();
        actions.push("close_as_is");
        asks.push(UNSETTLED_CLOSE_ASK.to_owned());
        (BY_OWNER, UNSETTLED_REASON, actions, asks)
    };
    json!({"kind":original["kind"],"taskId":original["taskId"],"code":original["code"],
        "reason":reason,"needsOwner":actor == BY_OWNER,"nextActor":actor,
        "ownerActions":actions,"asks":asks,
        "settling":{"since":since,"owner":actor == BY_OWNER,"hold":original}})
}

/// Codes the running queue derives again from the task once its effects are
/// known, without repeating any effect. Any other settled hold is restored as it was.
fn rederived(kind: &str, code: &str) -> bool {
    kind == "continuation"
        || kind == "settlement_blocked"
            && matches!(
                code,
                "task_failed"
                    | "task_blocked"
                    | "task_recovery"
                    | "task_stopped"
                    | "task_paused"
                    | "task_state_unknown"
                    | "dependencies_blocked"
                    | "task_missing"
                    | "conflict_resolving"
                    | "task_conflict"
                    | "queue_blocked"
                    | "archived_task"
                    | "stale_result"
                    | "workspace_drift"
                    | "task_files_changed"
                    | "review_required"
                    | "automatic_acceptance_blocked"
            )
}

/// Which unknown effects a sprint task's runtime has, read from the same receipts
/// `require_known_effects` checks. The runtime re-settles every command receipt on its
/// own passes (`resettle_orphaned_commands`) once nothing can still run it; a payload
/// that outlives its supervisor keeps it open until the host settles it. Any
/// other receipt (a write, an edit, a call) can never be settled by it.
pub(super) async fn unsettled_effects(
    runtime: &Runtime,
    project: &str,
    task: &str,
) -> Option<Unsettled> {
    let (project, task) = (project.to_owned(), task.to_owned());
    let read = runtime.host.executor.read(move |db| {
        use rusqlite::OptionalExtension;
        let runtime: Option<String> = db
            .query_row(
                "SELECT e.runtime_task_id FROM workspace_tasks w JOIN workspace_executions e
                 ON e.execution_id=w.execution_id AND e.task_id=w.task_id
                 WHERE w.project_id=? AND w.task_id=?",
                (&project, &task),
                |row| row.get(0),
            )
            .optional()?;
        let Some(runtime) = runtime else {
            return Ok(None);
        };
        Ok(db.query_row(
            "SELECT max(operation NOT IN ('command_run','outside_command'))
             FROM runtime_requests WHERE task_id=?
                AND operation IN ('outside_command','outside_write','command_run','project_write','project_edit',
                    'project_mkdir','project_delete','project_skill_link','draft_write','external_call')
                AND json_extract(result_json,'$.complete') IS NOT 1
                AND json_extract(result_json,'$.status') IS NOT 'complete'
                AND json_extract(result_json,'$.settled') IS NOT 1",
            [&runtime],
            |row| row.get::<_, Option<bool>>(0),
        )?)
    });
    match read.ok()?.await {
        Ok(Ok(Some(true))) => Some(Unsettled::Effects),
        Ok(Ok(Some(false))) => Some(Unsettled::Commands),
        _ => None,
    }
}

/// The same hold with its decision recomputed. A saved hold from an earlier
/// version (no `nextActor`) is rewritten on the next pass.
fn hold_current(prior: &Value) -> bool {
    prior["nextActor"].is_string()
        && prior["ownerActions"].is_array()
        && prior["asks"].is_array()
}

fn continuation_hold(task: &str, t: &Value) -> Option<Value> {
    let held = &t["automaticContinuation"]["pause"];
    held.is_object().then(|| {
        let code = held["kind"].as_str().unwrap_or("outcome_unknown");
        let reason = match code {
            "continuation_cap" | "owner_blocker" => "The agent continues this task with a changed approach.",
            "outcome_unknown" => "An action of this task may or may not have finished. The agent checks it and continues; it is not repeated blindly.",
            _ => match held["reason"].as_str() {
                Some("engine_authentication_required" | "authorization_required") => "The engine needs you to sign in again. Automatic continues by itself once it can connect.",
                Some("engine_quota_exhausted") => "The engine usage limit is reached. Automatic continues by itself when the engine is available again.",
                Some(cortex_core::lifecycle::automatic::NO_MODEL_LISTED_FOR_REVIEW) => "No model may review this work: no usable model has Review in its Preferred work. Add Review to a model in Settings > Orchestration > Engines and models; Automatic continues by itself once one can review.",
                Some(cortex_core::lifecycle::automatic::NO_MODEL_LISTED_FOR_WORK_KIND) => "No usable model has this kind of work in its Preferred work. Add it to a model in Settings > Orchestration > Engines and models; Automatic continues by itself once one can do it.",
                _ => "The engine or its route is not available right now. Automatic continues by itself once it is.",
            },
        };
        automatic_hold("continuation", Some(task), code, reason, &HoldFacts::default())
    })
}

/// Skipped members do not block the queue, but skipping is not completion
/// evidence for their dependents.
fn automatic_skip(record: &Value, disposition: Option<&(String, Option<String>)>) -> bool {
    record["purged"] == true
        || !record["archive"].is_null()
        || !record["archiveHold"].is_null()
        || disposition.is_some_and(|(_, closed)| {
            matches!(closed.as_deref(), Some("cancel" | "unconfirmed"))
        })
}

/// Whether the runtime's task projection carries this warning.
fn warned(task: &Value, warning: &str) -> bool {
    task["warnings"]
        .as_array()
        .is_some_and(|w| w.iter().any(|v| v == warning))
}

#[cfg(test)]
#[test]
fn automatic_queue_skips_a_retained_archive_without_certifying_its_work() {
    let members = ["archived", "dependent", "independent"].map(str::to_owned);
    let tasks = ["dependent", "independent"]
        .into_iter()
        .map(|id| (id.to_owned(), json!({"phase":"not_started"})))
        .collect();
    let records = json!({
        "archived":{"archive":{"at":1}},
        "dependent":{"dependencies":["archived"]},
        "independent":{}
    });
    let mut records = records;
    let disposition = HashMap::from([("archived".into(), ("ready_to_merge".into(), None))]);
    records["archived"]["archive"]["unfinished"] = json!(true);
    for hybrid in [false, true] {
        assert!(matches!(
            automatic_queue_step(&members, &tasks, &records, &disposition, hybrid),
            QueueStep::Start("independent")
        ));
    }
    // Even a restored draft must not return to either automatic queue.
    records["archived"]["archive"] = Value::Null;
    records["archived"]["archiveHold"] = json!("draft");
    for hybrid in [false, true] {
        assert!(matches!(
            automatic_queue_step(&members, &tasks, &records, &disposition, hybrid),
            QueueStep::Start("independent")
        ));
    }
}

#[cfg(test)]
#[test]
fn a_stopped_member_names_its_actor_and_never_says_open_it() {
    let members = ["held".to_owned()];
    let records = json!({"held":{}});
    let expect = [
        ("failed", "task_failed"),
        ("blocked", "task_blocked"),
        ("stopped", "task_stopped"),
        ("paused", "task_paused"),
        ("unknown", "task_state_unknown"),
        ("recovery", "task_recovery"),
    ];
    for (phase, code) in expect {
        let tasks = [("held".to_owned(), json!({"phase":phase}))]
            .into_iter()
            .collect();
        let QueueStep::Hold(hold) =
            automatic_queue_step(&members, &tasks, &records, &HashMap::new(), false)
        else {
            panic!("{phase} must hold the queue");
        };
        assert_eq!(hold["code"], code);
        assert_eq!(hold["nextActor"], BY_OWNER, "{hold}");
        let reason = hold["reason"].as_str().unwrap();
        assert!(
            !reason.contains("Open") && !reason.contains('_'),
            "{reason}"
        );
        assert!(
            !hold["ownerActions"].as_array().unwrap().is_empty(),
            "{hold}"
        );
    }
    assert_eq!(first_actor("settlement_blocked", "task_failed"), BY_AGENT);
    // Waiting on the continuation the runtime already owns is no hold at all.
    let tasks = [(
        "held".to_owned(),
        json!({"phase":"recovery","waitReason":"automatic_continuation_pending"}),
    )]
    .into_iter()
    .collect();
    assert!(matches!(
        automatic_queue_step(&members, &tasks, &records, &HashMap::new(), false),
        QueueStep::Wait
    ));
    // A session still being stopped is the runtime's to decide: no hold.
    let tasks = [(
        "held".to_owned(),
        json!({"phase":"recovery","waitReason":null,"warnings":["process_exit_pending"]}),
    )]
    .into_iter()
    .collect();
    assert!(matches!(
        automatic_queue_step(&members, &tasks, &records, &HashMap::new(), false),
        QueueStep::Wait
    ));
    // An exit the host recorded as unconfirmed is never "I checked it, continue"
    // (continuing is refused until it is known): the owner can only close it as is.
    let tasks = [(
        "held".to_owned(),
        json!({"phase":"recovery","waitReason":null,"warnings":["process_exit_unconfirmed"]}),
    )]
    .into_iter()
    .collect();
    let QueueStep::Hold(hold) =
        automatic_queue_step(&members, &tasks, &records, &HashMap::new(), false)
    else {
        panic!("an unconfirmed exit must hold the queue");
    };
    assert_eq!(hold["code"], "task_state_unknown");
    assert_eq!(hold["nextActor"], BY_OWNER);
    assert_eq!(hold["ownerActions"], json!(["close_as_is"]));
}

enum QueueStep<'a> {
    Hold(Value),
    Complete,
    Wait,
    Start(&'a str),
}

#[cfg(test)]
#[test]
fn owner_merge_handoff_requires_explicit_checks_in_both_queue_modes() {
    let members = vec!["t".to_owned()];
    let tasks = HashMap::from([("t".to_owned(), json!({"phase":"result"}))]);
    for state in ["not_requested", "request_unknown"] {
        let records = json!({"t":{"conflict":"none","merge":{"freshChecks":{"state":state}}}});
        for hybrid in [false, true] {
            let QueueStep::Hold(hold) =
                automatic_queue_step(&members, &tasks, &records, &HashMap::new(), hybrid)
            else {
                panic!("fresh checks require the owner in both modes");
            };
            assert_eq!(hold["code"], "task_conflict");
            assert_eq!(hold["nextActor"], BY_OWNER);
            assert_eq!(hold["ownerActions"][0], "resolve_conflict");
        }
    }
}

fn awaiting_owner_merge_checks(record: &Value) -> bool {
    matches!(record["merge"]["freshChecks"]["state"].as_str(),
        Some("not_requested" | "request_unknown"))
        && record["reviewActions"]["accept"] != true
}

type Disposition = HashMap<String, (String, Option<String>)>;

fn dependency_settled(id: &str, records: &Value, disposition: &Disposition, hybrid: bool) -> bool {
    disposition.get(id).is_some_and(|(status, closed)| {
        (status == "done" || hybrid && status == "ready_to_merge")
            && !matches!(closed.as_deref(), Some("cancel" | "unconfirmed"))
            && records[id]["purged"] != true
            && records[id]["archive"]["unfinished"] != true
            && records[id]["archiveHold"].is_null()
    })
}

fn dependencies_ready(record: &Value, done: impl Fn(&str) -> bool) -> bool {
    match record.get("dependencies") {
        None => true,
        Some(Value::Array(deps)) => deps.iter().all(|dep| dep.as_str().is_some_and(&done)),
        _ => false,
    }
}

#[cfg(test)]
fn automatic_queue_step<'a>(
    members: &'a [String], tasks: &HashMap<String, Value>, records: &Value,
    disposition: &Disposition, hybrid: bool,
) -> QueueStep<'a> {
    queue_step_in_order(members, members, tasks, records, disposition, hybrid)
}

fn queue_step_in_order<'a>(
    members: &'a [String],
    priority: &'a [String],
    tasks: &HashMap<String, Value>,
    records: &Value,
    disposition: &Disposition,
    hybrid: bool,
) -> QueueStep<'a> {
    let done = |id: &str| dependency_settled(id, records, disposition, hybrid);
    let pending: Vec<_> = members
        .iter()
        .map(String::as_str)
        .filter(|id| !done(id) && !automatic_skip(&records[*id], disposition.get(*id)))
        .collect();
    let mut waiting = false;
    let none = HoldFacts::default();
    for &id in &pending {
        let Some(task) = tasks.get(id) else {
            return QueueStep::Hold(automatic_hold(
                "settlement_blocked",
                None,
                "task_missing",
                "A sprint task could not be read. Automatic checks again by itself.",
                &none,
            ));
        };
        let record = &records[id];
        if matches!(record["merge"]["handoff"]["state"].as_str(), Some("unknown" | "dispatch_unknown"))
            || record["merge"]["gitState"] == "unknown"
        {
            return QueueStep::Hold(automatic_hold(
                "settlement_blocked", Some(id), "task_conflict",
                "Inspect the retained merge. Cortex will not repeat the handoff or touch the target.",
                &none,
            ));
        }
        if awaiting_owner_merge_checks(record) && matches!(
            task["phase"].as_str(), Some("result" | "failed" | "paused" | "stopped")
        ) {
            return QueueStep::Hold(automatic_hold(
                "settlement_blocked", Some(id), "task_conflict",
                "This merge needs a request for fresh independent review and verification.",
                &none,
            ));
        }
        match record["conflict"].as_str() {
            Some("resolving") => {
                return QueueStep::Hold(automatic_hold(
                    "settlement_blocked",
                    Some(id),
                    "conflict_resolving",
                    "A conflict of this task is being resolved. Automatic continues when it is done.",
                    &none,
                ));
            }
            Some("conflict" | "failed") => {
                return QueueStep::Hold(automatic_hold(
                    "settlement_blocked",
                    Some(id),
                    "task_conflict",
                    "This task has a conflict with the target branch.",
                    &none,
                ));
            }
            _ => {}
        }
        let (code, reason) = match task["phase"].as_str() {
            Some("not_started") => continue,
            // Repaired by the agent first (see `queue_pass`); held only after the bounded repairs.
            Some("failed") => ("task_failed", "The last run of this task ended as failed."),
            Some("blocked") => (
                "task_blocked",
                "The task could not continue: its engine or a required step did not start.",
            ),
            Some("stopped") => ("task_stopped", "This task was stopped before it finished."),
            Some("paused") => ("task_paused", "You paused this task."),
            // A session still being stopped: its shutdown observation decides it.
            Some("recovery") if warned(task, "process_exit_pending") => {
                waiting = true;
                continue;
            }
            // Continuing is refused while the exit is unknown. Each pass re-checks it
            // first (`resettle_unconfirmed_exits`) and, once settled, the task goes on
            // by itself; while the evidence stays inconclusive only Close as is (which
            // replays nothing) ends it.
            Some("recovery") if warned(task, "process_exit_unconfirmed") => (
                "task_state_unknown",
                "A process of this task did not confirm that it stopped. Nothing is repeated, and the task cannot continue until that is known.",
            ),
            Some("recovery") if task["waitReason"] != "automatic_continuation_pending" => (
                "task_recovery",
                "The task stopped and some of its actions may not have finished. They are not repeated automatically.",
            ),
            Some("unknown") | None => (
                "task_state_unknown",
                "How this task ended cannot be confirmed. Nothing is repeated automatically.",
            ),
            _ => {
                waiting = true;
                continue;
            }
        };
        return QueueStep::Hold(automatic_hold(
            "settlement_blocked",
            Some(id),
            code,
            reason,
            &none,
        ));
    }
    if waiting {
        return QueueStep::Wait;
    }
    if pending.is_empty() {
        return QueueStep::Complete;
    }
    let unmet = |id: &str| {
        records[id]["dependencies"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|dep| !dep.as_str().is_some_and(done))
            .map(|dep| dep.as_str().unwrap_or_default().to_owned())
    };
    // A draft that a proposed sprint revision holds back waits for it, without a hold.
    let held = |id: &str| records[id]["revisionPending"] == true;
    let eligible: BTreeSet<_> = pending.iter().copied().filter(|id| !held(id)).collect();
    if let Some(id) = priority.iter().map(String::as_str)
        .find(|id| eligible.contains(id) && records[*id]["deliverableBlocker"].is_null()
            && dependencies_ready(&records[*id], done))
    {
        return QueueStep::Start(id);
    }
    if pending.iter().any(|id| held(id)) {
        return QueueStep::Wait;
    }
    if let Some(id) = pending.iter().copied().find(|id| {
        records[*id]["deliverableBlocker"].is_object() && dependencies_ready(&records[*id], done)
    }) {
        let blocker = &records[id]["deliverableBlocker"];
        let input = &blocker["requirement"];
        let name = input["name"].as_str().unwrap_or("required output");
        let version = input["version"].as_str().unwrap_or("");
        let producer = input["from"].as_str().and_then(|producer| records[producer]["title"].as_str())
            .unwrap_or("the prerequisite");
        let reason = format!(
            "This task requires {name} {version} from {producer}. The matching reviewed, verified output is unavailable."
        );
        return QueueStep::Hold(automatic_hold(
            "settlement_blocked", Some(id), blocker["code"].as_str().unwrap_or("deliverable_missing"),
            &reason, &HoldFacts::default(),
        ));
    }
    // Name the task that is actually stuck: one whose unfinished prerequisite this
    // queue will never run (cancelled, skipped or outside the sprint), not merely
    // the first task waiting behind it. A cycle falls back to the first pending task.
    let stuck = |id: &str| {
        records[id]["dependencies"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .find(|dep| !done(dep) && !pending.contains(dep))
            .map(str::to_owned)
    };
    let (blocked, dep) = match pending
        .iter()
        .copied()
        .find_map(|id| Some((id, stuck(id)?)))
    {
        Some((id, dep)) => (Some(id), Some(dep)),
        None => {
            let first = pending.first().copied();
            (first, first.and_then(unmet))
        }
    };
    let name = dep
        .as_deref()
        .and_then(|d| records[d]["title"].as_str())
        .filter(|t| !t.trim().is_empty())
        .map(|t| {
            format!(
                "\u{201c}{}\u{201d}",
                t.trim().chars().take(120).collect::<String>()
            )
        })
        .unwrap_or_else(|| "a task it depends on".to_owned());
    let cancelled = dep.as_deref().is_some_and(|d| {
        disposition
            .get(d)
            .is_some_and(|(_, closed)| closed.as_deref() == Some("cancel"))
            || records[d]["closedAs"] == "cancel"
    });
    let reason = if cancelled {
        format!(
            "This task depends on {name}, which was cancelled, so it cannot start by itself."
        )
    } else {
        format!("This task depends on {name}, which is not finished and cannot be finished by this queue.")
    };
    let facts = HoldFacts {
        asks: vec![format!(
            "Close this task as is or cancel it, because {name} will not finish. The queue then continues."
        )],
        ..HoldFacts::default()
    };
    QueueStep::Hold(automatic_hold(
        "settlement_blocked",
        blocked,
        "dependencies_blocked",
        &reason,
        &facts,
    ))
}
fn queue_disposition(snapshot: &Snapshot) -> Disposition {
    snapshot.roster.iter().map(|id| {
        let fact = snapshot.tasks.get(id);
        let mut disposition = member_disposition(&snapshot.records[id], fact);
        // Neither a missing binding nor a contradictory Core result proves settlement.
        if snapshot.projection_member(id).is_some_and(|(_, _, status, _)| status != "confirmed")
            && !matches!(disposition.1.as_deref(), Some("cancel" | "unconfirmed"))
        {
            disposition.0.clear();
        }
        (id.clone(), disposition)
    }).collect()
}

fn automatic_checkout_unavailable(sprint: &Value) -> bool {
    // Automatic runs in the project folder or in one worktree shared by the whole sprint.
    sprint["checkout"]["kind"].as_str().is_some_and(|kind| {
        kind != "main" && !(kind == "worktree" && sprint["checkout"]["shared"] == true)
    })
}

fn observed_step<'a>(snapshot: &'a Snapshot, disposition: &Disposition, hybrid: bool) -> QueueStep<'a> {
    queue_step_in_order(&snapshot.roster, &snapshot.priority, &snapshot.tasks,
        &snapshot.records, disposition, hybrid)
}

pub(crate) fn selected_now<'a>(snapshot: &'a Snapshot) -> Option<&'a str> {
    let sprint = &snapshot.sprint;
    if sprint["queueState"] != "running" || !crate::machine::queue_driven_mode(sprint)
        || (!sprint["deleting"].is_null() && sprint["deleting"] != false)
    {
        return None;
    }
    let hybrid = sprint["runMode"] == "hybrid";
    let disposition = queue_disposition(snapshot);
    if !hybrid && (automatic_checkout_unavailable(sprint) || snapshot.roster.iter().any(|id| {
        !automatic_skip(&snapshot.records[id], disposition.get(id))
            && !dependency_settled(id, &snapshot.records, &disposition, false)
            && snapshot.tasks.get(id).is_some_and(|t| continuation_hold(id, t).is_some())
    })) {
        return None;
    }
    match observed_step(snapshot, &disposition, hybrid) {
        QueueStep::Start(id) => Some(id),
        _ => None,
    }
}

/// The first pick uses the queue decision itself. Future picks assume only
/// successful settlement and never skip a current hold to promise a launch.
pub(crate) fn live_execution_order(snapshot: &Snapshot) -> Value {
    let sprint = &snapshot.sprint;
    let mode = sprint["runMode"].as_str().filter(|m| matches!(*m, "hybrid" | "automatic")).unwrap_or("manual");
    let queue = sprint["queueState"].as_str().filter(|s| matches!(*s, "idle" | "running" | "paused" | "complete")).unwrap_or("idle");
    let closed = !sprint["archivedAt"].is_null()
        || (!sprint["deleting"].is_null() && sprint["deleting"] != false);
    let hybrid_settlement = mode != "automatic";
    let disposition = queue_disposition(snapshot);
    let mut working = Vec::new();
    let mut pending = BTreeSet::new();
    let mut unranked = HashMap::new();
    let mut simulated = BTreeSet::new();
    for id in &snapshot.roster {
        let (draft, claim, status, _) = snapshot.projection_member(id).unwrap();
        let record = &snapshot.records[id];
        let fact = snapshot.tasks.get(id);
        let phase = fact.and_then(|v| v["phase"].as_str());
        if dependency_settled(id, &snapshot.records, &disposition, hybrid_settlement) {
            simulated.insert(id.as_str());
        }
        if automatic_skip(record, disposition.get(id)) {
            continue;
        }
        if status != "confirmed" || phase == Some("unknown") {
            unranked.insert(id.as_str(), if status == "missing" { "task_missing" } else { "task_state_unknown" });
            continue;
        }
        if matches!(phase, Some("planning" | "execution" | "review" | "verification" | "finalizing")) {
            working.push((claim, id.as_str()));
        }
        if draft && record["revisionPending"] == true {
            unranked.insert(id.as_str(), "sprint_revision_pending");
        } else if draft && record["deliverableBlocker"].is_object() {
            unranked.insert(id.as_str(), record["deliverableBlocker"]["code"].as_str().unwrap_or("deliverable_missing"));
        } else if draft {
            pending.insert(id.as_str());
        } else if fact.is_some_and(|t| t["executionId"].is_string())
            && matches!(phase, Some("planning" | "execution" | "review" | "verification" | "finalizing" |
                "waiting" | "result"))
        {
            // The queue might get a successfully settled result before it can
            // start a draft. A failure, cancellation or hold is not assumed away.
            simulated.insert(id.as_str());
        }
    }
    working.sort_by_key(|&(claim, id)| (claim.is_none(), claim, id));
    let working: Vec<_> = working.into_iter().map(|(ordinal, id)| {
        json!({"taskId":id,"startOrder":ordinal.map(|x| x.to_string())})
    }).collect();
    let next_candidate = selected_now(snapshot);
    let step = observed_step(snapshot, &disposition, hybrid_settlement);
    let hold_code = if closed {
        Some("sprint_closed")
    } else if mode != "manual" && queue == "paused" {
        Some(if system_hold(sprint) { "queue_held" } else { "queue_paused" })
    } else if mode == "automatic" && automatic_checkout_unavailable(sprint) {
        Some("native_worktree_unavailable")
    } else if mode != "manual" && queue == "complete" {
        Some("queue_complete")
    } else if mode != "manual" && matches!(&step, QueueStep::Hold(hold) if hold["code"] != "dependencies_blocked") {
        Some("queue_blocked")
    } else {
        None
    };
    let mut not_started = Vec::new();
    let mut stop = hold_code;
    while stop.is_none() {
        // Prefer the exact live pick before simulating future successful
        // settlements; this avoids treating unsettled prerequisites as done now.
        let candidate = if not_started.is_empty() && next_candidate.is_some() {
            next_candidate
        } else {
            snapshot.priority.iter().map(String::as_str).find(|id| {
                pending.contains(id) && dependencies_ready(&snapshot.records[*id], |d| simulated.contains(d))
            })
        };
        let Some(id) = candidate else { break; };
        if !pending.contains(id) { break; }
        if mode == "automatic" && snapshot.sprint["checkout"]["shared"] != true && snapshot.projection_member(id).is_some_and(|(_, _, _, checkout)| checkout == Some("worktree")) {
            unranked.insert(id, "native_worktree_unavailable");
            pending.remove(id);
            stop = Some("queue_blocked");
            break;
        }
        pending.remove(id);
        let is_next = not_started.is_empty() && next_candidate == Some(id);
        not_started.push(json!({"taskId":id,"rank":not_started.len()+1,
            "reason":null,"next":is_next,"conditional":mode != "manual" && !is_next}));
        simulated.insert(id);
    }
    for id in &snapshot.priority {
        let reason = unranked.get(id.as_str()).copied().or_else(|| {
            pending.contains(id.as_str()).then_some(stop.unwrap_or("dependencies_blocked"))
        });
        if let Some(reason) = reason {
            not_started.push(json!({"taskId":id,"rank":null,"reason":reason,"next":false,"conditional":false}));
        }
    }
    json!({"mode":mode,"queueState":queue,"working":working,"notStarted":not_started})
}

fn other(code: String) -> io::Error {
    io::Error::other(code)
}
/// Least time between two passes of one project, however often it is woken: no wake-up storm
/// costs more than the fixed two-second loop this replaces did.
pub(crate) const PASS_FLOOR: Duration = PASS_EVERY;
/// A project with a sprint in motion (a queue running, planning, or results waiting for an
/// automatic settle) is looked at this often even without an event, for a change no event
/// reports (a runtime whose reader is not open yet, a settle that deferred). Others never are.
pub(crate) const SAFETY_TICK: Duration = Duration::from_secs(30);

/// Projects whose sprints should be looked at now, named by whatever changed them.
#[derive(Default)]
pub(crate) struct SprintWake {
    woken: std::sync::Mutex<BTreeSet<String>>,
    notify: tokio::sync::Notify,
}
impl SprintWake {
    /// Something that can move this project's sprints on changed.
    pub(crate) fn project(&self, project: &str) {
        self.woken
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(project.to_owned());
        self.notify.notify_one();
    }
    fn take(&self) -> BTreeSet<String> {
        std::mem::take(&mut *self.woken.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

/// When each project's sprints are next passed. A project is passed once when it first
/// appears (service start or activation), on an owner or loop change of its sprint or task
/// records, on a runtime task event while it has a sprint in motion, and on the safety tick
/// while it has one. A project with no sprint in motion costs nothing between changes.
#[derive(Default, Debug)]
pub(crate) struct Schedule {
    projects: std::collections::BTreeMap<String, Slot>,
}
#[derive(Default, Debug)]
struct Slot {
    last: Option<tokio::time::Instant>,
    live: bool,
    pending: bool,
}
impl Schedule {
    /// The projects that exist now: a new one is due at once, a gone one is forgotten.
    pub(crate) fn known<'a>(&mut self, names: impl IntoIterator<Item = &'a str>) {
        let names: BTreeSet<&str> = names.into_iter().collect();
        self.projects.retain(|p, _| names.contains(p.as_str()));
        for name in names {
            self.projects.entry(name.to_owned()).or_insert(Slot {
                pending: true,
                ..Slot::default()
            });
        }
    }
    /// An owner or loop change of this project's sprint or task records.
    pub(crate) fn wake(&mut self, project: &str) {
        if let Some(slot) = self.projects.get_mut(project) {
            slot.pending = true;
        }
    }
    /// Whether the last pass of `project` found something in motion.
    pub(crate) fn live(&self, project: &str) -> bool {
        self.projects.get(project).is_some_and(|s| s.live)
    }
    /// A runtime task event: it matters only to a project with a sprint in motion.
    pub(crate) fn task_event(&mut self, project: &str) {
        if let Some(slot) = self.projects.get_mut(project).filter(|s| s.live) {
            slot.pending = true;
        }
    }
    /// Events were lost: every project with a sprint in motion is looked at again.
    pub(crate) fn lagged(&mut self) {
        for slot in self.projects.values_mut().filter(|s| s.live) {
            slot.pending = true;
        }
    }
    /// The projects to pass now, each no sooner than `PASS_FLOOR` after its last pass.
    pub(crate) fn due(&mut self, now: tokio::time::Instant) -> Vec<String> {
        let mut due = vec![];
        for (name, slot) in &mut self.projects {
            if slot.live && slot.last.is_some_and(|at| now >= at + SAFETY_TICK) {
                slot.pending = true;
            }
            if slot.pending && slot.last.is_none_or(|at| now >= at + PASS_FLOOR) {
                slot.pending = false;
                due.push(name.clone());
            }
        }
        due
    }
    /// A pass of `project` ended; `live` says whether it has a sprint in motion.
    pub(crate) fn passed(&mut self, project: &str, live: bool, now: tokio::time::Instant) {
        if let Some(slot) = self.projects.get_mut(project) {
            slot.last = Some(now);
            slot.live = live;
        }
    }
    /// When the loop must look again without any wake: a deferred wake past its floor, or a
    /// safety tick. `None` when no project has anything pending or in motion.
    pub(crate) fn next(&self) -> Option<tokio::time::Instant> {
        self.projects
            .values()
            .filter_map(|slot| match (slot.pending, slot.live, slot.last) {
                (true, _, None) => Some(tokio::time::Instant::now()),
                (true, _, Some(at)) => Some(at + PASS_FLOOR),
                (false, true, Some(at)) => Some(at + SAFETY_TICK),
                _ => None,
            })
            .min()
    }
}
/// A service event that can move a sprint on: a runtime task change of that project.
fn task_event(event: &Value) -> Option<&str> {
    let project = event["projectId"].as_str()?;
    match event["type"].as_str()? {
        "native_resync" => Some(project),
        "native_delta" => event["batch"]["changes"]
            .as_array()?
            .iter()
            .any(|c| c["kind"] == "task")
            .then_some(project),
        _ => None,
    }
}
#[cfg(test)]
mod schedule_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn a_project_with_no_sprint_in_motion_costs_nothing_until_it_changes() {
        let mut s = Schedule::default();
        s.known(["p"]);
        let now = tokio::time::Instant::now();
        // Service start: one pass.
        assert_eq!(s.due(now), vec!["p".to_owned()]);
        s.passed("p", false, now);
        assert_eq!(s.next(), None, "nothing in motion: no timer at all");
        // Hours pass, runtime tasks change: nothing is read.
        let later = now + Duration::from_secs(3600);
        s.task_event("p");
        s.lagged();
        assert!(s.due(later).is_empty());
        assert_eq!(s.next(), None);
        // An owner change of a sprint (a run mode, Run): one pass.
        s.wake("p");
        assert_eq!(s.due(later), vec!["p".to_owned()]);
    }
    #[test]
    fn a_sprint_in_motion_wakes_on_each_event_and_the_safety_tick() {
        let mut s = Schedule::default();
        s.known(["p", "q"]);
        let now = tokio::time::Instant::now();
        assert_eq!(s.due(now).len(), 2);
        s.passed("p", true, now);
        s.passed("q", false, now);
        assert_eq!(s.next(), Some(now + SAFETY_TICK));
        // A task state change, a settle outcome, a sprint patch: each wakes p, none sooner than the floor.
        for wake in [
            Schedule::task_event as fn(&mut Schedule, &str),
            Schedule::wake,
        ] {
            let at = now + PASS_FLOOR * 10;
            s.passed("p", true, at - PASS_FLOOR * 2);
            wake(&mut s, "p");
            assert_eq!(s.due(at), vec!["p".to_owned()]);
            s.passed("p", true, at);
            wake(&mut s, "p");
            assert!(s.due(at + PASS_FLOOR / 2).is_empty(), "within the floor");
            assert_eq!(s.next(), Some(at + PASS_FLOOR));
            assert_eq!(s.due(at + PASS_FLOOR), vec!["p".to_owned()]);
            s.passed("p", true, at + PASS_FLOOR);
        }
        // q has nothing in motion: its task events are ignored.
        s.task_event("q");
        assert!(!s.due(now + PASS_FLOOR * 100).contains(&"q".to_owned()));
        // With no event, p is looked at on the safety tick only.
        let last = now + PASS_FLOOR * 100;
        s.passed("p", true, last);
        assert!(s
            .due(last + SAFETY_TICK - Duration::from_millis(1))
            .is_empty());
        assert_eq!(s.due(last + SAFETY_TICK), vec!["p".to_owned()]);
        s.passed("p", true, last + SAFETY_TICK);
        // Activation of a new project: passed at once; a removed one is forgotten.
        s.known(["p", "r"]);
        assert_eq!(s.due(last + SAFETY_TICK), vec!["r".to_owned()]);
    }
    #[test]
    fn only_runtime_task_changes_count_as_task_events() {
        let delta = |kind: &str| json!({"type":"native_delta","projectId":"p","batch":{"changes":[{"kind":kind}]}});
        assert_eq!(task_event(&delta("task")), Some("p"));
        assert_eq!(task_event(&delta("history")), None);
        assert_eq!(
            task_event(&json!({"type":"native_resync","projectId":"p"})),
            Some("p")
        );
        // The loop's own writes tell windows to refresh; they are not task events.
        assert_eq!(
            task_event(&json!({"type":"invalidation","projectId":"p"})),
            None
        );
    }
    #[test]
    fn a_wake_names_its_project_once() {
        let wake = SprintWake::default();
        wake.project("p");
        wake.project("p");
        wake.project("q");
        assert_eq!(wake.take().into_iter().collect::<Vec<_>>(), ["p", "q"]);
        assert!(wake.take().is_empty());
    }
}
impl Service {
    pub(crate) async fn native_sprints(self: Arc<Self>) {
        let mut shutdown = self.shutdown.subscribe();
        let mut events = self.events.subscribe();
        let mut jobs = Jobs::default();
        let mut seen = BTreeSet::new();
        let mut schedule = Schedule::default();
        while !*shutdown.borrow_and_update() {
            let native: Vec<(String, Arc<Runtime>)> = self
                .state
                .lock()
                .await
                .native
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            jobs.0.retain(|_, j| !j.is_finished());
            jobs.1
                .retain(|project, _| native.iter().any(|(id, _)| id == project));
            schedule.known(native.iter().map(|(k, _)| k.as_str()));
            for project in self.sprint_wake.take() {
                schedule.wake(&project);
            }
            let due: BTreeSet<String> = schedule
                .due(tokio::time::Instant::now())
                .into_iter()
                .collect();
            for (project, runtime) in native.iter().filter(|(k, _)| due.contains(k)) {
                let first = seen.insert(project.clone());
                // One project's unreadable state must not stop the others; it is looked at again on the safety tick.
                let live = self
                    .sprint_pass(project, runtime, &mut jobs, first)
                    .await
                    .unwrap_or(true)
                    || jobs.0.keys().any(|(p, _)| p == project);
                schedule.passed(project, live, tokio::time::Instant::now());
            }
            let next = schedule.next();
            tokio::select! {
                _ = shutdown.changed() => (),
                _ = self.sprint_wake.notify.notified() => (),
                event = events.recv() => match event {
                    Ok(event) => if let Some(project) = task_event(&event) {
                        // A session exit recorded as unconfirmed is worth a pass in any
                        // project (one read, no process scan); the pass re-checks it.
                        if !schedule.live(project) {
                            if let Some((_, runtime)) = native.iter().find(|(k, _)| k == project) {
                                if runtime.unconfirmed_exits_pending().await.unwrap_or(false) {
                                    schedule.wake(project);
                                }
                            }
                        }
                        schedule.task_event(project);
                    },
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => schedule.lagged(),
                    // No sender is left only while the service stops.
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => tokio::time::sleep(SAFETY_TICK).await,
                },
                _ = tokio::time::sleep_until(next.unwrap_or_else(tokio::time::Instant::now)), if next.is_some() => (),
            }
        }
    }
    /// One automatic settle. Never an error for the pass: a settle that could not
    /// finish is `Failed` with an owner-readable reason for that task only.
    async fn settle(
        &self,
        project: &str,
        task: &str,
        record: &Value,
        unattended: bool,
    ) -> Settle {
        let title = record["title"]
            .as_str()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or("A result");
        let failed = |why: &str| {
            let suffix = if unattended {
                ""
            } else {
                " Open the task to accept it yourself."
            };
            Settle::Failed(format!(
                "{title} could not be accepted automatically: {why}{suffix}"
            ))
        };
        match self
            .product_call(
                "product.task",
                project,
                json!({"taskId":task,"action":"auto_ready"}),
            )
            .await
        {
            Ok(r) if r["accepted"] == true => Settle::Accepted,
            Ok(r) if r["deferred"] == true => Settle::Deferred,
            Ok(r) if r["failed"] == "workspace_unobservable" => Settle::Unobservable(
                r["reason"].as_str().unwrap_or("The project folder could not be read.").to_owned(),
            ),
            Ok(r) if r["failed"].is_string() => failed(
                r["reason"]
                    .as_str()
                    .unwrap_or("the result could not be read."),
            ),
            Ok(r) => Settle::Waiting(r["reason"].as_str().unwrap_or(UNSETTLED).to_owned()),
            Err(_) => failed("the check stopped before it finished."),
        }
    }

    /// Re-read the same owned result the desktop shows. The board deliberately
    /// omits its checks. Block only on a real owner decision or exhausted repair.
    /// The agent fixes what it can within its granted permissions: the bounded,
    /// deduplicated automatic repair first, then exactly one more repair per owner
    /// consent recorded on the hold. Never for an unknown effect or a partial read.
    /// `automatic` is false where only the owner may send the work back.
    #[allow(clippy::too_many_arguments)]
    async fn automatic_repair(
        &self,
        project: &str,
        runtime: &Arc<Runtime>,
        sprint: &Value,
        task: &str,
        execution: &Value,
        changed_paths: Option<&Value>,
        code: &str,
        reason: &str,
        automatic: bool,
    ) -> io::Result<Repair> {
        use crate::machine::task_auto_repair as repair;
        let Some(execution) = execution.as_str() else {
            return Ok(Repair::Declined(HoldFacts::default()));
        };
        let Ok(product) = self
            .product_call("product.read", project, json!({"taskIds":[task]}))
            .await
        else {
            // Read again on the next pass; an unreadable result is no owner decision.
            return Ok(Repair::Later);
        };
        let record = &product["tasks"][task];
        let result = &record["result"];
        let actions = &record["reviewActions"];
        if actions["pending"] == true {
            return Ok(Repair::Later);
        }
        let (Some(version), Some(checks)) = (
            result["sourceVersion"].as_str(),
            result["checks"].as_array(),
        ) else {
            return Ok(Repair::Declined(HoldFacts::default()));
        };
        let partial = result["checksPartial"] == true;
        let stale = actions["workspaceChanged"] == true
            || matches!(
                code,
                "stale_result" | "workspace_drift" | "task_files_changed" | "review_required"
            );
        let mut facts = HoldFacts {
            finish: finish_offered(sprint, record, code, partial || stale),
            ..HoldFacts::default()
        };
        if facts.finish {
            facts.asks = repair::finish_asks(checks);
        }
        if partial || actions["requestChanges"] != true {
            return Ok(Repair::Declined(facts));
        }
        let hold = &sprint["automaticQueue"];
        let consent = (sprint["queueState"] == "paused"
            && hold["taskId"] == task
            && hold["code"] == code
            && hold["nextActor"] == BY_OWNER)
            .then(|| hold["allowedAt"].as_i64())
            .flatten()
            .map(|at| at.to_string());
        let mut context = reason.to_owned();
        if let Some(paths) = changed_paths.and_then(Value::as_array) {
            let shown: Vec<_> = paths.iter().filter_map(Value::as_str).take(12).collect();
            context.push_str(&format!(
                " Recorded task files (data): {}.",
                shown.join(", ")
            ));
        }
        let owner_plan = |consent: &str| repair::owner_plan(&context, checks, stale, consent);
        let plan = match consent.as_deref() {
            Some(consent) => owner_plan(consent),
            None if automatic => match changed_paths {
                Some(paths) => repair::reconcile_files(paths),
                None => repair::plan(
                    actions,
                    checks,
                    result["warnings"]
                        .as_array()
                        .is_some_and(|items| !items.is_empty()),
                ),
            },
            None => None,
        };
        let mut refused = repair::Dispatch::Refused(None);
        if let Some(plan) = plan {
            match repair::dispatch(
                &self.config.state_dir,
                runtime,
                project,
                sprint,
                task,
                execution,
                Some(version),
                plan,
                consent.is_some(),
            )
            .await?
            {
                repair::Dispatch::Started => return Ok(Repair::Started),
                // Busy decides nothing: asked again next pass, consent kept.
                repair::Dispatch::Busy => return Ok(Repair::Later),
                other => refused = other,
            }
        }
        if refused.unknown_effect() {
            facts.asks.push(UNKNOWN_EFFECT_ASK.into());
        } else if consent.is_some() {
            facts.asks.push(NOT_SENT_BACK_ASK.into());
        } else {
            // An acceptance the owner made by hand is never replaced by an agent repair.
            facts.repair = owner_plan("offer").is_some()
                && !(record["acceptance"].is_object()
                    && record["acceptance"]["automatic"] != true);
        }
        Ok(Repair::Declined(facts))
    }

    /// A run that ended as failed with every effect known goes back to the same
    /// coordinator, bounded like a result repair. Its review and verification run again.
    async fn failed_repair(
        &self,
        project: &str,
        runtime: &Arc<Runtime>,
        sprint: &Value,
        task: &str,
        fact: &Value,
    ) -> io::Result<Repair> {
        let Some(execution) = fact["executionId"].as_str() else {
            return Ok(Repair::Declined(HoldFacts::default()));
        };
        let hold = &sprint["automaticQueue"];
        let consent = (sprint["queueState"] == "paused"
            && hold["taskId"] == task
            && hold["code"] == "task_failed"
            && hold["nextActor"] == BY_OWNER)
            .then(|| hold["allowedAt"].as_i64())
            .flatten();
        let mut plan = crate::machine::task_auto_repair::failed_plan(
            fact["waitReason"].as_str(),
            execution,
        );
        if let Some(at) = consent {
            plan.fingerprint = format!("owner:{at}:{}", plan.fingerprint);
        }
        use crate::machine::task_auto_repair::Dispatch;
        let refused = match crate::machine::task_auto_repair::dispatch(
            &self.config.state_dir,
            runtime,
            project,
            sprint,
            task,
            execution,
            None,
            plan,
            consent.is_some(),
        )
        .await?
        {
            Dispatch::Started => return Ok(Repair::Started),
            // Busy decides nothing: asked again next pass, consent kept.
            Dispatch::Busy => return Ok(Repair::Later),
            refused => refused,
        };
        Ok(Repair::Declined(if refused.unknown_effect() {
            // An effect whose outcome is unknown is never replayed by an agent.
            HoldFacts {
                asks: vec![UNKNOWN_EFFECT_ASK.into()],
                ..HoldFacts::default()
            }
        } else if consent.is_some() {
            HoldFacts {
                asks: vec![NOT_SENT_BACK_ASK.into()],
                ..HoldFacts::default()
            }
        } else {
            HoldFacts {
                repair: true,
                asks: vec!["Let the agent finish this task again. It already used its automatic attempts. Review and verification run again before it can finish.".into()],
                ..HoldFacts::default()
            }
        }))
    }

    /// Persist a hold, after the agent had its chance to fix a failed run.
    async fn hold_or_repair(
        &self,
        project: &str,
        runtime: &Arc<Runtime>,
        observed: &Snapshot,
        hold: Value,
    ) -> io::Result<()> {
        let (sprint, tasks) = (&observed.sprint, &observed.tasks);
        if hold["code"] == "task_failed" {
            if let Some((id, fact)) = hold["taskId"]
                .as_str()
                .and_then(|id| tasks.get(id).map(|f| (id, f)))
            {
                match self
                    .failed_repair(project, runtime, sprint, id, fact)
                    .await?
                {
                    Repair::Started | Repair::Later => return Ok(()),
                    Repair::Declined(facts) => {
                        let reason = hold["reason"].as_str().unwrap_or_default().to_owned();
                        return self
                            .observed_pause(project, observed,
                                automatic_hold(
                                    "settlement_blocked",
                                    Some(id),
                                    "task_failed",
                                    &reason,
                                    &facts,
                                ),
                            )
                            .await;
                    }
                }
            }
        }
        self.observed_pause(project, observed, hold).await
    }
    async fn product_call(&self, op: &str, project: &str, params: Value) -> io::Result<Value> {
        // An automatic settle that accepted nothing changed nothing; it is retried on
        // every pass, so it must not make open windows refetch their facts.
        let settle = params["action"] == "auto_ready";
        let call = serde_json::from_value(json!({"op":op,"projectId":project,"params":params}))
            .map_err(|_| crate::failure("invalid_request"))?;
        let reply = self.machine(call).await?;
        if reply["ok"] != true {
            return Err(other(
                reply["error"]["code"]
                    .as_str()
                    .unwrap_or("machine_refused")
                    .to_owned(),
            ));
        }
        // Owner state changed here in the background (a planner question, a plan, the
        // queue) has no runtime event of its own. Tell open windows at once, so it never
        // waits for their slow facts poll.
        if !matches!(op, "product.board" | "product.read")
            && !(settle && reply["result"]["accepted"] != true)
        {
            let _ = self
                .events
                .send(json!({"projectId":project,"type":"invalidation"}));
        }
        Ok(reply["result"].clone())
    }
    #[cfg(test)]
    async fn sprint_patch(&self, project: &str, sprint: &str, patch: Value) -> io::Result<()> {
        self.product_call(
            "product.sprint",
            project,
            json!({"action":"patch","sprintId":sprint,"patch":patch}),
        )
        .await
        .map(|_| ())
    }
    async fn sprint_note(
        &self,
        project: &str,
        sprint: &str,
        kind: &str,
        text: &str,
    ) -> io::Result<()> {
        self.product_call(
            "product.sprint",
            project,
            json!({"action":"note","sprintId":sprint,"kind":kind,"text":text}),
        )
        .await
        .map(|_| ())
    }
    async fn sprint_pass(
        self: &Arc<Self>,
        project: &str,
        runtime: &Arc<Runtime>,
        jobs: &mut Jobs,
        first: bool,
    ) -> io::Result<bool> {
        // Exits a session could not confirm are looked at again first (bounded, with
        // backoff), so a task they alone held continues in this same pass.
        let exits_pending = runtime
            .resettle_unconfirmed_exits()
            .await
            .map_err(|e| crate::failure(e.code()))?;
        let automatic_active = runtime
            .continue_automatic_tasks()
            .await
            .map_err(|e| crate::failure(e.code()))?;
        let product = self
            .product_call("product.board", project, json!({}))
            .await?;
        let sprints = product["sprints"].as_array().into_iter().flatten();
        let ids: BTreeSet<_> = sprints.clone().filter_map(|s| s["id"].as_str()).collect();
        let failures = jobs.1.entry(project.to_owned()).or_default();
        failures.retain(|id, _| ids.contains(id.as_str()));
        for sprint in sprints {
            // One sprint's planning failure never stops the others.
            let result = self
                .planning_pass(project, runtime, sprint, &product, first)
                .await;
            self.observe_planning_pass(project, sprint, failures, result)
                .await;
        }
        // Settle waiting results in queue-driven modes, even when the queue
        // is paused (for example just after a switch from Manual). Manual
        // sprints cost nothing here: no task read, no settle call, no folder scan.
        let automatic: Vec<&Value> = product["sprints"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|s| {
                s["id"].is_string()
                    && s["runMode"] == "hybrid"
                    && crate::machine::queue_driven_mode(s)
            })
            .map(|s| &s["id"])
            .collect();
        let waiting: Vec<&String> = product["tasks"]
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(_, record)| {
                automatic.contains(&&record["sprintId"])
                    && record["acceptance"].is_null()
                    && !automatic_skip(record, None)
                    && record["status"] != "done"
            })
            .map(|(task, _)| task)
            .collect();
        let waiting_any = !waiting.is_empty();
        if !waiting.is_empty() {
            let finished = self.native_tasks(project).await?;
            for task in waiting {
                if finished.get(task).is_some_and(|t| t["phase"] == "result") {
                    let record = &product["tasks"][task.as_str()];
                    // A settle that could not finish is held, with its reason, by the
                    // queue's own pass. A paused queue without a hold is paused by the
                    // owner or a mode switch: its reason is never overwritten, so it is
                    // never mistaken for a system hold the scheduler may lift.
                    let _ = self.settle(project, task, record, false).await;
                }
            }
        }
        for sprint in product["sprints"].as_array().into_iter().flatten() {
            if sprint["archivedAt"].is_null()
                && (sprint["queueState"] == "running" || resumable_automatic_queue(sprint))
            {
                // One sprint's unreadable state never stops the others.
                let _ = self.queue_pass(project, runtime, sprint).await;
            }
        }
        // In motion: a queue running, planning under way, a result waiting for an automatic settle,
        // or a session exit still unconfirmed (looked at again on the safety tick). Anything else changes only through an owner or loop write, which wakes the loop itself.
        let sprints = product["sprints"].as_array().into_iter().flatten();
        let live = sprints.clone().any(|s| {
            s["archivedAt"].is_null()
                && (s["queueState"] == "running"
                    || s["planning"] == "planning"
                    || resumable_automatic_queue(s))
        }) || waiting_any
            || automatic_active
            || exits_pending;
        Ok(live)
    }
    async fn observe_planning_pass(
        &self,
        project: &str,
        sprint: &Value,
        failures: &mut HashMap<String, u8>,
        result: io::Result<()>,
    ) {
        const WARNING_THRESHOLD: u8 = 3;
        let Some(id) = sprint["id"].as_str() else {
            return;
        };
        if result.is_ok() {
            failures.remove(id);
            if !sprint["planningWarning"].is_null() {
                self.update_planning_warning(project, id, None).await;
            }
        } else {
            let count = failures.entry(id.to_owned()).or_default();
            if *count < WARNING_THRESHOLD {
                *count += 1;
                if *count == WARNING_THRESHOLD
                    && sprint["planningWarning"].is_null()
                    && !self.update_planning_warning(project, id, Some(*count)).await
                {
                    // Not saved: stay just below the threshold so the next failed
                    // pass of this streak tries again. The writer adds its event only
                    // while no warning is saved, so the streak still gets one event.
                    *count = WARNING_THRESHOLD - 1;
                }
            }
        }
    }

    /// Planning of one sprint: an ordinary read-only runtime task in the project
    /// folder, created and started by the service, observed here. Its validated
    /// plan becomes the sprint's proposal once the task completed.
    async fn planning_pass(
        &self,
        project: &str,
        runtime: &Arc<Runtime>,
        sprint: &Value,
        product: &Value,
        first: bool,
    ) -> io::Result<()> {
        let Some(id) = sprint["id"].as_str() else {
            return Ok(());
        };
        let closed = !sprint["archivedAt"].is_null()
            || sprint
                .get("deleting")
                .is_some_and(|d| !d.is_null() && *d != false);
        let task = sprint["planningTaskId"].as_str();
        let planning = sprint["planning"].as_str().unwrap_or("");
        if let Some(task) = task {
            let record = &product["tasks"][task];
            let fact = planning_fact(runtime, task).await?;
            let finished = matches!(
                fact.state.as_deref(),
                Some("COMPLETED" | "FAILED" | "CANCELLED") | None
            );
            if closed || planning == "cancelled" {
                // Close or delete stops the planning task through its execution control,
                // once per execution (a repeat replays the same stop). Stop in the sprint
                // already paused it through the same control.
                if closed && !finished {
                    if let Some(execution) = fact.execution.as_deref() {
                        runtime
                            .control(
                                project.to_owned(),
                                json!({"projectId":project,"executionId":execution,
                                    "requestId":format!("sprint-planning-stop-{execution}"),"action":"pause"}),
                            )
                            .await?;
                    }
                }
                return Ok(());
            }
            if planning == "created" {
                // Approved: the planning task closes as done.
                if finished && record["status"] != "done" && record["purged"] != true {
                    self.product_call(
                        "product.task",
                        project,
                        json!({"action":"close","taskId":task,"choice":"as_is","confirmed":true}),
                    )
                    .await?;
                }
                return Ok(());
            }
            if !matches!(planning, "planning" | "question" | "ready") {
                return Ok(());
            }
            let retry = planning == "planning" && sprint["planningRetry"] == true;
            if fact.execution.is_none() {
                if retry {
                    if self
                        .set_planning_task(project, id, Some(task.to_owned()), None, None)
                        .await?
                    {
                        let mut unlinked = sprint.clone();
                        unlinked["planningTaskId"] = Value::Null;
                        unlinked["planningRun"] = Value::Null;
                        return self.start_planning(project, runtime, &unlinked, None).await;
                    }
                    return Ok(());
                }
                // A prior create/link may have survived a restart or an unresolved
                // Start reply. Without an owner retry, report it, never start again.
                return self
                    .cancel_unstarted_planning(
                        project,
                        id,
                        task,
                        "planning_start_failed",
                        "The planning task did not start.",
                    )
                    .await;
            }
            if retry
                && !self
                    .consume_planning_retry(project, id, task.to_owned())
                    .await?
            {
                return Ok(());
            }
            let mut expected = sprint.clone();
            if retry {
                expected.as_object_mut().unwrap().remove("planningRetry");
            }
            return match planning_step(sprint, &fact) {
                PlanningStep::Wait => Ok(()),
                PlanningStep::Ready { items, run } => {
                    self.publish_planning_state(project, id, expected, "ready", Some(items), run).await
                }
                PlanningStep::Keep { run } => {
                    self.publish_planning_state(project, id, expected, "ready", None, run).await
                }
                PlanningStep::Correct { text, run } | PlanningStep::Continue { text, run } => {
                    let execution = fact.execution.clone().unwrap_or_default();
                    let request = format!(
                        "sprint-plan-{}-{}",
                        run["runtimeTask"].as_str().unwrap_or(""),
                        run["handled"]
                    );
                    let sent = runtime
                        .control_planning_prompt(project.to_owned(), json!({"projectId":project,"executionId":execution,
                            "action":"prompt","text":text,"attachments":[],"requestId":request}),
                            crate::machine::task_rework::SprintPlanningPrompt {
                                observed: expected.clone(),
                                run: run.clone(),
                            })
                        .await;
                    match sent {
                        Ok(reply) if reply["outcome"] == "recorded" => {
                            self.publish_planning_state(project, id, expected, "planning", None, run)
                                .await
                        }
                        Err(error) if error.to_string() == "planning_observation_stale" => Ok(()),
                        _ => {
                            self.cancel_planning(
                                project,
                                id,
                                "continue",
                                "planning_continue_failed",
                                "The planning task could not be asked to continue.",
                                Some(json!({"runtimeTask":run["runtimeTask"],"handled":run["handled"],
                                    "outcome":"cancelled","corrections":run["corrections"],"planEvent":run["planEvent"]})),
                            )
                            .await
                        }
                    }
                }
                PlanningStep::Cancel { note, code, run } => {
                    self.cancel_planning(project, id, "run", code, &note, Some(run))
                        .await
                }
                PlanningStep::Terminal { failure, run } => {
                    self.record_planning_failure(
                        project,
                        id,
                        failure.reason().into(),
                        "run",
                        failure.code(),
                        Some(run),
                        None,
                        Some((task.to_owned(), failure)),
                    )
                    .await
                }
                PlanningStep::Replace { retry } => {
                    // Linking the new planner also retires the previous one.
                    self.sprint_note(
                        project, id, "event", "Planning continues in a new planning task.",
                    )
                    .await?;
                    self.start_planning(project, runtime, sprint, retry).await
                }
            };
        }
        if closed || planning != "planning" {
            // A sprint saved by an older version without a planning task waits for
            // the owner's answer or Resume planning; nothing runs for it meanwhile.
            return Ok(());
        }
        if first {
            // An attempt claimed before a restart is never silently rerun.
            return self
                .cancel_planning(
                    project,
                    id,
                    "restart",
                    "planning_interrupted",
                    "Planning was interrupted.",
                    None,
                )
                .await;
        }
        self.start_planning(project, runtime, sprint, None).await
    }
    async fn cancel_planning(
        &self,
        project: &str,
        id: &str,
        step: &'static str,
        code: &'static str,
        note: &str,
        run: Option<Value>,
    ) -> io::Result<()> {
        let text = format!("{note} Resume planning to try again.");
        self.record_planning_failure(project, id, text, step, code, run, None, None)
            .await
    }

    async fn cancel_unstarted_planning(
        &self,
        project: &str,
        id: &str,
        task: &str,
        code: &'static str,
        reason: &str,
    ) -> io::Result<()> {
        let text = format!("{reason} Resume planning to try again.");
        self.record_planning_failure(
            project, id, text, "start", code, None, Some(task.into()), None,
        )
        .await
    }

    /// Create the read-only planning draft in the project folder, record it on the
    /// sprint, then start it bound to the sprint planning policy.
    async fn start_planning(
        &self,
        project: &str,
        runtime: &Arc<Runtime>,
        sprint: &Value,
        retry: Option<PlanningFailure>,
    ) -> io::Result<()> {
        let id = sprint["id"].as_str().unwrap_or("");
        // Saved files are references; their bodies are read for this draft only.
        let state = self.config.state_dir.clone();
        let inlined = {
            let sprint = sprint.clone();
            tokio::task::spawn_blocking(move || {
                let mut sprint = sprint;
                crate::machine::attachment_files::inline_for_delivery(&state, &mut sprint);
                sprint
            })
            .await
            .map_err(|_| crate::failure("planning_unavailable"))?
        };
        let description = match planning_request(&inlined, retry) {
            Ok(text) => text,
            Err(note) => {
                return self
                    .cancel_planning(
                        project,
                        id,
                        "request",
                        "planning_request_invalid",
                        &note,
                        Some(Value::Null),
                    )
                    .await
            }
        };
        let mut payload = json!({"description":description,"checkout":{"kind":"main"}});
        let files = planning_files(&inlined);
        if !files.is_empty() {
            payload["attachments"] = json!(files);
        }
        let settings = &sprint["settings"];
        if let Some(model) = settings["model"].as_str().filter(|m| !m.is_empty()) {
            payload["settings"] = json!({"model":model,
                "effort":settings["effort"].as_str().unwrap_or(""),
                "mode":settings["mode"].as_str().filter(|m| cortex_core::access::MODES.contains(m)).unwrap_or("guarded"),
                "language":settings["language"].as_str().filter(|l| matches!(*l, "auto" | "en" | "ru")).unwrap_or("auto")});
        }
        let request = format!("sprint-planning-{}", crate::identity()?);
        let created = runtime
            .create_draft(
                project.to_owned(),
                json!({"protocol":{"major":1,"minor":8},"requestId":request,"projectId":project,
                    "actorId":actor(),"operation":"tasks.create","target":project,
                    "expectedVersion":"0","ownerEpoch":null,"payload":payload}),
                Some(id.to_owned()),
            )
            .await;
        let task = match created {
            Ok(receipt) if receipt["outcome"] == "recorded" => (
                receipt["target"].as_str().map(str::to_owned),
                receipt["entityVersion"].clone(),
            ),
            _ => (None, Value::Null),
        };
        let (Some(task), version) = task else {
            return self
                .cancel_planning(
                    project,
                    id,
                    "create",
                    "planning_task_create_failed",
                    "The planning task could not be created. Check the selected model and attached files.",
                    Some(Value::Null),
                )
                .await;
        };
        if !self
            .set_planning_task(
                project,
                id,
                sprint["planningTaskId"].as_str().map(str::to_owned),
                Some(task.clone()),
                retry,
            )
            .await?
        {
            return Ok(());
        }
        let started = runtime
            .start_sprint_planning(
                project.to_owned(),
                json!({"protocol":{"major":1,"minor":8},"requestId":format!("{request}-start"),
                    "projectId":project,"actorId":actor(),"operation":"tasks.start","target":task,
                    "expectedVersion":if version.is_null() { json!("1") } else { version },
                    "ownerEpoch":null,"payload":{}}),
            )
            .await;
        self.finish_planning_start(project, runtime, id, &task, started)
            .await
    }

    async fn finish_planning_start(
        &self,
        project: &str,
        runtime: &Runtime,
        id: &str,
        task: &str,
        started: io::Result<Value>,
    ) -> io::Result<()> {
        let (code, reason) = match started {
            Ok(receipt) if receipt["outcome"] == "recorded" => return Ok(()),
            Ok(receipt) => (
                "planning_start_refused",
                planning_start_reason(receipt["outcome"].as_str().unwrap_or("refused")),
            ),
            Err(error) => (
                "planning_start_failed",
                planning_start_reason(&error.to_string()),
            ),
        };
        // A lost reply is not proof that Start did not commit. An unreadable
        // runtime is not proof either; leave the link for the next pass to check.
        if planning_fact(runtime, task).await?.execution.is_some() {
            return Ok(());
        }
        self.cancel_unstarted_planning(project, id, task, code, &reason)
            .await
    }
    // A concurrent writer (a running task) can advance the outbox between pages or
    // faster than the per-read retry, which stales the snapshot-bound cursor; the whole
    // scan then restarts from the first page, a bounded number of times.
    async fn native_tasks(&self, project: &str) -> io::Result<HashMap<String, Value>> {
        let mut attempt = 0;
        loop {
            match self.native_tasks_scan(project).await {
                Err(error) if error.to_string() == "snapshot_expired" && attempt < 20 => {
                    attempt += 1;
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
                result => return result,
            }
        }
    }
    async fn native_tasks_scan(&self, project: &str) -> io::Result<HashMap<String, Value>> {
        let mut tasks = HashMap::new();
        let mut cursor = Value::Null;
        for _ in 0..200 {
            let page = self
                .native_query(
                    json!({"protocol":{"major":2,"mapping":2},"requestId":"sprint-queue",
                    "projectId":project,"operation":"tasks.page",
                    "payload":{"target":project,"cursor":cursor,"limit":50}}),
                )
                .await?;
            let value = &page["data"]["value"];
            for item in value["items"].as_array().into_iter().flatten() {
                if let Some(id) = item["taskId"].as_str() {
                    tasks.insert(id.to_owned(), item.clone());
                }
            }
            cursor = value["nextCursor"].clone();
            if cursor.is_null() {
                return Ok(tasks);
            }
        }
        Err(crate::failure("scheduler_task_read_budget"))
    }
    // Effect outcomes (including uncertain Git effects) retain their existing
    // durable hold handling; the queue-only decisions use a fresh-facts fence.
    async fn automatic_pause(
        &self,
        project: &str,
        sprint: &Value,
        pause: Value,
    ) -> io::Result<()> {
        self.save_pause(project, sprint, pause, None).await
    }

    async fn observed_pause(&self, project: &str, observed: &Snapshot, pause: Value) -> io::Result<()> {
        self.save_pause(project, &observed.sprint, pause, Some(&observed.queue_revision)).await
    }

    async fn save_pause(
        &self,
        project: &str,
        sprint: &Value,
        pause: Value,
        revision: Option<&str>,
    ) -> io::Result<()> {
        // Every hold the scheduler persists is checked against the task's unknown
        // effects here, so no hold offers an action its handler would refuse.
        let pause = match pause["taskId"].as_str() {
            Some(task) if pause.is_object() => {
                let runtime = self.state.lock().await.native.get(project).cloned();
                let unsettled = match runtime {
                    Some(runtime) => unsettled_effects(&runtime, project, task).await,
                    None => None,
                };
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis() as i64);
                settling_hold(pause, &sprint["automaticQueue"], unsettled, now)
            }
            _ => pause,
        };
        let reply = self
            .automatic_sprint_action(
                project,
                "automatic.queue",
                json!({"expected":sprint,"queueRevision":revision,"pause":pause}),
            )
            .await?;
        if reply["ok"] != true {
            return Err(other(
                reply["code"]
                    .as_str()
                    .unwrap_or("automatic_queue_unavailable")
                    .into(),
            ));
        }
        Ok(())
    }
    /// True when the Automatic-specific step held or changed the queue.
    /// Otherwise the ordinary sequential scheduler can look for a draft.
    async fn automatic_pass(
        &self,
        project: &str,
        runtime: &Arc<Runtime>,
        observed: &Snapshot,
        disposition: &Disposition,
    ) -> io::Result<bool> {
        let (sprint, tasks, records, members) = (&observed.sprint, &observed.tasks, &observed.records, &observed.roster);
        let settled = |task: &str| {
            automatic_skip(&records[task], disposition.get(task))
                || disposition
                    .get(task)
                    .is_some_and(|(status, _)| status == "done")
        };
        if sprint["queueState"] == "paused" {
            let prior = &sprint["automaticQueue"];
            let kind = prior["kind"].as_str().unwrap_or_default();
            let code = prior["code"].as_str().unwrap_or_default();
            let task = prior["taskId"].as_str();
            if task.is_some_and(|id| !members.iter().any(|member| member == id) || settled(id))
            {
                self.observed_pause(project, observed, Value::Null).await?;
                return Ok(true);
            }
            // A hold saved before decisions were recorded on it is rewritten with
            // its current decision; the specific recheck runs on the next pass.
            let upgrade = || {
                let reason = match (kind, prior["reason"].as_str()) {
                    ("start_refused", _) => start_reason(code),
                    (_, Some(reason)) if !reason.contains('_') => reason.to_owned(),
                    _ => "Automatic stopped at this task.".to_owned(),
                };
                automatic_hold(kind, task, code, &reason, &HoldFacts::default())
            };
            let legacy = !hold_current(prior)
                && !matches!(
                    (kind, code),
                    (
                        "settlement_blocked",
                        "queue_blocked" | "dependencies_blocked" | "archived_task"
                    ) | ("continuation", _)
                );
            if legacy {
                self.observed_pause(project, observed, upgrade()).await?;
                return Ok(true);
            }
            // A hold waiting on an unknown effect is rechecked first: it stays while
            // the effect is unknown (`automatic_pause` hands it to the owner once the
            // runtime can no longer settle it), and once it is known the queue derives
            // the condition again or the replaced decision comes back as it was.
            if prior["settling"]["hold"].is_object() {
                let original = prior["settling"]["hold"].clone();
                let unsettled = match task {
                    Some(id) => unsettled_effects(runtime, project, id).await,
                    None => None,
                };
                let next = if unsettled.is_none() && rederived(kind, code) {
                    Value::Null
                } else {
                    original
                };
                self.observed_pause(project, observed, next).await?;
                return Ok(true);
            }
            match (kind, code, task) {
                ("start_refused", _, _) => {}
                ("continuation", _, Some(id)) => {
                    // Rechecked against the task as read now: a continuation that no
                    // longer pauses (or no longer exists) releases its hold.
                    if let Some(t) = tasks.get(id) {
                        self.observed_pause(project, observed,
                            continuation_hold(id, t).unwrap_or(Value::Null),
                        )
                        .await?;
                    }
                }
                ("owner_integration", "owner_accepted", Some(id))
                    if records[id]["acceptance"]["finishConsent"].is_object() =>
                {
                    // A manual Accept may later acquire explicit Automatic-finish
                    // consent. Re-enter the same validated finish path; without
                    // that consent, automatic_finish preserves this owner hold.
                    self.automatic_finish(
                        project,
                        runtime,
                        sprint,
                        id,
                        &tasks[id]["executionId"],
                        true,
                    )
                    .await?;
                }
                (
                    "settlement_blocked",
                    "queue_blocked"
                    | "dependencies_blocked"
                    | "archived_task"
                    | "task_missing"
                    | "conflict_resolving"
                    | "task_conflict"
                    | "task_paused"
                    | "task_blocked"
                    | "task_recovery"
                    | "task_failed"
                    | "task_stopped"
                    | "task_state_unknown",
                    _,
                ) => {
                    // Observe current conditions, not an unconditional clear/re-pause loop.
                    let hold = members
                        .iter()
                        .filter(|id| !settled(id))
                        .find_map(|id| tasks.get(id).and_then(|t| continuation_hold(id, t)));
                    let next = hold.unwrap_or_else(|| {
                        match queue_step_in_order(members, &observed.priority, tasks, records, disposition, false)
                        {
                            QueueStep::Hold(hold) => hold,
                            _ => Value::Null,
                        }
                    });
                    if next["code"] == "task_failed" {
                        // A failed run held for the owner keeps its decision until the
                        // owner allows a repair or the run changes.
                        let same = next["taskId"] == prior["taskId"] && code == "task_failed";
                        if !same || prior["allowedAt"].is_i64() {
                            self.hold_or_repair(project, runtime, observed, next)
                                .await?;
                        }
                    } else {
                        self.observed_pause(project, observed, next).await?;
                    }
                }
                ("settlement_blocked", "automatic_acceptance_blocked" | "workspace_unobservable", Some(id)) => {
                    match tasks.get(id).and_then(|t| t["phase"].as_str()) {
                        Some("result")
                            if records[id]["conflict"].as_str().is_none_or(|c| {
                                !matches!(c, "conflict" | "failed" | "resolving")
                            }) =>
                        {
                            let accepted = records[id]["status"] == "ready_to_merge"
                                && records[id]["acceptance"].is_object();
                            if accepted {
                                if records[id]["acceptance"]["finishConsent"].is_object() {
                                    self.automatic_finish(
                                        project,
                                        runtime,
                                        sprint,
                                        id,
                                        &tasks[id]["executionId"],
                                        true,
                                    )
                                    .await?;
                                } else {
                                    self.observed_pause(project, observed, Value::Null).await?;
                                }
                            } else {
                                match self.settle(project, id, &records[id], true).await {
                                    Settle::Accepted => {
                                        // This pass changed the acceptance it just checked.
                                        // Resume against the unchanged sprint; the next
                                        // pass must inspect the newly accepted result.
                                        self.automatic_pause(project, sprint, Value::Null).await?
                                    }
                                    Settle::Deferred => return Ok(true),
                                    Settle::Unobservable(reason) => {
                                        self.observation_blocked(project, observed, id, &reason).await?
                                    }
                                    Settle::Waiting(reason) | Settle::Failed(reason) => {
                                        self.acceptance_blocked(
                                            project, runtime, observed, id, &reason,
                                        )
                                        .await?
                                    }
                                }
                            }
                        }
                        Some(phase) if !matches!(phase, "result" | "unknown") => {
                            self.observed_pause(project, observed, Value::Null).await?;
                        }
                        _ => {}
                    }
                }
                ("settlement_blocked", _, Some(id))
                    if !matches!(code, "outcome_unknown" | "integration_outcome_unknown") =>
                {
                    let phase = tasks.get(id).and_then(|t| t["phase"].as_str());
                    if phase.is_some_and(|phase| !matches!(phase, "result" | "unknown")) {
                        self.observed_pause(project, observed, Value::Null).await?;
                    } else if phase == Some("result") && code != "integration_failed" {
                        self.automatic_finish(
                            project,
                            runtime,
                            sprint,
                            id,
                            &tasks[id]["executionId"],
                            true,
                        )
                        .await?;
                    } else if phase == Some("result") && prior["allowedAt"].is_i64() {
                        // A failed commit is never retried as is. With the owner's
                        // consent the agent fixes its cause and publishes a fresh result.
                        let reason = prior["reason"].as_str().unwrap_or_default().to_owned();
                        if let Repair::Declined(facts) = self
                            .automatic_repair(
                                project,
                                runtime,
                                sprint,
                                id,
                                &tasks[id]["executionId"],
                                None,
                                code,
                                &reason,
                                false,
                            )
                            .await?
                        {
                            self.observed_pause(project, observed,
                                automatic_hold(kind, Some(id), code, &reason, &facts),
                            )
                            .await?;
                        }
                    }
                }
                _ => {}
            }
            return Ok(true);
        }
        if automatic_checkout_unavailable(sprint) {
            self.observed_pause(project, observed,
                automatic_hold(
                    "start_refused",
                    None,
                    "native_worktree_unavailable",
                    WORKTREE_UNAVAILABLE,
                    &HoldFacts::default(),
                ),
            )
            .await?;
            return Ok(true);
        }
        for task in members.iter().filter(|id| !settled(id)) {
            if let Some(hold) = tasks.get(task).and_then(|t| continuation_hold(task, t)) {
                self.observed_pause(project, observed, hold).await?;
                return Ok(true);
            }
        }
        // Finish before the next draft can start. After an effect, use a
        // fresh board on the next pass instead of the old status/digest.
        for task in members.iter().filter(|id| !settled(id)) {
            let record = &records[task];
            if tasks.get(task).is_none_or(|t| t["phase"] != "result")
                || awaiting_owner_merge_checks(record)
                || matches!(
                    record["conflict"].as_str(),
                    Some("conflict" | "failed" | "resolving")
                )
            {
                continue;
            }
            let acceptance =
                if record["status"] == "ready_to_merge" && record["acceptance"].is_object() {
                    Settle::Accepted
                } else {
                    self.settle(project, task, record, true).await
                };
            let blocked = match acceptance {
                Settle::Accepted => None,
                Settle::Deferred => return Ok(true),
                Settle::Unobservable(reason) => {
                    self.observation_blocked(project, observed, task, &reason).await?;
                    return Ok(true);
                }
                Settle::Waiting(reason) | Settle::Failed(reason) => Some(reason),
            };
            if let Some(reason) = blocked {
                self.acceptance_blocked(project, runtime, observed, task, &reason)
                    .await?;
                return Ok(true);
            }
            self.automatic_finish(
                project,
                runtime,
                sprint,
                task,
                &tasks[task]["executionId"],
                false,
            )
            .await?;
            return Ok(true);
        }
        Ok(false)
    }

    async fn automatic_finish(
        &self,
        project: &str,
        runtime: &Arc<Runtime>,
        sprint: &Value,
        task: &str,
        execution: &Value,
        recheck: bool,
    ) -> io::Result<()> {
        let reply = self
            .automatic_sprint_action(
                project,
                "automatic.finish",
                json!({
                    "sprintId":sprint["id"],"taskId":task,
                    "expectedHold":if recheck { sprint.clone() } else { Value::Null }
                }),
            )
            .await;
        let (kind, code, reason) = match &reply {
            Ok(r) if r["result"]["done"] == true => {
                return if recheck {
                    self.automatic_pause(project, sprint, Value::Null).await
                } else {
                    Ok(())
                };
            }
            Ok(r) if r["result"]["deferred"] == true => return Ok(()),
            Ok(r) if r["result"]["repairPaths"].is_array() => (
                "settlement_blocked",
                "task_files_changed",
                "The recorded task files have changed. Review their current contents and publish a fresh result.",
            ),
            Ok(r) if r["result"]["ownerIntegration"] == true => (
                "owner_integration",
                r["result"]["code"].as_str().unwrap_or("different_target"),
                r["result"]["reason"]
                    .as_str()
                    .unwrap_or("Confirm how to integrate into the target branch."),
            ),
            Ok(r)
                if matches!(
                    r["code"].as_str(),
                    Some("machine_busy" | "integration_in_progress")
                ) =>
            {
                return Ok(())
            }
            Ok(r) => (
                "settlement_blocked",
                r["code"].as_str().unwrap_or("integration_unavailable"),
                r["message"]
                    .as_str()
                    .unwrap_or("The automatic finish could not be confirmed."),
            ),
            Err(_) => (
                "settlement_blocked",
                "outcome_unknown",
                "The automatic finish could not be confirmed. It was not retried.",
            ),
        };
        let mut facts = HoldFacts::default();
        // The machine's commit refusal asks for a Commit retry no hold offers.
        let reason = if code == "integration_failed" {
            INTEGRATION_FAILED
        } else {
            reason
        };
        // A changed or outdated result is agent work. Content Cortex cannot attribute
        // to the task, a failed commit, and an acceptance the finish cannot use go
        // back to the agent only with consent.
        if matches!(
            code,
            "stale_result"
                | "workspace_drift"
                | "task_files_changed"
                | "review_required"
                | "task_content_unknown"
                | "integration_failed"
                | "automatic_acceptance_required"
        ) {
            let automatic = first_actor(kind, code) == BY_AGENT;
            match self
                .automatic_repair(
                    project,
                    runtime,
                    sprint,
                    task,
                    execution,
                    reply
                        .as_ref()
                        .ok()
                        .and_then(|r| r["result"].get("repairPaths")),
                    code,
                    reason,
                    automatic,
                )
                .await?
            {
                Repair::Started | Repair::Later => return Ok(()),
                Repair::Declined(declined) => facts = declined,
            }
        }
        self.automatic_pause(
            project,
            sprint,
            automatic_hold(kind, Some(task), code, reason, &facts),
        )
        .await
    }

    /// Workspace observation is a host prerequisite, so a failed scan holds
    /// the queue without reopening verified work or offering agent repair.
    async fn observation_blocked(
        &self,
        project: &str,
        observed: &Snapshot,
        task: &str,
        reason: &str,
    ) -> io::Result<()> {
        self.observed_pause(
            project,
            observed,
            automatic_hold(
                "settlement_blocked",
                Some(task),
                "workspace_unobservable",
                reason,
                &HoldFacts::default(),
            ),
        )
        .await
    }

    /// A result Automatic could not accept: the agent repairs it when it can,
    /// otherwise the owner is asked exactly what to allow.
    async fn acceptance_blocked(
        &self,
        project: &str,
        runtime: &Arc<Runtime>,
        observed: &Snapshot,
        task: &str,
        reason: &str,
    ) -> io::Result<()> {
        let (sprint, tasks) = (&observed.sprint, &observed.tasks);
        let code = "automatic_acceptance_blocked";
        match self
            .automatic_repair(
                project,
                runtime,
                sprint,
                task,
                &tasks[task]["executionId"],
                None,
                code,
                reason,
                true,
            )
            .await?
        {
            Repair::Started | Repair::Later => Ok(()),
            Repair::Declined(facts) => {
                self.observed_pause(project, observed,
                    automatic_hold("settlement_blocked", Some(task), code, reason, &facts),
                )
                .await
            }
        }
    }

    /// Scheduler tick for one running sprint. Starts are idempotent by
    /// request identity and never replayed; anything unclear pauses the queue.
    async fn queue_pass(
        &self,
        project: &str,
        runtime: &Arc<Runtime>,
        sprint: &Value,
    ) -> io::Result<()> {
        let Some(id) = sprint["id"].as_str() else { return Ok(()); };
        let observed = runtime.sprint_queue_snapshot(project.to_owned(), id.to_owned()).await?;
        let sprint = &observed.sprint;
        if !sprint["archivedAt"].is_null()
            || (!sprint["deleting"].is_null() && sprint["deleting"] != false)
            || (sprint["queueState"] != "running" && !resumable_automatic_queue(sprint))
        {
            return Ok(());
        }
        let disposition = queue_disposition(&observed);
        match sprint["runMode"].as_str() {
            Some("automatic") => {
                if self.automatic_pass(project, runtime, &observed, &disposition).await? {
                    return Ok(());
                }
                match queue_step_in_order(&observed.roster, &observed.priority, &observed.tasks,
                    &observed.records, &disposition, false)
                {
                    QueueStep::Hold(hold) => self.hold_or_repair(project, runtime, &observed, hold).await,
                    QueueStep::Complete => self.complete_queue(project, &observed).await,
                    QueueStep::Wait => Ok(()),
                    QueueStep::Start(task) =>
                        self.start_queue_task(project, runtime, &observed, task, true).await,
                }
            }
            Some("hybrid") => self.hybrid_pass(project, runtime, &observed, disposition).await,
            _ => self.queue_transition(project, &observed, true).await,
        }
    }

    async fn complete_queue(&self, project: &str, observed: &Snapshot) -> io::Result<()> {
        let sprint = &observed.sprint;
        // The sprint's shared worktree branch is merged once, when every member has finished
        // onto it. Automatic merges by itself into the open branch; otherwise (and for a
        // refused merge) the queue still completes and the owner is offered the merge.
        if sprint["runMode"] == "automatic"
            && sprint["checkout"]["kind"] == "worktree"
            && sprint["checkout"]["shared"] == true
            && sprint["merge"]["state"] != "merged"
        {
            let reply = self
                .automatic_sprint_action(
                    project,
                    "automatic.finish",
                    json!({"sprintMerge":true,"sprintId":sprint["id"]}),
                )
                .await;
            if matches!(&reply, Ok(r) if r["result"]["done"] == true) {
                return Ok(());
            }
        }
        self.queue_transition(project, observed, false).await
    }

    async fn queue_transition(&self, project: &str, observed: &Snapshot, pause_manual: bool) -> io::Result<()> {
        let reply = self.automatic_sprint_action(project, "automatic.queue", json!({
            "expected":observed.sprint,"queueRevision":observed.queue_revision,
            "complete":!pause_manual,"pauseManual":pause_manual,
        })).await?;
        if reply["ok"] != true {
            return Err(other(reply["code"].as_str().unwrap_or("automatic_queue_unavailable").to_owned()));
        }
        Ok(())
    }

    /// Hybrid uses the same decision as Automatic, but an accepted result
    /// settles its dependents and merging stays with the owner.
    async fn hybrid_pass(
        &self,
        project: &str,
        runtime: &Arc<Runtime>,
        observed: &Snapshot,
        disposition: Disposition,
    ) -> io::Result<()> {
        let (sprint, tasks, records, members) = (&observed.sprint, &observed.tasks, &observed.records, &observed.roster);
        let paused = sprint["queueState"] == "paused";
        if paused && !system_hold(sprint) {
            return Ok(());
        }
        let prior = &sprint["automaticQueue"];
        // A refused start waits for the owner's Try again (or a mode change), like
        // Automatic; it is released by itself only once its task no longer waits.
        if paused && prior["kind"] == "start_refused" {
            let gone = prior["taskId"].as_str().is_some_and(|task| {
                !members.iter().any(|m| m == task)
                    || automatic_skip(&records[task], disposition.get(task))
                    || disposition
                        .get(task)
                        .is_some_and(|(s, _)| s == "done" || s == "ready_to_merge")
                    || tasks.get(task).is_some_and(|t| t["phase"] != "not_started")
            });
            return if gone {
                self.observed_pause(project, observed, Value::Null).await
            } else {
                Ok(())
            };
        }
        for task in members {
            let settled = disposition
                .get(task)
                .is_some_and(|(s, _)| s == "done" || s == "ready_to_merge");
            if settled
                || automatic_skip(&records[task], disposition.get(task))
                || tasks.get(task).is_none_or(|t| t["phase"] != "result")
                || awaiting_owner_merge_checks(&records[task])
                || records[task]["conflict"]
                    .as_str()
                    .is_some_and(|c| c != "none")
            {
                continue;
            }
            match self.settle(project, task, &records[task], true).await {
                Settle::Accepted => {
                    // Acceptance is a write; use fresh Core and overlay facts next pass.
                    return Ok(());
                }
                Settle::Deferred => return Ok(()),
                Settle::Unobservable(reason) => {
                    return self.observation_blocked(project, observed, task, &reason).await;
                }
                Settle::Waiting(reason) | Settle::Failed(reason) => {
                    let code = "automatic_acceptance_blocked";
                    match self
                        .automatic_repair(
                            project,
                            runtime,
                            sprint,
                            task,
                            &tasks[task.as_str()]["executionId"],
                            None,
                            code,
                            &reason,
                            true,
                        )
                        .await?
                    {
                        Repair::Started | Repair::Later => return Ok(()),
                        Repair::Declined(mut facts) => {
                            facts.accept_only = true;
                            return self
                                .observed_pause(project, observed,
                                    automatic_hold(
                                        "settlement_blocked",
                                        Some(task),
                                        code,
                                        &reason,
                                        &facts,
                                    ),
                                )
                                .await;
                        }
                    }
                }
            }
        }
        match queue_step_in_order(members, &observed.priority, tasks, records, &disposition, true) {
            QueueStep::Hold(hold) => {
                let same = hold["code"] == prior["code"] && hold["taskId"] == prior["taskId"];
                if hold["code"] == "task_failed" && same && !prior["allowedAt"].is_i64() {
                    return Ok(());
                }
                self.hold_or_repair(project, runtime, observed, hold)
                    .await
            }
            QueueStep::Complete => {
                self.complete_queue(project, observed)
                .await
            }
            // A cleared system hold resumes the queue by itself.
            QueueStep::Wait | QueueStep::Start(_) if paused => {
                self.observed_pause(project, observed, Value::Null).await
            }
            QueueStep::Wait => Ok(()),
            QueueStep::Start(task) => {
                self.start_queue_task(project, runtime, observed, task, false)
                    .await
            }
        }
    }

    async fn start_queue_task(
        &self,
        project: &str,
        runtime: &Arc<Runtime>,
        observed: &Snapshot,
        task: &str,
        automatic: bool,
    ) -> io::Result<()> {
        let version = &observed.tasks[task]["version"];
        let request = json!({"protocol":{"major":1,"minor":8},
            "requestId":format!("sprint-start-{task}-{}",version.as_str().unwrap_or("0")),
            "projectId":project,"actorId":actor(),"operation":"tasks.start","target":task,
            "expectedVersion":version,"ownerEpoch":null,"payload":{}});
        let start = runtime.start_queued(project.to_owned(), request, automatic, observed.expectation()).await;
        let code = match &start {
            Ok(receipt) if receipt["outcome"] == "recorded" => return Ok(()),
            Ok(receipt) => receipt["outcome"].as_str().unwrap_or("refused").to_owned(),
            Err(error) => error.to_string(),
        };
        if code == sprint_live::STALE || code == "stale_version" {
            self.sprint_wake.project(project);
            return Ok(());
        }
        let reason = start_reason(&code);
        self.observed_pause(project, observed, automatic_hold(
            "start_refused", Some(task), &code, &reason, &HoldFacts::default(),
        )).await
    }
}
