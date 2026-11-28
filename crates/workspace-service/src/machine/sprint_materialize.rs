//! Create tasks from a sprint plan: one owner action, one visible transition.
//!
//! Every task of the plan, its sprint membership, title and dependencies, and the
//! sprint's own `created` state commit together or not at all. The drafts are
//! written in one Core transaction and the saved sprint record is replaced inside
//! that same transaction, under the machine lock. A task's identity is derived from
//! the sprint, the plan version and the plan key, so a retry of the same plan never
//! creates a second set; a plan that is already created answers with its tasks.
use super::{
    context,
    native::*,
    settings::{self, Lock},
};
use cortex_core::{workspace as core_workspace, CoreResult, OpenMode};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::Path;

const PUBLICATION: &str = "sprintMaterializationInFlight";
const OPERATION: &str = "internal.sprint.materialize";
const REVISION_OPERATION: &str = "internal.sprint.revision";
const CHANGED_FIELDS: &[&str] = &[
    "planning", "materializedVersion", "materializedTaskIds", "taskIds",
    "planningAttempt", "planningFailure",
];

fn unconfirmed() -> Refused {
    refuse(
        "machine_outcome_unknown",
        "The saved sprint's tasks could not all be confirmed. Refresh the project before trying again.",
    )
}

fn digest(value: &Value) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(value.to_string().as_bytes()))
}

/// A product file may have been durably replaced before its SQLite transaction
/// committed. Project readers use the receipt in the same transaction as the
/// drafts and outbox rows to decide which snapshot is visible. Writers hold
/// the machine lock, so their next save naturally removes the publication marker.
pub(crate) fn resolve_publication(project: &str, product: &mut Value) -> Result<()> {
    resolve_publication_with(product, |marker| {
        let root = Path::new(marker["root"].as_str().ok_or_else(unconfirmed)?);
        let store = context::open_now(root, OpenMode::LiveReadOnly)?.ok_or_else(unconfirmed)?;
        store.read(|db| publication_receipt(db, project, marker))
            .map_err(|_| unconfirmed())
    })
}

pub(super) fn resolve_publication_on(
    db: &rusqlite::Connection,
    project: &str,
    product: &mut Value,
) -> CoreResult<()> {
    resolve_publication_with(product, |marker| {
        publication_receipt(db, project, marker).map_err(|_| unconfirmed())
    }).map_err(|err| cortex_core::CoreError::Refused(err.code))
}

fn publication_receipt(db: &rusqlite::Connection, project: &str, marker: &Value) -> CoreResult<Option<String>> {
    let unknown = || cortex_core::CoreError::Refused("machine_outcome_unknown");
    let token = marker["token"].as_str().ok_or_else(unknown)?;
    let root = marker["root"].as_str().ok_or_else(unknown)?;
    let actor = crate::actor();
    let bound: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM workspace_projects WHERE project_id=? AND project_root=? AND activated_by=?)",
        (project, root, &actor),
        |row| row.get(0),
    )?;
    if !bound {
        return Err(unknown());
    }
    let operation = match marker["operation"].as_str() {
        None if marker["operation"].is_null() => OPERATION,
        Some(REVISION_OPERATION) => REVISION_OPERATION,
        _ => return Err(unknown()),
    };
    let receipt = db.query_row(
        "SELECT digest FROM workspace_requests WHERE project_id=? AND actor_id=? AND request_id=? AND operation=?",
        (project, &actor, token, operation),
        |row| row.get(0),
    ).optional()?;
    if receipt.is_none() {
        let ids = marker["taskIds"].as_array().ok_or_else(unknown)?;
        let mut exists = db.prepare("SELECT EXISTS(SELECT 1 FROM workspace_tasks WHERE task_id=?)")?;
        for id in ids {
            let id = id.as_str().ok_or_else(unknown)?;
            if exists.query_row([id], |row| row.get::<_, bool>(0))? {
                return Err(unknown());
            }
        }
        if operation == REVISION_OPERATION {
            let versions = marker["draftVersions"].as_object().ok_or_else(unknown)?;
            let mut query = db.prepare("SELECT version FROM workspace_tasks WHERE task_id=? AND project_id=?")?;
            for (id, version) in versions {
                let actual: Option<i64> = query.query_row((id, project), |row| row.get(0)).optional()?;
                if actual.is_none() || actual != version.as_i64() {
                    return Err(unknown());
                }
            }
        }
    }
    Ok(receipt)
}

