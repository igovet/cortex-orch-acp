//! Sprint revisions: after the owner changed a running sprint member, its
//! coordinator re-checks the sprint's remaining unstarted drafts and records a
//! verdict through `sprint_revision_submit` (see `cortex_core::lifecycle::sprint_impact`).
//!
//! The service validates a proposed revision under the machine lock with the same
//! ID-only graph validator as materialization and live link edits, records it with
//! its mandatory independent review, applies an approved revision of an Automatic
//! sprint in one project-store transaction with the saved product record, and hands
//! one of a Hybrid or Manual sprint to the owner (`apply_revision`,
//! `reject_revision`). Only saved drafts with no execution identity change; the
//! approved plan, its history and `materializedVersion` are never written, Pause is
//! never lifted, nothing is deleted and proposed cancellations are only shown.
use super::{
    context,
    native::*,
    settings::{self, Lock},
    sprint_dag,
    sprint_live::{self, Snapshot},
};
use cortex_core::{
    lifecycle::sprint_impact::{self, Checked, Settlement},
    workspace as core_workspace, CoreError, CoreResult, OpenMode, Store,
};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

fn sha(value: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(value.as_ref()))
}

/// Every revision proposed by a task of `sprint`, oldest first, each naming the
/// Workspace task that proposed it (`draftTaskId`).
pub(crate) fn sprint_revisions(db: &Connection, sprint: &str) -> CoreResult<Vec<Value>> {
    let tasks = db
        .prepare(
            "SELECT DISTINCT task_id FROM runtime_events WHERE type=?
             AND json_extract(payload_json,'$.sprint_id')=?",
        )?
        .query_map((sprint_impact::SUBMITTED, sprint), |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut out = Vec::new();
    for task in tasks {
        let draft: Option<String> = db
            .query_row(
                "SELECT w.task_id FROM workspace_tasks w JOIN workspace_executions e
                 ON w.execution_id=e.execution_id AND w.task_id=e.task_id WHERE e.runtime_task_id=?",
                [&task],
                |r| r.get(0),
            )
            .optional()?;
        for mut r in sprint_impact::revisions(db, &task)? {
            if r["sprintId"] == sprint {
                r["draftTaskId"] = json!(draft);
                out.push(r);
            }
        }
    }
    out.sort_by_key(|r| r["event"].as_i64());
    Ok(out)
}

/// What the owner's live sprint read carries: the newest revisions, without the
/// runtime's internal identities.
pub(super) fn owner_view(revisions: &[Value]) -> Value {
    let start = revisions.len().saturating_sub(20);
    json!(revisions[start..]
        .iter()
        .map(|r| json!({"revisionId":r["revisionId"],"taskId":r["draftTaskId"],"status":r["status"],
            "applied":r["applied"],"submittedAt":r["submittedAt"],"appliedAt":r["appliedAt"],
            "reason":r["reason"],"changes":r["changes"],"held":r["held"],"findings":r["findings"],
            "ownerReason":r["ownerReason"],"updated":r["updated"],"added":r["added"],
            "previous":r["previous"],"actor":r["actor"],"code":r["code"]}))
        .collect::<Vec<_>>())
}

/// The Workspace task and project of a runtime task.
fn binding(db: &Connection, runtime: &str) -> CoreResult<Option<(String, String)>> {
    Ok(db
        .query_row(
            "SELECT w.task_id,w.project_id FROM workspace_tasks w JOIN workspace_executions e
             ON w.execution_id=e.execution_id AND w.task_id=e.task_id WHERE e.runtime_task_id=?",
            [runtime],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
}

fn core(error: Refused) -> CoreError {
    CoreError::Refused(error.code)
}

/// The authenticated task's sprint: its Workspace task, project, product record
/// and sprint index.
struct Membership {
    draft: String,
    project: String,
    product: Value,
    index: usize,
    sprint: String,
}

fn membership(store: &Store, state: &Path, runtime: &str) -> CoreResult<Option<Membership>> {
    let Some((draft, project)) = store.read(|db| binding(db, runtime))? else {
        return Ok(None);
    };
    let product = product(state, &project).map_err(core)?;
    let Some(sprint) = product["tasks"][&draft]["sprintId"].as_str().map(str::to_owned) else {
        return Ok(None);
    };
    let index = sprint_live::sprint_index(&product, &project, &sprint).map_err(core)?;
    let roster = product["sprints"][index]["taskIds"].as_array();
    if !roster.is_some_and(|r| r.iter().any(|id| id == draft.as_str())) {
        return Ok(None);
    }
    Ok(Some(Membership { draft, project, product, index, sprint }))
}

/// At each coordinator turn of a sprint member: record the sprint's unstarted
/// drafts for the impact trigger, settle approved revisions, and return the turn's
/// pointer to an open impact check (empty when none).
pub(crate) fn coordinator_turn(store: &Store, state: &Path, runtime: &str, routes: Routes<'_>) -> CoreResult<String> {
    let mut note = settle(store, state, runtime, routes)?;
    if let Some(m) = membership(store, state, runtime)? {
        let roster: Vec<String> = m.product["sprints"][m.index]["taskIds"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|id| *id != m.draft)
            .map(str::to_owned)
            .collect();
        store.write(|db| {
            let mut unstarted = Vec::new();
            for id in &roster {
                let draft: bool = db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM workspace_tasks w WHERE w.task_id=? AND w.project_id=?
                     AND w.execution_id IS NULL AND NOT EXISTS(SELECT 1 FROM workspace_starts s WHERE s.task_id=w.task_id))",
                    (id, &m.project),
                    |r| r.get(0),
                )?;
                if draft {
                    unstarted.push(id.clone());
                }
            }
            sprint_impact::bind_tx(db, runtime, &m.sprint, &unstarted)
        })?;
    }
    let pointer = store.read(|db| sprint_impact::prompt(db, runtime))?;
    if !pointer.is_empty() {
        note.push_str(if note.is_empty() { "" } else { "\n" });
        note.push_str(&pointer);
    }
    Ok(note)
}

/// Apply this task's approved revisions (Automatic) or hand them to the owner
/// (Hybrid, Manual). A refused apply is recorded and leaves the check open. Returns
/// a note for the coordinator's next turn.
pub(crate) fn settle(store: &Store, state: &Path, runtime: &str, routes: Routes<'_>) -> CoreResult<String> {
    let approved = store.read(|db| sprint_impact::approved(db, runtime))?;
    if approved.is_empty() {
        return Ok(String::new());
    }
    let _lock = Lock::new(state, "machine.lock").map_err(core)?;
    let mut notes = Vec::new();
    for r in approved {
        let id = r["revisionId"].as_str().unwrap_or_default().to_owned();
        let outcome = (|| -> Result<Option<Value>> {
            let (_, project) = store
                .read(|db| binding(db, runtime))
                .map_err(context::core)?
                .ok_or_else(|| refuse("sprint_membership_required", "The task is not in a sprint."))?;
            let path = product_path(state, &project)?;
            let mut product = settings::read_json(&path, json!({"tasks":{}, "sprints":[]}))?;
            let sprint = r["sprintId"].as_str().unwrap_or_default();
            let index = sprint_live::sprint_index(&product, &project, sprint)?;
            let snapshot = sprint_live::load(store, &product, &project, index, true)?;
            if snapshot.sprint["runMode"] != "automatic" {
                return Ok(None);
            }
            apply(store, state, &project, &mut product, index, &snapshot, runtime, &r, "runtime", routes).map(Some)
        })();
        match outcome {
            Ok(Some(applied)) => notes.push(format!(
                "Sprint revision {id} was reviewed and applied: {} task(s) changed, {} added.",
                applied["updated"].as_array().map_or(0, Vec::len),
                applied["added"].as_array().map_or(0, Vec::len)
            )),
            Ok(None) => {
                store.write(|db| {
                    sprint_impact::settle_tx(db, runtime, &id, Settlement::AwaitingOwner { reason: "owner_decision" })
                })?;
                notes.push(format!(
                    "Sprint revision {id} was reviewed; the owner applies or rejects it. This task may finish."
                ));
            }
            Err(error) => {
                store.write(|db| {
                    sprint_impact::settle_tx(db, runtime, &id, Settlement::Refused { code: error.code })
                })?;
            }
        }
    }
    Ok(notes.join("\n"))
}

/// The runtime's automatic route of a sprint task of one complexity (`RouteResolver::sprint_route`):
/// the same resolver sprint creation uses. `None` leaves the owner's default coordinator.
pub(crate) type Routes<'a> = &'a dyn Fn(&str) -> Option<Value>;

/// One change of a revision, resolved against the live graph.
enum Change<'a> {
    Update { member: &'a str, version: &'a str, title: Option<&'a str>, description: Option<&'a str>, depends: Option<Vec<&'a str>> },
    Add { id: String, title: &'a str, description: &'a str, complexity: &'a str },
    Cancel { member: &'a str },
}

fn version_text(v: &Value) -> Option<String> {
    v.as_str().map(str::to_owned).or_else(|| v.as_i64().map(|n| n.to_string()))
}

fn new_task_id(project: &str, sprint: &str, revision: &str, key: &str) -> String {
    let hex = sha(format!("cortex-sprint-revision-task\0{project}\0{sprint}\0{revision}\0{key}"));
    format!("wt_{}", &hex[..32])
}

fn invalid(code: &'static str, detail: impl Into<String>) -> Refused {
    refuse(code, detail)
}

/// The checked candidate: the changes, the drafts they touch, and the new link
/// sets by task id.
struct Candidate<'a> {
    changes: Vec<Change<'a>>,
    links: HashMap<String, Vec<String>>,
}

/// Validate `changes` against the live graph: only drafts with no execution
/// identity, at their expected versions; no unknown, cross-sprint, self or cyclic
/// link and no new link to a cancelled or otherwise unavailable prerequisite.
fn check<'a>(
    snapshot: &Snapshot,
    all: &Value,
    project: &str,
    sprint: &str,
    revision: &str,
    changes: &'a [Value],
) -> Result<Candidate<'a>> {
    let members: HashMap<&str, &sprint_live::Member> =
        snapshot.members.iter().map(|m| (m.id.as_str(), m)).collect();
    let mut keys: HashMap<&str, String> = HashMap::new();
    for c in changes {
        if c["op"] == "add" {
            let key = c["key"].as_str().unwrap_or_default();
            if key.is_empty() || members.contains_key(key) || keys.contains_key(key) {
                return Err(invalid("duplicate_change", format!("the new task key {key} is repeated or names an existing task")));
            }
            keys.insert(key, new_task_id(project, sprint, revision, key));
        }
    }
    let resolve = |dependent: &str, r: &'a str| -> Result<String> {
        if members.contains_key(r) {
            return Ok(r.to_owned());
        }
        if let Some(id) = keys.get(r) {
            return Ok(id.clone());
        }
        Err(sprint_live::dag_error(sprint_dag::Error::Missing(dependent, r), all))
    };
    for c in changes {
        let need: &[&str] = match c["op"].as_str() {
            Some("update") => &["member_id", "expected_version"],
            Some("add") => &["key", "title", "description", "dependsOn"],
            Some("propose_cancel") => &["member_id", "reason"],
            _ => &[],
        };
        if let Some(field) = need.iter().find(|f| c[**f].is_null()) {
            return Err(invalid("invalid_change", format!("op {} needs {field}", c["op"].as_str().unwrap_or("?"))));
        }
    }
    let mut touched = HashSet::new();
    let mut out = Vec::new();
    let mut links: HashMap<String, Vec<String>> = HashMap::new();
    for c in changes {
        let deps = |c: &'a Value| -> Option<Vec<&'a str>> {
            c["dependsOn"].as_array().map(|d| d.iter().filter_map(Value::as_str).collect())
        };
        match c["op"].as_str() {
            Some("update") | Some("propose_cancel") => {
                let id = c["member_id"].as_str().unwrap_or_default();
                let member = members
                    .get(id)
                    .ok_or_else(|| invalid("task_outside_sprint", format!("{id} is not a task of this sprint")))?;
                if !member.draft {
                    return Err(invalid("sprint_member_read_only", format!(
                        "{id} has started, finished or is archived; only drafts that have not started can change")));
                }
                if !touched.insert(id) {
                    return Err(invalid("duplicate_change", format!("{id} is changed more than once")));
                }
                if c["op"] == "propose_cancel" {
                    out.push(Change::Cancel { member: id });
                    continue;
                }
                let version = c["expected_version"].as_str().unwrap_or_default();
                let current = snapshot.tasks.get(id).and_then(|t| version_text(&t["version"]));
                if current.as_deref() != Some(version) {
                    return Err(invalid("stale_version", format!(
                        "{id} is at version {} now; read it again with sprint_graph include \"pending_descriptions\"",
                        current.unwrap_or_default())));
                }
                let (title, description, depends) =
                    (c["title"].as_str(), c["description"].as_str(), deps(c));
                if title.is_none() && description.is_none() && depends.is_none() {
                    return Err(invalid("invalid_change", format!("the update of {id} changes nothing")));
                }
                if let Some(depends) = &depends {
                    let mut replacement = Vec::new();
                    for d in depends {
                        if *d == id {
                            return Err(sprint_live::dag_error(sprint_dag::Error::SelfEdge(id), all));
                        }
                        replacement.push(resolve(id, d)?);
                    }
                    links.insert(id.to_owned(), replacement);
                }
                out.push(Change::Update { member: id, version, title, description, depends });
            }
            Some("add") => {
                let key = c["key"].as_str().unwrap_or_default();
                let depends = deps(c).unwrap_or_default();
                let mut replacement = Vec::new();
                for d in &depends {
                    if *d == key {
                        return Err(sprint_live::dag_error(sprint_dag::Error::SelfEdge(key), all));
                    }
                    replacement.push(resolve(key, d)?);
                }
                links.insert(keys[key].clone(), replacement);
                let complexity = match &c["complexity"] {
                    Value::Null => "normal",
                    Value::String(x) if ["low", "normal", "high"].contains(&x.as_str()) => x.as_str(),
                    _ => return Err(invalid("invalid_change", format!("the new task {key} has an unknown complexity"))),
                };
                out.push(Change::Add {
                    complexity,
                    id: keys[key].clone(),
                    title: c["title"].as_str().unwrap_or_default(),
                    description: c["description"].as_str().unwrap_or_default(),
                });
            }
            _ => return Err(invalid("invalid_change", "unknown change op")),
        }
    }
    // New links onto existing members must name a runnable or settled prerequisite;
    // a retained bad link may stay.
    for (dependent, replacement) in &links {
        let before: HashSet<&str> = members
            .get(dependent.as_str())
            .and_then(|m| m.dependencies.as_deref())
            .into_iter()
            .flatten()
            .map(String::as_str)
            .collect();
        for d in replacement {
            if let Some(m) = members.get(d.as_str()) {
                if !before.contains(d.as_str()) && !sprint_live::predecessor_allowed(snapshot, m) {
                    return Err(invalid("predecessor_unavailable", format!(
                        "{d} is cancelled, unconfirmed or otherwise cannot be a prerequisite")));
                }
            }
        }
    }
    // The whole candidate graph, with the ID-only validator materialization uses.
    let mut nodes: Vec<(&str, Vec<&str>)> = Vec::with_capacity(snapshot.members.len() + keys.len());
    for m in &snapshot.members {
        let deps = match links.get(&m.id) {
            Some(replacement) => replacement.iter().map(String::as_str).collect(),
            None => m
                .dependencies
                .as_deref()
                .ok_or_else(sprint_live::invalid_members)?
                .iter()
                .map(String::as_str)
                .collect(),
        };
        nodes.push((m.id.as_str(), deps));
    }
    for id in keys.values() {
        nodes.push((id.as_str(), links[id].iter().map(String::as_str).collect()));
    }
    sprint_dag::ordered(&nodes).map_err(|e| sprint_live::dag_error(e, all))?;
    Ok(Candidate { changes: out, links })
}

