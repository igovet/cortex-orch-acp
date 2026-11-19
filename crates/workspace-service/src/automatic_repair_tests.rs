use serde_json::json;

#[test]
fn stale_automatic_acceptance_is_actionable_when_the_result_can_be_rebuilt() {
    let actions = json!({
        "accept": false,
        "workspaceChanged": true,
        "reason": "This result or its workspace changed. Rerun checks and review the new result."
    });
    let checks = vec![json!({
        "name": "pilot deliverable checks",
        "outcome": "failed",
        "kind": "automated",
        "rerunnable": true,
        "command": ["node", "check.mjs", "pilot-demo"]
    })];
    let repair = crate::machine::task_auto_repair::plan(&actions, &checks, true)
        .expect("ordinary stale result must be repaired by Automatic");
    assert!(repair.prompt.contains("pilot deliverable checks"));
    assert!(repair.prompt.contains("fresh result"));
    assert!(repair.fingerprint.starts_with("sha256:"));
}

#[test]
fn owner_only_evidence_stays_actionable_for_the_owner() {
    let actions = json!({
        "accept": false,
        "reason": "A restricted check must be acknowledged by the owner."
    });
    let checks = vec![json!({
        "name": "paid engine qualification",
        "outcome": "not_run",
        "kind": "policy",
        "rerunnable": false
    })];
    assert!(crate::machine::task_auto_repair::plan(&actions, &checks, false).is_none());
}

#[test]
fn unknown_effect_never_gets_an_automatic_repair_prompt() {
    let actions = json!({
        "accept": false,
        "reason": "The dispatched action has an unknown outcome."
    });
    let checks = vec![json!({
        "name": "build",
        "outcome": "failed",
        "kind": "automated",
        "source": "runtime",
        "rerunnable": true,
        "technicalReason": "native_outside_outcome_unknown",
        "command": ["cargo", "test"]
    })];
    assert!(crate::machine::task_auto_repair::plan(&actions, &checks, false).is_none());
}

/// Every hold the Automatic scheduler can persist, with and without a task.
pub(crate) const HOLDS: &[(&str, &str)] = &[
    ("continuation", "external_blocker"),
    ("continuation", "continuation_cap"),
    ("continuation", "outcome_unknown"),
    ("owner_integration", "owner_accepted"),
    ("owner_integration", "different_target"),
    ("start_refused", "native_worktree_unavailable"),
    ("start_refused", "stale_version"),
    ("start_refused", "refused"),
    ("settlement_blocked", "task_missing"),
    ("settlement_blocked", "conflict_resolving"),
    ("settlement_blocked", "task_conflict"),
    ("settlement_blocked", "task_failed"),
    ("settlement_blocked", "task_blocked"),
    ("settlement_blocked", "task_stopped"),
    ("settlement_blocked", "task_paused"),
    ("settlement_blocked", "task_recovery"),
    ("settlement_blocked", "task_state_unknown"),
    ("settlement_blocked", "dependencies_blocked"),
    ("settlement_blocked", "automatic_acceptance_blocked"),
    ("settlement_blocked", "automatic_acceptance_required"),
    ("settlement_blocked", "stale_result"),
    ("settlement_blocked", "workspace_drift"),
    ("settlement_blocked", "task_files_changed"),
    ("settlement_blocked", "task_content_unknown"),
    ("settlement_blocked", "review_required"),
    ("settlement_blocked", "integration_failed"),
    ("settlement_blocked", "outcome_unknown"),
    ("settlement_blocked", "integration_outcome_unknown"),
    ("settlement_blocked", "integration_unavailable"),
    ("settlement_blocked", "unknown_branch"),
    ("settlement_blocked", "dirty_target"),
    ("settlement_blocked", "git_required"),
    ("settlement_blocked", "merge_conflict"),
    ("settlement_blocked", "native_worktree_unavailable"),
    // Saved by earlier versions; rewritten on the next pass.
    ("settlement_blocked", "queue_blocked"),
    ("settlement_blocked", "archived_task"),
];