fn resolve_publication_with(
    product: &mut Value,
    receipt: impl FnOnce(&Value) -> Result<Option<String>>,
) -> Result<()> {
    let Some(mut marker) = product.as_object_mut().and_then(|p| p.remove(PUBLICATION)) else {
        return Ok(());
    };
    let stored = marker.as_object_mut().and_then(|marker| marker.remove("fingerprint"));
    let fingerprint = digest(&json!([product, marker]));
    if stored.as_ref().and_then(Value::as_str) != Some(fingerprint.as_str()) {
        return Err(unconfirmed());
    }
    if let Some(receipt) = receipt(&marker)? {
        return if receipt == fingerprint { Ok(()) } else { Err(unconfirmed()) };
    }
    if marker["operation"] == REVISION_OPERATION {
        return restore_revision(product, &marker);
    }
    // SQLite exposed no commit for this exact publication, so reverse only the
    // changes that this writer made. The original plan and history never moved.
    let sprint = product["sprints"].as_array_mut()
        .and_then(|sprints| sprints.iter_mut().find(|sprint| sprint["id"] == marker["sprintId"]))
        .ok_or_else(unconfirmed)?;
    let ids = sprint["taskIds"].as_array().ok_or_else(unconfirmed)?.clone();
    if marker["taskIds"].as_array() != Some(&ids) {
        return Err(unconfirmed());
    }
    let saved = sprint.as_object_mut().ok_or_else(unconfirmed)?;
    let previous = marker["previous"].as_object().ok_or_else(unconfirmed)?;
    for &key in CHANGED_FIELDS {
        if let Some(value) = previous.get(key) {
            saved.insert(key.into(), value.clone());
        } else {
            saved.remove(key);
        }
    }
    let tasks = product["tasks"].as_object_mut().ok_or_else(unconfirmed)?;
    for id in ids {
        let id = id.as_str().ok_or_else(unconfirmed)?;
        if tasks.get(id).is_none_or(|record| record["sprintId"] != marker["sprintId"]) {
            return Err(unconfirmed());
        }
        tasks.remove(id);
    }
    Ok(())
}

/// A revision changes only the live roster and its named task overlays. Retain
/// their before-images until SQLite exposes the exact matching receipt.
pub(super) fn stage_revision(
    product: &mut Value,
    root: &str,
    sprint: &str,
    token: &str,
    roster: &Value,
    new_tasks: &[String],
    previous_tasks: Value,
    draft_versions: Value,
) -> String {
    let mut marker = json!({"operation":REVISION_OPERATION,"root":root,"token":token,"sprintId":sprint,
        "taskIds":new_tasks,"previousRoster":roster,"previousTasks":previous_tasks,"draftVersions":draft_versions});
    let fingerprint = digest(&json!([product, marker]));
    marker["fingerprint"] = json!(fingerprint);
    product[PUBLICATION] = marker;
    fingerprint
}

pub(super) fn record_revision_receipt(
    db: &rusqlite::Connection,
    project: &str,
    actor: &str,
    token: &str,
    fingerprint: &str,
) -> CoreResult<()> {
    db.execute(
        "INSERT INTO workspace_requests(project_id,actor_id,request_id,operation,digest,receipt_json)
         VALUES (?,?,?,?,?,'{}')",
        (project, actor, token, REVISION_OPERATION, fingerprint),
    )?;
    Ok(())
}

pub(super) fn finish_revision(
    store: &cortex_core::Store,
    path: &Path,
    project: &str,
    product: &mut Value,
    committed: bool,
) -> Result<()> {
    if committed {
        product.as_object_mut().ok_or_else(unconfirmed)?.remove(PUBLICATION);
    } else {
        let marker = &product[PUBLICATION];
        if store.read(|db| publication_receipt(db, project, marker)).map_err(|_| unconfirmed())?.is_some() {
            return Err(unconfirmed());
        }
        resolve_publication_with(product, |_| Ok(None))?;
    }
    // Losing cleanup leaves the verifiable marker. Every reader uses the receipt
    // to select the committed or prior projection even when no writer can clean up.
    let _ = settings::write_json(path, product);
    Ok(())
}

