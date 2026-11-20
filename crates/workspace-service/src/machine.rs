//! Native machine facts and owner actions, with an explicit migration port for Core-owned work. The client names an operation
//! and a registered project; the root and state directory come from this service, never from JSON.
// Action-qualified native metadata; other variants retain their original provider.
// Crate-visible only for the startup first-run catalogue observation facade.
pub(crate) mod config_action;
// Owner attachment bodies; the service boundary resolves references for Core and engines.
pub(crate) mod attachment_files;
mod close_commit;
mod config_read;
mod config_save;
mod context;
mod coordinator;
mod engine_connection;
pub(crate) mod files;
mod git;
mod integration;
mod integration_squash;
mod ledger;
mod legacy_source;
mod legacy_source_git;
pub(crate) mod maintenance;
mod mcp_discovery;
mod mcp_redact;
mod usage_refresh;
// Display activity redacts selected tool inputs with the same credential rules.
pub(crate) use mcp_redact::redact as redact_display_text;
mod native;
// The native owner read and its result identity use the same saved task records.
pub(crate) use native::{product as native_product, queue_driven_mode};
mod product;
// The service takes the owner's queued messages at a task boundary.
pub(crate) use product::{
    settle_taken as settle_taken_message, take_queued as take_queued_messages,
};
mod project_config;
mod project_config_save;
pub use project_config_save::user_defaults::update_management as update_user_defaults;
pub(crate) use project_config_save::{routing_observation, valid_cached_entry};
mod project_registry;
pub(crate) mod project_routing;
mod project_skills;
mod retention;
mod settings;
pub mod settings_api;
pub(crate) mod short_text;
mod sprint_materialize;
mod task_actions;
pub(crate) mod task_auto_repair;
pub(crate) mod task_facts;
mod task_facts_attempts;
mod task_facts_git;
mod task_facts_presented;
mod task_facts_rerun;
mod task_facts_shadow;
mod task_facts_work;
pub(crate) use task_facts_work::withdrawn as withdrawn_checks;
pub(crate) mod task_lifecycle;
mod task_merge;
mod task_resolution;
pub(crate) mod task_review;
mod worker_history;
mod workers;
mod workflow;
use crate::local::failure;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{io, time::Duration};
use tokio::sync::Semaphore;

pub(crate) fn seed_default_profile(
    bytes: &[Option<Vec<u8>>; 4],
    now: &str,
) -> io::Result<Option<Vec<u8>>> {
    config_action::seed_default_profile(bytes, now).map_err(|e| io::Error::other(e.code))
}

/// Operations that need a registered project.
const PROJECT_OPERATIONS: &[&str] = &[
    "project.config.read",
    "project.config.save",
    "repo.snapshot",
    "repo.context",
    "repo.file",
    "repo.file.save",
    "repo.locate",
    "repo.list",
    "repo.skills",
    "git",
    "git_init",
    "workers",
    "product.read",
    "product.board",
    "product.facts",
    "product.task",
    "product.integrate",
    "automatic.finish",
    "automatic.queue",
    "product.sprint",
    "product.message",
    "product.comment",
];
/// Git over a network and engine discovery can be slow, and they still answer inside the bridge's own deadline.
const HELPER_DEADLINE: Duration = Duration::from_secs(30);

/// Whether a successful call can change what a sprint does next, so the sprint loop looks at that project now.
/// Reads never do; neither does an automatic settle that accepted nothing (the loop's own retry) nor a short model text.
fn sprint_wake(call: &MachineCall) -> bool {
    match call.op.as_str() {
        "product.sprint" | "product.integrate" => true,
        // An automatic settle that did accept is decided by the reply (see `machine`).
        "product.task" => !short_text::handles(&call.params),
        _ => false,
    }
}

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
    reads: std::sync::Arc<Semaphore>,
    /// Repository and engine-login inspections: Git and process work that can run long.
    inspections: std::sync::Arc<Semaphore>,
    /// The machine's own settings (models, efforts, storage). Local file reads that must never queue behind a slow repository.
    settings: std::sync::Arc<Semaphore>,
    actions: std::sync::Arc<Semaphore>,
}
impl Default for Helper {
    fn default() -> Self {
        Self {
            reads: std::sync::Arc::new(Semaphore::new(4)),
            inspections: std::sync::Arc::new(Semaphore::new(2)),
            settings: std::sync::Arc::new(Semaphore::new(2)),
            actions: std::sync::Arc::new(Semaphore::new(4)),
        }
    }
}

