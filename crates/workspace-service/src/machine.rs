//! Machine facts and owner actions, one bounded helper run per call. The client names an operation
//! and a registered project; the root and state directory come from this service, never from JSON.
use crate::local::failure;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{io, process::Stdio, time::Duration};
use tokio::{io::AsyncWriteExt, process::Command, sync::Semaphore};

/// Operations with no project, then those that need a registered one.
const MACHINE_OPERATIONS: &[&str] = &[
    "overview",
    "logs.folder",
    "maintenance",
    "workflow.describe",
    "engine.connection",
    "machine.update",
    "diagnostics.run",
    "cleanup.run",
    "config.read",
    "config.save",
    "config.action",
    "config.mcp.discover",
];
const PROJECT_OPERATIONS: &[&str] = &[
    "project.config.read",
    "project.config.save",
    "repo.snapshot",
    "repo.file",
    "repo.file.save",
    "repo.locate",
    "git",
    "git_init",
    "workers",
    "product.read",
    "product.task",
    "product.integrate",
    "product.sprint",
    "product.message",
    "product.comment",
];
/// Git over a network and engine discovery can be slow, and they still answer inside the bridge's own deadline.
const HELPER_DEADLINE: Duration = Duration::from_secs(30);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MachineCall {
    op: String,
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    params: Value,
}

pub struct Helper {
    reads: Semaphore,
    inspections: Semaphore,
    actions: Semaphore,
}
impl Default for Helper {
    fn default() -> Self {
        Self {
            reads: Semaphore::new(4),
            inspections: Semaphore::new(2),
            actions: Semaphore::new(4),
        }
    }
}

impl crate::Service {
    pub async fn machine(&self, call: MachineCall) -> io::Result<Value> {
        self.accepting()?;
        let (python, tools) = self
            .config
            .launch
            .tools()
            .ok_or_else(|| failure("machine_tools_unavailable"))?;
        let scoped = PROJECT_OPERATIONS.contains(&call.op.as_str());
        if !scoped && !MACHINE_OPERATIONS.contains(&call.op.as_str()) {
            return Err(failure("unsupported_operation"));
        }
        let (root, projects) = {
            let state = self.state.lock().await;
            let root = match (&call.project_id, scoped) {
                (Some(id), true) => Some(
                    state
                        .projects
                        .iter()
                        .find(|p| &p.project_id == id)
                        .ok_or_else(|| failure("project_not_found"))?
                        .canonical_root
                        .clone(),
                ),
                (None, true) => return Err(failure("invalid_project")),
                _ => None,
            };
            let projects: Vec<_> = state
                .projects
                .iter()
                .map(|p| json!({"id":p.project_id,"root":p.canonical_root,"name":p.display_name}))
                .collect();
            (root, projects)
        };
        if let Some(root) = &root {
            crate::project_guard::check(root)?;
        }
        let request = serde_json::to_vec(&json!({
            "op":call.op,"projectId":call.project_id,"root":root,"params":call.params,
            "stateDir":self.config.state_dir,"projects":projects,
        }))
        .map_err(|_| failure("invalid_json"))?;
        // One deadline for the queue and the run: a saturated helper answers busy, inside the
        // bridge's own deadline, instead of the bridge giving up on the service.
        let deadline = tokio::time::Instant::now() + HELPER_DEADLINE;
        // Slow Git/disk/config inspections cannot exhaust the task-read or owner-action lane.
        let slots = match call.op.as_str() {
            "product.read" | "workers" | "repo.file" | "repo.locate" | "workflow.describe" => {
                &self.helper.reads
            }
            "repo.snapshot"
            | "overview"
            | "config.read"
            | "project.config.read"
            | "config.mcp.discover" => &self.helper.inspections,
            _ => &self.helper.actions,
        };
        let _slot = tokio::time::timeout_at(deadline, slots.acquire())
            .await
            .map_err(|_| failure("machine_busy"))?
            .map_err(|_| failure("service_stopping"))?;
        let mut command = Command::new(python);
        command
            .args(["-I", "-B"])
            .arg(tools)
            .current_dir("/")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        crate::child::runtime_environment(&mut command, &self.config)?;
        let mut process = command.spawn()?;
        let mut input = process
            .stdin
            .take()
            .ok_or_else(|| failure("machine_tool_failed"))?;
        let write = async move {
            input.write_all(&request).await?;
            input.shutdown().await
        };
        let run = async {
            let (written, finished) = tokio::join!(write, process.wait_with_output());
            written?;
            finished
        };
        let output = tokio::time::timeout_at(deadline, run)
            .await
            .map_err(|_| failure("machine_tool_timeout"))??;
        if output.stdout.len() > workspace_protocol::FRAME_BYTES - 1024 {
            return Err(failure("machine_reply_too_large"));
        }
        serde_json::from_slice(&output.stdout).map_err(|_| failure("machine_tool_failed"))
    }
}
