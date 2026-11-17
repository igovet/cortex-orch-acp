//! Generation-bound public deltas, separate from private runtime event IDs.
use super::core::{TaskSummary, Version};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case", deny_unknown_fields)]
pub enum StreamKey {
    Project {
        project_id: String,
    },
    Machine {
        channel: String,
    },
    Repository {
        project_id: String,
        checkout_id: String,
    },
    Terminal {
        terminal_id: String,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Task,
    Execution,
    Decision,
    Result,
    Project,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum PublicEntity {
    Task(TaskSummary),
    TaskProjection(super::read::TaskProjection),
    Execution {
        id: String,
        version: Version,
        state: super::core::TaskState,
    },
    Decision {
        id: String,
        version: Version,
        pending: bool,
    },
    Result {
        id: String,
        version: Version,
        available: bool,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum DeltaOperation {
    Upsert(PublicEntity),
    Remove,
    Invalidate,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityChange {
    pub kind: EntityKind,
    pub id: String,
    pub base_version: Version,
    pub new_version: Version,
    pub operation: DeltaOperation,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeltaBatch {
    pub generation: String,
    pub stream_key: StreamKey,
    pub from_sequence: Version,
    pub through_sequence: Version,
    pub changes: Vec<EntityChange>,
}
impl DeltaBatch {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.generation.is_empty()
            || self.from_sequence.get() == 0
            || self.through_sequence < self.from_sequence
            || self.changes.is_empty()
        {
            return Err("invalid_delta");
        }
        for c in &self.changes {
            super::core::identity(&c.id)?;
            if c.new_version <= c.base_version {
                return Err("invalid_delta_version");
            }
            if let DeltaOperation::Upsert(ref entity) = c.operation {
                let (kind, id, version) = match entity {
                    PublicEntity::Task(t) => (EntityKind::Task, &t.id, t.version),
                    PublicEntity::TaskProjection(t) => {
                        crate::validate(
                            "Task",
                            &serde_json::to_value(t).map_err(|_| "invalid_task")?,
                        )
                        .map_err(|_| "invalid_task")?;
                        (EntityKind::Task, &t.task_id, t.version)
                    }
                    PublicEntity::Execution { id, version, .. } => {
                        (EntityKind::Execution, id, *version)
                    }
                    PublicEntity::Decision { id, version, .. } => {
                        (EntityKind::Decision, id, *version)
                    }
                    PublicEntity::Result { id, version, .. } => (EntityKind::Result, id, *version),
                };
                if kind != c.kind || id != &c.id || version != c.new_version {
                    return Err("conflicting_delta");
                }
            }
        }
        if serde_json::to_vec(self).map_err(|_| "invalid_delta")?.len() > 64 * 1024 {
            return Err("page_too_large");
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Subscribe {
    pub stream_key: StreamKey,
    pub snapshot_token: Option<String>,
    pub after_cursor: Option<Version>,
    pub generation: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResyncRequired {
    pub stream_key: StreamKey,
    pub generation: String,
    pub reason: String,
}