fn descriptions(store: &Store, project: &str, ids: &[&str]) -> Result<HashMap<String, String>> {
    store
        .read(|db| {
            let mut out = HashMap::new();
            for id in ids {
                if let Some(d) = db
                    .query_row(
                        "SELECT description FROM workspace_tasks WHERE task_id=? AND project_id=?",
                        (id, project),
                        |r| r.get::<_, String>(0),
                    )
                    .optional()?
                {
                    out.insert((*id).to_owned(), d);
                }
            }
            Ok(out)
        })
        .map_err(context::core)
}

fn reply(value: Value, error: bool) -> Value {
    json!({"content":[{"type":"text","text":value.to_string()}],"structuredContent":value,"isError":error})
}

fn refused(error: Refused) -> Value {
    reply(
        json!({"recorded":false,"code":error.code,"detail":error.message,
            "hint":"Correct the named change and rule (read the graph again when it changed), then call sprint_revision_submit again with a new request_key."}),
        true,
    )
}

/// `sprint_revision_submit` after its channel and schema checks.
pub(crate) fn submit(store: &Store, state: &Path, runtime: &str, actor: &str, args: &Value) -> CoreResult<Value> {
    let key = args["request_key"].as_str().ok_or(CoreError::Refused("invalid_request_key"))?;
    let replay: bool = store.read(|db| {
        Ok(db
            .query_row(
                "SELECT 1 FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=?",
                (runtime, actor, key),
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .is_some())
    })?;
    if replay {
        let value = store.write(|db| sprint_impact::submit_tx(db, runtime, actor, key, args, None))?;
        return Ok(reply(value, false));
    }
    let _lock = Lock::new(state, "machine.lock").map_err(core)?;
    let m = membership(store, state, runtime)?.ok_or(CoreError::Refused("sprint_membership_required"))?;
    let trigger = args["trigger"].as_i64().unwrap_or(0);
    if let Err(CoreError::Refused(code)) = store.write(|db| sprint_impact::check_submission(db, runtime, trigger)) {
        return Ok(refused(refuse(code, match code {
            "sprint_impact_not_expected" => "No sprint impact check is open for this task.".to_owned(),
            "sprint_impact_trigger_stale" => "A newer owner change requires the check; use the trigger named in the newest prompt.".to_owned(),
            _ => "Submit the verdict from a planning or reconciling turn with no worker running.".to_owned(),
        })));
    }
    let changes = args["changes"].as_array().cloned().unwrap_or_default();
    if args["verdict"] == "unchanged" {
        if !changes.is_empty() {
            return Ok(refused(refuse("sprint_revision_changes_unexpected", "Verdict unchanged takes no changes.")));
        }
        let value = store.write(|db| sprint_impact::submit_tx(db, runtime, actor, key, args, None))?;
        return Ok(reply(value, false));
    }
    if changes.is_empty() {
        return Ok(refused(refuse("sprint_revision_changes_required", "Verdict revise needs at least one change.")));
    }
    let outcome = (|| -> Result<Checked> {
        let snapshot = sprint_live::load(store, &m.product, &m.project, m.index, true)?;
        if !sprint_live::editable_sprint(&snapshot.sprint) {
            return Err(refuse("sprint_read_only", "Only an open, created sprint can be revised."));
        }
        if args["base_graph_revision"].as_str() != Some(snapshot.graph_revision.as_str()) {
            return Err(refuse("stale_graph_revision",
                "The sprint changed; read it again with sprint_graph include \"pending_descriptions\"."));
        }
        let revision_id = format!("sr_{}", &sha(format!("{runtime}\0{actor}\0{key}"))[..16]);
        let candidate = check(&snapshot, &m.product["tasks"], &m.project, &m.sprint, &revision_id, &changes)?;
        let mut affected = Vec::new();
        let mut cancel = Vec::new();
        for c in &candidate.changes {
            match c {
                Change::Update { member, .. } => affected.push((*member).to_owned()),
                Change::Cancel { member } => cancel.push((*member).to_owned()),
                Change::Add { .. } => {}
            }
        }
        let current = descriptions(store, &m.project, &affected.iter().map(String::as_str).collect::<Vec<_>>())?;
        let context: Vec<Value> = affected
            .iter()
            .map(|id| json!({"member_id":id,"title":m.product["tasks"][id]["title"],
                "dependsOn":m.product["tasks"][id]["dependencies"],"description":current.get(id)}))
            .collect();
        Ok(Checked {
            revision_id,
            sprint_id: m.sprint.clone(),
            base_graph_revision: snapshot.graph_revision.clone(),
            changes: json!(changes),
            affected,
            cancel,
            context: json!(context),
        })
    })();
    match outcome {
        Ok(checked) => {
            let value = store.write(|db| sprint_impact::submit_tx(db, runtime, actor, key, args, Some(checked)))?;
            Ok(reply(value, false))
        }
        Err(error) => Ok(refused(error)),
    }
}

/// Apply one approved revision, the caller holding the machine lock: every draft
/// change, every new draft, the saved product record and the settlement commit
/// together or not at all. Changes are checked again against the current graph.
#[allow(clippy::too_many_arguments)]
fn apply(
    store: &Store,
    state: &Path,
    project: &str,
    product: &mut Value,
    index: usize,
    snapshot: &Snapshot,
    runtime: &str,
    revision: &Value,
    actor: &str,
    routes: Routes<'_>,
) -> Result<Value> {
    let path = product_path(state, project)?;
    let prior = product.clone();
    let sprint = product["sprints"][index].clone();
    let sprint_id = sprint["id"].as_str().unwrap_or_default().to_owned();
    if !sprint_live::editable_sprint(&snapshot.sprint) {
        return Err(refuse("sprint_read_only", "Only an open, created sprint can be revised."));
    }
    let id = revision["revisionId"].as_str().unwrap_or_default().to_owned();
    let changes = revision["changes"].as_array().cloned().unwrap_or_default();
    let candidate = check(snapshot, &product["tasks"], project, &sprint_id, &id, &changes)?;
    let updated: Vec<&str> = candidate
        .changes
        .iter()
        .filter_map(|c| match c { Change::Update { member, .. } => Some(*member), _ => None })
        .collect();
    let before_text = descriptions(store, project, &updated)?;
    let graph = |product: &Value, texts: &HashMap<String, String>| {
        let roster = &product["sprints"][index]["taskIds"];
        let links: Vec<Value> = roster
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|t| json!([t, product["tasks"][t]["dependencies"], product["tasks"][t]["title"], texts.get(t)]))
            .collect();
        sha(json!([roster, links]).to_string())
    };
    let before_digest = graph(product, &before_text);
    let previous: Vec<Value> = updated
        .iter()
        .map(|t| json!({"memberId":t,"title":product["tasks"][t]["title"],"dependencies":product["tasks"][t]["dependencies"],
            "description":before_text.get(*t),"version":snapshot.tasks.get(*t).and_then(|v| version_text(&v["version"]))}))
        .collect();
    let mut after_text = before_text.clone();
    let route = super::sprint_materialize::owner_sprint_route(&sprint);
    // Each added task gets the owner's sprint route, or else the route of its own
    // complexity inside the sprint's mode and language (as at sprint creation).
    let mut options: HashMap<String, Value> = HashMap::new();
    let mut assigned: HashMap<String, (Value, &str)> = HashMap::new();
    for c in &candidate.changes {
        let Change::Add { id, complexity, .. } = c else { continue };
        let mut o = json!({"checkout":{"kind":sprint.get("checkout").filter(|c| !c.is_null()).map(|c| c["kind"].clone()).unwrap_or_else(|| json!("main"))}});
        if let Some(route) = &route {
            o["settings"] = route.clone();
        } else if let Some(a) = routes(complexity).filter(|r| {
            r["routeId"].as_str().is_some_and(|m| !m.is_empty()) && r["effort"].as_str().is_some_and(|e| !e.is_empty())
        }) {
            let mut merged = json!({"mode":"guarded","language":"auto"});
            for (k, v) in sprint["settings"].as_object().into_iter().flatten() {
                merged[k] = v.clone();
            }
            merged["model"] = a["routeId"].clone();
            merged["effort"] = a["effort"].clone();
            o["settings"] = merged;
            assigned.insert(id.clone(), (a, complexity));
        }
        options.insert(id.clone(), o);
    }
    let mut added = Vec::new();
    for c in &candidate.changes {
        match c {
            Change::Update { member, title, description, depends, .. } => {
                let record = &mut product["tasks"][*member];
                if let Some(title) = title.filter(|_| record["titleState"] != "user") {
                    record["title"] = json!(title.trim());
                    record["titleState"] = json!("generated");
                }
                if depends.is_some() {
                    record["dependencies"] = json!(candidate.links[*member]);
                }
                if let Some(d) = description {
                    after_text.insert((*member).to_owned(), (*d).to_owned());
                }
            }
            Change::Add { id, title, description, .. } => {
                product["tasks"][id.as_str()] = json!({"sprintId":sprint_id,"dependencies":candidate.links[id],
                    "planning":false,"title":title.trim(),"titleState":"generated",
                    "targetBranch":sprint["targetBranch"],"targetInherited":true,
                    "revisionId":revision["revisionId"]});
                if let Some((a, complexity)) = assigned.get(id) {
                    product["tasks"][id.as_str()]["sprintRoute"] = json!({"routeId":a["routeId"],"engine":a["engine"],
                        "model":a["model"],"tier":a["tier"],"effort":a["effort"],
                        "complexity":complexity,"usedPercent":a["usedPercent"]});
                }
                after_text.insert(id.clone(), (*description).to_owned());
                added.push(id.clone());
            }
            Change::Cancel { .. } => {}
        }
    }
    if let Some(roster) = product["sprints"][index]["taskIds"].as_array_mut() {
        roster.extend(added.iter().map(|id| json!(id)));
    }
    let after_digest = graph(product, &after_text);
    let drafts: Vec<core_workspace::BatchDraft> = candidate
        .changes
        .iter()
        .filter_map(|c| match c {
            Change::Add { id, description, .. } => Some(core_workspace::BatchDraft { task_id: id, description, options: &options[id] }),
            _ => None,
        })
        .collect();
    let store_root = store.key().root().to_string_lossy().into_owned();
    let machine_actor = crate::actor();
    let wrote = std::cell::Cell::new(false);
    let payload = json!({"before_digest":before_digest,"after_digest":after_digest,"updated":updated,
        "added":added,"previous":previous});
    let result = store.write(|db| {
        for c in &candidate.changes {
            if let Change::Update { member, version, title, description, .. } = c {
                core_workspace::edits::revise_draft_tx(db, project, member, version, *description,
                    title.filter(|_| prior["tasks"][*member]["titleState"] != "user"))?;
            }
        }
        if !drafts.is_empty() {
            core_workspace::create_drafts_tx(db, &store_root, project, &machine_actor, &drafts)?;
        }
        sprint_impact::settle_tx(db, runtime, &id, Settlement::Applied { actor, payload: payload.clone() })?;
        wrote.set(true);
        settings::write_json(&path, product).map_err(|e| CoreError::Refused(e.code))?;
        Ok(())
    });
    match result {
        Ok(()) => Ok(payload),
        Err(error) => {
            if wrote.get() && settings::write_json(&path, &prior).is_err() {
                return Err(refuse("machine_outcome_unknown",
                    "Applying the sprint revision failed and the sprint could not be restored. Refresh before trying again."));
            }
            *product = prior;
            Err(context::core(error))
        }
    }
}

