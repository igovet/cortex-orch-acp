//! Typed desktop read projections. Display content never conveys mutation authority.
use crate::core::{Protocol, Version};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskProjection {
    pub task_id: String,
    /// Owning sprint of a service-created planning draft, not sprint membership.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planning_sprint_id: Option<String>,
    pub description: String,
    pub title: Option<String>,
    pub title_availability: String,
    pub version: Version,
    pub execution_id: Option<String>,
    pub phase: String,
    pub runtime_state: Option<String>,
    pub execution_state: String,
    pub wait_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files_wait: Option<FilesWait>,
    pub last_confirmed_at: Option<String>,
    pub coordinator_route: Option<RouteLabel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub automatic_continuation: Option<serde_json::Value>,
    pub warnings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description_availability: Option<String>,
}
/// Display context for a file wait with a holder in the same Workspace project.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FilesWait {
    pub holder_task_id: String,
    pub release_target: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteLabel {
    pub model: String,
    pub effort: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryRow {
    pub history_id: String,
    pub sequence: Version,
    pub revision: Version,
    pub kind: String,
    pub text: Option<String>,
    pub tool_status: Option<String>,
    pub recorded_at: String,
    pub availability: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadPage<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
    pub high_watermark: Version,
    /// History only: where a later read continues to fetch just the rows written after this page (a forward cursor
    /// past `highWatermark`). Absent on other pages and from a peer that cannot read forward.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tail_cursor: Option<String>,
    /// History read backwards (`history.earlier`) only: where the next read continues with the rows before this page's
    /// first one. Absent when the page reaches the session's first row, and on every other page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev_cursor: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum QueryReply {
    Tasks(ReadPage<TaskProjection>),
    Board(ReadPage<TaskProjection>),
    Task(TaskProjection),
    State(TaskProjection),
    History(ReadPage<HistoryRow>),
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadReply {
    pub protocol: Protocol,
    pub generation: String,
    pub project_id: String,
    pub version: Version,
    pub high_watermark: Version,
    pub data: QueryReply,
}
impl ReadReply {
    pub fn validate(&self) -> Result<(), &'static str> {
        self.protocol.validate()?;
        crate::core::identity(&self.project_id)?;
        if self.generation.is_empty() {
            return Err("invalid_generation");
        }
        let check = |task: &TaskProjection| {
            crate::validate(
                "Task",
                &serde_json::to_value(task).map_err(|_| "invalid_task")?,
            )
            .map_err(|_| "invalid_task")
        };
        match &self.data {
            QueryReply::Tasks(page) | QueryReply::Board(page) => {
                let maximum = if matches!(self.data, QueryReply::Board(_)) {
                    1000
                } else {
                    50
                };
                if page.items.len() > maximum {
                    return Err("page_too_large");
                }
                for t in &page.items {
                    check(t)?;
                }
            }
            QueryReply::Task(t) | QueryReply::State(t) => check(t)?,
            QueryReply::History(p) => {
                crate::validate(
                    "HistoryPage",
                    &serde_json::to_value(p).map_err(|_| "invalid_history")?,
                )
                .map_err(|_| "invalid_history")?;
            }
        }
        let budget = if matches!(self.data, QueryReply::Board(_)) {
            786432
        } else {
            64 * 1024
        };
        if serde_json::to_vec(self).map_err(|_| "invalid_reply")?.len() > budget {
            return Err("page_too_large");
        }
        Ok(())
    }
}

/// Whether a `machine.call` operation is an owner read: it answers the owner's screen,
/// starts no task process and changes no saved state. The service runs an owner read
/// until it completes (bounded only by its lane's concurrency), and a client waits for
/// its answer: a slow read is never turned into a timeout or a "busy" refusal.
pub fn owner_read_op(op: &str, params: &serde_json::Value) -> bool {
    matches!(
        op,
        "overview"
            | "project.config.read"
            | "config.read"
            | "repo.snapshot"
            | "repo.context"
            | "repo.file"
            | "repo.file.lease.status"
            | "repo.locate"
            | "repo.list"
            | "repo.skills"
            | "product.read"
            | "product.facts"
            | "product.board"
            | "workers"
            | "command.output"
    ) || (op == "product.sprint" && params["action"] == "live_graph")
}

/// Whether a service request is a read a client waits for without a deadline: the board
/// and history projections, a workspace query (no command payload), the project and
/// owner-prompt lists, and every `machine.call` owner read ([`owner_read_op`]).
pub fn read_request(method: &str, params: &serde_json::Value) -> bool {
    match method {
        "native.query" | "native.board" | "projects.list" | "projects.status"
        | "service.capabilities" | "askpass.list" | "owner_actions.list" | "remote.status" => true,
        "workspace" => params.get("payload").is_none(),
        "machine.call" => params["op"]
            .as_str()
            .is_some_and(|op| owner_read_op(op, &params["params"])),
        _ => false,
    }
}

#[cfg(test)]
mod read_request_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn reads_are_classified_and_actions_are_not() {
        assert!(read_request("machine.call", &json!({"op":"product.facts","projectId":"p","params":{}})));
        assert!(read_request("machine.call", &json!({"op":"workers","params":{"workerId":"coordinator:x"}})));
        assert!(read_request("machine.call", &json!({"op":"product.sprint","params":{"action":"live_graph"}})));
        assert!(read_request("workspace", &json!({"operation":"history.page"})));
        assert!(read_request("native.board", &json!({})));
        assert!(!read_request("machine.call", &json!({"op":"product.sprint","params":{"action":"delete"}})));
        assert!(!read_request("machine.call", &json!({"op":"product.task","params":{"action":"close"}})));
        assert!(!read_request("machine.call", &json!({"op":"git","params":{}})));
        assert!(!read_request("workspace", &json!({"operation":"tasks.start","payload":{}})));
        assert!(!read_request("executions.prompt", &json!({})));
        assert!(!read_request("service.status", &json!({})));
    }
}
