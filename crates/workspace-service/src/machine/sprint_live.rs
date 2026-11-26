//! Owner live graph, separate from approved plans and the model-facing read.
use super::{context, native::*, settings, sprint_dag, sprint_materialize, task_rework};
use cortex_core::{CoreError, CoreResult, OpenMode, Store};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

const MAX_MEMBERS: usize = 1000;
const MAX_EDGES: usize = 10_000;
pub(crate) const STALE: &str = "stale_sprint_selection";

pub(crate) fn handles(p: &Value) -> bool {
    matches!(
        p["action"].as_str(),
        Some("live_graph" | "set_dependency" | "move_priority" | "apply_revision" | "reject_revision")
    )
}

pub(super) fn identity(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && !id.chars().any(char::is_control)
}

fn id_param<'a>(p: &'a Value, key: &str) -> Result<&'a str> {
    let id = string(p, key)?;
    if !identity(id) {
        return Err(refuse("invalid_request", "Invalid sprint or task ID."));
    }
    Ok(id)
}

pub(super) fn sprint_index(product: &Value, project: &str, id: &str) -> Result<usize> {
    let rows = product["sprints"].as_array().ok_or_else(invalid_members)?;
    let mut matches = rows.iter().enumerate().filter(|(_, s)| s["id"] == id);
    let (index, sprint) = matches
        .next()
        .ok_or_else(|| refuse("unknown_sprint", "Unknown sprint."))?;
    if matches.next().is_some()
        || (!sprint["projectId"].is_null() && sprint["projectId"] != project)
    {
        return Err(invalid_members());
    }
    Ok(index)
}

pub(super) fn invalid_members() -> Refused {
    refuse(
        "sprint_membership_invalid",
        "Saved sprint membership is ambiguous or unreadable.",
    )
}

fn over_budget() -> Refused {
    refuse(
        "sprint_graph_too_large",
        "This sprint exceeds the live graph read limit.",
    )
}

pub(super) fn open_sprint(sprint: &Value) -> bool {
    sprint["archivedAt"].is_null() && (sprint["deleting"].is_null() || sprint["deleting"] == false)
}

pub(super) fn editable_sprint(sprint: &Value) -> bool {
    open_sprint(sprint)
        && sprint["materializedVersion"]
            .as_u64()
            .is_some_and(|v| v > 0)
}

fn deps(record: &Value) -> Option<Vec<String>> {
    match record.get("dependencies") {
        None => Some(vec![]), // legacy bound overlays with no explicit edge array
        Some(Value::Array(values)) => values
            .iter()
            .map(|v| v.as_str().filter(|s| identity(s)).map(str::to_owned))
            .collect(),
        _ => None,
    }
}

pub(super) struct Member {
    pub(super) id: String,
    pub(super) status: &'static str,
    pub(super) dependencies: Option<Vec<String>>,
    pub(super) draft: bool,
    claim_order: Option<i64>,
    checkout: Option<String>,
}

pub(crate) struct Snapshot {
    pub sprint: Value,
    pub records: Value,
    pub tasks: HashMap<String, Value>,
    pub roster: Vec<String>,
    pub priority: Vec<String>,
    pub(super) members: Vec<Member>,
    pub(super) graph_revision: String,
    /// Revisions proposed by this sprint's tasks (`sprint_impact`), oldest first.
    pub(super) revisions: Vec<Value>,
    pub queue_revision: String,
}

#[derive(Clone)]
pub(crate) struct Expectation {
    pub sprint_id: String,
    pub revision: String,
}

impl Snapshot {
    pub(crate) fn expectation(&self) -> Expectation {
        Expectation {
            sprint_id: self.sprint["id"].as_str().unwrap().to_owned(),
            revision: self.queue_revision.clone(),
        }
    }
}

fn hash(value: &Value) -> String {
    format!("{:x}", Sha256::digest(value.to_string()))
}