/// `product.sprint` actions `apply_revision {sprintId, revisionId, expectedGraphRevision}`
/// and `reject_revision {sprintId, revisionId}` of the local owner, under the
/// machine lock the caller holds.
pub(super) fn owner_decision(
    root: &Path,
    state: &Path,
    project: &str,
    product: &mut Value,
    index: usize,
    snapshot: &Snapshot,
    p: &Value,
    routes: Routes<'_>,
) -> Result<Value> {
    let id = string(p, "revisionId")?;
    let revision = snapshot
        .revisions
        .iter()
        .find(|r| r["revisionId"] == id)
        .ok_or_else(|| refuse("unknown_revision", "Unknown sprint revision."))?;
    if revision["status"] != "awaiting_owner" {
        return Err(refuse("revision_not_pending", "This revision is not waiting for your decision."));
    }
    let runtime = revision["taskId"].as_str().unwrap_or_default().to_owned();
    let store = context::open(root, OpenMode::RuntimeValidate)?
        .ok_or_else(|| refuse("project_store_unavailable", "The bound project store is unavailable."))?;
    if p["action"] == "reject_revision" {
        store
            .write(|db| sprint_impact::settle_tx(db, &runtime, id, Settlement::Rejected { actor: "owner" }))
            .map_err(context::core)?;
        return Ok(json!({"changed":true}));
    }
    if revision["applied"] == true {
        return Err(refuse("revision_already_applied",
            "Its changes are applied; only the proposed cancellations remain. Cancel those tasks yourself or dismiss the proposal."));
    }
    if string(p, "expectedGraphRevision")? != snapshot.graph_revision {
        return Err(refuse("stale_graph_revision", "The sprint changed. Refresh its graph before trying again."));
    }
    let applied = apply(&store, state, project, product, index, snapshot, &runtime, revision, "owner", routes)?;
    Ok(json!({"changed":true,"updated":applied["updated"],"added":applied["added"]}))
}

