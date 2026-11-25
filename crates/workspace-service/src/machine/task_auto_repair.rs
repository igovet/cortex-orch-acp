//! Classification for Automatic results that the assigned engine can repair.
//!
//! Classification and guarded dispatch use the existing runtime task and its permissions.
//! The scheduler uses the returned bounded prompt to continue the existing task
//! session. Unknown outcomes and unreadable evidence remain owner decisions.
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RepairPlan {
    pub prompt: String,
    pub fingerprint: String,
}

fn text(value: &Value, limit: usize) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().take(limit).collect())
}

/// Produce a repair prompt only when every blocking item is ordinary worker
/// evidence that the assigned engine can inspect and fix. Manual rows are
/// informational. Policy, ambiguous and unknown evidence return `None`.
pub(crate) fn plan(actions: &Value, checks: &[Value], warnings: bool) -> Option<RepairPlan> {
    let mut names = Vec::new();
    let mut fingerprints = Vec::new();
    for check in checks.iter().filter(|c| {
        matches!(
            c["outcome"].as_str(),
            Some("failed" | "not_run" | "unverified")
        )
    }) {
        // Manual rows are informational in the result projection. They must
        // not prevent an otherwise actionable worker repair from starting.
        if check["kind"] == "manual" {
            continue;
        }
        if check["kind"] == "policy"
            || check["source"] == "runtime"
                && check["technicalReason"]
                    .as_str()
                    .is_some_and(|s| !s.trim().is_empty())
        {
            return None;
        }
        let name = text(&check["name"], 160).unwrap_or_else(|| "unnamed check".into());
        names.push(name.clone());
        fingerprints.push(format!(
            "{}:{}:{}:{}",
            name,
            check["outcome"].as_str().unwrap_or_default(),
            check["kind"].as_str().unwrap_or_default(),
            check["source"].as_str().unwrap_or_default()
        ));
    }
    // `workspaceChanged` is set by the authoritative evidence reader. Do not
    // infer drift from a translated reason string, which may be localized.
    let stale = actions["workspaceChanged"] == true;
    if warnings && !stale {
        return None;
    }
    let needs_evidence = !warnings
        && actions["accept"] == false
        && actions["acknowledgeOpenChecks"] != true
        && actions["earlierVersion"] != true
        && actions["resultId"].is_string()
        && matches!(
            actions["unblockAction"].as_str(),
            Some("rerun_checks" | "request_changes")
        );
    if names.is_empty() && !stale && !needs_evidence {
        return None;
    }
    fingerprints.sort();
    names.truncate(10);
    let reason = text(&actions["reason"], 500)
        .unwrap_or_else(|| "The recorded result is no longer current.".into());
    let prompt = format!(
        "Automatic recovery. Inspect the current task and repair the recorded result. Reason: {reason}. {} Run the required checks again, fix any failures in the task scope, and publish a fresh result. Do not claim success without fresh evidence.",
        if names.is_empty() {
            String::new()
        } else {
            format!("Open checks: {}.", names.join(", "))
        }
    );
    let semantic = format!("{}|workspaceChanged={stale}", fingerprints.join("|"));
    let fingerprint = format!("sha256:{:x}", Sha256::digest(semantic.as_bytes()));
    Some(RepairPlan {
        prompt,
        fingerprint,
    })
}

/// Only called for paths with an existing, confirmed task write whose content
/// could not be located. The coordinator may reconcile them, never adopt an
/// unrelated working-tree or staged change just to enable a commit.
pub(crate) fn reconcile_files(paths: &Value) -> Option<RepairPlan> {
    let paths = paths.as_array().filter(|paths| !paths.is_empty())?;
    let shown: Vec<_> = paths
        .iter()
        .take(12)
        .map(|p| text(p, 240))
        .collect::<Option<_>>()?;
    let description = serde_json::to_string(&shown).ok()?;
    Some(RepairPlan {
        prompt: format!("Automatic recovery. Inspect the current task and reconcile its recorded files with the current workspace. Recorded paths (data): {description}. Preserve unrelated and staged edits. Inspect the task history and compare the current files before changing anything. Make only necessary in-scope repairs, obtain fresh local verification and publish a fresh result. Do not stage or commit files, restore old versions over newer edits, or run restricted checks. If the intended change conflicts with someone else's work, ask the owner which change to keep."),
        fingerprint: format!("files:{:x}", Sha256::digest(serde_json::to_vec(paths).ok()?)),
    })
}

