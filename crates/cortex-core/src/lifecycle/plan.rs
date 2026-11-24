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
pub fn validate(plan: &Value) -> CoreResult<Value> {
    let obj = plan
        .as_object()
        .ok_or(CoreError::Refused("invalid_plan_fields"))?;
    if obj.len() != 2 || !obj.contains_key("tasks") || !obj.contains_key("verification") {
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
            if s.starts_with('/')
                || s.contains(['\\', '\0'])
                || s.split('/').any(|p| {
                    p == ".."
                        || p == ".git"
                        || p == ".cortex"
                        || p == ".cortex-dev"
                        || p == ".cortex-dev-owner.json"
                        || p == ".cortex-workspace-dev.json"
                })
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
    Ok(json!({"tasks":tasks,"verification":stored["verification"]}))
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
    let mut next = super::sprint_planning::validate_plan(db, task, &args["plan"], validate)?;
    if old["verification"] != next["verification"] {
        return refuse("active_plan_verification_locked");
    }
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
    let canonical = super::sprint_planning::validate_plan(db, task, plan, validate)?;
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
        super::automatic::require_failed_checks(db, task, &canonical["verification"])?;
        super::completion::require_repair_commands(db, task, &canonical["verification"])?;
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
        domain::state_tx(db, task, TaskState::PlanReady, None)?;
        Ok(json!({"revision":revision,"state":"PLAN_READY","model_hints":model_hints}))
    })
}