fn restore_revision(product: &mut Value, marker: &Value) -> Result<()> {
    let sprint = product["sprints"].as_array_mut()
        .and_then(|sprints| sprints.iter_mut().find(|sprint| sprint["id"] == marker["sprintId"]))
        .ok_or_else(unconfirmed)?;
    let added = marker["taskIds"].as_array().ok_or_else(unconfirmed)?;
    let before = marker["previousRoster"].as_array().ok_or_else(unconfirmed)?;
    let roster = sprint["taskIds"].as_array().ok_or_else(unconfirmed)?;
    if roster.len() != before.len() + added.len() || !roster.starts_with(before) || !roster.ends_with(added) {
        return Err(unconfirmed());
    }
    sprint["taskIds"] = marker["previousRoster"].clone();
    let tasks = product["tasks"].as_object_mut().ok_or_else(unconfirmed)?;
    for id in added {
        let id = id.as_str().ok_or_else(unconfirmed)?;
        if tasks.get(id).is_none_or(|record| record["sprintId"] != marker["sprintId"]) {
            return Err(unconfirmed());
        }
        tasks.remove(id);
    }
    let previous = marker["previousTasks"].as_object().ok_or_else(unconfirmed)?;
    for (id, record) in previous {
        if record["sprintId"] != marker["sprintId"]
            || tasks.get(id).is_none_or(|current| current["sprintId"] != marker["sprintId"])
        {
            return Err(unconfirmed());
        }
        tasks.insert(id.clone(), record.clone());
    }
    Ok(())
}

/// The identity of the task a plan item becomes: the same sprint, plan version and
/// key always name the same task, so no retry can create it twice.
pub(super) fn task_id(project: &str, sprint: &str, version: u64, key: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(
        format!("cortex-sprint-task\0{project}\0{sprint}\0{version}\0{key}").as_bytes(),
    );
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("wt_{}", &hex[..32])
}

/// The owner-chosen sprint route every task of the sprint uses, merged over the
/// default mode and language; `None` when each task gets its automatic route.
/// The sprint's `settings.model` is always the planning conversation's model; it
/// is a route for the tasks only when the owner picked it (`taskRoute: "sprint"`).
/// A sprint saved without `taskRoute` (an older client) keeps the earlier rule:
/// any saved model applies to every task.
pub(super) fn owner_sprint_route(sprint: &Value) -> Option<Value> {
    if sprint["taskRoute"] == "automatic" {
        return None;
    }
    sprint["settings"]
        .as_object()
        .filter(|s| {
            s.get("model")
                .and_then(Value::as_str)
                .is_some_and(|m| !m.is_empty())
        })
        .map(|s| {
            let mut merged = json!({"mode":"guarded","language":"auto"});
            for (k, v) in s {
                merged[k] = v.clone();
            }
            merged
        })
}

struct Item<'a> {
    key: &'a str,
    description: &'a str,
    title: Option<String>,
    depends: Vec<&'a str>,
    checkout: Option<&'a Value>,
    complexity: &'a str,
    produces: Vec<cortex_core::deliverables::Output>,
    consumes: Vec<cortex_core::deliverables::Input>,
}

fn invalid_plan(detail: impl std::fmt::Display) -> Refused {
    refuse(
        "invalid_plan",
        format!("The plan cannot become tasks: {detail}. Edit the plan and try again."),
    )
}