pub(super) fn load(
    store: &Store,
    product: &Value,
    project: &str,
    index: usize,
    owner_budget: bool,
) -> Result<Snapshot> {
    let saved = &product["sprints"][index];
    let id = saved["id"].as_str().ok_or_else(invalid_members)?;
    let roster = saved["taskIds"].as_array().ok_or_else(invalid_members)?;
    if roster.len() > if owner_budget { MAX_MEMBERS } else { 10_000 } {
        return Err(over_budget());
    }
    let roster: Vec<String> = roster
        .iter()
        .map(|id| {
            id.as_str()
                .filter(|s| identity(s))
                .map(str::to_owned)
                .ok_or_else(invalid_members)
        })
        .collect::<Result<_>>()?;
    let unique: HashSet<_> = roster.iter().map(String::as_str).collect();
    if unique.len() != roster.len() {
        return Err(invalid_members());
    }

    let foreign: HashSet<_> = product["sprints"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != index)
        .flat_map(|(_, s)| s["taskIds"].as_array().into_iter().flatten())
        .filter_map(Value::as_str)
        .filter(|task| unique.contains(task))
        .collect();
    let mut seen = HashSet::new();
    let priority: Vec<String> = saved["taskPriority"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .chain(roster.iter().map(String::as_str))
        .filter(|task| unique.contains(task) && seen.insert(*task))
        .map(str::to_owned)
        .collect();
    if owner_budget
        && roster.iter().fold(0usize, |count, task| {
            count.saturating_add(
                product["tasks"][task]["dependencies"]
                    .as_array()
                    .map_or(0, Vec::len),
            )
        }) > MAX_EDGES
    {
        return Err(over_budget());
    }

    let root = store.key().root().to_string_lossy();
    let actor = crate::actor();
    let mut projected = json!({"tasks":{}, "sprints":[saved]});
    if product.get("sprintRunModeVersion").is_none() && saved["runMode"] == "automatic" {
        projected["sprints"][0]["runMode"] = json!("hybrid");
    }
    let mut members = Vec::with_capacity(roster.len());
    let mut tasks = HashMap::new();
    let mut graph_facts = Vec::new();
    let mut queue_facts = Vec::new();
    store.read(|db| {
        let authorized: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM workspace_projects
             WHERE project_id=? AND project_root=? AND activated_by=?)",
            (project, &*root, &actor), |r| r.get(0),
        )?;
        if !authorized {
            return Err(CoreError::Refused("project_store_unavailable"));
        }
        let reservations: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='native_worktree_starts')",
            [], |r| r.get(0),
        )?;
        let mut binding = db.prepare(
            "SELECT w.execution_id, e.runtime_task_id, json_extract(o.options_json,'$.checkout.kind'),
             json_extract(o.options_json,'$.deletingSprint'),
             EXISTS(SELECT 1 FROM workspace_starts s WHERE s.task_id=w.task_id),
             (SELECT min(ev.id) FROM runtime_events ev WHERE ev.task_id=e.runtime_task_id
              AND ev.type='NATIVE_START_ADMITTED' AND json_extract(ev.payload_json,'$.draft')=w.task_id)
             FROM workspace_tasks w
             LEFT JOIN workspace_executions e ON e.execution_id=w.execution_id AND e.task_id=w.task_id
             LEFT JOIN workspace_task_options o ON o.task_id=w.task_id
             WHERE w.project_id=? AND w.task_id=?",
        )?;
        for task in &roster {
            let record = &product["tasks"][task];
            let mut status = "confirmed";
            let mut checkout = None;
            let mut draft = false;
            let mut claim_order = None;
            let mut fact = None;
            let bound_overlay = record["sprintId"] == id && record["planning"] != true
                && saved["planningTaskId"] != *task && !foreign.contains(task.as_str());
            let dependencies = if bound_overlay { deps(record) } else { None };
            projected["tasks"][task] = if bound_overlay { record.clone() } else { json!({}) };
            if !bound_overlay {
                status = if record.is_null() && !foreign.contains(task.as_str()) { "missing" } else { "unreadable" };
            } else if record["purged"] == true {
                status = "purged";
            } else if let Some((execution, runtime, kind, deleting, start_exists, order)) = binding
                .query_row((project, task), |r| Ok((
                    r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?, r.get::<_, Option<String>>(3)?,
                    r.get::<_, bool>(4)?, r.get::<_, Option<i64>>(5)?,
                ))).optional()?
            {
                if let Some(runtime) = &runtime {
                    task_rework::normalize_on(db, project, &mut projected, task, runtime)?;
                }
                checkout = kind;
                if let Some(projection) = cortex_core::projections::task_records(db, project, &root, task)? {
                    let projected_fact = serde_json::to_value(projection)?;
                    let reservation = execution.is_none() && reservations && db.query_row(
                        "SELECT EXISTS(SELECT 1 FROM native_worktree_starts WHERE project_id=? AND draft_id=?
                         AND state IN ('pending','uncertain','bound'))",
                        (project, task), |r| r.get::<_, bool>(0),
                    )?;
                    let closed_unconfirmed = projected_fact["warnings"].as_array()
                        .is_some_and(|warnings| warnings.iter().any(|w| w == "owner_closed_unconfirmed"));
                    if (projected_fact["phase"] == "unknown" && !closed_unconfirmed)
                        || !projected_fact["planningSprintId"].is_null()
                        || (execution.is_some() && runtime.is_none()) || (execution.is_none() && start_exists)
                        || deleting.is_some() || dependencies.is_none()
                    {
                        status = "unreadable";
                    } else {
                        claim_order = order;
                        let current = &projected["tasks"][task];
                        draft = execution.is_none() && !reservation && projected_fact["phase"] == "not_started"
                            && (current["status"].is_null() || matches!(current["status"].as_str(), Some("draft" | "not_started")))
                            && current["closedAs"].is_null() && current["archive"].is_null()
                            && current["archiveHold"].is_null();
                    }
                    fact = Some(projected_fact);
                } else {
                    status = "missing";
                }
            } else {
                status = "missing";
            }

            // Keep a trustworthy existing Core projection for scheduler holds, even if
            // an execution is unknown. A malformed draft must not become runnable.
            if let Some(mut value) = fact {
                if !draft && value["phase"] == "not_started" {
                    value["phase"] = json!("unknown");
                }
                let record = &projected["tasks"][task];
                let (outcome, closed) = crate::native_driver::sprints::member_disposition(record, Some(&value));
                graph_facts.push(json!([task, status, &dependencies, draft, &value["executionId"],
                    if draft { &value["version"] } else { &Value::Null }, outcome, closed, &record["archive"], &record["archiveHold"]]));
                // Activity on a skipped member is not an input to the queue
                // decision (a cancelled runtime may still publish late updates).
                let skipped = !record["archive"].is_null() || !record["archiveHold"].is_null()
                    || matches!(closed.as_deref(), Some("cancel" | "unconfirmed"));
                if !skipped {
                    let exit: Vec<_> = value["warnings"].as_array().into_iter().flatten()
                        .filter(|w| matches!(w.as_str(), Some("process_exit_pending" | "process_exit_unconfirmed")))
                        .collect();
                    queue_facts.push(json!([task, &checkout, &value["phase"], &value["waitReason"],
                        exit, &value["automaticContinuation"]["pause"]]));
                }
                tasks.insert(task.clone(), value);
            } else {
                graph_facts.push(json!([task, status, &dependencies]));
            }
            members.push(Member {id:task.clone(), status, dependencies, draft, claim_order, checkout});
        }
        Ok(())
    }).map_err(context::core)?;
    // A draft that a proposed sprint revision still holds back (`sprint_impact`)
    // does not start; independent drafts continue.
    let revisions = store
        .read(|db| super::sprint_revision::sprint_revisions(db, id))
        .map_err(context::core)?;
    for held in revisions.iter().flat_map(|r| r["held"].as_array().into_iter().flatten()) {
        if let Some(record) = held.as_str().and_then(|h| projected["tasks"].get_mut(h)).filter(|r| r.is_object()) {
            record["revisionPending"] = json!(true);
        }
    }

    let sprint = projected["sprints"][0].take();
    let records = projected["tasks"].take();
    let graph_revision = hash(&json!([
        project,
        id,
        saved["materializedVersion"],
        saved["archivedAt"],
        saved["deleting"],
        &roster,
        &priority,
        graph_facts
    ]));
    let queue_revision = hash(&json!([
        saved,
        &sprint,
        &graph_revision,
        &records,
        queue_facts
    ]));
    Ok(Snapshot {
        sprint,
        records,
        tasks,
        roster,
        priority,
        members,
        graph_revision,
        queue_revision,
        revisions,
    })
}