impl crate::Service {
    pub async fn machine(&self, call: MachineCall) -> io::Result<Value> {
        if call.op == "product.sprint" && call.params["action"] == "delete" {
            // One bounded owner action, using the existing session cancellation route.
            // A timeout never means stopped: the durable deletion fence remains for Retry.
            if let Some(project) = call.project_id.as_deref() {
                // A planning turn of the sprint being deleted stops on the next pass.
                self.sprint_wake.project(project);
            }
            return match tokio::time::timeout(Duration::from_secs(24), self.delete_sprint(call))
                .await
            {
                Ok(reply) => reply,
                Err(_) => Ok(json!({"ok":true,"result":{"done":false,
                    "reason":"Deletion is not confirmed yet. The queue stays stopped. Retry Delete sprint."}})),
            };
        }
        let wakes = sprint_wake(&call);
        let project = call.project_id.clone();
        let call_action = call.params["action"].as_str().map(str::to_owned);
        let reply = self.machine_helper(call, false).await;
        // The sprint loop sleeps until something can move a sprint on: an owner or loop change of a sprint
        // or task record is one (a run mode, a queue state, an acceptance, a sprint membership).
        if let (true, Some(project), Ok(reply)) = (wakes, project.as_deref(), reply.as_ref()) {
            let idle_settle =
                call_action.as_deref() == Some("auto_ready") && reply["result"]["accepted"] != true;
            if reply["ok"] == true && !idle_settle {
                self.sprint_wake.project(project);
            }
        }
        reply
    }