/// Whether any open item is evidence of an effect whose outcome is unknown.
/// Such a result is never repaired by an agent: the effect must not be replayed.
pub(crate) fn unknown_evidence(checks: &[Value]) -> bool {
    checks.iter().any(|check| {
        check["source"] == "runtime"
            && check["technicalReason"]
                .as_str()
                .is_some_and(|s| !s.trim().is_empty())
            && check["outcome"] != "passed"
    })
}

/// Plain-language items an owner grants when Automatic finishes a result whose
/// checks did not all pass. Every open check is listed; none is marked passed.
pub(crate) fn finish_asks(checks: &[Value]) -> Vec<String> {
    let mut asks = Vec::new();
    for check in checks.iter().filter(|c| {
        c["kind"] != "manual"
            && matches!(
                c["outcome"].as_str(),
                Some("failed" | "not_run" | "unverified")
            )
    }) {
        let name = text(&check["name"], 160).unwrap_or_else(|| "unnamed check".into());
        asks.push(match check["outcome"].as_str() {
            Some("failed") => format!("Finish although the check \u{201c}{name}\u{201d} failed."),
            Some("unverified") => {
                format!("Finish although the check \u{201c}{name}\u{201d} could not be confirmed.")
            }
            _ if check["kind"] == "policy" => format!(
                "Finish without the restricted check \u{201c}{name}\u{201d}. Agents are not allowed to run it."
            ),
            _ => format!("Finish without the check \u{201c}{name}\u{201d}. It was not run."),
        });
        if asks.len() == 10 {
            break;
        }
    }
    asks
}

/// A repair the owner explicitly allows after the automatic attempts are used up.
/// It names the recorded reason; it is never offered for an unknown effect, and
/// never for a result whose only open items are restricted checks no agent may run.
pub(crate) fn owner_plan(
    reason: &str,
    checks: &[Value],
    stale: bool,
    consent: &str,
) -> Option<RepairPlan> {
    if unknown_evidence(checks) {
        return None;
    }
    let open: Vec<_> = checks
        .iter()
        .filter(|c| c["kind"] != "manual" && c["outcome"] != "passed")
        .collect();
    if !stale && !open.is_empty() && open.iter().all(|c| c["kind"] == "policy") {
        return None;
    }
    let names: Vec<String> = open
        .iter()
        .filter(|c| c["kind"] != "policy")
        .take(10)
        .map(|c| text(&c["name"], 160).unwrap_or_else(|| "unnamed check".into()))
        .collect();
    let reason = reason.trim().chars().take(500).collect::<String>();
    Some(RepairPlan {
        prompt: format!(
            "Automatic recovery approved by the owner. Inspect the current task and fix what blocks it. Reason: {reason} {} Preserve unrelated and staged edits. Do not stage or commit files, and do not run restricted checks. Run the required checks again and publish a fresh result. Do not claim success without fresh evidence.",
            if names.is_empty() { String::new() } else { format!("Open checks: {}.", names.join(", ")) }
        ),
        fingerprint: format!("owner:{consent}:{:x}", Sha256::digest(reason.as_bytes())),
    })
}