pub(crate) fn read_snapshot(
    store: &Store,
    state: &Path,
    project: &str,
    sprint: &str,
) -> Result<Snapshot> {
    let _lock = settings::Lock::new(state, "machine.lock")?;
    let mut product = product(state, project)?;
    // Files given to the sprint reach members created without them; best effort.
    let _ = super::sprint_files::backfill(store, state, project, &mut product, sprint);
    load(
        store,
        &product,
        project,
        sprint_index(&product, project, sprint)?,
        false,
    )
}

/// Called with machine.lock already held, on the same publication lane as the
/// native claim. Core's own start/version and worktree checks still apply.
pub(crate) fn check_start(
    store: &Store,
    product: &Value,
    project: &str,
    expected: &Expectation,
    task: &str,
) -> CoreResult<()> {
    let stale = || CoreError::Refused(STALE);
    let index = sprint_index(product, project, &expected.sprint_id).map_err(|_| stale())?;
    let sprint = &product["sprints"][index];
    if !open_sprint(sprint) || !queue_driven_mode(sprint) || sprint["queueState"] != "running" {
        return Err(stale());
    }
    let latest = load(store, product, project, index, false).map_err(|_| stale())?;
    if latest.queue_revision != expected.revision
        || crate::native_driver::sprints::selected_now(&latest) != Some(task)
        || !latest.members.iter().any(|m| m.id == task && m.draft)
    {
        return Err(stale());
    }
    Ok(())
}