/// The plan's items in the order an automatic run would use: dependencies first,
/// plan order breaking ties. Unknown, repeated or circular keys refuse the plan.
fn ordered(items: &[Value]) -> Result<Vec<Item<'_>>> {
    if items.is_empty() {
        return Err(invalid_plan("it has no tasks"));
    }
    crate::native_driver::sprints::check_deliverables(items)
        .map_err(|error| refuse(error.code, &error.detail))?;
    let mut parsed = Vec::with_capacity(items.len());
    let mut keys = HashSet::new();
    for (n, item) in items.iter().enumerate() {
        let key = item["key"]
            .as_str()
            .filter(|k| !k.trim().is_empty())
            .ok_or_else(|| invalid_plan(format!("task {} has no key", n + 1)))?;
        if !keys.insert(key) {
            return Err(invalid_plan(format!("two tasks share the key {key}")));
        }
        let description = item["description"]
            .as_str()
            .filter(|d| !d.trim().is_empty())
            .ok_or_else(|| invalid_plan(format!("task {} has no description", n + 1)))?;
        let depends = match &item["dependsOn"] {
            Value::Null => Vec::new(),
            Value::Array(deps) => deps
                .iter()
                .map(|d| {
                    d.as_str().ok_or_else(|| {
                        invalid_plan(format!("task {} has an unreadable dependency", n + 1))
                    })
                })
                .collect::<Result<Vec<_>>>()?,
            _ => {
                return Err(invalid_plan(format!(
                    "task {} has unreadable dependencies",
                    n + 1
                )))
            }
        };
        let title = match &item["title"] {
            Value::String(t) if !t.trim().is_empty() && t.trim() != description.trim() => {
                Some(t.trim().chars().take(200).collect())
            }
            _ => None,
        };
        let complexity = match &item["complexity"] {
            Value::Null => "normal",
            Value::String(c) if ["low", "normal", "high"].contains(&c.as_str()) => c.as_str(),
            _ => {
                return Err(invalid_plan(format!(
                    "task {} has an unknown complexity",
                    n + 1
                )))
            }
        };
        parsed.push(Item {
            key,
            description,
            title,
            depends,
            complexity,
            checkout: item.get("checkout").filter(|c| !c.is_null()),
            produces: cortex_core::deliverables::outputs(item).expect("validated outputs"),
            consumes: cortex_core::deliverables::inputs(item).expect("validated inputs"),
        });
    }
    let graph: Vec<_> = parsed.iter().map(|item| (item.key, item.depends.clone())).collect();
    use super::sprint_dag::Error;
    let indices = super::sprint_dag::ordered(&graph).map_err(|error| match error {
        Error::DuplicateId(key) => invalid_plan(format!("two tasks share the key {key}")),
        Error::SelfEdge(key) | Error::Missing(key, _) => invalid_plan(format!(
            "task {key} waits for a task that is not in the plan"
        )),
        Error::DuplicateEdge => invalid_plan("a task repeats the same dependency"),
        Error::Cycle => invalid_plan("its dependencies form a cycle"),
    })?;
    let mut left: Vec<_> = parsed.into_iter().map(Some).collect();
    Ok(indices.into_iter().map(|i| left[i].take().unwrap()).collect())
}

/// Earlier, partly finished Create tasks could leave tasks that name the sprint but
/// are not its members. They are never adopted (their set is unknown); they are
/// reported so the owner can delete them, and Delete sprint removes them too.
fn strays(product: &Value, sprint: &Value) -> Vec<Value> {
    let members = sprint["taskIds"].as_array().cloned().unwrap_or_default();
    product["tasks"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(id, record)| {
            record["sprintId"] == sprint["id"]
                && record["planning"] != true
                && sprint["planningTaskId"].as_str() != Some(id.as_str())
                && !members.contains(&json!(id))
        })
        .map(|(id, _)| json!(id))
        .collect()
}

fn confirm_created(root: &Path, project: &str, product: &Value, sprint: &Value) -> Result<Value> {
    let ids = sprint.get("materializedTaskIds").unwrap_or(&sprint["taskIds"]);
    let entries = ids.as_array().filter(|ids| !ids.is_empty()).ok_or_else(unconfirmed)?;
    if sprint.get("materializedTaskIds").is_some() {
        let sprint_id = sprint["id"].as_str().ok_or_else(unconfirmed)?;
        let version = sprint["materializedVersion"].as_u64().ok_or_else(unconfirmed)?;
        let items = sprint["plan"]["items"].as_array().ok_or_else(unconfirmed)?;
        let expected = ordered(items)?.into_iter()
            .map(|item| json!(task_id(project, sprint_id, version, item.key)))
            .collect::<Vec<_>>();
        if sprint["plan"]["version"] != version || *entries != expected {
            return Err(unconfirmed());
        }
    }
    let store = context::open(root, OpenMode::LiveReadOnly)?.ok_or_else(unconfirmed)?;
    let confirmed = store.read(|db| {
        let mut task = db.prepare("SELECT EXISTS(SELECT 1 FROM workspace_tasks WHERE task_id=? AND project_id=?)")?;
        let mut seen = HashSet::new();
        for id in entries {
            let Some(id) = id.as_str() else { return Ok(false) };
            if !seen.insert(id) {
                return Ok(false);
            }
            if sprint.get("materializedTaskIds").is_some() && product["tasks"][id]["purged"] == true {
                continue;
            }
            if product["tasks"][id]["sprintId"] != sprint["id"] {
                return Ok(false);
            }
            if !task.query_row((id, project), |row| row.get::<_, bool>(0))? {
                return Ok(false);
            }
        }
        Ok(true)
    }).map_err(|_| unconfirmed())?;
    if !confirmed {
        return Err(unconfirmed());
    }
    Ok(ids.clone())
}