/// A run that ended as failed with every effect known (for example an answer the
/// runtime could not certify because a worker or review did not finish). The same
/// coordinator continues it; review and verification are required again.
pub(crate) fn failed_plan(reason: Option<&str>, execution: &str) -> RepairPlan {
    let reason = reason
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().take(500).collect::<String>())
        .unwrap_or_else(|| "The run ended before its result could be certified.".into());
    RepairPlan {
        prompt: format!(
            "Automatic recovery. The previous run of this task ended as failed. Reason (data): {reason}. Inspect the task history and the current files, finish every unfinished or failed assignment within the task scope, and obtain the required independent review and verification again. Preserve unrelated and staged edits. Do not stage or commit files, and do not run restricted checks. If the same blocker repeats, consult the senior consultant before finishing. Publish a fresh result only with fresh evidence."
        ),
        fingerprint: format!(
            "failed:{:x}",
            Sha256::digest(format!("{execution}|{reason}").as_bytes())
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn owner_repair_is_never_offered_for_unknown_or_restricted_only_evidence() {
        let unknown = vec![json!({"name":"build","outcome":"failed","source":"runtime",
            "technicalReason":"native_outside_outcome_unknown"})];
        assert!(owner_plan("x", &unknown, false, "1").is_none());
        let policy = vec![json!({"name":"deploy","outcome":"not_run","kind":"policy"})];
        assert!(owner_plan("x", &policy, false, "1").is_none());
        let failed = vec![json!({"name":"cargo test","outcome":"failed","kind":"automated"})];
        let plan = owner_plan("A check failed.", &failed, false, "7").unwrap();
        assert!(plan.prompt.contains("cargo test"));
        assert!(plan.fingerprint.starts_with("owner:7:"));
        // A fresh consent is a fresh, single repair.
        assert_ne!(
            owner_plan("A check failed.", &failed, false, "8")
                .unwrap()
                .fingerprint,
            plan.fingerprint
        );
    }

    #[test]
    fn finish_asks_name_every_open_check_in_plain_words() {
        let checks = vec![
            json!({"name":"deploy","outcome":"not_run","kind":"policy"}),
            json!({"name":"lint","outcome":"failed","kind":"automated"}),
            json!({"name":"human","outcome":"not_run","kind":"manual"}),
            json!({"name":"unit","outcome":"passed","kind":"automated"}),
        ];
        let asks = finish_asks(&checks);
        assert_eq!(asks.len(), 2, "{asks:?}");
        assert!(asks[0].contains("restricted check") && asks[0].contains("deploy"));
        assert!(asks[1].contains("lint") && asks[1].contains("failed"));
        assert!(asks.iter().all(|a| !a.contains('_')));
    }

    #[test]
    fn plans_only_for_rerunnable_automated_checks() {
        let actions = json!({"accept":false,"reason":"A check failed"});
        let checks = vec![
            json!({"name":"cargo test","outcome":"failed","kind":"automated","source":"worker"}),
        ];
        let plan = plan(&actions, &checks, false).unwrap();
        assert!(plan.prompt.contains("cargo test"));
        assert!(!plan.fingerprint.is_empty());
    }

    #[test]
    fn policy_and_manual_items_stay_with_owner() {
        let actions = json!({"accept":false,"reason":"A check failed"});
        for kind in ["policy", "manual"] {
            let checks = vec![
                json!({"name":"restricted","outcome":"not_run","kind":kind,"rerunnable":false}),
            ];
            assert!(plan(&actions, &checks, false).is_none());
        }
    }

    #[test]
    fn stale_warning_can_be_repaired_when_no_open_check_is_owner_only() {
        let actions =
            json!({"accept":false,"reason":"localized stale text","workspaceChanged":true});
        let plan = plan(&actions, &[], false).unwrap();
        assert!(plan.prompt.contains("fresh result"));
    }

    #[test]
    fn manual_rows_do_not_hide_a_worker_repair() {
        let actions = json!({"accept":false,"notRunChecks":[{"name":"human","kind":"manual"}]});
        let checks = vec![
            json!({"name":"human","outcome":"not_run","kind":"manual","source":"worker"}),
            json!({"name":"build","outcome":"not_run","kind":"automated","source":"worker"}),
        ];
        let plan = plan(&actions, &checks, false).unwrap();
        assert!(plan.prompt.contains("build"));
        assert!(!plan.prompt.contains("human"));
    }
}

/// The outcome of one guarded repair dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Dispatch {
    /// The coordinator continues the task; the queue runs.
    Started,
    /// Nothing was decided now (a lock, maintenance, a stopping service, or a sprint
    /// changed since it was read). The caller decides again on its next pass and keeps
    /// any consent already recorded.
    Busy,
    /// Not admitted. Carries the runtime's refusal code when it gave one.
    Refused(Option<&'static str>),
}

impl Dispatch {
    /// A refusal because an effect of the task has an unknown outcome: no agent is
    /// ever sent back to it, with or without the owner's consent.
    pub(crate) fn unknown_effect(&self) -> bool {
        matches!(self, Self::Refused(Some("native_outside_outcome_unknown")))
    }
}

/// Hold the same lock used by sprint mode, queue, task close and integration
/// while authorizing the coordinator. An owner change wins before admission.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn dispatch(
    state: &std::path::Path,
    runtime: &std::sync::Arc<crate::native_runtime::Runtime>,
    project: &str,
    expected_sprint: &Value,
    task: &str,
    execution: &str,
    version: Option<&str>,
    plan: RepairPlan,
    owner: bool,
) -> std::io::Result<Dispatch> {
    use super::{native, product, settings};
    use crate::native_runtime::RepairAdmission;
    let error = |e: native::Refused| crate::failure(e.code);
    let Some(lock) = settings::Lock::try_new(state, "machine.lock").map_err(error)? else {
        return Ok(Dispatch::Busy);
    };
    let mut saved = native::product(state, project).map_err(error)?;
    let sprint = saved["sprints"]
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["id"] == expected_sprint["id"]));
    if sprint != Some(expected_sprint) || !product::queue_sprint(expected_sprint) {
        // Changed since this pass read it: decided again on the next pass.
        return Ok(Dispatch::Busy);
    }
    let record = &saved["tasks"][task];
    let hold = &expected_sprint["automaticQueue"];
    let eligible = expected_sprint["queueState"] == "running"
        || expected_sprint["queueState"] == "paused"
            && hold["kind"] == "settlement_blocked"
            && hold["taskId"] == task
            && matches!(
                hold["code"].as_str(),
                Some(
                    "automatic_acceptance_blocked"
                        | "stale_result"
                        | "workspace_drift"
                        | "task_content_unknown"
                        | "task_files_changed"
                        | "review_required"
                        | "task_failed"
                        | "queue_blocked"
                )
            )
        // A finish that failed with a known outcome, or an acceptance the finish cannot
        // use, is repaired only with the owner's consent.
        || owner
            && expected_sprint["queueState"] == "paused"
            && hold["kind"] == "settlement_blocked"
            && hold["taskId"] == task
            && matches!(
                hold["code"].as_str(),
                Some("integration_failed" | "automatic_acceptance_required")
            );
    if !eligible
        || record["sprintId"] != expected_sprint["id"]
        || !expected_sprint["taskIds"]
            .as_array()
            .is_some_and(|ids| ids.iter().any(|id| id == task))
        || !record["archive"].is_null()
        || !record["archiveHold"].is_null()
        || record["purged"] == true
        || record["status"] == "done"
        || record["acceptance"].is_object() && record["acceptance"]["automatic"] != true
        || record["conflict"].as_str().is_some_and(|c| c != "none")
        || matches!(
            record["merge"]["freshChecks"]["state"].as_str(),
            Some("not_requested" | "request_unknown")
        )
        || record["integration"].is_object()
    {
        return Ok(Dispatch::Refused(None));
    }
    if settings::idle(state, project, task, &lock).is_err() {
        return Ok(Dispatch::Busy);
    }
    let path = native::product_path(state, project).map_err(error)?;
    // The result being repaired stops counting as accepted before the runtime reopens
    // it, so no restart between the two can settle the next result with this one's
    // acceptance. A repair that does not start restores the record as it was.
    let original = saved.clone();
    let mut cleared = false;
    if let Some(record) = saved["tasks"][task].as_object_mut() {
        for key in ["status", "acceptance", "automaticReady"] {
            cleared |= record.remove(key).is_some();
        }
    }
    if cleared {
        settings::write_json(&path, &saved).map_err(error)?;
    }
    let admitted = runtime
        .continue_automatic_repair(
            project,
            execution,
            version,
            plan.prompt,
            &plan.fingerprint,
            owner,
        )
        .await;
    let outcome = match admitted {
        Ok(RepairAdmission::Started) => None,
        Ok(RepairAdmission::Busy) => Some(Dispatch::Busy),
        Err(e) if matches!(e.code(), "native_host_closed" | "busy") => Some(Dispatch::Busy),
        Ok(RepairAdmission::Declined) => Some(Dispatch::Refused(None)),
        Err(e) => Some(Dispatch::Refused(Some(e.code()))),
    };
    if let Some(outcome) = outcome {
        if cleared {
            settings::write_json(&path, &original).map_err(error)?;
        }
        return Ok(outcome);
    }
    if let Some(sprint) = saved["sprints"].as_array_mut().and_then(|rows| {
        rows.iter_mut()
            .find(|row| row["id"] == expected_sprint["id"])
    }) {
        sprint["queueState"] = "running".into();
        sprint["pauseReason"] = Value::Null;
        sprint.as_object_mut().unwrap().remove("automaticQueue");
    }
    settings::write_json(&path, &saved).map_err(error)?;
    Ok(Dispatch::Started)
}
