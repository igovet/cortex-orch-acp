//! Typed desktop read projections. Display content never conveys mutation authority.
use crate::core::{Protocol, Version};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskProjection {
    pub task_id: String,
    pub description: String,
    pub title: Option<String>,
    pub title_availability: String,
    pub version: Version,
    pub execution_id: Option<String>,
    pub phase: String,
    pub runtime_state: Option<String>,
    pub execution_state: String,
    pub wait_reason: Option<String>,
    pub last_confirmed_at: Option<String>,
    pub coordinator_route: Option<RouteLabel>,
    pub warnings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description_availability: Option<String>,
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