/// Compare a queue-only decision with the graph and Core facts it used.
pub(super) fn matches_queue(
    root: &Path,
    product: &Value,
    project: &str,
    index: usize,
    revision: &str,
) -> Result<bool> {
    let Some(store) = context::open(root, OpenMode::LiveReadOnly)? else {
        return Ok(false);
    };
    Ok(load(&store, product, project, index, false)?.queue_revision == revision)
}

pub(super) fn dag_error(error: sprint_dag::Error<'_>, all: &Value) -> Refused {
    use sprint_dag::Error::*;
    match error {
        DuplicateId(_) => invalid_members(),
        SelfEdge(_) => refuse("dependency_self", "A task cannot depend on itself."),
        Missing(task, dep)
            if all[dep]["sprintId"].is_string()
                && all[dep]["sprintId"] != all[task]["sprintId"] =>
        {
            refuse(
                "dependency_cross_sprint",
                "The prerequisite is outside this sprint's bound roster.",
            )
        }
        Missing(_, _) => refuse("dependency_missing", "A prerequisite is missing."),
        DuplicateEdge => refuse(
            "dependency_duplicate",
            "A prerequisite is listed more than once.",
        ),
        Cycle => refuse("dependency_cycle", "These dependencies would form a cycle."),
    }
}

fn validate_candidate(
    snapshot: &Snapshot,
    all: &Value,
    changed: &str,
    replacement: &[String],
) -> Result<()> {
    let mut nodes = Vec::with_capacity(snapshot.members.len());
    for member in &snapshot.members {
        // A known purged/missing predecessor may be detached from a readable
        // graph. Do not infer the edges of a member whose overlay is unreadable.
        let dependencies = if member.id == changed {
            replacement
        } else {
            member.dependencies.as_deref().ok_or_else(invalid_members)?
        };
        nodes.push((
            member.id.as_str(),
            dependencies.iter().map(String::as_str).collect::<Vec<_>>(),
        ));
    }
    if nodes.iter().map(|(_, deps)| deps.len()).sum::<usize>() > MAX_EDGES {
        return Err(over_budget());
    }
    sprint_dag::ordered(&nodes).map_err(|error| dag_error(error, all))?;
    let unreadable: HashSet<_> = snapshot
        .members
        .iter()
        .filter(|m| matches!(m.status, "missing" | "unreadable"))
        .map(|m| m.id.as_str())
        .collect();
    for (_, predecessors) in nodes {
        for id in predecessors {
            if unreadable.contains(id) {
                return Err(refuse(
                    "dependency_missing",
                    "A prerequisite's saved binding could not be read.",
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn predecessor_allowed(snapshot: &Snapshot, member: &Member) -> bool {
    let record = &snapshot.records[&member.id];
    let fact = snapshot.tasks.get(&member.id);
    let (status, closed) = crate::native_driver::sprints::member_disposition(record, fact);
    member.status == "confirmed"
        && !matches!(closed.as_deref(), Some("cancel" | "unconfirmed"))
        && status != "closed_unverified"
        && (record["archive"].is_null() || status == "done")
        && (member.draft
            || matches!(status.as_str(), "done" | "ready_to_merge")
            || fact.is_some_and(|v| {
                v["executionId"].is_string()
                    && !matches!(v["phase"].as_str(), Some("unknown" | "stopped"))
            }))
}

fn describe(snapshot: &Snapshot, project: &str) -> Value {
    let sprint = &snapshot.sprint;
    let complete = snapshot
        .members
        .iter()
        .all(|m| matches!(m.status, "confirmed" | "purged"));
    let version = sprint["materializedVersion"].as_u64();
    let plan = if sprint["plan"]["version"].as_u64() == version {
        Some(&sprint["plan"])
    } else {
        sprint["history"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|p| p["version"].as_u64() == version)
    };
    let approved: HashMap<_, _> = version
        .into_iter()
        .flat_map(|version| {
            plan.and_then(|p| p["items"].as_array())
                .into_iter()
                .flatten()
                .filter_map(move |item| {
                    let key = item["key"].as_str()?;
                    Some((
                        sprint_materialize::task_id(project, sprint["id"].as_str()?, version, key),
                        key,
                    ))
                })
        })
        .collect();
    let members: Vec<_> = snapshot.members.iter().map(|m| json!({
        "taskId": m.id, "readStatus": m.status,
        "dependencies": if matches!(m.status, "confirmed" | "purged") { &m.dependencies } else { &None },
        "editable": complete && editable_sprint(sprint) && m.draft,
        "canBePredecessor": complete && editable_sprint(sprint) && predecessor_allowed(snapshot, m),
        "approvedPlanKey": approved.get(&m.id),
    })).collect();
    let order = crate::native_driver::sprints::live_execution_order(snapshot);
    json!({"contractVersion":1,"sprintId":sprint["id"],"graphRevision":snapshot.graph_revision,
        "complete":complete,"members":members,"priority":snapshot.priority,"order":order,
        "revisions":super::sprint_revision::owner_view(&snapshot.revisions)})
}

pub(super) fn owner_action(root: &Path, state: &Path, project: &str, p: &Value, routes: super::sprint_revision::Routes<'_>) -> Result<Value> {
    let id = id_param(p, "sprintId")?;
    let reading = p["action"] == "live_graph";
    let _lock = settings::Lock::new(state, "machine.lock")?;
    // Owner reads do not run the legacy attachment migration. Graph edits
    // replace the exact saved object, changing only the requested field.
    let path = product_path(state, project)?;
    let mut product = settings::read_json(&path, json!({"tasks":{}, "sprints":[]}))?;
    let index = sprint_index(&product, project, id)?;
    let store = context::open(root, OpenMode::LiveReadOnly)?.ok_or_else(|| {
        refuse(
            "project_store_unavailable",
            "The bound project store is unavailable.",
        )
    })?;
    let snapshot = load(&store, &product, project, index, true)?;
    if reading {
        drop(_lock);
        return Ok(describe(&snapshot, project));
    }
    if matches!(p["action"].as_str(), Some("apply_revision" | "reject_revision")) {
        drop(store);
        return super::sprint_revision::owner_decision(root, state, project, &mut product, index, &snapshot, p, routes);
    }
    if !editable_sprint(&snapshot.sprint) {
        return Err(refuse(
            "sprint_read_only",
            "Only an open, created sprint can be edited.",
        ));
    }
    if string(p, "expectedGraphRevision")? != snapshot.graph_revision {
        return Err(refuse(
            "stale_graph_revision",
            "The sprint changed. Refresh its graph before trying again.",
        ));
    }
    let field = if p["action"] == "set_dependency" {
        "dependentTaskId"
    } else {
        "taskId"
    };
    let task = id_param(p, field)?;
    let member = snapshot
        .members
        .iter()
        .find(|m| m.id == task)
        .ok_or_else(|| refuse("task_outside_sprint", "The task is not in this sprint."))?;
    if !member.draft {
        return Err(refuse(
            "sprint_member_read_only",
            "Only a confirmed draft that has not started can be edited.",
        ));
    }
    let changed = if p["action"] == "set_dependency" {
        let predecessor = id_param(p, "predecessorTaskId")?;
        let present = p["present"]
            .as_bool()
            .ok_or_else(|| refuse("invalid_request", "Name the desired link state."))?;
        let mut replacement = member.dependencies.clone().ok_or_else(invalid_members)?;
        let exists = replacement.iter().any(|d| d == predecessor);
        if present == exists {
            false
        } else {
            if present {
                if predecessor == task {
                    return Err(dag_error(
                        sprint_dag::Error::SelfEdge(task),
                        &product["tasks"],
                    ));
                }
                let m = snapshot
                    .members
                    .iter()
                    .find(|m| m.id == predecessor)
                    .ok_or_else(|| {
                        dag_error(
                            sprint_dag::Error::Missing(task, predecessor),
                            &product["tasks"],
                        )
                    })?;
                if !predecessor_allowed(&snapshot, m) {
                    return Err(refuse("predecessor_unavailable", "This prerequisite cannot be confirmed or is no longer runnable or settled."));
                }
                replacement.push(predecessor.to_owned());
            } else {
                replacement.retain(|d| d != predecessor);
            }
            validate_candidate(&snapshot, &product["tasks"], task, &replacement)?;
            product["tasks"][task]["dependencies"] = json!(replacement);
            true
        }
    } else {
        let direction = match p["direction"].as_str() {
            Some("up") => -1isize,
            Some("down") => 1isize,
            _ => return Err(refuse("invalid_request", "Choose up or down.")),
        };
        if snapshot
            .members
            .iter()
            .any(|m| matches!(m.status, "missing" | "unreadable"))
        {
            return Err(invalid_members());
        }
        let editable: HashSet<_> = snapshot
            .members
            .iter()
            .filter(|m| m.draft)
            .map(|m| m.id.as_str())
            .collect();
        let mut priority = snapshot.priority.clone();
        let at = priority.iter().position(|id| id == task).unwrap();
        let other = if direction < 0 {
            (0..at)
                .rev()
                .find(|&i| editable.contains(priority[i].as_str()))
        } else {
            ((at + 1)..priority.len()).find(|&i| editable.contains(priority[i].as_str()))
        };
        if let Some(other) = other {
            priority.swap(at, other);
            product["sprints"][index]["taskPriority"] = json!(priority);
            true
        } else {
            false
        }
    };
    if changed {
        settings::write_json(&path, &product).map_err(|_| {
            refuse(
                "graph_save_unconfirmed",
                "The save could not be confirmed. Refresh before trying again.",
            )
        })?;
    }
    Ok(json!({"changed":changed}))
}

impl Snapshot {
    pub(crate) fn projection_member(
        &self,
        id: &str,
    ) -> Option<(bool, Option<i64>, &str, Option<&str>)> {
        self.members
            .iter()
            .find(|m| m.id == id)
            .map(|m| (m.draft, m.claim_order, m.status, m.checkout.as_deref()))
    }
}
