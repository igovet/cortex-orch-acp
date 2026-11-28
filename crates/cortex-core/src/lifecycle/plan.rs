//! Transactional plan intake. Schema validation happens at the authenticated gateway.
use super::{domain, event_tx, request_tx, MAX_NODES};
use crate::{
    error::refuse,
    scheduler::{self, policy},
    CoreError, CoreResult, TaskState,
};
use rusqlite::{Connection, OptionalExtension, Transaction};
use serde_json::{json, Value};
use std::collections::BTreeSet;
const CONSULTATION: &str = "_consultation";
const RESERVED_ROLES: [&str; 3] = ["code-reviewer", "build-verification", "senior-consultant"];
pub const PLANNABLE_ROLES: [&str; 2] = ["explorer", "general"];

/// Retained role identities keep their original tool catalogue, routing and
/// access checks. Only instruction assembly resolves a role to a base profile.
pub fn legacy_profile(role: &str) -> Option<(&'static str, &'static [&'static str])> {
    Some(match role {
        "worker-accessibility-auditor" => ("worker-explorer", &["accessibility"]),
        "worker-accessibility-fixer" => ("worker-general", &["accessibility"]),
        "worker-architect" => ("worker-explorer", &["architecture"]),
        "worker-backend-dev" => ("worker-general", &["backend"]),
        "worker-data-engineer" => ("worker-general", &["data"]),
        "worker-database-architect" => ("worker-explorer", &["database"]),
        "worker-debugger" => ("worker-general", &["debugging"]),
        "worker-devops-engineer" => ("worker-general", &["operations"]),
        "worker-frontend-dev" => ("worker-general", &["frontend"]),
        "worker-fullstack-dev" => ("worker-general", &["backend", "frontend"]),
        "worker-mobile-dev" => ("worker-general", &["mobile"]),
        "worker-performance-engineer" => ("worker-explorer", &["performance"]),
        "worker-planner" => ("worker-explorer", &["planning"]),
        "worker-qa-engineer" => ("worker-general", &["testing"]),
        "worker-refactorer" => ("worker-general", &["refactoring"]),
        "worker-security-auditor" => ("worker-explorer", &["security"]),
        "worker-technical-writer" => ("worker-general", &["documentation"]),
        "worker-ux-designer" => ("worker-explorer", &["ux"]),
        _ => return None,
    })
}

pub const COMPETENCIES: &[&str] = &[
    "accessibility",
    "architecture",
    "backend",
    "data",
    "database",
    "debugging",
    "documentation",
    "frontend",
    "mobile",
    "operations",
    "performance",
    "planning",
    "refactoring",
    "security",
    "testing",
    "ux",
];

fn validated_competencies(value: &Value) -> CoreResult<BTreeSet<&str>> {
    let items = value
        .as_array()
        .filter(|items| items.len() <= 4)
        .ok_or(CoreError::Refused("invalid_plan_competencies"))?;
    let mut ids = BTreeSet::new();
    for item in items {
        let id = item
            .as_str()
            .filter(|id| COMPETENCIES.contains(id))
            .ok_or(CoreError::Refused("invalid_plan_competencies"))?;
        if !ids.insert(id) {
            return refuse("invalid_plan_competencies");
        }
    }
    Ok(ids)
}

pub const BASE_SUFFICIENCY_REASON_MAX: usize = 600;

fn validate_competency_decision(node: &Value) -> CoreResult<()> {
    let selected = node.get("competencies").ok_or(CoreError::Refused("plan_competencies_required"))?;
    let modules = validated_competencies(selected)?;
    if modules.is_empty() && node.get("base_sufficiency_reason").is_none() {
        return refuse("plan_base_sufficiency_reason_required");
    }
    Ok(())
}

/// Exact authored selection, with absence preserved for pre-contract plans.
pub fn competency_decision(node: &Value) -> Option<Value> {
    let selected = node.get("competencies")?;
    let mut decision = json!({"competencies":selected});
    if let Some(reason) = node.get("base_sufficiency_reason") {
        decision["base_sufficiency_reason"] = reason.clone();
    }
    if let Some(source) = node.get("competency_decision_source") {
        decision["source"] = source.clone();
    }
    Some(decision)
}

pub fn assignment_competencies(
    db: &rusqlite::Connection,
    task: &str,
    assignment: &str,
) -> CoreResult<Vec<String>> {
    let source: Option<(String, String, Option<String>)> = db
        .query_row(
            "SELECT n.node_id,n.role,t.plan_json FROM runtime_nodes n JOIN runtime_tasks t
          ON t.task_id=n.task_id AND t.revision=n.revision
          WHERE n.task_id=? AND n.assignment_ref=? AND n.stage='execution'",
            (task, assignment),
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((node, role, plan)) = source else {
        return Ok(Vec::new());
    };
    let plan = serde_json::from_str(plan.as_deref().unwrap_or("{}"))?;
    let data = node_routing(&plan, &node);
    data.get("competencies").map_or_else(
        || Ok(legacy_profile(&role).map_or(&[][..], |(_, ids)| ids)
            .iter().map(|id| (*id).to_owned()).collect()),
        |items| {
            Ok(validated_competencies(items)?
                .into_iter()
                .map(str::to_owned)
                .collect())
        },
    )
}

fn unprefixed_role(role: &str) -> &str {
    role.strip_prefix("worker-").unwrap_or(role)
}

pub fn role(value: &str) -> CoreResult<String> {
    if RESERVED_ROLES.contains(&unprefixed_role(value)) {
        return refuse("runtime_reserved_role");
    }
    let role = if value.starts_with("worker-") {
        value.to_owned()
    } else {
        format!("worker-{value}")
    };
    if ![
        "accessibility-auditor",
        "accessibility-fixer",
        "architect",
        "backend-dev",
        "data-engineer",
        "database-architect",
        "debugger",
        "devops-engineer",
        "explorer",
        "frontend-dev",
        "fullstack-dev",
        "general",
        "mobile-dev",
        "performance-engineer",
        "planner",
        "qa-engineer",
        "refactorer",
        "security-auditor",
        "technical-writer",
        "ux-designer",
    ]
    .iter()
    .any(|r| role == format!("worker-{r}"))
    {
        return refuse("unknown_worker_role");
    }
    Ok(role)
}
pub fn verification(v: &Value) -> CoreResult<Vec<Vec<String>>> {
    let commands = v
        .as_array()
        .filter(|a| (1..=16).contains(&a.len()))
        .ok_or(CoreError::Refused("verification_required"))?;
    commands
        .iter()
        .map(|v| {
            let a = v
                .as_array()
                .filter(|a| (1..=64).contains(&a.len()))
                .ok_or(CoreError::Refused("invalid_verification_command"))?;
            a.iter()
                .map(|v| {
                    let s = v
                        .as_str()
                        .filter(|s| !s.is_empty() && s.chars().count() <= 4096 && !s.contains('\0'))
                        .ok_or(CoreError::Refused("invalid_verification_command"))?;
                    if ["<", ">", ">>", "|", "&&", "||", ";", "(", ")"].contains(&s)
                        || s.contains("$(")
                        || s.contains('`')
                    {
                        return refuse("verification_shell_syntax");
                    }
                    Ok(s.to_owned())
                })
                .collect()
        })
        .collect()
}
/// Routing fields a node carries into model/route selection.
pub const ROUTING_FIELDS: [&str; 5] = [
    "complexity",
    "work_kind",
    "provider",
    "model",
    "model_reason",
];
/// Routing view of a node: a planned node's own entry, or the routing recorded for
/// a runtime-created node (fix, review). Unknown nodes route with defaults.
pub fn node_routing(plan: &Value, node: &str) -> Value {
    plan["tasks"]
        .as_array()
        .and_then(|tasks| tasks.iter().find(|n| n["id"] == node))
        .cloned()
        .or_else(|| plan["routing"].get(node).cloned())
        .unwrap_or_else(|| json!({}))
}
/// Keep only the routing fields of a planned node.
pub fn routing_fields(node: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for key in ROUTING_FIELDS {
        if let Some(v) = node.get(key).filter(|v| !v.is_null()) {
            out.insert(key.to_owned(), v.clone());
        }
    }
    Value::Object(out)
}
/// Store routing for a runtime-created node beside the accepted plan, so routing
/// finds it by node id. A replan replaces `plan_json`, which scopes it to its revision.
pub fn record_node_routing_tx(
    db: &Transaction<'_>,
    task: &str,
    node: &str,
    routing: &Value,
) -> CoreResult<()> {
    let raw: Option<String> = db.query_row(
        "SELECT plan_json FROM runtime_tasks WHERE task_id=?",
        [task],
        |r| r.get(0),
    )?;
    let Some(raw) = raw else { return Ok(()) };
    let mut plan: Value = serde_json::from_str(&raw)?;
    let Some(object) = plan.as_object_mut() else {
        return Ok(());
    };
    let entry = object.entry("routing").or_insert_with(|| json!({}));
    if let Some(entry) = entry.as_object_mut() {
        entry.insert(node.to_owned(), routing.clone());
    }
    db.execute(
        "UPDATE runtime_tasks SET plan_json=? WHERE task_id=?",
        (plan.to_string(), task),
    )?;
    Ok(())
}
/// Most goal outcomes one plan names.
pub const MAX_OUTCOMES: usize = 32;
pub(crate) fn outcome_id(v: &Value) -> Option<&str> {
    v.as_str().filter(|id| {
        id.len() <= 64
            && id.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
            && id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    })
}
fn blank(v: &Value, max: usize) -> bool {
    v.as_str()
        .is_none_or(|s| s.trim().is_empty() || s.chars().count() > max)
}
/// The required results of the goal a plan names (`plan.outcomes`), each delivered
/// by a node (`delivers`) or explicitly deferred with its reason. A deferred
/// outcome stays part of the goal: completion still needs it done.
fn validate_outcomes(plan: &Value) -> CoreResult<BTreeSet<String>> {
    let outcomes = plan["outcomes"]
        .as_array()
        .filter(|a| !a.is_empty() && a.len() <= MAX_OUTCOMES)
        .ok_or(CoreError::Refused("plan_outcomes_required"))?;
    let mut ids = BTreeSet::new();
    let mut deferred = BTreeSet::new();
    for outcome in outcomes {
        let fields = outcome
            .as_object()
            .ok_or(CoreError::Refused("invalid_plan_outcome"))?;
        if fields
            .keys()
            .any(|k| !["id", "statement", "deferred", "deferred_by"].contains(&k.as_str()))
        {
            return refuse("invalid_plan_outcome");
        }
        let id = outcome_id(&outcome["id"]).ok_or(CoreError::Refused("invalid_plan_outcome"))?;
        // An entry without a statement refers to an outcome of the plan in force by its
        // id; acceptance fills the recorded statement (`carry_outcomes`).
        let statement = outcome.get("statement").filter(|v| !v.is_null());
        if !ids.insert(id.to_owned()) || statement.is_some_and(|s| blank(s, 1000)) {
            return refuse("invalid_plan_outcome");
        }
        match outcome.get("deferred").filter(|v| !v.is_null()) {
            Some(reason) if blank(reason, 1000) => {
                return refuse("plan_outcome_deferral_reason_required")
            }
            Some(_) => {
                deferred.insert(id.to_owned());
            }
            None => {}
        }
    }
    let mut delivered = BTreeSet::new();
    for node in plan["tasks"].as_array().into_iter().flatten() {
        let Some(list) = node.get("delivers").filter(|v| !v.is_null()) else {
            continue;
        };
        let list = list
            .as_array()
            .ok_or(CoreError::Refused("invalid_plan_node"))?;
        let mut own = BTreeSet::new();
        for id in list {
            let id = outcome_id(id).ok_or(CoreError::Refused("plan_outcome_unknown"))?;
            if !ids.contains(id) {
                return refuse("plan_outcome_unknown");
            }
            if !own.insert(id) {
                return refuse("invalid_plan_node");
            }
            delivered.insert(id.to_owned());
        }
    }
    for id in &ids {
        match (delivered.contains(id), deferred.contains(id)) {
            (false, false) => return refuse("plan_outcome_uncovered"),
            (true, true) => return refuse("plan_outcome_deferral_conflict"),
            _ => {}
        }
    }
    Ok(ids)
}
/// What a plan refused for `plan_outcome_uncovered`, `plan_outcome_unknown`,
/// `plan_outcome_deferral_conflict` or `plan_outcome_deferral_reason_required`
/// must fix, each item named: outcomes no task delivers and none defers
/// (`uncovered`), `delivers` ids that no outcome defines (`unknown_delivers`, with
/// the task), outcomes both delivered and deferred (`delivered_and_deferred`) and
/// deferred outcomes without a usable reason (`deferral_reason_missing`).
pub fn outcome_diagnosis(plan: &Value) -> Value {
    let outcomes: Vec<&Value> = plan["outcomes"].as_array().into_iter().flatten().collect();
    let ids: Vec<&str> = outcomes.iter().filter_map(|o| o["id"].as_str()).collect();
    let mut delivered: BTreeSet<&str> = BTreeSet::new();
    let mut unknown = Vec::new();
    for node in plan["tasks"].as_array().into_iter().flatten() {
        for id in node["delivers"].as_array().into_iter().flatten().filter_map(|v| v.as_str()) {
            if ids.contains(&id) {
                delivered.insert(id);
            } else {
                unknown.push(json!({"task":node["id"],"id":id}));
            }
        }
    }
    let deferred = |o: &Value| o.get("deferred").is_some_and(|d| !d.is_null());
    let pick = |f: &dyn Fn(&Value, &str) -> bool| -> Vec<Value> {
        outcomes
            .iter()
            .filter_map(|o| o["id"].as_str().filter(|id| f(o, id)).map(|id| json!(id)))
            .collect()
    };
    json!({
        "uncovered":pick(&|o, id| !delivered.contains(id) && !deferred(o)),
        "unknown_delivers":unknown,
        "delivered_and_deferred":pick(&|o, id| delivered.contains(id) && deferred(o)),
        "deferral_reason_missing":pick(&|o, _| deferred(o) && blank(&o["deferred"], 1000)),
        "defined_outcome_ids":ids,
    })
}
/// The goal outcomes of a stored plan, or None for a plan accepted before plans
/// named them (legacy) or no plan at all.
pub fn stored_outcomes(plan: &Value) -> Option<Vec<Value>> {
    plan["outcomes"]
        .as_array()
        .filter(|a| !a.is_empty())
        .cloned()
}
/// The goal outcomes of the task's plan in force (see [`stored_outcomes`]).
pub fn task_outcomes(db: &rusqlite::Connection, task: &str) -> CoreResult<Option<Vec<Value>>> {
    let raw: Option<String> = db
        .query_row(
            "SELECT plan_json FROM runtime_tasks WHERE task_id=?",
            [task],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    Ok(raw
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|plan| stored_outcomes(&plan)))
}
fn same_statement(a: &Value, b: &Value) -> bool {
    let norm = |v: &Value| {
        v.as_str()
            .unwrap_or("")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    norm(a) == norm(b)
}
/// Every outcome of the plan in force stays in its replacement with the same
/// statement: a replan, a steering amendment or a continuation may add outcomes (an
/// owner message can add one), never drop or reword one.
pub fn require_kept_outcomes(previous: &Value, next: &Value) -> CoreResult<()> {
    if dropped_outcomes(previous, next).is_empty() {
        Ok(())
    } else {
        refuse("plan_outcome_dropped")
    }
}
/// The outcomes of the plan in force (`previous`) that `next` does not keep with the
/// same id and statement, verbatim (id, statement, deferred), so a refusal can name
/// exactly what the replacement must carry. An id-only entry of `next` keeps its
/// outcome (see [`carry_outcomes`]).
pub fn dropped_outcomes(previous: &Value, next: &Value) -> Vec<Value> {
    let Some(before) = stored_outcomes(previous) else {
        return Vec::new();
    };
    let after = next["outcomes"].as_array().cloned().unwrap_or_default();
    before
        .into_iter()
        .filter(|outcome| {
            !after.iter().any(|o| {
                o["id"] == outcome["id"]
                    && (o.get("statement").is_none_or(Value::is_null)
                        || same_statement(&o["statement"], &outcome["statement"]))
            })
        })
        .map(|o| verbatim(&o))
        .collect()
}
/// One outcome as the coordinator must carry it: id, exact statement and, when the
/// plan deferred it, the recorded reason.
pub fn verbatim(outcome: &Value) -> Value {
    let mut v = json!({"id":outcome["id"],"statement":outcome["statement"]});
    if outcome.get("deferred").is_some_and(|d| !d.is_null()) {
        v["deferred"] = outcome["deferred"].clone();
    }
    if outcome["deferred_by"] == RUNTIME_DEFERRED {
        v["deferred_by"] = json!(RUNTIME_DEFERRED);
    }
    v
}
/// The ids of `plan`'s outcome entries that carry no statement and name no outcome
/// of the plan in force (`plan_outcome_statement_required`).
pub fn unknown_references(previous: Option<&Value>, plan: &Value) -> Vec<Value> {
    let before = previous.and_then(stored_outcomes).unwrap_or_default();
    plan["outcomes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|o| o.get("statement").is_none_or(Value::is_null))
        .filter(|o| !before.iter().any(|p| p["id"] == o["id"]))
        .map(|o| o["id"].clone())
        .collect()
}
/// Carry-over by reference: an outcome entry with its id and no statement keeps
/// that outcome of the plan in force, and the runtime fills the recorded statement,
/// so a replan never needs the model to retype it. An id-only entry that names no
/// outcome of the plan in force is refused (`plan_outcome_statement_required`). A
/// supplied statement is kept as written; [`require_kept_outcomes`] refuses one that
/// rewords a kept outcome.
pub fn carry_outcomes(previous: Option<&Value>, plan: &Value) -> CoreResult<Value> {
    if !plan["outcomes"].is_array() {
        return Ok(plan.clone());
    }
    if !unknown_references(previous, plan).is_empty() {
        return refuse("plan_outcome_statement_required");
    }
    let before = previous.and_then(stored_outcomes).unwrap_or_default();
    let mut filled = plan.clone();
    for o in filled["outcomes"].as_array_mut().into_iter().flatten() {
        let Some(fields) = o.as_object_mut() else {
            continue;
        };
        // `deferred_by` is the runtime's record, never the model's: it is derived here.
        fields.remove("deferred_by");
        let prior = before.iter().find(|p| p["id"] == fields["id"]);
        if fields.get("statement").is_none_or(Value::is_null) {
            if let Some(prior) = prior {
                fields.insert("statement".into(), prior["statement"].clone());
            }
        }
        // An outcome the runtime kept stays marked while a plan still defers it.
        if fields.get("deferred").is_some_and(|d| !d.is_null())
            && prior.is_some_and(|p| p["deferred_by"] == RUNTIME_DEFERRED)
        {
            fields.insert("deferred_by".into(), json!(RUNTIME_DEFERRED));
        }
    }
    Ok(filled)
}
/// The `deferred_by` value of an outcome the runtime kept when a plan dropped it
/// ([`refused_outcomes_tx`]). Like every deferred outcome it is still owed: only a
/// final result reporting it done, or the owner's close as is, completes the task.
pub const RUNTIME_DEFERRED: &str = "runtime";
/// Identical `plan_outcome_dropped` refusals of one coordinator before the runtime
/// keeps the missing outcomes itself (see [`refused_outcomes_tx`]).
pub const OUTCOME_REFUSAL_LIMIT: usize = 3;
/// One plan refused because it dropped or reworded outcomes of the plan in force.
pub const OUTCOME_REFUSED: &str = "PLAN_OUTCOME_REFUSED";
const KEPT_REASON: &str = "Kept by the runtime: the replacement plan did not carry this outcome of the plan in force; a later plan delivers it.";
/// `plan` with every outcome of the plan in force it dropped or reworded restored
/// verbatim: a reworded entry gets the recorded statement back (its delivery is
/// kept), a dropped one is appended as deferred, so it stays owed.
pub fn keep_dropped(previous: &Value, plan: &Value) -> Value {
    let missing = dropped_outcomes(previous, plan);
    let mut kept = plan.clone();
    if !kept["outcomes"].is_array() {
        kept["outcomes"] = json!([]);
    }
    let list = kept["outcomes"].as_array_mut().unwrap();
    for m in missing {
        match list.iter_mut().find(|o| o["id"] == m["id"] && o.is_object()) {
            Some(o) => o["statement"] = m["statement"].clone(),
            None => list.push(json!({"id":m["id"],"statement":m["statement"],"deferred":KEPT_REASON})),
        }
    }
    kept
}
/// After a plan was refused with `plan_outcome_dropped` (in its own, rolled back
/// transaction): record the refusal and, once the same outcomes were refused
/// [`OUTCOME_REFUSAL_LIMIT`] times since the plan in force was accepted or the owner
/// last spoke, accept the plan with those outcomes kept by the runtime
/// ([`keep_dropped`]) and record `LOOP_BROKEN`, so a coordinator that cannot carry
/// them never spins. Nothing of the goal is dropped: a kept outcome stays owed.
/// Returns the acceptance, or None when only the refusal was recorded.
pub fn refused_outcomes_tx(
    db: &Transaction<'_>,
    task: &str,
    actor: &str,
    key: &str,
    plan: &Value,
) -> CoreResult<Option<Value>> {
    let Some(previous) = plan_in_force(db, task)? else {
        return Ok(None);
    };
    let plan = carry_outcomes(Some(&previous), plan)?;
    let mut missing: Vec<String> = dropped_outcomes(&previous, &plan)
        .iter()
        .filter_map(|o| o["id"].as_str().map(str::to_owned))
        .collect();
    if missing.is_empty() {
        return Ok(None);
    }
    missing.sort();
    let window: i64 = db.query_row(
        "SELECT coalesce(max(id),0) FROM runtime_events WHERE task_id=? AND type IN
         ('PLAN_ACCEPTED','USER_STEERING','USER_ANSWER_RECEIVED','TASK_REOPENED','LOOP_BROKEN')",
        [task],
        |r| r.get(0),
    )?;
    let same: usize = db
        .prepare("SELECT payload_json FROM runtime_events WHERE task_id=? AND type=? AND id>?")?
        .query_map((task, OUTCOME_REFUSED, window), |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter_map(|raw| serde_json::from_str::<Value>(&raw).ok())
        .filter(|p| p["missing"] == json!(missing))
        .count();
    event_tx(db, task, OUTCOME_REFUSED, json!({"missing":missing,"count":same + 1}))?;
    if same + 1 < OUTCOME_REFUSAL_LIMIT {
        return Ok(None);
    }
    let kept = keep_dropped(&previous, &plan);
    let relief_key = format!("outcomes-kept:{}", &crate::digest(key.as_bytes())[..32]);
    // A plan the relief still cannot accept (for another reason) keeps its refusal.
    // Its partial writes are undone, while the recorded refusal stays.
    db.execute_batch("SAVEPOINT outcome_relief")?;
    let accepted = match accept_routed_tx(db, task, actor, &relief_key, &kept, &json!([])) {
        Ok(v) => {
            db.execute_batch("RELEASE outcome_relief")?;
            v
        }
        Err(e) => {
            db.execute_batch("ROLLBACK TO outcome_relief; RELEASE outcome_relief")?;
            return match e {
                CoreError::Refused(_) => Ok(None),
                e => Err(e),
            };
        }
    };
    // Outcomes the plan left out entirely are marked as deferred by the runtime, apart
    // from those the coordinator deferred itself with a reason. Still owed.
    let appended: Vec<&String> = missing
        .iter()
        .filter(|id| !plan["outcomes"].as_array().into_iter().flatten().any(|o| o["id"] == id.as_str()))
        .collect();
    if !appended.is_empty() {
        if let Some(mut stored) = plan_in_force(db, task)? {
            for o in stored["outcomes"].as_array_mut().into_iter().flatten() {
                if appended.iter().any(|id| o["id"] == id.as_str()) {
                    o["deferred_by"] = json!(RUNTIME_DEFERRED);
                }
            }
            db.execute("UPDATE runtime_tasks SET plan_json=? WHERE task_id=?", (stored.to_string(), task))?;
        }
    }
    event_tx(db, task, "LOOP_BROKEN", json!({
        "key":super::loop_guard::key("plan_outcome_refusal", "", &missing.join("|")),
        "kind":"plan_outcome_refusal","code":"plan_outcome_dropped","count":OUTCOME_REFUSAL_LIMIT,
        "action":"outcomes_kept","kept":missing}))?;
    let mut reply = accepted;
    reply["outcomes_kept_by_runtime"] = json!(missing);
    Ok(Some(reply))
}
/// Recorded when the runtime appended required verification commands a plan left out.
pub const VERIFICATION_KEPT: &str = "PLAN_VERIFICATION_KEPT";
/// Why a verification command is owed by the next plan (`required_verification`).
pub const OWED_GOAL_NOT_MET: &str = "goal_not_met";
pub const OWED_FAILED_CHECK: &str = "failed_check";
/// The verification commands the next plan owes, each argv verbatim with the reason:
/// those of an approved stage that missed the goal before verification ran
/// (`goal_not_met`) and the unresolved failed checks (`failed_check`). They are
/// runtime-owned obligations: a coordinator cannot waive them by leaving them out.
pub fn required_verification(db: &rusqlite::Connection, task: &str) -> CoreResult<Vec<(Value, &'static str)>> {
    let mut out: Vec<(Value, &'static str)> = Vec::new();
    let sets = [
        (super::review::owed_goal_commands(db, task)?, OWED_GOAL_NOT_MET),
        (super::automatic::owed_failed_checks(db, task)?, OWED_FAILED_CHECK),
        (super::completion::unaccepted_commands(db, task)?, OWED_FAILED_CHECK),
    ];
    for (commands, why) in sets {
        for argv in commands {
            if !out.iter().any(|(a, _)| *a == argv) {
                out.push((argv, why));
            }
        }
    }
    Ok(out)
}
/// The required verification commands as the coordinator reads them in
/// `plan_summary.required_verification` (argv arrays and reasons).
pub fn required_verification_view(db: &rusqlite::Connection, task: &str) -> CoreResult<Vec<Value>> {
    Ok(required_verification(db, task)?
        .into_iter()
        .map(|(argv, why)| json!({"argv":argv,"reason":why}))
        .collect())
}
/// `plan` with every required verification command it leaves out appended itself
/// (never refused: the commands are runtime-owned). Returns the appended commands.
fn keep_required_verification(
    db: &Transaction<'_>,
    task: &str,
    plan: &mut Value,
) -> CoreResult<Vec<Value>> {
    let mut kept = Vec::new();
    let list = plan["verification"].as_array().cloned().unwrap_or_default();
    let mut list = list;
    for (argv, why) in required_verification(db, task)? {
        if !list.contains(&argv) {
            list.push(argv.clone());
            kept.push(json!({"argv":argv,"reason":why}));
        }
    }
    if !kept.is_empty() {
        plan["verification"] = json!(list);
    }
    Ok(kept)
}
/// The plan in force of `task`, if any.
pub(crate) fn plan_in_force(db: &rusqlite::Connection, task: &str) -> CoreResult<Option<Value>> {
    let raw: Option<String> = db
        .query_row(
            "SELECT plan_json FROM runtime_tasks WHERE task_id=?",
            [task],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    Ok(raw.and_then(|raw| serde_json::from_str::<Value>(&raw).ok()))
}

pub(crate) fn plan_competency_decisions(plan: &Value) -> Vec<Value> {
    plan["tasks"].as_array().into_iter().flatten().filter_map(|node| {
        let mut decision = competency_decision(node)?;
        decision["node"] = node["id"].clone();
        decision["role"] = node["role"].clone();
        if let Some(objective) = node["objective"].as_str() {
            decision["objective_excerpt"] = json!(objective.chars().take(500).collect::<String>());
        }
        Some(decision)
    }).collect()
}
/// The plan the coordinator submitted with every id-only outcome reference of the
/// plan in force filled with its recorded statement.
pub fn carried_plan(db: &rusqlite::Connection, task: &str, plan: &Value) -> CoreResult<Value> {
    carry_outcomes(plan_in_force(db, task)?.as_ref(), plan)
}
pub fn validate(plan: &Value) -> CoreResult<Value> {
    let obj = plan
        .as_object()
        .ok_or(CoreError::Refused("invalid_plan_fields"))?;
    if !obj.contains_key("tasks")
        || !obj.contains_key("verification")
        || obj
            .keys()
            .any(|k| !["tasks", "verification", "outcomes"].contains(&k.as_str()))
    {
        return refuse("invalid_plan_fields");
    }
    // A command that checks nothing (`true`, `echo ...`, `sh -c "exit 0"`) can never
    // be the verification that lets work complete.
    if verification(&plan["verification"])?
        .iter()
        .any(|argv| super::completion::trivial_command(argv))
    {
        return refuse("verification_trivial_command");
    }
    let tasks = plan["tasks"]
        .as_array()
        .filter(|a| !a.is_empty() && a.len() <= MAX_NODES)
        .ok_or(CoreError::Refused("invalid_plan_size"))?;
    let mut normalized = plan.clone();
    let mut graph = Vec::new();
    for (index, n) in tasks.iter().enumerate() {
        let fields = n
            .as_object()
            .ok_or(CoreError::Refused("invalid_plan_node"))?;
        if fields.keys().any(|k| {
            ![
                "id",
                "role",
                "objective",
                "dependencies",
                "scope",
                "complexity",
                "provider",
                "model",
                "model_reason",
                "work_kind",
                "read_only",
                "delivers",
                "competencies",
                "base_sufficiency_reason",
                "required_reports",
            ]
            .contains(&k.as_str())
        }) {
            return refuse("invalid_plan_node");
        }
        if n.get("read_only").is_some_and(|v| !v.is_boolean()) {
            return refuse("invalid_plan_node");
        }
        if let Some(model) = n.get("model") {
            let model = model
                .as_str()
                .ok_or(CoreError::Refused("invalid_plan_node"))?;
            if model != "auto"
                && n["model_reason"]
                    .as_str()
                    .is_none_or(|s| s.is_empty() || s.chars().count() > 300)
            {
                return refuse("model_reason_required");
            }
        } else if n.get("model_reason").is_some() {
            return refuse("invalid_plan_node");
        }
        let id = n["id"]
            .as_str()
            .ok_or(CoreError::Refused("duplicate_or_invalid_node"))?;
        if id.len() > 64
            || !id.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
            || !id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        {
            return refuse("duplicate_or_invalid_node");
        }
        let role = role(
            n["role"]
                .as_str()
                .ok_or(CoreError::Refused("invalid_plan_node"))?,
        )?;
        if n["objective"]
            .as_str()
            .is_none_or(|s| s.is_empty() || s.chars().count() > 8000)
        {
            return refuse("invalid_node_objective");
        }
        let scopes = n["scope"]
            .as_array()
            .filter(|a| !a.is_empty() && a.len() <= 64)
            .ok_or(CoreError::Refused("invalid_assignment_scope"))?;
        let mut canonical = Vec::new();
        for s in scopes {
            let s = s
                .as_str()
                .filter(|s| !s.is_empty() && s.chars().count() <= 512)
                .ok_or(CoreError::Refused("invalid_assignment_scope"))?;
            // Cortex private state and Git control paths may be named: a scope grants no
            // write there, each write is decided by the runtime (owner decision 2026-11-25).
            if s.starts_with('/') || s.contains(['\\', '\0']) || s.split('/').any(|p| p == "..")
            {
                return refuse("forbidden_assignment_scope");
            }
            let p = policy::canonical(s);
            if !canonical.contains(&p) {
                canonical.push(p);
            }
        }
        let deps = n["dependencies"]
            .as_array()
            .ok_or(CoreError::Refused("invalid_dependencies"))?
            .iter()
            .map(|d| {
                d.as_str()
                    .map(str::to_owned)
                    .ok_or(CoreError::Refused("invalid_dependencies"))
            })
            .collect::<CoreResult<Vec<_>>>()?;
        if deps.iter().collect::<BTreeSet<_>>().len() != deps.len() {
            return refuse("invalid_dependencies");
        }
        crate::required_reports::validate(n)?;
        normalized["tasks"][index]["scope"] = json!(canonical);
        normalized["tasks"][index]["role"] = json!(role);
        if let Some(items) = n.get("competencies") {
            let ids = validated_competencies(items)?;
            normalized["tasks"][index]["competencies"] = json!(ids);
        }
        if let Some(reason) = n.get("base_sufficiency_reason") {
            if n.get("competencies").is_none() || reason.as_str().is_none_or(|text|
                text.trim().is_empty() || text.chars().count() > BASE_SUFFICIENCY_REASON_MAX)
            {
                return refuse("invalid_plan_base_sufficiency_reason");
            }
        }
        graph.push(scheduler::Node {
            id: id.into(),
            stage: "execution".into(),
            status: "pending".into(),
            dependencies: deps,
        });
    }
    scheduler::dependency_ready(&graph, "execution")?;
    validate_outcomes(plan)?;
    Ok(normalized)
}
pub fn accept_tx(
    db: &Transaction<'_>,
    task: &str,
    actor: &str,
    key: &str,
    plan: &Value,
) -> CoreResult<Value> {
    accept_routed_tx(db, task, actor, key, plan, &json!([]))
}
fn plan_nodes(plan: &Value) -> CoreResult<&[Value]> {
    plan["tasks"]
        .as_array()
        .map(Vec::as_slice)
        .ok_or(CoreError::Refused("invalid_plan_fields"))
}

fn runtime_node_ids(nodes: &[Value]) -> BTreeSet<&str> {
    nodes
        .iter()
        .filter_map(|node| {
            let id = node["id"].as_str()?;
            let role = node["role"].as_str().map(unprefixed_role);
            (id.starts_with('_') || role.is_some_and(|r| RESERVED_ROLES.contains(&r))).then_some(id)
        })
        .collect()
}

/// The editable view of the active plan. The digest still covers the complete stored plan.
pub fn steerable_plan(stored: &Value) -> CoreResult<Value> {
    let prior = plan_nodes(stored)?;
    let runtime_ids = runtime_node_ids(prior);
    let mut tasks = Vec::new();
    for node in prior {
        if node["id"].as_str().is_some_and(|id| runtime_ids.contains(id)) {
            continue;
        }
        let mut node = node.clone();
        if let Some(role) = node["role"].as_str().map(unprefixed_role) {
            node["role"] = json!(role);
        }
        node["dependencies"]
            .as_array_mut()
            .ok_or(CoreError::Refused("invalid_dependencies"))?
            .retain(|dep| !dep.as_str().is_some_and(|id| runtime_ids.contains(id)));
        tasks.push(node);
    }
    let mut editable = json!({"tasks":tasks,"verification":stored["verification"]});
    if let Some(outcomes) = stored_outcomes(stored) {
        editable["outcomes"] = json!(outcomes);
    }
    Ok(editable)
}

/// Overlay runtime-owned nodes and edges from the digest-bound plan, never from
/// the proposal. In a consultation revision even newly added work must wait on
/// the consultation. Compare and persist only the fully restored graph.
fn restore_runtime_nodes(prior: &[Value], supplied: &mut Vec<Value>) -> CoreResult<()> {
    let runtime_ids = runtime_node_ids(prior);
    if runtime_ids.is_empty() {
        return Ok(());
    }
    for node in supplied.iter_mut() {
        let previous = prior.iter().find(|p| p["id"] == node["id"]);
        let deps = node["dependencies"].as_array_mut().unwrap(); // Validated proposal.
        if let Some(previous) = previous {
            let old_deps = previous["dependencies"]
                .as_array()
                .ok_or(CoreError::Refused("invalid_dependencies"))?;
            for (i, dep) in old_deps.iter().enumerate() {
                if dep.as_str().is_some_and(|id| runtime_ids.contains(id)) {
                    deps.insert(i.min(deps.len()), dep.clone());
                }
            }
        } else if runtime_ids.contains(CONSULTATION) {
            deps.insert(0, json!(CONSULTATION));
        }
    }
    for (i, node) in prior.iter().enumerate() {
        if node["id"].as_str().is_some_and(|id| runtime_ids.contains(id)) {
            supplied.insert(i.min(supplied.len()), node.clone());
        }
    }
    Ok(())
}

/// Nodes that cannot be amended even if their current status has returned to
/// pending. Retry history and the original claim both belong to that node id.
pub fn locked_execution_nodes(
    db: &rusqlite::Connection,
    task: &str,
    revision: i64,
) -> CoreResult<BTreeSet<String>> {
    let mut statement = db.prepare(
        "SELECT n.node_id FROM runtime_nodes n WHERE n.task_id=? AND n.revision=? AND (
            n.stage!='execution' OR n.status!='pending' OR n.assignment_ref IS NOT NULL
            OR n.result_json IS NOT NULL OR EXISTS(
                SELECT 1 FROM runtime_events e WHERE e.task_id=n.task_id AND e.type='ASSIGNMENT_CREATED'
                AND json_extract(e.payload_json,'$.node')=n.node_id
                AND json_extract(e.payload_json,'$.revision')=n.revision))",
    )?;
    let rows = statement.query_map((task, revision), |r| r.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Validate the actual scheduler graph, including runtime-created fixes and
/// review nodes absent from the editable plan.
fn validate_amended_graph(
    db: &Transaction<'_>,
    task: &str,
    revision: i64,
    prior: &[Value],
    supplied: &[Value],
) -> CoreResult<()> {
    let mut graph: Vec<_> = scheduler::revision_nodes(db, task, revision)?
        .into_iter()
        .map(|(node, _)| node)
        .collect();
    if prior.iter().any(|p| !graph.iter().any(|n| p["id"] == n.id)) {
        return refuse("active_plan_graph_mismatch");
    }
    graph.retain(|n| !prior.iter().any(|p| p["id"] == n.id && !supplied.contains(p)));
    for n in supplied.iter().filter(|n| !prior.contains(n)) {
        graph.push(scheduler::Node {
            id: n["id"].as_str().unwrap().into(),
            stage: "execution".into(),
            status: "pending".into(),
            dependencies: serde_json::from_value(n["dependencies"].clone())?,
        });
    }
    scheduler::dependency_ready(&graph, "execution")?;
    Ok(())
}

/// Replace only work that has never been claimed, on the same execution revision.
/// Keeping both the node rows and active custody unchanged preserves channel and
/// result authority. The current digest and claim status are checked in the writer transaction.
pub fn amend_active_tx(
    db: &Transaction<'_>,
    task: &str,
    args: &Value,
) -> CoreResult<Value> {
    let (state, revision) = domain::task_tx(db, task)?;
    if state != TaskState::Executing || super::automatic::held(db, task)? {
        return refuse("active_plan_not_expected");
    }
    let raw: String = db.query_row("SELECT plan_json FROM runtime_tasks WHERE task_id=?", [task], |r| r.get(0))?;
    if args["expected_plan"] != crate::digest(raw.as_bytes()) {
        return refuse("active_plan_changed");
    }
    let mut old: Value = serde_json::from_str(&raw)?;
    let carried = carry_outcomes(Some(&old), &args["plan"])?;
    let mut next = super::sprint_planning::validate_plan(db, task, &carried, validate)?;
    crate::required_reports::validate_existing(db, task, &next)?;
    if old["verification"] != next["verification"] {
        return refuse("active_plan_verification_locked");
    }
    require_kept_outcomes(&old, &next)?;
    super::sprint_planning::require_read_only_plan(db, task, &next)?;
    let prior = plan_nodes(&old)?;
    let supplied = next["tasks"].as_array_mut().unwrap();
    restore_runtime_nodes(prior, supplied)?;
    let locked = locked_execution_nodes(db, task, revision)?;
    let cancelled: Vec<_> = prior.iter()
        .filter(|n| !supplied.iter().any(|s| s["id"] == n["id"]))
        .filter_map(|n| n["id"].as_str())
        .collect();
    let added: Vec<_> = supplied.iter()
        .filter(|n| !prior.iter().any(|p| p["id"] == n["id"]))
        .filter_map(|n| n["id"].as_str())
        .collect();
    let changed: Vec<_> = prior.iter()
        .filter(|n| supplied.iter().any(|s| s["id"] == n["id"] && s != *n))
        .filter_map(|n| n["id"].as_str())
        .collect();
    for id in cancelled.iter().chain(&changed) {
        if locked.contains(*id) {
            return refuse("active_plan_node_already_claimed");
        }
    }
    validate_amended_graph(db, task, revision, prior, supplied)?;
    let changes = json!({"added":added,"changed":changed,"cancelled":cancelled});
    for id in cancelled {
        db.execute("DELETE FROM runtime_nodes WHERE task_id=? AND revision=? AND node_id=?", (task, revision, id))?;
    }
    let denials = super::steering::denials(db, task)?;
    for node in supplied.iter() {
        if prior.contains(node) { continue; }
        let id = node["id"].as_str().unwrap();
        let role = node["role"].as_str().unwrap();
        // An amendment cannot select or change a provider/model outside ordinary
        // plan intake. Unchanged route choices on an edited pending node are retained.
        let previous = prior.iter().find(|p| p["id"] == id);
        if !PLANNABLE_ROLES.contains(&unprefixed_role(role))
            && previous.is_none_or(|p| p["role"] != node["role"])
        {
            return refuse("plan_requires_base_role");
        }
        validate_competency_decision(node)?;
        let allowed_routing = previous.map(routing_fields).unwrap_or_else(|| json!({"model":"auto"}));
        if routing_fields(node) != allowed_routing {
            return refuse("active_plan_routing_locked");
        }
        if !policy::plan_node_read_only(node) && node["scope"].as_array().unwrap().iter().any(|s| {
            denials.iter().any(|p| policy::overlaps(s.as_str().unwrap(), p))
        }) {
            return refuse("write_denied");
        }
        if previous.is_some() {
            db.execute(
                "UPDATE runtime_nodes SET role=?,objective=?,dependencies_json=?,scope_json=?
                 WHERE task_id=? AND revision=? AND node_id=?",
                (role, node["objective"].as_str(), node["dependencies"].to_string(),
                 node["scope"].to_string(), task, revision, id),
            )?;
        } else {
            db.execute("INSERT INTO runtime_nodes VALUES (?,?,?,?,?,?,?,'execution','pending',NULL,NULL)",
                (task, revision, id, role, node["objective"].as_str(),
                 node["dependencies"].to_string(), node["scope"].to_string()))?;
        }
    }
    old["tasks"] = next["tasks"].take();
    old["outcomes"] = next["outcomes"].take();
    let updated = old.to_string();
    let digest = crate::digest(updated.as_bytes());
    if updated != raw {
        db.execute("UPDATE runtime_tasks SET plan_json=? WHERE task_id=?", (&updated, task))?;
        let strategy = crate::strategy::select_plan(db, task, &old)?;
        event_tx(db, task, "PLAN_AMENDED", json!({"revision":revision,"plan":old,
            "source_request":args["source_request"],"plan_digest":digest,"nodes":changes,"strategy":strategy}))?;
    }
    let warnings = policy::plan_scope_warnings(db, task, &old)?;
    Ok(json!({"accepted":true,"revision":revision,"plan_digest":digest,"changed":updated!=raw,"nodes":changes,"scope_warnings":warnings}))
}

fn normalized_consultation_text(value: &Value) -> String {
    value
        .as_str()
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn consultation_subject(db: &Connection, task: &str, proposal: &Value) -> CoreResult<Value> {
    let mut reports = std::collections::BTreeMap::new();
    for id in proposal["report_ids"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        let digest = db.query_row(
            "SELECT e.digest FROM reports r JOIN editions e ON e.report_id=r.id
             WHERE r.task_id=? AND r.id=? AND r.kind='report' ORDER BY e.sequence DESC LIMIT 1",
            (task, id), |row| row.get::<_, String>(0),
        ).optional()?;
        if let Some(digest) = digest {
            reports.insert(id, digest);
        }
    }
    Ok(json!({"question":normalized_consultation_text(&proposal["question"]),
        "reason":normalized_consultation_text(&proposal["reason"]),"reports":reports}))
}

/// A replay of the original request key is answered before this validation.
/// A new key needs a changed question, basis, retained report, or owner input.
pub(super) fn validate_consultation_request(
    db: &Connection,
    task: &str,
    revision: i64,
    proposal: &Value,
) -> CoreResult<Value> {
    let subject = consultation_subject(db, task, proposal)?;
    if subject["question"] == "" || subject["reason"] == "" {
        return refuse("consultation_question_and_reason_required");
    }
    let input: i64 = db.query_row(
        "SELECT coalesce(max(id),0) FROM runtime_events WHERE task_id=? AND type IN
          ('USER_STEERING','USER_ANSWER_RECEIVED','WORKER_USER_ANSWER_RECEIVED','TASK_REOPENED')",
        [task], |row| row.get(0),
    )?;
    let source: i64 = db.query_row(
        "SELECT coalesce(max(revision),0) FROM source_revisions WHERE task_id=?",
        [task], |row| row.get(0),
    )?;
    let identity = json!({"subject":subject,"owner_input":input,"source_revision":source});
    let mut stmt = db.prepare(
        "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='REPLAN_REQUEST_ACCEPTED'
         AND id>? AND json_extract(payload_json,'$.operation')='escalation_request'
         AND json_extract(payload_json,'$.requested_operation') IS NULL ORDER BY id DESC",
    )?;
    let rows = stmt.query_map((task, input), |row| row.get::<_, String>(0))?;
    for raw in rows {
        let previous: Value = serde_json::from_str(&raw?)?;
        let old = &previous["consultation_identity"];
        if old == &identity || (old.is_null() && consultation_subject(db, task, &previous["proposal"])? == subject) {
            return refuse("consultation_request_already_recorded");
        }
    }
    if explicit_consultation(db, task, revision)?.is_some() {
        return refuse("consultation_request_pending");
    }
    Ok(identity)
}

pub(super) fn consultation_consumed(db: &Connection, task: &str, id: i64, revision: i64) -> CoreResult<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='CONSULTATION_REQUIRED'
         AND id>? AND (? IN (json_extract(payload_json,'$.request_event'),
                             json_extract(payload_json,'$.recovery_request_event')) OR
           (json_extract(payload_json,'$.request_event') IS NULL
            AND json_extract(payload_json,'$.source_revision')=?)))",
        (task, id, id, revision), |row| row.get(0),
    )?)
}

fn explicit_consultation(db: &Connection, task: &str, revision: i64) -> CoreResult<Option<(i64, Value)>> {
    let request = db.query_row(
        "SELECT id,payload_json FROM runtime_events WHERE task_id=? AND type='REPLAN_REQUEST_ACCEPTED'
         AND json_extract(payload_json,'$.revision')=?
         AND json_extract(payload_json,'$.operation')='escalation_request'
         AND json_extract(payload_json,'$.requested_operation') IS NULL
         ORDER BY id DESC LIMIT 1",
        (task, revision), |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
    ).optional()?;
    let Some((id, raw)) = request else { return Ok(None); };
    let event: Value = serde_json::from_str(&raw)?;
    if consultation_consumed(db, task, id, revision)? {
        return Ok(None);
    }
    Ok(Some((id, event)))
}

/// A previous completed consultation with an exact, published result and digest.
pub(crate) fn previous_consultation(db: &Connection, task: &str, through_revision: i64) -> CoreResult<Option<Value>> {
    Ok(db.query_row(
        "SELECT r.id,e.digest,e.sequence,n.revision
         FROM runtime_nodes n
         JOIN delegation_provisioning p ON p.task_id=n.task_id AND p.idempotency_key=n.assignment_ref
         JOIN reports r ON r.task_id=n.task_id AND r.id=json_extract(n.result_json,'$.report_ref') AND r.kind='report'
         JOIN editions e ON e.report_id=r.id AND e.digest=json_extract(n.result_json,'$.report_digest')
         WHERE n.task_id=? AND n.revision<=? AND n.node_id='_consultation'
         AND n.role='worker-senior-consultant' AND n.stage='execution' AND n.status='completed'
         AND EXISTS(SELECT 1 FROM task_events t WHERE t.task_id=n.task_id
           AND t.event_type='worker_result' AND t.assignment_ref IN (n.assignment_ref,p.attempt_id)
           AND json_extract(t.payload_json,'$.report_ref')=r.id)
         AND EXISTS(SELECT 1 FROM runtime_events w WHERE w.task_id=n.task_id AND w.type='WORKER_COMPLETED'
           AND json_extract(w.payload_json,'$.assignment_ref')=n.assignment_ref
           AND json_extract(w.payload_json,'$.attempt_id')=p.attempt_id
           AND json_extract(w.payload_json,'$.result.report_ref')=r.id
           AND json_extract(w.payload_json,'$.result.report_digest')=e.digest)
         ORDER BY n.revision DESC,e.sequence DESC LIMIT 1",
        (task, through_revision), |row| Ok(json!({"report_id":row.get::<_, String>(0)?,
            "node":CONSULTATION,"role":"senior-consultant","sha256":row.get::<_, String>(1)?,
            "artifact_revision":row.get::<_, i64>(2)?,"plan_revision":row.get::<_, i64>(3)?,"availability":"recorded"})),
    ).optional()?)
}

/// Runtime-derived previews are retained with the exact plan receipt, never recomputed on replay.
pub fn accept_routed_tx(
    db: &Transaction<'_>,
    task: &str,
    actor: &str,
    key: &str,
    plan: &Value,
    model_hints: &Value,
) -> CoreResult<Value> {
    // A read-only sprint planning task's study names no commands: the runtime
    // verifies its deliverable, the submitted plan (see `sprint_planning`).
    // An id-only outcome entry keeps that outcome of the plan in force with its recorded
    // statement; the receipt still binds the plan exactly as submitted.
    let carried = carried_plan(db, task, plan)?;
    let canonical = super::sprint_planning::validate_plan(db, task, &carried, validate)?;
    request_tx(db, task, actor, key, "plan_submit", plan, || {
        let (state, revision) = domain::task_tx(db, task)?;
        if state != TaskState::Planning {
            return refuse("plan_not_expected");
        }
        for node in canonical["tasks"].as_array().unwrap() {
            validate_competency_decision(node)?;
        }
        let live:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM capacity_reservations WHERE task_id=? AND kind='worker' AND state!='released')",[task],|r|r.get(0))?;
        if live {
            return refuse("workers_still_live_or_unknown");
        }
        crate::required_reports::validate_existing(db, task, &canonical)?;
        // A read-only sprint planning task dispatches only read-only specialists.
        super::sprint_planning::require_read_only_plan(db, task, &canonical)?;
        // The owner's write denials in force (lifted ones excluded).
        let denied = super::steering::denials(db, task)?;
        for n in canonical["tasks"].as_array().unwrap() {
            if policy::plan_node_read_only(n) {
                continue;
            }
            if n["scope"].as_array().unwrap().iter().any(|s| {
                denied
                    .iter()
                    .any(|p| policy::overlaps(s.as_str().unwrap(), p))
            }) {
                return refuse("write_denied");
            }
        }
        // An open owner decision is asked before the blocked work is planned again.
        super::obstacles::require_owner_question_tx(db, task)?;
        // Required verification commands (failed checks, an approved stage that missed the
        // goal) are runtime-owned: a plan that leaves one out gets it appended, never refused.
        let mut canonical = canonical.clone();
        let kept_verification = keep_required_verification(db, task, &mut canonical)?;
        // The goal outcomes of the plan in force survive the replan, and a plan whose
        // approved work missed the goal keeps that plan's verification commands.
        let previous: Option<String> = db.query_row(
            "SELECT plan_json FROM runtime_tasks WHERE task_id=?",
            [task],
            |r| r.get(0),
        )?;
        if let Some(previous) = previous.and_then(|p| serde_json::from_str::<Value>(&p).ok()) {
            require_kept_outcomes(&previous, &canonical)?;
        }
        let revision = revision + 1;
        let mut proposed = canonical["tasks"].as_array().unwrap().clone();
        let explicit = explicit_consultation(db, task, revision - 1)?;
        let automatic = super::automatic::consultation(db, task, revision - 1)?;
        let (escalation, additional_recovery) = match (explicit, automatic) {
            (Some(explicit), recovery) => (Some(explicit), recovery),
            (None, recovery) => (recovery, None),
        };
        if let Some((request_event, escalation)) = escalation.as_ref() {
                let proposal = &escalation["proposal"];
                let mut objective = format!(
                    "Answer this independent consultation question: {}\nReason: {}",
                    proposal["question"]
                        .as_str()
                        .or_else(|| proposal["reason"].as_str())
                        .unwrap_or(""),
                    proposal["reason"].as_str().unwrap_or("")
                );
                let mut reports = proposal["report_ids"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                if let Some((_, recovery)) = &additional_recovery {
                    objective.push_str("\nAutomatic recovery context: ");
                    objective.push_str(recovery["proposal"]["reason"].as_str().unwrap_or(""));
                    for report in recovery["proposal"]["report_ids"].as_array().into_iter().flatten() {
                        if !reports.contains(report) {
                            reports.push(report.clone());
                        }
                    }
                }
                let prior=db.prepare("SELECT json_extract(result_json,'$.report_ref') FROM runtime_nodes WHERE task_id=? AND revision=? AND result_json IS NOT NULL ORDER BY rowid")?
                    .query_map((task,revision-1),|r|r.get::<_,Option<String>>(0))?.collect::<Result<Vec<_>,_>>()?;
                for report in prior.into_iter().flatten() {
                    if !reports.contains(&json!(report)) {
                        reports.push(json!(report));
                    }
                }
                let previous = previous_consultation(db, task, revision - 1)?;
                let mut packet = previous.into_iter().collect::<Vec<_>>();
                for report in reports {
                    if packet.iter().any(|entry| entry["report_id"] == report) {
                        continue;
                    }
                    let metadata:Option<(String,i64)>=db.query_row("SELECT digest,sequence FROM editions e JOIN reports r ON r.id=e.report_id WHERE r.task_id=? AND r.id=? AND r.kind='report' ORDER BY sequence DESC LIMIT 1",
                        (task,report.as_str()),|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
                    packet.push(if let Some((sha,sequence))=metadata { json!({"report_id":report,"sha256":sha,"artifact_revision":sequence,"availability":"recorded"}) }
                        else { json!({"report_id":report,"availability":"unavailable"}) });
                }
                for n in &mut proposed {
                    n["dependencies"]
                        .as_array_mut()
                        .unwrap()
                        .insert(0, json!(CONSULTATION));
                }
                proposed.insert(0,json!({"id":CONSULTATION,"role":"worker-senior-consultant","objective":objective,"scope":["."],"dependencies":[]}));
                let mut receipt = json!({"role":"worker-senior-consultant","source_revision":revision-1,
                    "revision":revision,"node":CONSULTATION,"request_event":request_event,"dependency_reports":packet});
                if let Some((recovery_event, _)) = &additional_recovery {
                    receipt["recovery_request_event"] = json!(recovery_event);
                }
                event_tx(db, task, "CONSULTATION_REQUIRED", receipt)?;
        }
        // The stored plan and PLAN_ACCEPTED carry exactly the nodes and edges that
        // were inserted, including a runtime-added `_consultation` node.
        let mut stored = canonical.clone();
        stored["tasks"] = json!(proposed);
        // `review_cycle` restarts with the revision: it numbers this revision's review
        // and fix nodes and bounds its reviews, and every new revision needs its own
        // mandatory review. It is not the review budget: that belongs to the task
        // lineage across revisions and grants further fix rounds only while the
        // findings strictly shrink (`review::advance_tx`), so a continuation's new
        // plan never starts a fresh budget.
        db.execute("UPDATE pending_questions SET status='superseded',answered_at=?
            WHERE task_id=? AND dedup_key LIKE 'obstacle:%' AND status='open'", (crate::now(), task))?;
        db.execute("UPDATE runtime_tasks SET revision=?,plan_json=?,synthesis=NULL,review_cycle=0 WHERE task_id=?",
            (revision,stored.to_string(),task))?;
        for n in &proposed {
            db.execute(
                "INSERT INTO runtime_nodes VALUES (?,?,?,?,?,?,?,'execution','pending',NULL,NULL)",
                (
                    task,
                    revision,
                    n["id"].as_str(),
                    n["role"].as_str(),
                    n["objective"].as_str(),
                    n["dependencies"].to_string(),
                    n["scope"].to_string(),
                ),
            )?;
        }
        let mut strategy = crate::strategy::select_plan(db, task, &stored)?;
        if escalation.is_some() {
            strategy["consultation"] = json!("scheduled");
        }
        let warnings = policy::plan_scope_warnings(db, task, &stored)?;
        let mut accepted = json!({"revision":revision,"plan":stored,"strategy":strategy});
        if crate::deliverables::runtime_contract(db, task)?
            .is_some_and(|contract| contract["consumes"].as_array().is_some_and(|v| !v.is_empty()))
        {
            accepted["sprint_consumer_channel"] = json!(actor);
        }
        if !warnings.is_empty() {
            accepted["scope_warnings"] = json!(warnings);
        }
        event_tx(db, task, "PLAN_ACCEPTED", accepted)?;
        if !kept_verification.is_empty() {
            event_tx(db, task, VERIFICATION_KEPT, json!({"revision":revision,"kept":kept_verification}))?;
        }
        domain::state_tx(db, task, TaskState::PlanReady, None)?;
        let mut reply = json!({"revision":revision,"state":"PLAN_READY","model_hints":model_hints});
        if !warnings.is_empty() {
            reply["scope_warnings"] = json!(warnings);
        }
        if !kept_verification.is_empty() {
            reply["verification_kept_by_runtime"] = json!(kept_verification);
        }
        Ok(reply)
    })
}