pub(super) fn validate_items(items: &Value) -> Result<()> {
    ordered(items.as_array().ok_or_else(|| invalid_plan("it has no tasks"))?).map(|_| ())
}

#[cfg(test)]
thread_local! {
    /// Test-only failure injection: 1 fails after every draft is written, 2 after the
    /// saved sprint record is replaced (as a failed commit would).
    pub(super) static FAIL: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
}

/// `product.sprint` action `materialize {sprintId, planVersion, requestId?}`. The reply
/// is `{done, created, taskIds, strayTaskIds}`. A `requestId` is accepted but not needed:
/// the sprint and plan version are the action's identity, so any repeat (the same or
/// a new request) answers with the same tasks and `created: 0`.
#[cfg(test)]
pub fn materialize(root: &Path, project: &str, state: &Path, p: &Value) -> Result<Value> {
    materialize_routed(root, project, state, p, &|_| None)
}

/// [`materialize`] with the runtime's automatic route of each task: without an
/// owner-chosen sprint model, `routes(complexity)` names the profile route and
/// effort of a task of that complexity (see `RouteResolver::sprint_route`); a
/// task it names no route for starts on the owner's default coordinator.
pub fn materialize_routed(
    root: &Path,
    project: &str,
    state: &Path,
    p: &Value,
    routes: &dyn Fn(&str) -> Option<Value>,
) -> Result<Value> {
    let sprint_id = string(p, "sprintId")?;
    let version = p["planVersion"]
        .as_u64()
        .filter(|v| *v > 0)
        .ok_or_else(|| {
            refuse(
                "invalid_request",
                "Name the plan version to create tasks from.",
            )
        })?;
    let _lock = Lock::new(state, "machine.lock")?;
    let path = product_path(state, project)?;
    let prior = settings::read_json(&path, json!({}))?;
    let mut product = product(state, project)?;
    let index = super::sprint_live::sprint_index(&product, project, sprint_id)?;
    let sprint = product["sprints"][index].clone();
    if !sprint["archivedAt"].is_null() || sprint["deleting"] == true {
        return Err(refuse(
            "sprint_closed",
            "This sprint is closed and read-only.",
        ));
    }
    if !sprint["materializedVersion"].is_null() {
        // Created already: the same plan answers with its tasks, never a second set.
        if sprint["materializedVersion"].as_u64() == Some(version) {
            let ids = confirm_created(root, project, &product, &sprint)?;
            return Ok(json!({"done":true,"created":0,"taskIds":ids,
                "strayTaskIds":strays(&product, &sprint)}));
        }
        return Err(refuse(
            "sprint_started",
            format!(
                "Tasks were already created from Plan v{}.",
                sprint["materializedVersion"]
            ),
        ));
    }
    let current = sprint["plan"]["version"].as_u64();
    if current != Some(version) {
        return Err(refuse(
            "stale_plan",
            match current {
                Some(v) => format!("The plan changed to v{v}. Review it, then create its tasks."),
                None => "This sprint has no plan yet.".into(),
            },
        ));
    }
    if sprint["planning"] != "ready" {
        return Err(refuse(
            "plan_not_ready",
            match sprint["planning"].as_str() {
                Some("question") => "Answer the open planning question first.",
                Some("planning") => "The plan is still being worked on.",
                Some("cancelled") => "Planning was stopped. Resume planning first.",
                _ => "The plan is not ready to create tasks.",
            },
        ));
    }
    let items = sprint["plan"]["items"]
        .as_array()
        .ok_or_else(|| invalid_plan("it has no tasks"))?;
    let order = ordered(items)?;
    let ids: HashMap<&str, String> = order
        .iter()
        .map(|i| (i.key, task_id(project, sprint_id, version, i.key)))
        .collect();
    // The sprint's route and checkout apply to every task it creates; a sprint-wide
    // checkout wins over a plan item's own.
    let route = owner_sprint_route(&sprint);
    // Without an owner-chosen sprint model each task gets the runtime's route
    // for its complexity, inside the sprint's own mode and language.
    let assigned: Vec<Option<Value>> = order
        .iter()
        .map(|item| {
            if route.is_some() {
                return None;
            }
            routes(item.complexity).filter(|r| {
                r["routeId"].as_str().is_some_and(|m| !m.is_empty())
                    && r["effort"].as_str().is_some_and(|e| !e.is_empty())
            })
        })
        .collect();
    let options: Vec<Value> = order
        .iter()
        .zip(&assigned)
        .map(|(item, assigned)| {
            let checkout = sprint
                .get("checkout")
                .filter(|c| !c.is_null())
                .or(item.checkout)
                .map(|c| c["kind"].clone())
                .unwrap_or_else(|| json!("main"));
            let mut options = json!({"checkout":{"kind":checkout}});
            if !item.produces.is_empty() || !item.consumes.is_empty() {
                let mut consumes = item.consumes.clone();
                for input in &mut consumes {
                    input.from = ids[input.from.as_str()].clone();
                }
                options["sprintDeliverables"] =
                    json!({"sprintId":sprint_id,"produces":item.produces,"consumes":consumes});
            }
            if checkout == "worktree" && sprint["checkout"]["shared"] == true {
                options["sprintWorktree"] = json!({"sprintId":sprint_id,
                    "title":sprint["title"].as_str().filter(|t| !t.trim().is_empty()).unwrap_or("sprint")});
            }
            if let Some(route) = &route {
                options["settings"] = route.clone();
            } else if let Some(assigned) = assigned {
                let mut merged = json!({"mode":"guarded","language":"auto"});
                for (k, v) in sprint["settings"].as_object().into_iter().flatten() {
                    merged[k] = v.clone();
                }
                merged["model"] = assigned["routeId"].clone();
                merged["effort"] = assigned["effort"].clone();
                options["settings"] = merged;
            }
            options
        })
        .collect();
    let sprint_files = json!(super::sprint_files::wire(state, &super::sprint_files::references(&product, &sprint)));
    let drafts: Vec<core_workspace::BatchDraft> = order
        .iter()
        .zip(&options)
        .map(|(item, options)| core_workspace::BatchDraft {
            task_id: &ids[item.key],
            description: item.description,
            options,
            attachments: Some(&sprint_files),
        })
        .collect();
    // The records the board reads: membership, the plan's title, dependencies by
    // task identity and the sprint's target branch, then the sprint's one transition.
    let task_ids: Vec<Value> = order.iter().map(|i| json!(ids[i.key])).collect();
    if !product["tasks"].is_object() {
        return Err(refuse("state_unreadable", "Saved tasks are unreadable."));
    }
    for ((item, assigned), options) in order.iter().zip(&assigned).zip(&options) {
        let id = &ids[item.key];
        if product["tasks"].get(id).is_some() {
            return Err(refuse("payload_conflict", "A planned task identity already exists."));
        }
        let mut record = json!({});
        record["sprintId"] = json!(sprint_id);
        match assigned {
            Some(a) => {
                record["sprintRoute"] = json!({"routeId":a["routeId"],"engine":a["engine"],
                    "model":a["model"],"tier":a["tier"],"effort":a["effort"],
                    "complexity":item.complexity,"usedPercent":a["usedPercent"]});
            }
            None => {
                if let Some(r) = record.as_object_mut() {
                    r.remove("sprintRoute");
                }
            }
        }
        record["dependencies"] = json!(item.depends.iter().map(|d| &ids[d]).collect::<Vec<_>>());
        if let Some(contract) = options.get("sprintDeliverables") {
            record["produces"] = contract["produces"].clone();
            record["consumes"] = contract["consumes"].clone();
        }
        record["planning"] = json!(false);
        if let Some(title) = item
            .title
            .as_ref()
            .filter(|_| record["titleState"] != "user")
        {
            record["title"] = json!(title);
            record["titleState"] = json!("generated");
        }
        if record.get("targetBranch").is_none() || record["targetInherited"] == true {
            record["targetBranch"] = sprint["targetBranch"].clone();
            record["targetInherited"] = json!(true);
        }
        product["tasks"][id] = record;
    }
    let saved = &mut product["sprints"][index];
    saved["planning"] = json!("created");
    saved["materializedVersion"] = json!(version);
    saved["taskIds"] = json!(task_ids);
    saved["materializedTaskIds"] = json!(task_ids);
    for key in ["planningAttempt", "planningFailure"] {
        saved.as_object_mut().unwrap().remove(key);
    }
    let store = context::open(root, OpenMode::RuntimeValidate)?.ok_or_else(|| {
        refuse(
            "project_store_unavailable",
            "The project store is not ready. Open the project again, then create the tasks.",
        )
    })?;
    let store_root = store.key().root().to_string_lossy().into_owned();
    let actor = crate::actor();
    let token = format!("sprint-materialize-{}", settings::unique());
    let mut previous = serde_json::Map::new();
    for &key in CHANGED_FIELDS {
        if let Some(value) = sprint.get(key) {
            previous.insert(key.into(), value.clone());
        }
    }
    let mut marker = json!({"root":store_root,"token":token,"sprintId":sprint_id,"previous":previous,
        "taskIds":task_ids});
    let fingerprint = digest(&json!([product, marker]));
    marker["fingerprint"] = json!(fingerprint);
    product[PUBLICATION] = marker;
    let wrote = std::cell::Cell::new(false);
    let result = store.write(|db| {
        let created = core_workspace::create_drafts_tx(db, &store_root, project, &actor, &drafts)?;
        if created != drafts.len() {
            // A confirmed retry returned above. An identity already present here
            // has no confirmed materialization binding and must not be adopted.
            return Err(cortex_core::CoreError::Refused("payload_conflict"));
        }
        #[cfg(test)]
        if FAIL.with(|f| f.get()) == 1 {
            return Err(cortex_core::CoreError::Refused("injected_failure"));
        }
        db.execute(
            "INSERT INTO workspace_requests(project_id,actor_id,request_id,operation,digest,receipt_json) VALUES (?,?,?,?,?,'{}')",
            (project, &actor, &token, OPERATION, &fingerprint),
        )?;
        wrote.set(true);
        settings::write_json(&path, &product)
            .map_err(|e| cortex_core::CoreError::Refused(e.code))?;
        #[cfg(test)]
        if FAIL.with(|f| f.get()) == 3 {
            std::process::exit(83);
        }
        #[cfg(test)]
        if FAIL.with(|f| f.get()) == 2 {
            return Err(cortex_core::CoreError::Refused("injected_failure"));
        }
        Ok(created)
    });
    match result {
        Ok(created) => {
            #[cfg(test)]
            if FAIL.with(|f| f.get()) == 4 {
                std::process::exit(84);
            }
            product.as_object_mut().unwrap().remove(PUBLICATION);
            // Compact the committed projection. A lost cleanup still leaves the
            // original, verifiable marker and the durable SQLite receipt.
            let _ = settings::write_json(&path, &product);
            Ok(json!({"done":true,"created":created,"taskIds":task_ids,
                "strayTaskIds":strays(&product, &product["sprints"][index])}))
        }
        Err(error) => {
            // The drafts rolled back; the saved sprint goes back to the plan it had.
            if wrote.get() {
                let committed = store.read(|db| publication_receipt(db, project, &product[PUBLICATION]))
                    .map_err(|_| unconfirmed())?;
                if committed.is_some() || settings::write_json(&path, &prior).is_err() {
                    return Err(unconfirmed());
                }
            }
            Err(if error.code() == "unauthorized" {
                refuse(
                    "project_store_unavailable",
                    "The project store is not bound to this project. Open the project again, then create the tasks.",
                )
            } else {
                context::core(error)
            })
        }
    }
}

#[cfg(test)]
#[path = "sprint_materialize_tests.rs"]
mod tests;