const PAGE_BYTES: usize = 48_000;

/// `sprint_graph` with `include: "pending_descriptions"`: the full saved
/// descriptions and versions of the sprint's unstarted drafts, with the live
/// `graphRevision` a revision is based on, in byte-bounded pages.
pub(crate) fn pending_descriptions(store: &Store, state: &Path, runtime: &str, args: &Value) -> CoreResult<Value> {
    let limit = args["limit"].as_u64().unwrap_or(10);
    if !(1..=50).contains(&limit) {
        return Err(CoreError::Refused("invalid_limit"));
    }
    let m = {
        let _lock = Lock::new(state, "machine.lock").map_err(core)?;
        membership(store, state, runtime)?.ok_or(CoreError::Refused("sprint_membership_required"))?
    };
    let snapshot = sprint_live::load(store, &m.product, &m.project, m.index, false).map_err(core)?;
    let drafts: Vec<&sprint_live::Member> =
        snapshot.members.iter().filter(|x| x.draft && x.id != m.draft).collect();
    let binding = sha(format!("{}\0{}", snapshot.graph_revision, m.draft));
    let binding = &binding[..32];
    let offset = match args["cursor"].as_str() {
        Some(cursor) => cursor
            .strip_prefix(binding)
            .and_then(|s| s.strip_prefix('.'))
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|n| *n < drafts.len())
            .ok_or(CoreError::Refused("invalid_cursor"))?,
        None => 0,
    };
    let ids: Vec<&str> = drafts.iter().skip(offset).take(limit as usize).map(|x| x.id.as_str()).collect();
    let texts = descriptions(store, &m.project, &ids).map_err(core)?;
    let mut rows = Vec::new();
    let mut bytes = 0;
    for x in drafts.iter().skip(offset).take(limit as usize) {
        let row = json!({"member_id":x.id,"title":m.product["tasks"][&x.id]["title"],
            "version":snapshot.tasks.get(&x.id).and_then(|v| version_text(&v["version"])),
            "dependsOn":x.dependencies,"held":snapshot.records[&x.id]["revisionPending"] == true,
            "description":texts.get(&x.id)});
        bytes += row.to_string().len();
        if !rows.is_empty() && bytes > PAGE_BYTES {
            break;
        }
        rows.push(row);
    }
    let end = offset + rows.len();
    Ok(json!({"sprintId":m.sprint,"taskId":m.draft,"graphRevision":snapshot.graph_revision,
        "tasks":rows,"total":drafts.len(),
        "next_cursor":(end < drafts.len()).then(|| format!("{binding}.{end}"))}))
}

#[cfg(test)]
#[path = "sprint_revision_tests.rs"]
mod tests;
