//! Private Workspace conversation/sprint JSON only. Runtime task edits and acceptance stay Core-owned.
use super::{
    native::*,
    settings::{self, Lock},
};
use serde_json::{json, Value};
use std::path::Path;
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
fn persist(state: &Path, project: &str, product: &mut Value) -> Result<()> {
    trim_retained(product, FRAME_BUDGET);
    settings::write_json(&product_path(state, project)?, product)
}
#[cfg(test)]
mod tests {
    use super::*;
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
}
pub fn conversation(state: &Path, project: &str, p: &Value, comments: bool) -> Result<Value> {
    let _lock = Lock::new(state, "machine.lock")?;
    let mut product = product(state, project)?;
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
        attachments(item.get("attachments").unwrap_or(&json!([])))?;
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
                    attachments(items)?;
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
    persist(state, project, &mut product)?;
    Ok(if comments {
        product["comments"][task].clone()
    } else {
        product["messages"].clone()
    })
}
pub fn sprint(state: &Path, project: &str, p: &Value) -> Result<Value> {
    let _lock = Lock::new(state, "machine.lock")?;
    let mut product = product(state, project)?;
    let action = string(p, "action")?;
    if action == "create" {
        let description = p["description"]
            .as_str()
            .unwrap_or("")
            .chars()
            .take(32000)
            .collect::<String>();
        let mode = p["runMode"].as_str().unwrap_or("manual");
        if description.trim().is_empty() || !["manual", "automatic"].contains(&mode) {
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
        attachments(&sprint["attachments"])?;
        product["sprints"]
            .as_array_mut()
            .ok_or_else(|| refuse("state_unreadable", "Invalid sprint state."))?
            .push(sprint);
        persist(state, project, &mut product)?;
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
            for key in ["pauseKind", "questionExecutions", "planningAttempt"] {
                sprint.as_object_mut().unwrap().remove(key);
            }
        }
        persist(state, project, &mut product)?;
        return Ok(json!({"done":true}));
    }
    if !sprint["archivedAt"].is_null() {
        return Err(refuse(
            "sprint_closed",
            "This sprint is closed and read-only.",
        ));
    }
    let mut target = None;
    if action == "patch" {
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
            ]
            .contains(&k.as_str())
        }) {
            return Err(refuse(
                "invalid_sprint",
                "That sprint change is not supported.",
            ));
        }
        if let Some(settings) = patch.get("settings").filter(|v| !v.is_null()) {
            workspace_protocol::validate(
                "CreatePayload",
                &json!({"description":"sprint","settings":settings}),
            )
            .map_err(|_| refuse("invalid_sprint", "Invalid sprint settings."))?;
        }
        if patch.contains_key("planning") {
            for key in ["planningAttempt", "planningFailure"] {
                sprint.as_object_mut().unwrap().remove(key);
            }
        }
        if patch.contains_key("queueState") {
            for key in ["pauseKind", "questionExecutions"] {
                sprint.as_object_mut().unwrap().remove(key);
            }
        }
        for (key, value) in patch {
            sprint[key] = value.clone();
        }
        target = patch.get("targetBranch").cloned();
    } else if action == "note" {
        let items = p.get("attachments").cloned().unwrap_or_else(|| json!([]));
        attachments(&items)?;
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
    attachments(&json!(combined))?;
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
    persist(state, project, &mut product)?;
    Ok(json!({"done":true}))
}
