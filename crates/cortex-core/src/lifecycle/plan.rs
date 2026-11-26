//! Transactional plan intake. Schema validation happens at the authenticated gateway.
use super::{domain, event_tx, request_tx, MAX_NODES};
use crate::{
    error::refuse,
    scheduler::{self, policy},
    CoreError, CoreResult, TaskState,
};
use rusqlite::{OptionalExtension, Transaction};
use serde_json::{json, Value};
use std::collections::BTreeSet;
const CONSULTATION: &str = "_consultation";
const RESERVED_ROLES: [&str; 3] = ["code-reviewer", "build-verification", "senior-consultant"];

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
fn plan_in_force(db: &rusqlite::Connection, task: &str) -> CoreResult<Option<Value>> {
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
                "delivers",
            ]
            .contains(&k.as_str())
        }) {
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
        normalized["tasks"][index]["scope"] = json!(canonical);
        normalized["tasks"][index]["role"] = json!(role);
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
    let graph = supplied
        .iter()
        .map(|n| {
            Ok(scheduler::Node {
                id: n["id"]
                    .as_str()
                    .ok_or(CoreError::Refused("invalid_plan_node"))?
                    .into(),
                stage: "execution".into(),
                status: "pending".into(),
                dependencies: serde_json::from_value(n["dependencies"].clone())?,
            })
        })
        .collect::<CoreResult<Vec<_>>>()?;
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
    if old["verification"] != next["verification"] {
        return refuse("active_plan_verification_locked");
    }
    require_kept_outcomes(&old, &next)?;
    super::sprint_planning::require_read_only_plan(db, task, &next)?;
    let prior = plan_nodes(&old)?;
    let supplied = next["tasks"].as_array_mut().unwrap();
    restore_runtime_nodes(prior, supplied)?;
    let denials = super::steering::denials(db, task)?;
    for node in prior {
        let id = node["id"].as_str().unwrap_or_default();
        if supplied.iter().any(|n| n == node) { continue; }
        let unclaimed: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM runtime_nodes n WHERE task_id=?1 AND revision=?2 AND node_id=?3
                AND stage='execution' AND status='pending' AND assignment_ref IS NULL AND result_json IS NULL
                AND NOT EXISTS(SELECT 1 FROM runtime_events e WHERE e.task_id=n.task_id AND type='ASSIGNMENT_CREATED'
                  AND json_extract(e.payload_json,'$.node')=n.node_id AND json_extract(e.payload_json,'$.revision')=n.revision))",
            (task, revision, id), |r| r.get(0),
        )?;
        if !unclaimed { return refuse("active_plan_node_already_claimed"); }
        db.execute("DELETE FROM runtime_nodes WHERE task_id=? AND revision=? AND node_id=?", (task, revision, id))?;
    }
    for node in supplied.iter() {
        if prior.contains(node) { continue; }
        let id = node["id"].as_str().unwrap();
        let role = node["role"].as_str().unwrap();
        // An amendment cannot select or change a provider/model outside ordinary
        // plan intake. Unchanged route choices on an edited pending node are retained.
        let previous = prior.iter().find(|p| p["id"] == id);
        let allowed_routing = previous.map(routing_fields).unwrap_or_else(|| json!({"model":"auto"}));
        if routing_fields(node) != allowed_routing {
            return refuse("active_plan_routing_locked");
        }
        if !policy::read_only(role, "execution") && node["scope"].as_array().unwrap().iter().any(|s| {
            denials.iter().any(|p| policy::overlaps(s.as_str().unwrap(), p))
        }) {
            return refuse("write_denied");
        }
        db.execute("INSERT INTO runtime_nodes VALUES (?,?,?,?,?,?,?,'execution','pending',NULL,NULL)",
            (task, revision, id, role, node["objective"].as_str(),
             node["dependencies"].to_string(), node["scope"].to_string()))?;
    }
    old["tasks"] = next["tasks"].take();
    old["outcomes"] = next["outcomes"].take();
    let updated = old.to_string();
    let digest = crate::digest(updated.as_bytes());
    if updated != raw {
        db.execute("UPDATE runtime_tasks SET plan_json=? WHERE task_id=?", (&updated, task))?;
        event_tx(db, task, "PLAN_AMENDED", json!({"revision":revision,"plan":old,
            "source_request":args["source_request"],"plan_digest":digest}))?;
    }
    Ok(json!({"accepted":true,"revision":revision,"plan_digest":digest,"changed":updated!=raw}))
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
        let live:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM capacity_reservations WHERE task_id=? AND kind='worker' AND state!='released')",[task],|r|r.get(0))?;
        if live {
            return refuse("workers_still_live_or_unknown");
        }
        // A read-only sprint planning task dispatches only read-only specialists.
        super::sprint_planning::require_read_only_plan(db, task, &canonical)?;
        // The owner's write denials in force (lifted ones excluded).
        let denied = super::steering::denials(db, task)?;
        for n in canonical["tasks"].as_array().unwrap() {
            if policy::read_only(n["role"].as_str().unwrap(), "execution") {
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
        let automatic = super::automatic::consultation(db, task)?;
        let escalation:Option<String>=db.query_row("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='REPLAN_REQUEST_ACCEPTED' ORDER BY id DESC LIMIT 1",
            [task],|r|r.get(0)).optional()?;
        let escalation = automatic.map(|v| v.to_string()).or(escalation);
        if let Some(raw) = escalation {
            let escalation: Value = serde_json::from_str(&raw)?;
            if escalation["operation"] == "escalation_request"
                && (escalation["revision"] == revision - 1 || escalation["automatic"] == true)
            {
                let proposal = &escalation["proposal"];
                let objective = format!(
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
                let prior=db.prepare("SELECT json_extract(result_json,'$.report_ref') FROM runtime_nodes WHERE task_id=? AND revision=? AND result_json IS NOT NULL ORDER BY rowid")?
                    .query_map((task,revision-1),|r|r.get::<_,Option<String>>(0))?.collect::<Result<Vec<_>,_>>()?;
                for report in prior.into_iter().flatten() {
                    if !reports.contains(&json!(report)) {
                        reports.push(json!(report));
                    }
                }
                let mut packet = vec![];
                for report in reports {
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
                event_tx(
                    db,
                    task,
                    "CONSULTATION_REQUIRED",
                    json!({"role":"worker-senior-consultant","source_revision":revision-1,
                    "revision":revision,"node":CONSULTATION,"dependency_reports":packet}),
                )?;
            }
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
        event_tx(
            db,
            task,
            "PLAN_ACCEPTED",
            json!({"revision":revision,"plan":stored}),
        )?;
        if !kept_verification.is_empty() {
            event_tx(db, task, VERIFICATION_KEPT, json!({"revision":revision,"kept":kept_verification}))?;
        }
        domain::state_tx(db, task, TaskState::PlanReady, None)?;
        let mut reply = json!({"revision":revision,"state":"PLAN_READY","model_hints":model_hints});
        if !kept_verification.is_empty() {
            reply["verification_kept_by_runtime"] = json!(kept_verification);
        }
        Ok(reply)
    })
}
