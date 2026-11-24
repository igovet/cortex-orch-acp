//! Private Workspace conversation/sprint JSON only. Runtime task edits and acceptance stay Core-owned.
use super::{
    attachment_files as files,
    native::*,
    settings::{self, Lock},
};
use crate::native_driver::sprints::PlanningFailure;
use serde_json::{json, Value};
use std::path::Path;
/// A sprint's saved files together: at most 8 references totaling 4 MiB.
fn saved_total(items: &[Value]) -> Result<()> {
    let size: u64 = items.iter().map(|i| i["size"].as_u64().unwrap_or(0)).sum();
    if items.len() > 8 || size > files::MAX_BYTES as u64 {
        return Err(invalid_attachments());
    }
    Ok(())
}
fn invalid_attachments() -> Refused {
    refuse("invalid_attachments","Choose valid files: at most 8 files totaling 4 MiB, with readable contents and simple filenames.")
}
pub fn attachments(items: &Value) -> Result<()> {
    checked_attachments(items, false)
}
pub(super) fn checked_attachments(items: &Value, draft_edit: bool) -> Result<()> {
    let invalid_attachments = || {
        if draft_edit {
            refuse("invalid_attachment", "Invalid attachment.")
        } else {
            invalid_attachments()
        }
    };
    workspace_protocol::validate("Attachments", items).map_err(|_| invalid_attachments())?;
    let mut total = 0usize;
    for item in items.as_array().ok_or_else(invalid_attachments)? {
        let name = string(item, "name")?;
        let mime = string(item, "mimeType")?;
        let data = string(item, "data")?.as_bytes();
        if name.chars().any(|c| {
            c == '/'
                || c == '\\'
                || if draft_edit {
                    c <= '\u{1f}' || c == '\u{7f}'
                } else {
                    c.is_control()
                }
        }) {
            return Err(invalid_attachments());
        }
        let Some((left, right)) = mime.split_once('/') else {
            return Err(invalid_attachments());
        };
        if [left, right].iter().any(|s| {
            s.is_empty()
                || !s
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b".+_-".contains(&c))
        }) {
            return Err(invalid_attachments());
        }
        if data.len() % 4 != 0 {
            return Err(invalid_attachments());
        }
        let sextet = |b: u8| -> Option<u8> {
            match b {
                b'A'..=b'Z' => Some(b - b'A'),
                b'a'..=b'z' => Some(b - b'a' + 26),
                b'0'..=b'9' => Some(b - b'0' + 52),
                b'+' => Some(62),
                b'/' => Some(63),
                _ => None,
            }
        };
        for (i, chunk) in data.chunks_exact(4).enumerate() {
            let last = (i + 1) * 4 == data.len();
            let (Some(_), Some(b)) = (sextet(chunk[0]), sextet(chunk[1])) else {
                return Err(invalid_attachments());
            };
            if chunk[2] == b'=' {
                if !last || chunk[3] != b'=' || b & 15 != 0 {
                    return Err(invalid_attachments());
                }
                total += 1;
            } else {
                let Some(c) = sextet(chunk[2]) else {
                    return Err(invalid_attachments());
                };
                if chunk[3] == b'=' {
                    if !last || c & 3 != 0 {
                        return Err(invalid_attachments());
                    }
                    total += 2;
                } else {
                    if sextet(chunk[3]).is_none() {
                        return Err(invalid_attachments());
                    }
                    total += 3;
                }
            }
        }
    }
    if total > 4 * 1024 * 1024 {
        return Err(if draft_edit {
            refuse(
                "attachments_too_large",
                "Attachments exceed the byte limit.",
            )
        } else {
            invalid_attachments()
        });
    }
    Ok(())
}
fn trim_retained(product: &mut Value, limit: usize) {
    let mut size = product.to_string().len();
    if let Some(rows) = product["messages"].as_array_mut() {
        for row in rows {
            if size <= limit {
                break;
            }
            if row["delivery"] == "recorded" {
                let before = row["attachments"].to_string().len();
                if let Some(items) = row["attachments"].as_array_mut() {
                    for item in items {
                        if item.is_object() {
                            *item = item["name"].clone();
                        }
                    }
                }
                size = size - before + row["attachments"].to_string().len();
            }
        }
    }
}
/// Saves under the held machine lock; bodies the save stopped referencing are swept.
fn persist(
    state: &Path,
    project: &str,
    product: &mut Value,
    before: &std::collections::HashSet<String>,
) -> Result<()> {
    trim_retained(product, FRAME_BUDGET);
    settings::write_json(&product_path(state, project)?, product)?;
    files::release(state, before, product);
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;

    /// L2: a rewrite of the same condition keeps the hold's identity; any other hold,
    /// even with the same code and task, is a new one.
    #[test]
    fn a_queue_hold_keeps_its_identity_only_while_it_is_the_same_hold() {
        let hold = |reason: &str| {
            json!({"kind":"settlement_blocked","code":"task_failed","taskId":"t","reason":reason,
                "nextActor":"owner","ownerActions":["allow_repair","close_as_is"]})
        };
        let first = with_hold_id(&Value::Null, hold("Failed."));
        let id = first["holdId"].as_str().unwrap().to_owned();
        let mut decided = hold("Failed.");
        decided["ownerActions"] = json!(["close_as_is"]);
        assert_eq!(with_hold_id(&first, decided)["holdId"], id.as_str());
        let other = with_hold_id(&first, hold("Failed again."));
        assert_ne!(other["holdId"], id.as_str());
        let mut moved = hold("Failed.");
        moved["taskId"] = json!("u");
        assert_ne!(with_hold_id(&first, moved)["holdId"], id.as_str());
    }

    #[test]
    fn sprint_run_modes_migrate_legacy_product_without_changing_queue_state() {
        use std::os::unix::fs::DirBuilderExt;
        let state = std::env::temp_dir().join(format!("cw-sprint-modes-{}", settings::unique()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&state)
            .unwrap();
        let path = product_path(&state, "p").unwrap();
        let legacy = json!({"tasks":{"task":{"status":"ready_to_merge","acceptance":{"automatic":true}}},
            "sprints":[{"id":"old","runMode":"automatic","queueState":"running","taskIds":["task"],"plan":{"version":2}},
                {"id":"manual","runMode":"manual","queueState":"idle"}]});
        settings::write_json(&path, &legacy).unwrap();
        let loaded = product(&state, "p").unwrap();
        let mut expected = legacy.clone();
        expected["sprints"][0]["runMode"] = json!("hybrid");
        assert_eq!(loaded["tasks"], expected["tasks"]);
        assert_eq!(loaded["sprints"], expected["sprints"]);
        assert_eq!(settings::read_json(&path, json!({})).unwrap(), legacy);
        assert!(queue_driven_mode(&loaded["sprints"][0]));
        assert!(!queue_driven_mode(&loaded["sprints"][1]));

        let created = sprint(
            &state,
            "p",
            &json!({"action":"create","description":"New","runMode":"automatic"}),
            None,
        )
        .unwrap();
        let loaded = product(&state, "p").unwrap();
        assert_eq!(loaded["sprints"][0]["runMode"], "hybrid");
        assert_eq!(loaded["sprints"][2]["id"], created["sprintId"]);
        assert_eq!(loaded["sprints"][2]["runMode"], "automatic");
        assert!(queue_driven_mode(&loaded["sprints"][2]));
        assert!(!queue_driven_mode(
            &json!({"runMode":"hybrid","archivedAt":1})
        ));
        std::fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn sprint_run_modes_validate_create_and_patch_before_persisting() {
        use std::os::unix::fs::DirBuilderExt;
        let state = std::env::temp_dir().join(format!("cw-sprint-enum-{}", settings::unique()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&state)
            .unwrap();
        let created = sprint(
            &state,
            "p",
            &json!({"action":"create","description":"Default"}),
            None,
        )
        .unwrap();
        let id = created["sprintId"].clone();
        assert_eq!(
            product(&state, "p").unwrap()["sprints"][0]["runMode"],
            "manual"
        );
        for mode in ["manual", "hybrid", "automatic"] {
            sprint(
                &state,
                "p",
                &json!({"action":"create","description":"Valid","runMode":mode}),
                None,
            )
            .unwrap();
            sprint(
                &state,
                "p",
                &json!({"action":"patch","sprintId":id,"patch":{"runMode":mode}}),
                None,
            )
            .unwrap();
            assert_eq!(product(&state, "p").unwrap()["sprints"][0]["runMode"], mode);
        }
        let before = product(&state, "p").unwrap();
        for mode in [
            json!("unknown"),
            json!(null),
            json!(false),
            json!(["hybrid"]),
        ] {
            assert_eq!(
                sprint(
                    &state,
                    "p",
                    &json!({"action":"create","description":"Invalid","runMode":mode}),
                    None,
                )
                .unwrap_err()
                .code,
                "invalid_sprint"
            );
            assert_eq!(sprint(&state, "p", &json!({"action":"patch","sprintId":id,"patch":{"runMode":mode,"title":"Not saved"}}), None).unwrap_err().code, "invalid_sprint");
        }
        assert_eq!(product(&state, "p").unwrap(), before);
        std::fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn only_oldest_sent_bytes_give_way_and_pending_bytes_stay() {
        let file = json!({"name":"a","data":"x".repeat(200),"mimeType":"text/plain"});
        let mut p = json!({"messages":[
            {"delivery":"queued","attachments":[file.clone()]},
            {"delivery":"recorded","attachments":[file.clone()]},
            {"delivery":"recorded","attachments":[file]}]});
        let limit = p.to_string().len() - 100;
        trim_retained(&mut p, limit);
        assert!(p["messages"][0]["attachments"][0].is_object());
        assert_eq!(p["messages"][1]["attachments"][0], "a");
        assert!(p["messages"][2]["attachments"][0].is_object());
        assert!(p.to_string().len() <= limit);
    }
    /// The service takes queued messages in order; a taken one is no longer the owner's
    /// to edit, move or remove; a take repeated after a stop returns it again; settling
    /// removes it, puts it back in the queue, or marks it failed.
    #[test]
    fn queued_messages_are_taken_in_order_and_locked_once_taken() {
        use std::os::unix::fs::DirBuilderExt;
        let state = std::env::temp_dir().join(format!("cw-queue-take-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&state);
        std::fs::DirBuilder::new()
            .mode(0o700)
            .recursive(true)
            .create(&state)
            .unwrap();
        let add = |id: &str, task: &str| {
            conversation(&state, "p", &json!({"action":"add","message":{"id":id,"taskId":task,"workerId":null,"text":format!("Text {id}"),
                "mode":"queue","delivery":"queued","at":1,"deliveredAt":null,"failure":null}}), false).unwrap();
        };
        add("m1", "t");
        add("other", "u");
        add("m2", "t");
        conversation(
            &state,
            "p",
            &json!({"action":"move","messageId":"m2","before":"m1"}),
            false,
        )
        .unwrap();
        let taken = take_queued(&state, "p", "t").unwrap();
        let ids: Vec<_> = taken.iter().map(|m| m["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["m2", "m1"], "queue order, this task only");
        for action in [
            json!({"action":"edit","messageId":"m1","text":"changed"}),
            json!({"action":"move","messageId":"m1","before":null}),
            json!({"action":"remove","messageId":"m1"}),
        ] {
            assert_eq!(
                conversation(&state, "p", &action, false).unwrap_err().code,
                "message_taken"
            );
        }
        // Not settled (the service stopped): the next take returns both again, nothing new.
        assert_eq!(take_queued(&state, "p", "t").unwrap().len(), 2);
        settle_taken(&state, "p", "m2", Ok(())).unwrap();
        settle_taken(
            &state,
            "p",
            "m1",
            Err("queued_message_not_at_boundary".into()),
        )
        .unwrap();
        let rows = product(&state, "p").unwrap()["messages"].clone();
        assert!(
            !rows.as_array().unwrap().iter().any(|m| m["id"] == "m2"),
            "{rows}"
        );
        assert_eq!(
            rows.as_array()
                .unwrap()
                .iter()
                .find(|m| m["id"] == "m1")
                .unwrap()["delivery"],
            "queued"
        );
        take_queued(&state, "p", "t").unwrap();
        settle_taken(&state, "p", "m1", Err("invalid_message".into())).unwrap();
        let rows = product(&state, "p").unwrap()["messages"].clone();
        let m1 = rows
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["id"] == "m1")
            .unwrap()
            .clone();
        assert_eq!(
            (m1["delivery"].as_str(), m1["failure"].as_str()),
            (Some("failed"), Some("Not delivered: invalid_message"))
        );
        let _ = std::fs::remove_dir_all(&state);
    }
}
pub fn conversation(state: &Path, project: &str, p: &Value, comments: bool) -> Result<Value> {
    let _lock = Lock::new(state, "machine.lock")?;
    let mut product = product(state, project)?;
    let before = files::held(&product);
    let task = if comments {
        string(p, "taskId").map_err(|_| refuse("unknown_task", "Unknown task."))?
    } else {
        ""
    };
    if comments && task.is_empty() {
        return Err(refuse("unknown_task", "Unknown task."));
    }
    if comments && product["comments"].get(task).is_none() {
        product["comments"][task] = json!([]);
    }
    let rows = if comments {
        product["comments"][task].as_array_mut()
    } else {
        product["messages"].as_array_mut()
    }
    .ok_or_else(|| {
        refuse(
            "state_unreadable",
            "Saved conversation state could not be read.",
        )
    })?;
    let action = string(p, "action")?;
    if action == "add" {
        let item = &p[if comments { "comment" } else { "message" }];
        let text = item["text"].as_str().unwrap_or("");
        if !item["id"].is_string()
            || text.trim().is_empty()
            || text.chars().count() > 32000
            || !comments
                && (!["queued", "recorded"].contains(&item["delivery"].as_str().unwrap_or(""))
                    || !item["workerId"].is_null())
        {
            return Err(refuse(
                "invalid_message",
                "Write a message first, up to 32000 characters.",
            ));
        }
        files::checked_references(state, item.get("attachments").unwrap_or(&json!([])))?;
        if let Some(old) = rows.iter().find(|r| r["id"] == item["id"]) {
            if old != item {
                return Err(refuse(
                    "payload_conflict",
                    "This message identity already has different content.",
                ));
            }
        } else {
            rows.push(item.clone());
        }
    } else if action == "mark_sent" && comments {
        if let Some(ids) = p["ids"].as_array() {
            for row in rows.iter_mut() {
                if ids.contains(&row["id"]) {
                    row["sent"] = json!(true);
                }
            }
        }
    } else {
        let key = &p[if comments { "commentId" } else { "messageId" }];
        let index = rows
            .iter()
            .position(|r| r["id"] == *key)
            .ok_or_else(|| refuse("unknown_message", "This item is no longer here."))?;
        if comments && rows[index]["sent"] == true {
            return Err(refuse("already_sent", "Sent comments cannot be changed."));
        }
        // A queued message is the owner's to change until the service takes it for
        // delivery (`sending`); from then on it is the runtime's record.
        if !comments
            && (rows[index]["delivery"] == "sending"
                || action != "remove" && rows[index]["delivery"] != "queued")
        {
            return Err(refuse(
                "message_taken",
                "This message was already taken for delivery and can no longer be changed.",
            ));
        }
        match action {
            "remove" => {
                rows.remove(index);
            }
            "move" if !comments => {
                let before = &p["before"];
                if before == key {
                    return Ok(json!(rows));
                }
                if !before.is_null()
                    && !rows
                        .iter()
                        .any(|r| r["id"] == *before && r["taskId"] == rows[index]["taskId"])
                {
                    return Err(refuse(
                        "unknown_message",
                        "The destination changed or belongs to another task. Refresh the queue.",
                    ));
                }
                let item = rows.remove(index);
                let at = rows
                    .iter()
                    .position(|r| r["id"] == *before)
                    .unwrap_or(rows.len());
                rows.insert(at, item);
            }
            "edit" => {
                let text = p["text"]
                    .as_str()
                    .filter(|s| !s.trim().is_empty() && s.chars().count() <= 32000)
                    .ok_or_else(|| refuse("unsupported_action", "This action is not available."))?;
                if let Some(items) = p.get("attachments") {
                    files::checked_references(state, items)?;
                    rows[index]["attachments"] = items.clone();
                }
                rows[index]["text"] = json!(text);
            }
            _ => {
                return Err(refuse(
                    "unsupported_action",
                    "This action is not available.",
                ))
            }
        }
    }
    persist(state, project, &mut product, &before)?;
    Ok(if comments {
        product["comments"][task].clone()
    } else {
        product["messages"].clone()
    })
}
/// The service takes the owner's queued messages of one task for delivery, in queue
/// order: each becomes `sending` (no longer editable, movable or removable) and is
/// returned. Messages a previous take left `sending` (the service stopped before
/// settling them) are returned again first; delivery is idempotent on the message id.
pub fn take_queued(state: &Path, project: &str, task: &str) -> Result<Vec<Value>> {
    let _lock = Lock::new(state, "machine.lock")?;
    let mut product = product(state, project)?;
    let before = files::held(&product);
    let Some(rows) = product["messages"].as_array_mut() else {
        return Ok(Vec::new());
    };
    let mine = |r: &Value| r["taskId"] == task && r["workerId"].is_null();
    let mut taken: Vec<Value> = rows
        .iter()
        .filter(|r| mine(r) && r["delivery"] == "sending")
        .cloned()
        .collect();
    let mut changed = false;
    for row in rows
        .iter_mut()
        .filter(|r| mine(r) && r["delivery"] == "queued")
    {
        row["delivery"] = json!("sending");
        changed = true;
        taken.push(row.clone());
    }
    if changed {
        persist(state, project, &mut product, &before)?;
    }
    Ok(taken)
}
/// Settle one taken message: recorded by the runtime (kept only when it carries files,
/// whose chips the conversation shows, as after Send now), put back in the queue when
/// the task had left its boundary, or failed with the runtime's refusal.
pub fn settle_taken(
    state: &Path,
    project: &str,
    id: &str,
    outcome: std::result::Result<(), String>,
) -> Result<()> {
    let _lock = Lock::new(state, "machine.lock")?;
    let mut product = product(state, project)?;
    let before = files::held(&product);
    let Some(rows) = product["messages"].as_array_mut() else {
        return Ok(());
    };
    let Some(index) = rows
        .iter()
        .position(|r| r["id"] == id && r["delivery"] == "sending")
    else {
        return Ok(());
    };
    match outcome {
        Ok(())
            if rows[index]["attachments"]
                .as_array()
                .is_some_and(|a| !a.is_empty()) =>
        {
            rows[index]["delivery"] = json!("recorded");
        }
        Ok(()) => {
            rows.remove(index);
        }
        Err(code) if code == "queued_message_not_at_boundary" => {
            rows[index]["delivery"] = json!("queued");
        }
        Err(code) => {
            rows[index]["delivery"] = json!("failed");
            rows[index]["failure"] = json!(format!("Not delivered: {code}"));
        }
    }
    persist(state, project, &mut product, &before)
}
fn sprint_run_mode(value: &Value) -> Result<&str> {
    match value.as_str() {
        Some(mode @ ("manual" | "hybrid" | "automatic")) => Ok(mode),
        _ => Err(refuse(
            "invalid_sprint",
            "Choose Manual, Hybrid or Automatic for the sprint.",
        )),
    }
}

/// A queue hold keeps its identity (`holdId`) while the scheduler rewrites the same
/// condition (same kind, code, task and reason); any other hold is a new one. An owner
/// decision names the hold it answers, so a click on an older card never applies to
/// a newer hold that happens to share its code.
pub(super) fn with_hold_id(prior: &Value, mut pause: Value) -> Value {
    let same = ["kind", "code", "taskId", "reason"]
        .iter()
        .all(|key| prior[*key] == pause[*key]);
    pause["holdId"] = match prior["holdId"].as_str() {
        Some(id) if same => json!(id),
        _ => json!(format!("h_{}", settings::unique())),
    };
    pause
}

/// Compare the scheduler's observed sprint against the saved one so a
/// background pause/resume cannot overwrite a concurrent owner action.
pub(super) fn automatic_queue(state: &Path, project: &str, p: &Value) -> Result<Value> {
    let _lock = Lock::new(state, "machine.lock")?;
    let mut product = product(state, project)?;
    let expected = &p["expected"];
    let Some(sprint) = product["sprints"]
        .as_array_mut()
        .and_then(|sprints| sprints.iter_mut().find(|s| s["id"] == expected["id"]))
    else {
        return Ok(json!({"changed":false}));
    };
    if sprint != expected || !queue_sprint(sprint) {
        return Ok(json!({"changed":false}));
    }
    let pause = &p["pause"];
    sprint["queueState"] = json!(if pause.is_null() { "running" } else { "paused" });
    sprint["pauseReason"] = pause["reason"].clone();
    if pause.is_null() {
        sprint.as_object_mut().unwrap().remove("automaticQueue");
    } else {
        sprint["automaticQueue"] = with_hold_id(&sprint["automaticQueue"], pause.clone());
    }
    if sprint == expected {
        return Ok(json!({"changed":false}));
    }
    settings::write_json(&product_path(state, project)?, &product)?;
    Ok(json!({"changed":true}))
}

/// An open sprint whose queue the scheduler drives (Hybrid or Automatic). Both keep
/// their system holds in `automaticQueue`, decided by the same next-actor rule.
pub(super) fn queue_sprint(sprint: &Value) -> bool {
    matches!(sprint["runMode"].as_str(), Some("automatic" | "hybrid"))
        && sprint["archivedAt"].is_null()
        && (sprint["deleting"].is_null() || sprint["deleting"] == false)
}

pub(super) fn automatic_sprint(sprint: &Value) -> bool {
    sprint["runMode"] == "automatic"
        && sprint["archivedAt"].is_null()
        && (sprint["deleting"].is_null() || sprint["deleting"] == false)
}

impl crate::Service {
    // Internal driver writes share the same lock as owner changes. A diagnostic
    // and its conversation event are always persisted together.
    async fn write_planning_state(
        &self,
        project: &str,
        id: &str,
        update: impl FnOnce(&mut Value) -> Result<bool> + Send + 'static,
    ) -> std::io::Result<bool> {
        let state = self.config.state_dir.clone();
        let (project_id, id) = (project.to_owned(), id.to_owned());
        let changed = tokio::task::spawn_blocking(move || -> Result<bool> {
            let _lock = Lock::new(&state, "machine.lock")?;
            let mut product = product(&state, &project_id)?;
            let sprints = product["sprints"]
                .as_array_mut()
                .ok_or_else(|| refuse("state_unreadable", "Invalid sprint state."))?;
            let Some(sprint) = sprints.iter_mut().find(|s| s["id"] == id) else {
                return Ok(false); // Deleted since the driver's board read.
            };
            if !update(sprint)? {
                return Ok(false);
            }
            settings::write_json(&product_path(&state, &project_id)?, &product)?;
            Ok(true)
        })
        .await
        .map_err(|_| crate::failure("planning_unavailable"))?
        .map_err(|e| crate::failure(e.code))?;
        if changed {
            let _ = self
                .events
                .send(json!({"projectId":project,"type":"invalidation"}));
        }
        Ok(changed)
    }

    // Change links only while the same run is still being planned. A stopped or
    // closed sprint must not be woken by a late create or start result.
    pub(crate) async fn set_planning_task(
        &self,
        project: &str,
        id: &str,
        expected: Option<String>,
        task: Option<String>,
        retry: Option<PlanningFailure>,
    ) -> std::io::Result<bool> {
        self.write_planning_state(project, id, move |sprint| {
            if !planning_open(sprint)
                || sprint["planning"] != "planning"
                || sprint["planningTaskId"].as_str() != expected.as_deref()
            {
                return Ok(false);
            }
            sprint["planningTaskId"] = json!(task);
            sprint["planningRun"] = Value::Null;
            let saved = sprint.as_object_mut().unwrap();
            saved.remove("planningRetry");
            saved.remove("planningLimitRetry");
            if let Some(limit) = retry {
                saved.insert("planningLimitRetry".into(), json!(limit.code()));
            }
            Ok(true)
        })
        .await
    }

    pub(crate) async fn consume_planning_retry(
        &self,
        project: &str,
        id: &str,
        task: String,
    ) -> std::io::Result<bool> {
        self.write_planning_state(project, id, move |sprint| {
            Ok(planning_open(sprint)
                && sprint["planning"] == "planning"
                && sprint["planningTaskId"] == task
                && sprint
                    .as_object_mut()
                    .unwrap()
                    .remove("planningRetry")
                    .is_some())
        })
        .await
    }

    pub(crate) async fn record_planning_failure(
        &self,
        project: &str,
        id: &str,
        text: String,
        step: &'static str,
        code: &'static str,
        run: Option<Value>,
        unstarted: Option<String>,
        terminal: Option<(String, PlanningFailure)>,
    ) -> std::io::Result<()> {
        let changed = self
            .write_planning_state(project, id, move |sprint| {
                // Do not turn an owner stop, close or deletion into a failure.
                if !matches!(
                    sprint["planning"].as_str(),
                    Some("planning" | "question" | "ready")
                ) || !planning_open(sprint)
                    || unstarted
                        .as_ref()
                        .is_some_and(|task| sprint["planningTaskId"] != *task)
                    || terminal
                        .as_ref()
                        .is_some_and(|(task, _)| sprint["planningTaskId"] != *task)
                {
                    return Ok(false);
                }
                let at = now_ms();
                let mut diagnostic = json!({"text":text,"step":step,"code":code,"at":at});
                if let Some((_, failure)) = terminal {
                    let repeated = failure.is_limit() && sprint["planningLimitRetry"] == code;
                    diagnostic["kind"] = json!(failure.kind());
                    diagnostic["consecutiveLimitReplacementFailures"] = json!(u8::from(repeated));
                    diagnostic["recommendedAction"] = json!(if repeated {
                        "change_request"
                    } else {
                        "retry"
                    });
                    let next = match (repeated, failure) {
                        (true, _) => " The smaller-parts retry met the same limit. Change the request before trying again.",
                        (false, PlanningFailure::OwnerStopped) => "",
                        (false, limit) if limit.is_limit() => " Retry starts a new planning task using smaller parts from the outset.",
                        (false, _) => " Retry starts a new planning task.",
                    };
                    diagnostic["text"] = json!(format!("{text}{next}"));
                }
                append_planning_event(sprint, diagnostic["text"].as_str().unwrap(), at)?;
                let saved = sprint.as_object_mut().unwrap();
                saved.remove("planningAttempt");
                saved.remove("planningRetry");
                saved.remove("planningWarning");
                saved.remove("planningLimitRetry");
                if unstarted.is_some() {
                    saved.insert("planningTaskId".into(), Value::Null);
                    saved.insert("planningRun".into(), Value::Null);
                }
                saved.insert("planning".into(), json!("cancelled"));
                saved.insert("planningFailure".into(), diagnostic);
                if let Some(run) = run {
                    saved.insert("planningRun".into(), run);
                }
                Ok(true)
            })
            .await?;
        if changed {
            self.sprint_wake.project(project);
        }
        Ok(())
    }

    pub(crate) async fn update_planning_warning(&self, project: &str, id: &str, count: Option<u8>) {
        let written = self
            .write_planning_state(project, id, move |sprint| {
                let Some(count) = count else {
                    return Ok(sprint.as_object_mut().unwrap().remove("planningWarning").is_some());
                };
                if !sprint["planningWarning"].is_null() {
                    return Ok(false);
                }
                let text = "The service could not check or advance sprint planning. It will keep retrying.";
                let at = now_ms();
                append_planning_event(sprint, text, at)?;
                sprint["planningWarning"] = json!({"text":text,"step":"internal",
                    "code":"planning_pass_failed","at":at,"count":count});
                Ok(true)
            })
            .await;
        if written.is_err() {
            let state = self.config.state_dir.clone();
            let _ = tokio::task::spawn_blocking(move || {
                settings::service_log(&state, "scheduler_failed")
            })
            .await;
        }
    }
}

fn append_planning_event(sprint: &mut Value, text: &str, at: u64) -> Result<()> {
    sprint["conversation"]
        .as_array_mut()
        .ok_or_else(|| refuse("state_unreadable", "Invalid conversation state."))?
        .push(json!({"kind":"event","text":text,"at":at,"attachments":[]}));
    Ok(())
}

fn planning_recovery_text<'a>(p: &'a Value, key: &str) -> Result<&'a str> {
    p[key]
        .as_str()
        .filter(|text| !text.trim().is_empty() && text.chars().count() <= 32_000)
        .ok_or_else(|| {
            refuse(
                "invalid_sprint",
                "Enter non-blank sprint text of at most 32,000 characters.",
            )
        })
}

fn planning_open(sprint: &Value) -> bool {
    sprint["archivedAt"].is_null() && (sprint["deleting"].is_null() || sprint["deleting"] == false)
}

fn planning_change_required(sprint: &Value) -> bool {
    sprint["planningFailure"]["recommendedAction"] == "change_request"
}

fn planning_change_refusal() -> Refused {
    refuse(
        "planning_change_request_required",
        "The smaller-parts retry met the same limit. Change the request before trying again.",
    )
}

pub(super) fn check_planning_recovery(sprint: &Value, action: &str) -> Result<()> {
    if !planning_open(sprint)
        || sprint["planning"] != "cancelled"
        || sprint["planningFailure"].is_null()
        || !sprint["materializedVersion"].is_null()
        || (action == "replace_planning_request"
            && sprint["planningFailure"]["code"] != "planning_request_invalid"
            && !planning_change_required(sprint))
    {
        return Err(refuse(
            "planning_recovery_unavailable",
            "This sprint is not waiting for that planning recovery. Refresh the sprint.",
        ));
    }
    Ok(())
}

pub fn sprint(state: &Path, project: &str, p: &Value, root: Option<&Path>) -> Result<Value> {
    let _lock = Lock::new(state, "machine.lock")?;
    let mut product = product(state, project)?;
    let action = string(p, "action")?;
    if action == "close" {
        if let Some(root) = root {
            if let Some(store) = super::context::open(root, cortex_core::OpenMode::LiveReadOnly)? {
                // Project into a copy: only the owner disposition below is saved.
                let mut current = product.clone();
                super::task_rework::project(&store, project, &mut current, false)?;
                if current["sprints"].as_array().into_iter().flatten()
                    .find(|s| s["id"] == p["sprintId"])
                    .is_some_and(|s| super::task_rework::sprint_has_open_rework(&current, s))
                {
                    return Err(super::task_rework::error("rework_pending"));
                }
            }
        }
    }
    let before = files::held(&product);
    if action == "create" {
        let description = p["description"]
            .as_str()
            .unwrap_or("")
            .chars()
            .take(32000)
            .collect::<String>();
        let mode = p
            .get("runMode")
            .map(sprint_run_mode)
            .transpose()?
            .unwrap_or("manual");
        if description.trim().is_empty() {
            return Err(refuse("invalid_sprint", "Describe the sprint first."));
        }
        let id = format!("sp_{}", settings::unique());
        let mut sprint = json!({"id":id,"projectId":project,"description":description,"title":null,"runMode":mode,
            "targetBranch":p["targetBranch"],"settings":p["settings"],"planning":"planning","conversation":[],"plan":null,
            "history":[],"materializedVersion":null,"taskIds":[],"queueState":"idle","pauseReason":null,
            "attachments":p.get("attachments").cloned().unwrap_or_else(||json!([]))});
        if let Some(checkout) = p.get("checkout") {
            if !checkout.as_object().is_some_and(|m| {
                m.len() == 1
                    && ["main", "worktree"].contains(&checkout["kind"].as_str().unwrap_or(""))
            }) {
                return Err(refuse(
                    "invalid_checkout",
                    "Choose Project folder or Separate worktree for the sprint.",
                ));
            }
            sprint["checkout"] = checkout.clone();
        }
        if !sprint["settings"].is_null() {
            workspace_protocol::validate(
                "CreatePayload",
                &json!({"description":"sprint","settings":sprint["settings"]}),
            )
            .map_err(|_| refuse("invalid_sprint", "Invalid sprint settings."))?;
        }
        files::checked_references(state, &sprint["attachments"])?;
        product["sprints"]
            .as_array_mut()
            .ok_or_else(|| refuse("state_unreadable", "Invalid sprint state."))?
            .push(sprint);
        if mode == "automatic" {
            product["sprintRunModeVersion"] = json!(1);
        }
        persist(state, project, &mut product, &before)?;
        return Ok(json!({"sprintId":id}));
    }
    let sprints = product["sprints"]
        .as_array_mut()
        .ok_or_else(|| refuse("state_unreadable", "Invalid sprint state."))?;
    let sprint = sprints
        .iter_mut()
        .find(|s| s["id"] == p["sprintId"])
        .ok_or_else(|| refuse("unknown_sprint", "Unknown sprint."))?;
    if action == "close" {
        if p["confirmed"] != true {
            return Err(refuse(
                "confirmation_required",
                "Confirm before closing the sprint.",
            ));
        }
        if sprint["archivedAt"].is_null() {
            sprint["archivedAt"] = json!(now_ms());
            sprint["queueState"] = json!("paused");
            sprint["pauseReason"] = json!("Sprint closed by you");
            for key in [
                "pauseKind",
                "questionExecutions",
                "planningAttempt",
                "planningRetry",
                "automaticQueue",
            ] {
                sprint.as_object_mut().unwrap().remove(key);
            }
        }
        persist(state, project, &mut product, &before)?;
        return Ok(json!({"done":true}));
    }
    if !sprint["archivedAt"].is_null() {
        return Err(refuse(
            "sprint_closed",
            "This sprint is closed and read-only.",
        ));
    }
    let mut target = None;
    if action == "allow" {
        // The owner's consent to the one decision the queue hold asks for. The
        // scheduler acts on it in its next pass, through its own bounded route.
        let hold = &sprint["automaticQueue"];
        let wanted = p["ownerAction"].as_str().unwrap_or("");
        if !queue_sprint(sprint)
            || sprint["queueState"] != "paused"
            || hold["nextActor"] != "owner"
            || hold["taskId"] != p["taskId"]
            || hold["code"] != p["code"]
            // A decision that names its hold applies only to that hold. A request
            // without `holdId` (an older window) is matched by task and code alone.
            || !p["holdId"].is_null() && hold["holdId"] != p["holdId"]
            || wanted != "allow_repair"
            || !hold["ownerActions"]
                .as_array()
                .is_some_and(|actions| actions.iter().any(|a| a == wanted))
        {
            return Err(refuse(
                "automatic_hold_changed",
                "Automatic is no longer waiting for this decision. Check the task again.",
            ));
        }
        if hold["allowedAt"].is_null() {
            sprint["automaticQueue"]["allowedAt"] = json!(now_ms());
        }
    } else if action == "patch" {
        let patch = p["patch"]
            .as_object()
            .ok_or_else(|| refuse("unsupported_action", "This action is not available."))?;
        if patch.keys().any(|k| {
            ![
                "title",
                "runMode",
                "targetBranch",
                "planning",
                "plan",
                "history",
                "materializedVersion",
                "taskIds",
                "queueState",
                "pauseReason",
                "settings",
                "planningTaskId",
                "planningRun",
            ]
            .contains(&k.as_str())
        }) {
            return Err(refuse(
                "invalid_sprint",
                "That sprint change is not supported.",
            ));
        }
        if let Some(mode) = patch.get("runMode") {
            sprint_run_mode(mode)?;
            // A mode switch re-evaluates every waiting result of the project.
            super::task_review::forget_refusals(state, project);
            if sprint["runMode"] != *mode {
                sprint.as_object_mut().unwrap().remove("automaticQueue");
            }
        }
        if let Some(settings) = patch.get("settings").filter(|v| !v.is_null()) {
            workspace_protocol::validate(
                "CreatePayload",
                &json!({"description":"sprint","settings":settings}),
            )
            .map_err(|_| refuse("invalid_sprint", "Invalid sprint settings."))?;
        }
        if let Some(next) = patch.get("planning") {
            let change_required = planning_change_required(sprint);
            if next == "planning" && change_required {
                return Err(planning_change_refusal());
            }
            // A late duplicate Stop cannot discard the already-recorded limit.
            if !(next == "cancelled" && sprint["planning"] == "cancelled" && change_required) {
                let retry = next == "planning"
                    && (sprint["planning"] == "cancelled" || sprint["planningRetry"] == true)
                    && sprint["planningTaskId"].is_string()
                    && !patch.contains_key("planningTaskId");
                let saved = sprint.as_object_mut().unwrap();
                for key in ["planningAttempt", "planningFailure", "planningRetry"] {
                    saved.remove(key);
                }
                if next != "planning" || patch.contains_key("planningTaskId") {
                    saved.remove("planningLimitRetry");
                }
                if retry {
                    saved.insert("planningRetry".into(), json!(true));
                }
            }
        }
        if patch.contains_key("queueState") {
            for key in ["pauseKind", "questionExecutions", "automaticQueue"] {
                sprint.as_object_mut().unwrap().remove(key);
            }
        }
        for (key, value) in patch {
            sprint[key] = value.clone();
        }
        target = patch.get("targetBranch").cloned();
    } else if matches!(
        action,
        "recover_planning_message" | "replace_planning_request"
    ) {
        check_planning_recovery(sprint, action)?;
        // machine() supplies this checked snapshot, never trusting the caller's values.
        if p.get("expectedPlanningTaskId") != Some(&sprint["planningTaskId"])
            || p.get("expectedPlanningFailure") != Some(&sprint["planningFailure"])
        {
            return Err(refuse(
                "planning_recovery_unavailable",
                "The sprint planning state changed. Refresh before retrying.",
            ));
        }
        if action == "replace_planning_request" {
            let description = planning_recovery_text(p, "description")?;
            if planning_change_required(sprint)
                && sprint["description"].as_str().map(str::trim) == Some(description.trim())
            {
                return Err(planning_change_refusal());
            }
            sprint["description"] = json!(description);
        } else {
            let items = p.get("attachments").cloned().unwrap_or_else(|| json!([]));
            files::checked_references(state, &items)?;
            let text = p["text"]
                .as_str()
                .filter(|text| {
                    text.chars().count() <= 32_000
                        && (!text.trim().is_empty() || items != json!([]))
                })
                .ok_or_else(|| {
                    refuse(
                        "invalid_sprint",
                        "Send a message of at most 32,000 characters or attach a file.",
                    )
                })?;
            sprint["conversation"]
                .as_array_mut()
                .ok_or_else(|| refuse("state_unreadable", "Invalid conversation state."))?
                .push(json!({"kind":"message","text":text,"at":now_ms(),"attachments":items}));
        }
        // The request/note and the restart intent commit together; the next pass
        // can now create a fresh draft, without deleting the previous task.
        for key in ["planningAttempt", "planningFailure", "planningRetry", "planningLimitRetry"] {
            sprint.as_object_mut().unwrap().remove(key);
        }
        sprint["planningTaskId"] = Value::Null;
        sprint["planningRun"] = Value::Null;
        sprint["planning"] = json!("planning");
    } else if action == "note" {
        let items = p.get("attachments").cloned().unwrap_or_else(|| json!([]));
        files::checked_references(state, &items)?;
        let mut note = json!({
            "kind":p.get("kind").cloned().unwrap_or_else(||json!("note")),"text":p["text"].as_str().unwrap_or("").chars().take(32000).collect::<String>(),
            "at":now_ms(),"attachments":items});
        // A planner question keeps its answer choices as selectable options,
        // with Python planning_jobs' bounds (at most 12 non-blank, <=500 chars).
        if let Some(options) = p.get("options") {
            if note["kind"] != "question"
                || options.as_array().is_none_or(|o| {
                    o.len() > 12
                        || o.iter().any(|o| {
                            o.as_str()
                                .is_none_or(|s| s.trim().is_empty() || s.chars().count() > 500)
                        })
                })
            {
                return Err(refuse("invalid_options", "Invalid question options."));
            }
            note["options"] = options.clone();
        }
        sprint["conversation"]
            .as_array_mut()
            .ok_or_else(|| refuse("state_unreadable", "Invalid conversation state."))?
            .push(note);
    } else {
        return Err(refuse(
            "unsupported_action",
            "This action is not available.",
        ));
    }
    let mut combined = sprint["attachments"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for note in sprint["conversation"].as_array().into_iter().flatten() {
        combined.extend(note["attachments"].as_array().cloned().unwrap_or_default());
    }
    saved_total(&combined)?;
    if action == "patch" && p["patch"]["runMode"] == "automatic" {
        product["sprintRunModeVersion"] = json!(1);
    }
    if let Some(target) = target {
        for record in product["tasks"]
            .as_object_mut()
            .into_iter()
            .flat_map(|m| m.values_mut())
        {
            if record["sprintId"] == p["sprintId"] && record["targetInherited"] == true {
                record["targetBranch"] = target.clone();
            }
        }
    }
    persist(state, project, &mut product, &before)?;
    Ok(json!({"done":true}))
}