    async fn delete_sprint(&self, call: MachineCall) -> io::Result<Value> {
        let project = call
            .project_id
            .as_deref()
            .ok_or_else(|| failure("invalid_project"))?;
        let mut params = call.params.clone();
        params["action"] = json!("delete_prepare");
        let prepared = self
            .machine_helper(
                MachineCall {
                    op: call.op.clone(),
                    project_id: call.project_id.clone(),
                    params: params.clone(),
                },
                false,
            )
            .await?;
        if prepared["ok"] != true || prepared["result"]["done"] == true {
            return Ok(prepared);
        }
        let mut stop_failed = false;
        if let Some(executions) = prepared["result"]["executionIds"].as_array() {
            for execution in executions {
                let id = execution
                    .as_str()
                    .ok_or_else(|| failure("invalid_execution"))?;
                if self
                    .control_execution(json!({"projectId":project,"executionId":id,
                    "action":"close_task","confirmed":true,"requestId":format!("delete-{}", id)}))
                    .await
                    .is_err()
                {
                    stop_failed = true;
                }
            }
        }
        if stop_failed {
            return Ok(json!({"ok":true,"result":{"done":false,
                "reason":"An execution could not be stopped or confirmed. The queue stays stopped. Retry Delete sprint."}}));
        }
        params["action"] = json!("delete_finish");
        let until = tokio::time::Instant::now() + Duration::from_secs(15);
        loop {
            let reply = self
                .machine_helper(
                    MachineCall {
                        op: call.op.clone(),
                        project_id: call.project_id.clone(),
                        params: params.clone(),
                    },
                    false,
                )
                .await?;
            if reply["ok"] != true
                || reply["result"]["done"] == true
                || tokio::time::Instant::now() >= until
            {
                return Ok(reply);
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }

    // These operations are not accepted through the machine RPC. The scheduler
    // invokes them via Rust, not by supplying a confirmation flag on the wire.
    pub(crate) async fn automatic_sprint_action(
        &self,
        project: &str,
        op: &str,
        params: Value,
    ) -> io::Result<Value> {
        let reply = self
            .machine_helper(
                MachineCall {
                    op: op.to_owned(),
                    project_id: Some(project.to_owned()),
                    params,
                },
                true,
            )
            .await?;
        if reply["result"]["changed"] == true || reply["result"]["done"] == true {
            self.sprint_wake.project(project);
            let _ = self
                .events
                .send(json!({"projectId":project,"type":"invalidation"}));
        }
        Ok(reply)
    }

    async fn machine_helper(&self, call: MachineCall, scheduler: bool) -> io::Result<Value> {
        self.accepting()?;
        // A coordinator page is the session's own retained, cursor-paged history (see
        // `coordinator::read`); it is not one of the native owner's worker rows.
        let coordinator_page = call.op == "workers"
            && call.params["workerId"]
                .as_str()
                .is_some_and(|w| w.starts_with("coordinator:"));
        if !coordinator_page
            && matches!(
                call.op.as_str(),
                "product.read" | "product.facts" | "workers"
            )
        {
            let runtime = match call.project_id.as_ref() {
                Some(id) => {
                    let state = self.state.lock().await;
                    state.native.get(id).cloned().zip(
                        state
                            .projects
                            .iter()
                            .find(|p| &p.project_id == id)
                            .map(|p| p.canonical_root.clone()),
                    )
                }
                None => None,
            };
            if let Some((runtime, root)) = runtime {
                if call.op == "product.read" && call.params["attachments"] == true {
                    let project = call.project_id.clone().unwrap();
                    let task = call.params["taskId"]
                        .as_str()
                        .ok_or_else(|| failure("invalid_task"))?
                        .to_owned();
                    let mut files = json!({"attachments":runtime
                        .draft_attachments(project, task)
                        .await
                        .map_err(|e| failure(e.code()))?});
                    // The reply names each file; its body is stored here and read on request.
                    let state = self.config.state_dir.clone();
                    let files = tokio::task::spawn_blocking(move || {
                        attachment_files::externalize(&state, &mut files)
                            .map(|_| files["attachments"].take())
                            .map_err(|e| failure(e.code))
                    })
                    .await
                    .map_err(|_| failure("machine_tool_failed"))??;
                    return Ok(json!({"ok":true,"result":files}));
                }
                if call.op == "workers" {
                    // Worker rows alone: the saved overlay, its Git facts, the folder
                    // observation and the result reviews are product facts a worker read
                    // never returns. The project/store binding is still enforced first.
                    let bound = root.clone();
                    let check = tokio::task::spawn_blocking(move || {
                        task_facts::materialized(&bound, "").map_err(|e| failure(e.code))
                    });
                    tokio::time::timeout(HELPER_DEADLINE, check)
                        .await
                        .map_err(|_| failure("machine_tool_timeout"))?
                        .map_err(|_| failure("machine_tool_failed"))??;
                    let workers = runtime
                        .owner_workers(call.project_id.as_deref().unwrap(), call.params)
                        .await
                        .map_err(|e| failure(e.code()))?;
                    return Ok(json!({"ok":true,"result":workers}));
                }
                // The saved overlay alone misses every task created since it was last
                // written (no checkout, so a project-folder task read as Ready to merge
                // after a relaunch). Read it with the store's task options, on its own
                // thread so a slow filesystem or Git never holds the async reactor.
                // The folder observation results are compared with depends on neither:
                // it runs while they are read (see `owner_facts_observed`). A recent
                // observation of the unchanged folder is reused (`recent_observation`).
                let observing = tokio::spawn(task_facts::recent_observation(root.clone()));
                let config = self.config.clone();
                let project = call.project_id.clone().unwrap();
                // A deferring read (the screen's first read of what it shows) never waits
                // for the Ready-to-merge inspection or a slow folder observation: those parts
                // say they are pending, and the screen's next read settles them.
                let defer = call.params["defer"] == true;
                let read = tokio::task::spawn_blocking(move || -> io::Result<Value> {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()?;
                    runtime
                        .block_on(task_facts::native_presented(
                            &root,
                            &config.state_dir,
                            &project,
                            &mut git::Git::for_service(&config),
                            defer,
                        ))
                        .map_err(|e| failure(e.code))
                });
                let saved = tokio::time::timeout(HELPER_DEADLINE, read)
                    .await
                    .map_err(|_| failure("machine_tool_timeout"))
                    .and_then(|r| r.map_err(|_| failure("machine_tool_failed")))
                    .and_then(|r| r);
                let saved = match saved {
                    Ok(saved) => saved,
                    Err(e) => {
                        observing.abort();
                        return Err(e);
                    }
                };
                let facts = runtime
                    .owner_facts_observed(
                        call.project_id.as_deref().unwrap(),
                        saved,
                        call.params,
                        observing,
                    )
                    .await
                    .map_err(|e| failure(e.code()))?;
                return Ok(json!({"ok":true,"result":match call.op.as_str() {
                    "product.read" => facts["product"].clone(),
                    "workers" => facts["workers"].clone(),
                    _ => facts,
                }}));
            }
        }
        let native_supported = (scheduler
            && matches!(call.op.as_str(), "automatic.finish" | "automatic.queue"))
            || native::supported(&call.op)
            || matches!(call.op.as_str(), "product.read" | "product.facts")
            || call.op == "product.task" && task_actions::supported(&call.params)
            || call.op == "config.action"
            || call.op == "engine.connection" && engine_connection::supported(&call.params);
        if !native_supported {
            return Err(failure("unsupported_operation"));
        }
        let scoped = PROJECT_OPERATIONS.contains(&call.op.as_str());
        let (root, projects) = {
            let state = self.state.lock().await;
            let root = if scoped {
                let id = call
                    .project_id
                    .as_ref()
                    .ok_or_else(|| failure("invalid_project"))?;
                Some(
                    state
                        .projects
                        .iter()
                        .find(|p| &p.project_id == id)
                        .ok_or_else(|| failure("project_not_found"))?
                        .canonical_root
                        .clone(),
                )
            } else {
                None
            };
            let projects = json!(state
                .projects
                .iter()
                .map(|p| json!({"id":p.project_id,"root":p.canonical_root,"name":p.display_name}))
                .collect::<Vec<_>>());
            (root, projects)
        };
        if let Some(root) = &root {
            crate::project_guard::check(root)?;
        }
        let deadline = tokio::time::Instant::now() + HELPER_DEADLINE;
        let slots = match call.op.as_str() {
            "overview" | "project.config.read" | "config.read" => self.helper.settings.clone(),
            "repo.snapshot" => self.helper.inspections.clone(),
            "engine.connection" if call.params["action"] == "login_command" => {
                self.helper.inspections.clone()
            }
            "repo.file" | "repo.locate" | "repo.list" | "repo.skills" | "product.read"
            | "product.facts" | "workers" => self.helper.reads.clone(),
            _ => self.helper.actions.clone(),
        };
        let slot = tokio::time::timeout_at(deadline, slots.acquire_owned())
            .await
            .map_err(|_| failure("machine_busy"))?
            .map_err(|_| failure("service_stopping"))?;
        let config = self.config.clone();
        let state = config.state_dir.clone();
        let (mut send, receive) = tokio::sync::oneshot::channel();
        // Bounded native lanes retain their permit even if a filesystem call is stuck.
        // A stuck mount cannot occupy the async reactor or delay runtime shutdown.
        std::thread::Builder::new().name("machine-native".into()).spawn(move || {
            let _slot = slot;
            let result = (|| {
                let intent = if matches!(call.op.as_str(), "git" | "git_init") {
                    if send.is_closed() { return Err(failure("machine_tool_cancelled")); }
                    match ledger::Intent::begin(&state, &call.op, call.project_id.as_deref().unwrap_or(""), root.as_deref(), &call.params) {
                        Ok(ledger::Begin::Replay(reply)) => return Ok(reply),
                        Ok(ledger::Begin::Run(intent)) => Some(intent),
                        Err(error) => return Ok(native::envelope(Err(error))),
                    }
                } else {None};
                let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
                let result = runtime.block_on(async {
                    tokio::select! {
                        reply = tokio::time::timeout_at(deadline, native::run(&call.op, root.as_deref(),
                            call.project_id.as_deref().unwrap_or(""), &state, &call.params, &projects, &config, None, deadline.into_std())) =>
                            reply.unwrap_or_else(|_|Err(native::refuse("machine_tool_timeout",
                                "The operation deadline expired. A write may have completed; inspect before retrying."))),
                        _ = send.closed() => Err(native::refuse("machine_tool_cancelled",
                            "The caller disconnected. Inspect Git before retrying a write.")),
                    }
                });
                // The retained wrapper emitted no JSON for AttributeError; clients
                // received this ordinary RPC error. Preserve that envelope without
                // crashing a helper/thread or exposing exception details.
                if result.as_ref().is_err_and(|e| e.code == "machine_tool_failed"
                    && ((call.op == "config.save" && e.message == "AttributeError")
                        || e.message == native::UNCAUGHT_ATTRIBUTE_ERROR
                        || (call.op == "engine.connection" && e.message == "InstallUnresolvable"))) {
                    return Err(failure("machine_tool_failed"));
                }
                let reply = native::envelope(result);
                if let Some(intent) = intent {
                    if intent.finish(&state, &reply).is_err() {
                        return Err(failure("machine_outcome_unknown"));
                    }
                }
                let _ = settings::service_log(&state, if reply["ok"]==true {"helper_ok"}else{"helper_refused"});
                io::Result::Ok(reply)
            })();
            let _ = send.send(result);
        })?;
        let reply = tokio::time::timeout_at(deadline, receive)
            .await
            .map_err(|_| failure("machine_tool_timeout"))?
            .map_err(|_| failure("machine_tool_failed"))??;
        Ok(reply)
    }
}

#[cfg(test)]
mod lane_tests {
    use super::*;
    use crate::{Config, LaunchSpec, Service};
    /// The first screen waits for the machine's settings. They are read from local files, so a repository inspection that holds
    /// every inspection slot (a slow Git call on a large or remote-backed project) must not make them wait or refuse.
    #[tokio::test]
    async fn settings_reads_do_not_queue_behind_busy_repository_inspections() {
        let base = std::env::temp_dir().join(format!("cw-lanes-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        std::fs::set_permissions(&base, std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .unwrap();
        let service = Service::bind(Config {
            runtime_dir: base.join("runtime"),
            state_dir: base.join("state"),
            allowed_root: base.clone(),
            launch: LaunchSpec::bare(),
            request_timeout: Duration::from_secs(30),
            shutdown_timeout: Duration::from_millis(100),
            event_capacity: 16,
        })
        .await
        .unwrap();
        let held = service
            .helper
            .inspections
            .clone()
            .acquire_many_owned(2)
            .await
            .unwrap();
        let reply = tokio::time::timeout(
            Duration::from_secs(5),
            service.machine(MachineCall {
                op: "overview".into(),
                project_id: None,
                params: json!({}),
            }),
        )
        .await
        .expect("a settings read waited for a repository inspection slot")
        .unwrap();
        assert_eq!(reply["ok"], true, "{reply}");
        drop(held);
        let _ = std::fs::remove_dir_all(&base);
    }
}