pub(crate) const OWNER_ACTIONS: &[&str] = &[
    "allow_finish",
    "accept_result",
    "allow_repair",
    "allow_retries",
    "confirm_and_continue",
    "resume_task",
    "retry",
    "confirm_integration",
    "resolve_conflict",
    "change_run_mode",
    "close_as_is",
];

#[test]
fn every_automatic_hold_has_one_next_actor_and_owner_holds_have_actions() {
    use crate::native_driver::sprints::{automatic_hold, first_actor, HoldFacts};
    let variants = [
        HoldFacts::default(),
        HoldFacts {
            repair: true,
            ..HoldFacts::default()
        },
        HoldFacts {
            finish: true,
            asks: vec!["Finish without the check \u{201c}deploy\u{201d}.".into()],
            ..HoldFacts::default()
        },
        HoldFacts {
            repair: true,
            finish: true,
            ..HoldFacts::default()
        },
        HoldFacts {
            repair: true,
            finish: true,
            accept_only: true,
            ..HoldFacts::default()
        },
    ];
    for &(kind, code) in HOLDS {
        assert!(
            ["runtime", "agent", "owner"].contains(&first_actor(kind, code)),
            "{kind}/{code}"
        );
        for task in [Some("t1"), None] {
            for facts in &variants {
                let hold = automatic_hold(kind, task, code, "A plain reason.", facts);
                let actor = hold["nextActor"].as_str().unwrap();
                let actions: Vec<&str> = hold["ownerActions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|a| a.as_str().unwrap())
                    .collect();
                let asks = hold["asks"].as_array().unwrap();
                assert!(
                    ["runtime", "owner"].contains(&actor),
                    "{kind}/{code}: {hold}"
                );
                assert_eq!(hold["needsOwner"], actor == "owner", "{hold}");
                if actor == "owner" {
                    assert!(!actions.is_empty(), "owner hold without an action: {hold}");
                    assert!(!asks.is_empty(), "owner hold without a request: {hold}");
                    for ask in asks {
                        let ask = ask.as_str().unwrap();
                        assert!(!ask.trim().is_empty() && !ask.contains('_'), "{hold}");
                    }
                    if task.is_some() {
                        assert_eq!(actions.last(), Some(&"close_as_is"), "{hold}");
                    }
                    assert!(actions.iter().all(|a| OWNER_ACTIONS.contains(a)), "{hold}");
                    // The primary action is never the close; it is only offered alone.
                    if actions.len() > 1 {
                        assert_ne!(actions[0], "close_as_is", "{hold}");
                    }
                } else {
                    assert!(actions.is_empty() && asks.is_empty(), "{hold}");
                }
                // A result repair the owner may allow is offered only when possible.
                if actions.contains(&"allow_repair") {
                    assert!(facts.repair, "{hold}");
                }
                if actions.contains(&"allow_finish") {
                    assert!(
                        facts.finish && !facts.accept_only || code == "owner_accepted",
                        "{hold}"
                    );
                }
                if actions.contains(&"accept_result") {
                    assert!(facts.finish && facts.accept_only, "{hold}");
                }
            }
        }
    }
    // Unknown effects are never handed to an agent and never retried automatically.
    for code in [
        "outcome_unknown",
        "integration_outcome_unknown",
        "task_state_unknown",
    ] {
        let facts = HoldFacts {
            repair: true,
            ..HoldFacts::default()
        };
        let hold = automatic_hold("settlement_blocked", Some("t1"), code, "Unknown.", &facts);
        assert_eq!(hold["nextActor"], "owner");
        assert_eq!(hold["ownerActions"], json!(["close_as_is"]), "{hold}");
    }
    // A run that failed with known effects is agent work first.
    assert_eq!(first_actor("settlement_blocked", "task_failed"), "agent");
    assert_eq!(first_actor("continuation", "external_blocker"), "runtime");
}
