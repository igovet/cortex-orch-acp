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
use cortex_core::{workspace as core_workspace, OpenMode};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::Path;

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

struct Item<'a> {
    key: &'a str,
    description: &'a str,
    title: Option<String>,
    depends: Vec<&'a str>,
    checkout: Option<&'a Value>,
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
        parsed.push(Item {
            key,
            description,
            title,
            depends,
            checkout: item.get("checkout").filter(|c| !c.is_null()),
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
pub fn materialize(root: &Path, project: &str, state: &Path, p: &Value) -> Result<Value> {
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
    let index = product["sprints"]
        .as_array()
        .and_then(|s| s.iter().position(|s| s["id"] == sprint_id))
        .ok_or_else(|| refuse("unknown_sprint", "Unknown sprint."))?;
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
            return Ok(json!({"done":true,"created":0,"taskIds":sprint["taskIds"],
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
    let route = sprint["settings"]
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
        });
    let options: Vec<Value> = order
        .iter()
        .map(|item| {
            let checkout = sprint
                .get("checkout")
                .filter(|c| !c.is_null())
                .or(item.checkout)
                .map(|c| c["kind"].clone())
                .unwrap_or_else(|| json!("main"));
            let mut options = json!({"checkout":{"kind":checkout}});
            if let Some(route) = &route {
                options["settings"] = route.clone();
            }
            options
        })
        .collect();
    let drafts: Vec<core_workspace::BatchDraft> = order
        .iter()
        .zip(&options)
        .map(|(item, options)| core_workspace::BatchDraft {
            task_id: &ids[item.key],
            description: item.description,
            options,
        })
        .collect();
    // The records the board reads: membership, the plan's title, dependencies by
    // task identity and the sprint's target branch, then the sprint's one transition.
    let task_ids: Vec<Value> = order.iter().map(|i| json!(ids[i.key])).collect();
    if !product["tasks"].is_object() {
        return Err(refuse("state_unreadable", "Saved tasks are unreadable."));
    }
    for item in &order {
        let id = &ids[item.key];
        let mut record = product["tasks"]
            .get(id)
            .cloned()
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}));
        record["sprintId"] = json!(sprint_id);
        record["dependencies"] = json!(item.depends.iter().map(|d| &ids[d]).collect::<Vec<_>>());
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
    let wrote = std::cell::Cell::new(false);
    let result = store.write(|db| {
        let created = core_workspace::create_drafts_tx(db, &store_root, project, &actor, &drafts)?;
        #[cfg(test)]
        if FAIL.with(|f| f.get()) == 1 {
            return Err(cortex_core::CoreError::Refused("injected_failure"));
        }
        wrote.set(true);
        settings::write_json(&path, &product)
            .map_err(|e| cortex_core::CoreError::Refused(e.code))?;
        #[cfg(test)]
        if FAIL.with(|f| f.get()) == 2 {
            return Err(cortex_core::CoreError::Refused("injected_failure"));
        }
        Ok(created)
    });
    match result {
        Ok(created) => {
            let product = super::native::product(state, project)?;
            Ok(json!({"done":true,"created":created,"taskIds":task_ids,
                "strayTaskIds":strays(&product, &product["sprints"][index])}))
        }
        Err(error) => {
            // The drafts rolled back; the saved sprint goes back to the plan it had.
            if wrote.get() && settings::write_json(&path, &prior).is_err() {
                return Err(refuse(
                    "machine_outcome_unknown",
                    "Creating the tasks failed and the sprint could not be restored. Refresh before trying again.",
                ));
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
