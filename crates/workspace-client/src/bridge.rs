//! The bridge between a shell's web interface and the machine service: one fresh
//! connection per request, one long-lived subscription connection, the owner-facing
//! refusal texts and the subscription's event validation.
//!
//! How a connection is made is the [`Connector`]'s business: the desktop's Unix socket
//! (with its service-manager start) or the phone's tunnel over a paired machine.
use serde_json::{json, Value};
use std::{
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    time::timeout,
};

use crate::frames::{read_frame, write_frame};

fn with_refusal_text(code: String) -> String {
    match refusal(&code) {
        Some(text) => format!("{REFUSED}{text}"),
        None => code,
    }
}

/// Carry a typed store refusal across the shells' existing string error channel.
fn storage_refusal(reply: &Value) -> Option<String> {
    let error = &reply["error"];
    let data = &error["data"];
    if reply.get("result").is_some() || error["code"] != -32000 || data["kind"] != "project_store" {
        return None;
    }
    let reason = data["reason"].as_str()?;
    let message = error["message"].as_str()?;
    if reason.len() > 64
        || !reason.as_bytes().first()?.is_ascii_lowercase()
        || !reason
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        || message.is_empty()
        || message.len() > 1024
        || message.chars().any(char::is_control)
    {
        return None;
    }
    Some(format!("{REFUSED}project_store:{reason}: {message}"))
}

/// What the owner is told for a refusal. A refusal changed nothing and the service is still there:
/// it is reported as itself, never as a lost service or an unknown outcome.
fn refusal(code: &str) -> Option<&'static str> {
    Some(match code {
        "version_mismatch" | "service_protocol_mismatch" | "bridge_invalid_capabilities"
        | "unsupported_protocol" => "This app and the machine service are different versions. Reinstall the package so both are the same version.",
        #[cfg(feature = "remote")]
        workspace_remote::INCOMPATIBLE_VERSION_CODE => crate::remote::UPDATE_NEEDED,
        "project_folder_not_directory" => "This path is a file, not a folder.",
        "project_folder_unreadable" => "Cortex has no permission to open this folder.",
        "project_folder_not_writable" => {
            "You cannot write to this folder, so it cannot hold a project."
        }
        "project_folder_exists" => "A folder with this name is already there.",
        "project_path_refused" => {
            "This folder is private to Cortex or outside what this machine allows."
        }
        "absolute_path_required" => "Enter the full path of the folder, starting with / or ~/.",
        "invalid_path" | "symlink_refused" => {
            "This path cannot be used. Enter the folder's real path."
        }
        "invalid_display_name" => "Enter a project name of up to 256 characters.",
        "project_activation_running" => "Cortex is already starting for this project.",
        "project_not_found" => "This project is no longer on this machine.",
        "project_inactive" | "resync_required" | "child_unavailable" => {
            "Cortex is restarting for this project. Try again in a moment."
        }
        "machine_busy" | "machine_tool_timeout" => {
            "The machine is busy and did not answer in time. Try again."
        }
        "machine_tool_failed" => "The machine could not finish this request.",
        "directory_search_timeout" => "The search took too long. Type more of the path.",
        "directory_search_failed" => "This machine could not search its folders.",
        "stale_version" => "This task changed on the machine. Refresh it before trying again.",
        "stale_epoch" | "execution_session_unavailable" => {
            "This session no longer owns the task. Reopen its current state."
        }
        "task_paused" => "The task is paused. Resume it before sending or answering.",
        "pause_settling" => "The interrupted turn is still stopping. Wait before resuming.",
        "task_not_paused" => "This task is no longer paused. Refresh its current state.",
        "task_not_resumable" => "This task has ended and cannot be resumed.",
        "native_route_authorization_unavailable" => "Native Start is unavailable: engine routing has not been qualified. Nothing was started.",
        "portable_coordinator_unavailable" | "portable_coordinator_ambiguous" | "pinned_route_unavailable"
        | "connection_unavailable" | "authorization_required" => "Native Start has no eligible configured engine route. Check Cortex connection and model settings. Nothing was started.",
        "native_route_executable_unavailable" => "The configured engine command is missing or is not executable. Nothing was started.",
        "native_automatic_routing_unavailable" | "native_route_policy_unsupported" | "native_default_effort_unavailable"
        | "unsupported_dependent_setting" => "These routing or effort settings are not supported by native execution yet. Nothing was started.",
        "model_policy_migration_required" | "model_policy_version_unsupported" | "invalid_routing_overrides"
        | "native_route_settings_unreadable" | "native_route_settings_invalid" | "native_connection_registry_invalid"
        | "invalid_catalog_snapshot" | "untrusted_project_override" | "native_route_catalog_too_large"
        | "native_route_home_unavailable" | "native_route_engine_unsupported" | "native_route_auth_unsupported"
        | "native_route_environment_invalid" | "connection_storage_under_project" | "relative_storage_root"
        | "policy_missing" => "Native routing configuration could not be qualified. Check the configured connections, routing policy and observed model catalog. Nothing was started.",
        "native_start_options_unavailable" => "Native Start does not support these saved task options. Nothing was started.",
        "native_worktree_unavailable" => "A separate worktree per task is not available on this machine yet. Edit the task to work in the project folder, then start it. Nothing was started.",
        "native_task_route_unavailable" => "The model saved with this task is not available on any enabled engine connection. Choose another model in the task settings. Nothing was started.",
        "native_task_settings_invalid" => "The settings saved with this task are invalid. Choose the model, effort and access mode again. Nothing was started.",
        "native_attachments_unavailable" => "Attachments are not supported by this native action. Nothing was sent.",
        "native_operation_unavailable" => "This control is not available for native execution yet.",
        "native_execution_binding_invalid" | "stale_owner" => "This execution no longer has the expected owner. Reopen its current state.",
        "native_control_outcome_unknown" | "native_shutdown_unconfirmed" | "lost_worker_termination_unconfirmed" => "The action or process exit could not be confirmed. Inspect the current state; do not repeat the action.",
        "retry_not_expected" => "This task is not waiting for a retry. Refresh its current state.",
        "native_decision_history_limit" | "native_decision_page_unavailable" => "The question history could not be read completely. No answer was sent.",
        "native_capacity_unavailable" => "Native execution capacity is full. Nothing was started.",
        "legacy_runtime_tasks_active" => "This project still has legacy work. Native execution cannot adopt it.",
        "report_unavailable" => "No published worker report is available for this task.",
        "service_stopping" => "The machine service is stopping. Try again in a moment.",
        "remote_unavailable" => "Remote access is not available on this machine.",
        "remote_access_disabled" => "Turn on remote access first.",
        "unknown_pairing_request" => "This pairing request is no longer waiting.",
        "unknown_device" => "This device is no longer paired.",
        "invalid_remote_relay" => "Enter a relay address that starts with https:// or http://.",
        "remote_bind_failed" => "Remote access could not open a network port on this machine.",
        "remote_state_unavailable" => "Remote access could not read or save its private settings.",
        "remote_operation_refused" => "This is not available from a phone.",
        _ => return None,
    })
}
/// Marks an error as the machine's own answer, so a caller can tell it from a reply that never came.
const REFUSED: &str = "refused:";
/// Starting a child includes engine and store setup, which can outlast an ordinary request.
const ACTIVATION_DEADLINE: Duration = Duration::from_secs(120);
/// The deadline of an action. A read (`workspace_protocol::read::read_request`) has none: it
/// waits for the machine's answer, and only a lost connection ends it.
const REQUEST_DEADLINE: Duration = Duration::from_secs(35);
/// How long a first subscription may fail silently while a freshly started service binds.
pub const STARTUP_DEADLINE: Duration = Duration::from_secs(30);
/// How long a subscription that held may be re-established silently before the window says the
/// machine is offline. A resubscribe and resync inside it is not a lost service.
pub const RECONNECT_GRACE: Duration = Duration::from_secs(5);
/// Ordinary requests the window keeps in flight at once, each on its own connection. The service
/// serves a bounded number of connections (`workspace_service::CONNECTION_LIMIT`); this leaves
/// room for the status check and the subscription's own reads, which never wait for a permit, so
/// the window's slow reads cannot crowd out the one request that says whether the machine is there.
pub const ORDINARY_REQUESTS: usize = 48;

/// A byte stream to the machine service.
pub trait StreamIo: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + ?Sized> StreamIo for T {}
/// One fresh connection to the machine service.
pub type Stream = Box<dyn StreamIo>;
/// A boxed, sendable future.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// How the bridge reaches the machine service.
pub trait Connector: Send + Sync {
    /// Opens one fresh, authenticated connection. The error is a short code
    /// (`service_unavailable`, `machine_unreachable`, …), never connection content.
    fn connect(&self) -> BoxFuture<'_, Result<Stream, String>>;
    /// After a failed connection: a plain reason when the service cannot be reached
    /// until the owner acts (for example this phone was revoked), shown as the
    /// `unavailable` connection state instead of `disconnected`.
    fn unavailable(&self) -> Option<String> {
        None
    }
}

pub struct Bridge {
    connector: Arc<dyn Connector>,
    sequence: AtomicU64,
    ordinary: tokio::sync::Semaphore,
    startup_grace: Duration,
    phone: bool,
    client_name: String,
}
impl Bridge {
    /// A desktop-profile bridge over `connector`.
    pub fn new(connector: Arc<dyn Connector>) -> Self {
        Self {
            connector,
            sequence: AtomicU64::new(1),
            ordinary: tokio::sync::Semaphore::new(ORDINARY_REQUESTS),
            startup_grace: STARTUP_DEADLINE,
            phone: false,
            client_name: format!(
                "workspace-desktop-1.0.6-{}",
                workspace_protocol::channel::current().name
            ),
        }
    }
    /// A phone-profile bridge: `service.capabilities` lists only methods the phone may
    /// use, and the shared capability identity names the phone.
    pub fn phone(connector: Arc<dyn Connector>) -> Self {
        let mut bridge = Self::new(connector);
        bridge.phone = true;
        bridge.client_name = format!(
            "workspace-phone-1.0.6-{}",
            workspace_protocol::channel::current().name
        );
        bridge
    }
    /// How long the first subscription may fail before the window is told the machine is
    /// offline ([`STARTUP_DEADLINE`] by default).
    pub fn with_startup_grace(mut self, grace: Duration) -> Self {
        self.startup_grace = grace;
        self
    }
    async fn connection(&self) -> Result<Stream, String> {
        self.connector.connect().await.map_err(with_refusal_text)
    }
    async fn send(
        &self,
        stream: &mut Stream,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        let id = self.sequence.fetch_add(1, Ordering::Relaxed).to_string();
        write_frame(
            stream,
            &json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
        )
        .await
        .map_err(|_| "service_disconnected")?;
        let reply = read_frame(stream)
            .await
            .map_err(|_| "service_disconnected")?;
        if method == "service.subscribe.code" && reply["jsonrpc"] == "2.0"
            && reply["id"] == id && reply["error"]["message"] == "unsupported_method"
        {
            return Err("unsupported_method".into());
        }
        if method.starts_with("voice.") && reply["jsonrpc"] == "2.0" && reply["id"] == id {
            if let Some(
                code @ ("voice_busy"
                | "voice_unavailable"
                | "voice_settings_unavailable"
                | "voice_session_unknown"
                | "voice_request_expired"),
            ) = reply["error"]["message"].as_str()
            {
                return Err(code.into());
            }
        }
        if method.starts_with("terminal.") && reply["jsonrpc"] == "2.0" && reply["id"] == id {
            let message = match reply["error"]["message"].as_str() {
                Some("terminal_session_limit") => {
                    Some("Eight project shells are already open. Close a shell first.")
                }
                Some("terminal_session_unknown") => {
                    Some("This terminal session is gone. Reconnect to open a new shell.")
                }
                Some("terminal_shell_unavailable" | "terminal_unavailable") => {
                    Some("Your interactive shell could not be opened.")
                }
                Some("terminal_exited") => {
                    Some("The shell exited. Close it before opening a new shell.")
                }
                Some("terminal_input_refused") => {
                    Some("Terminal input was refused. Nothing was resent.")
                }
                Some("terminal_resize_failed") => Some("The terminal could not be resized."),
                Some("terminal_project_changed" | "project_not_found") => {
                    Some("This terminal's project is no longer available.")
                }
                _ => None,
            };
            if let Some(message) = message {
                return Err(message.into());
            }
        }
        if reply["error"]["message"]
            .as_str()
            .is_some_and(|message| message.starts_with("dev_project_refused"))
        {
            return Err(format!("{REFUSED}Cortex Workspace Dev did not open this folder: its .cortex-dev store was not created by Dev on this machine, so it was left untouched. Released Cortex data in .cortex is never the reason."));
        }
        if reply["jsonrpc"] == "2.0" && reply["id"] == id {
            if let Some(refusal) = storage_refusal(&reply) {
                return Err(refusal);
            }
            let error = &reply["error"];
            if method == "workspace"
                && reply.get("result").is_none()
                && error["code"] == -32000
                && error["data"]["kind"] == "worktree_start"
                && matches!(
                    error["data"]["reason"].as_str(),
                    Some("worktree_git_required" | "worktree_head_unborn" | "worktree_git_unavailable")
                )
            {
                if let Some(message) = error["message"].as_str().filter(|message| {
                    !message.is_empty()
                        && message.len() <= 1024
                        && !message.chars().any(char::is_control)
                }) {
                    return Err(format!("{REFUSED}{message}"));
                }
            }
            match error["message"].as_str() {
                // These two steer the caller, so they stay codes.
                Some(
                    code @ ("project_folder_missing"
                    | "unknown_outcome"
                    | "native_start_outcome_unknown"),
                ) => return Err(code.into()),
                Some(code) => {
                    if let Some(text) = refusal(code) {
                        return Err(format!("{REFUSED}{text}"));
                    }
                }
                None => (),
            }
        }
        if reply["jsonrpc"] != "2.0"
            || reply["id"] != id
            || reply.get("error").is_some()
            || reply.get("result").is_none()
        {
            return Err("service_request_failed".into());
        }
        Ok(reply["result"].clone())
    }
    /// One attempt on a fresh authenticated connection. A timeout drops this connection;
    /// nothing retries a mutation or lets a late response satisfy another request.
    pub async fn rpc(&self, method: &str, params: Value) -> Result<Value, String> {
        self.call(method, params, method != "service.status").await
    }
    async fn call(&self, method: &str, params: Value, ordinary: bool) -> Result<Value, String> {
        // A read waits for its answer however long the machine takes: a slow read is never
        // a timeout. Only a lost connection (`service_disconnected`) ends it as an error.
        if workspace_protocol::read::read_request(method, &params) {
            let _permit = if ordinary {
                Some(self.ordinary.acquire().await.map_err(|_| {
                    format!("{REFUSED}{}", refusal("service_stopping").unwrap_or_default())
                })?)
            } else {
                None
            };
            let mut stream = self.connection().await?;
            return self.send(&mut stream, method, params).await;
        }
        let deadline = if method == "projects.activate" {
            ACTIVATION_DEADLINE
        } else {
            REQUEST_DEADLINE
        };
        // Waiting for a permit sends nothing, so a request that never gets one is a refusal,
        // never an unknown outcome.
        let busy = || format!("{REFUSED}{}", refusal("machine_busy").unwrap_or_default());
        let _permit = if ordinary {
            Some(
                timeout(deadline, self.ordinary.acquire())
                    .await
                    .map_err(|_| busy())?
                    .map_err(|_| busy())?,
            )
        } else {
            None
        };
        // Readiness has its own budget; it must not consume a request deadline.
        // No request bytes are sent until the authenticated socket is available.
        let mut stream = self.connection().await?;
        timeout(deadline, self.send(&mut stream, method, params))
            .await
            .map_err(|_| "service_timeout")?
    }
    /// The requests both shells forward as they are, after their policy allowed them.
    /// Desktop-only `native.*` operations are the desktop shell's own; they never reach
    /// the service from here.
    pub async fn request(&self, call: crate::policy::Call) -> Result<Value, String> {
        match call.operation.as_str() {
            "native.query" | "native.board" => {
                let project = call.message["projectId"]
                    .as_str()
                    .ok_or("service_invalid_request")?
                    .to_owned();
                let value = self.rpc(&call.operation, call.message).await?;
                let reply: workspace_protocol::read::ReadReply =
                    serde_json::from_value(value.clone())
                        .map_err(|_| "service_invalid_response")?;
                reply.validate().map_err(|_| "service_invalid_response")?;
                if reply.project_id != project {
                    return Err("service_invalid_response".into());
                }
                Ok(value)
            }
            "service.status"
            | "projects.register"
            | "projects.remove"
            | "projects.rename"
            | "projects.list"
            | "projects.status"
            | "projects.activate"
            | "executions.cancel"
            | "executions.control"
            | "executions.prompt"
            | "machine.call"
            | "directories.search"
            | "directories.cancel"
            | "voice.settings"
            | "voice.setLanguage"
            | "voice.start"
            | "voice.download"
            | "voice.read"
            | "voice.stop"
            | "voice.cancel"
            | "terminal.open"
            | "terminal.read"
            | "terminal.write"
            | "terminal.resize"
            | "terminal.close"
            | "remote.status"
            | "remote.enable"
            | "remote.pairing.start"
            | "remote.pairing.cancel"
            | "remote.pairing.decide"
            | "remote.devices.revoke"
            | "remote.relay.set"
            | "remote.sudo.set"
            | "askpass.list"
            | "askpass.answer"
            | "owner_actions.list"
            | "owner_actions.act"
            | "notifications.report"
            | "notifications.take" => self.rpc(&call.operation, call.message).await,
            "service.capabilities" => {
                let mut value = self.rpc("service.capabilities", call.message).await?;
                if self.phone {
                    if let Some(methods) = value["methods"].as_array_mut() {
                        methods.retain(|m| m.as_str().is_some_and(crate::policy::phone_method));
                    }
                }
                Ok(value)
            }
            "capabilities.get" => {
                let mut value = self.capabilities().await.map_err(with_refusal_text)?;
                if call.message["protocol"]["minor"]
                    .as_u64()
                    .is_some_and(|minor| minor <= 6)
                {
                    value["limits"]["frameBytes"] = json!(1024 * 1024);
                }
                Ok(value)
            }
            operation if operation.starts_with("native.") => Err("bridge_request_refused".into()),
            _ => self.rpc("workspace", call.message).await,
        }
    }
    async fn capabilities(&self) -> Result<Value, String> {
        let projects = self.rpc("projects.list", json!({})).await?;
        let projects = projects.as_array().ok_or("service_invalid_response")?;
        // projects.status carries a negotiated subset, not a full adapter Capabilities.
        // Publish the native shell's own identity with only that observed intersection.
        let mut shared = workspace_protocol::capabilities(
            &self.client_name,
            "0",
            &[
                "capabilities.get",
                "project.snapshot",
                "tasks.list",
                "tasks.get",
                "executions.get",
                "decisions.list",
                "decisions.get",
                "results.get",
                "reports.read",
                "history.page",
                "events.page",
                "requests.get",
                "tasks.create",
                "tasks.start",
                "decisions.answer",
            ],
        )
        .map_err(|_| "bridge_invalid_capabilities")?;
        let mut active = false;
        let mut attachments = true;
        for project in projects.iter().filter(|p| !p["activation"].is_null()) {
            let status = self
                .rpc("projects.status", json!({"projectId":project["projectId"]}))
                .await?;
            // A project with no working child says so on its own page. The machine, and every
            // other project on it, keeps working.
            if status["ready"] != true {
                continue;
            }
            active = true;
            attachments &= status["capabilities"]["attachments"] == true;
            let mut candidate = shared.clone();
            for key in ["protocol", "operations", "limits", "mappingVersion"] {
                candidate[key] = status["capabilities"]
                    .get(key)
                    .ok_or("service_invalid_response")?
                    .clone();
            }
            workspace_protocol::validate("Capabilities", &candidate)
                .map_err(|_| "service_invalid_response")?;
            let common = workspace_protocol::negotiate(&shared, &candidate)
                .map_err(|_| "service_protocol_mismatch")?;
            if common["mappingVersion"] != "1" {
                return Err("service_protocol_mismatch".into());
            }
            for key in ["protocol", "operations", "limits", "mappingVersion"] {
                shared[key] = common[key].clone();
            }
        }
        if !active {
            shared["operations"] = json!([]);
        }
        shared["attachments"] = json!(active && attachments);
        Ok(shared)
    }
    async fn subscription(&self, notify: &impl Fn(Value)) -> Result<(), String> {
        let mut stream = self.connection().await?;
        let ack = timeout(Duration::from_secs(5), async {
            if !self.phone {
                match self.send(&mut stream, "service.subscribe.code", json!({})).await {
                    Err(error) if error == "unsupported_method" => {}
                    result => return result,
                }
            }
            self.send(&mut stream, "service.subscribe", json!({})).await
        }).await.map_err(|_| "service_timeout")??;
        if ack["subscribed"] != true {
            return Err("service_invalid_response".into());
        }
        // A lost connection can have missed invalidations. Read a new snapshot before
        // declaring it fresh, including the service's explicit overflow recovery.
        self.resync(notify).await?;
        notify(json!({"type":"connection","state":"connected"}));
        loop {
            let message = read_frame(&mut stream)
                .await
                .map_err(|error| format!("service_disconnected:{}", frame_failure(&error)))?;
            match incoming(&message) {
                // Notification clicks belong to the desktop that raised them, never a phone.
                Ok(Incoming::Forward(event))
                    if self.phone && matches!(event["type"].as_str(), Some("notification_open" | "code_lsp" | "code_lsp_resync")) => {}
                Ok(Incoming::Forward(event)) => notify(event),
                Ok(Incoming::Resync) => self.resync(notify).await?,
                // One delta this window cannot apply says nothing about the connection. The
                // project re-reads its authoritative state instead of trusting or dropping it.
                Err(Refused {
                    code,
                    project: Some(project),
                }) => {
                    log_line("event_refused", code, None);
                    notify(json!({"type":"native_resync","projectId":project}));
                }
                Err(Refused {
                    code,
                    project: None,
                }) => return Err(format!("service_invalid_event:{code}")),
            }
        }
    }
    async fn resync(&self, notify: &impl Fn(Value)) -> Result<(), String> {
        // The subscription's own read never waits behind the window's ordinary requests.
        let projects = self.call("projects.list", json!({}), false).await?;
        for project in projects
            .as_array()
            .ok_or("service_invalid_response")?
            .iter()
            .filter(|p| !p["activation"].is_null())
        {
            // Reconnect invalidates presentation caches, not runtime state. The service
            // publishes native deltas independently; asking it for anything here would stop
            // draining this socket and turn a backlog into a false disconnect.
            notify(json!({"type":"invalidated","projectId":project["projectId"]}));
        }
        notify(json!({"type":"refresh"}));
        Ok(())
    }
    pub async fn subscribe(&self, notify: impl Fn(Value)) {
        self.subscribe_with(notify, RECONNECT_GRACE).await
    }
    /// [`Bridge::subscribe`] with an explicit reconnect grace.
    pub async fn subscribe_with(&self, notify: impl Fn(Value), grace: Duration) {
        let started = tokio::time::Instant::now();
        let connects = AtomicU64::new(0);
        let publish = |event: Value| {
            if event["state"] == "connected" {
                connects.fetch_add(1, Ordering::Relaxed);
            }
            notify(event);
        };
        notify(json!({"type":"connection","state":"connecting"}));
        // When the window last lost a subscription that had held, while it has not said so.
        let mut lost: Option<tokio::time::Instant> = None;
        let mut logged = String::new();
        let mut shown_unavailable: Option<String> = None;
        loop {
            let before = connects.load(Ordering::Relaxed);
            let attempt = tokio::time::Instant::now();
            let reason = match self.subscription(&publish).await {
                Err(reason) => reason,
                Ok(()) => "subscription_ended".into(),
            };
            let held = connects.load(Ordering::Relaxed) > before;
            // Every end is recorded once per distinct reason, so a long outage is one line.
            if held || reason != logged {
                log_line("ended", &reason, Some(attempt.elapsed()));
                logged = reason;
            }
            // A machine that refuses this connection until the owner acts is shown as
            // such at once, and once; later attempts stay quiet until it changes.
            if let Some(why) = self.connector.unavailable() {
                if shown_unavailable.as_ref() != Some(&why) {
                    notify(json!({"type":"connection","state":"unavailable","reason":why}));
                    shown_unavailable = Some(why);
                }
                lost = None;
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
            shown_unavailable = None;
            // A first-launch service is allowed to bind before an offline state is shown.
            if connects.load(Ordering::Relaxed) == 0 && started.elapsed() < self.startup_grace {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
            // A subscription that held ended: resubscribe and resync silently first. Only one
            // that cannot be re-established within the grace reaches the owner as offline. A
            // subscription that keeps ending at once does not restart the grace.
            if held && (lost.is_none() || attempt.elapsed() >= grace) {
                lost = Some(tokio::time::Instant::now());
            }
            if lost.is_some_and(|since| since.elapsed() < grace) {
                tokio::time::sleep(Duration::from_millis(250)).await;
                continue;
            }
            lost = None;
            notify(
                json!({"type":"connection","state":"disconnected","reason":"Machine service disconnected"}),
            );
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }
}

/// What one subscription frame asks of the window, after the desktop's own validation.
#[derive(Debug)]
pub enum Incoming {
    /// Publish this event to the window as it is.
    Forward(Value),
    /// The service asks for a full resync of every active project.
    Resync,
}
/// Why one subscription frame was refused. `code` is a fixed protocol code, never frame content.
/// `project` is the frame's own valid project identity, when it named one, so the refusal can be
/// recovered by re-reading that project alone.
#[derive(Debug, PartialEq, Eq)]
pub struct Refused {
    pub code: &'static str,
    pub project: Option<String>,
}
/// Validates one subscription frame exactly as the window does before anything reaches it.
pub fn incoming(message: &Value) -> Result<Incoming, Refused> {
    let refused = |code| Refused {
        code,
        project: None,
    };
    if message["jsonrpc"] != "2.0" || message["method"] != "service.event" {
        return Err(refused("not_service_event"));
    }
    let event = &message["params"];
    match event["type"].as_str() {
        Some("native_delta") => {
            let project = event["projectId"]
                .as_str()
                .ok_or(refused("invalid_project"))?
                .to_owned();
            let named = project_named(event);
            let refuse = |code| Refused {
                code,
                project: named.clone(),
            };
            let batch: workspace_protocol::events::DeltaBatch =
                serde_json::from_value(event["batch"].clone())
                    .map_err(|_| refuse("delta_unreadable"))?;
            batch.validate().map_err(refuse)?;
            if batch.stream_key
                != (workspace_protocol::events::StreamKey::Project {
                    project_id: project.clone(),
                })
            {
                return Err(refuse("delta_stream_mismatch"));
            }
            Ok(Incoming::Forward(event.clone()))
        }
        Some("native_resync") => {
            project_named(event).ok_or(refused("invalid_project"))?;
            Ok(Incoming::Forward(event.clone()))
        }
        Some("code_lsp") => {
            workspace_protocol::validate("CodeLspEvent", event)
                .map_err(|_| refused("invalid_code_event"))?;
            Ok(Incoming::Forward(event.clone()))
        }
        Some("code_lsp_resync") if *event == json!({"type":"code_lsp_resync"}) => Ok(Incoming::Forward(event.clone())),
        Some("resync_required") => Ok(Incoming::Resync),
        // A click on one of the service's desktop notifications: a bounded nonce and the
        // target the service kept for it. The desktop shell checks the target's exact shape.
        Some("notification_open") => {
            let nonce = event["nonce"].as_str().unwrap_or("");
            let map = event.as_object().map_or(0, |m| m.len());
            if nonce.is_empty()
                || nonce.len() > 128
                || !nonce
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                || !event["target"].is_object()
                || map != 3
            {
                return Err(refused("invalid_notification"));
            }
            Ok(Incoming::Forward(event.clone()))
        }
        // Remote access changed (pairing, a device, the relay): an open remote access page rereads it, and nothing else.
        Some("remote_state") => Ok(Incoming::Forward(json!({"type":"remote_state"}))),
        // One project gained or lost its runtime, or settled a merge: the window rereads that project, not every project.
        // One it cannot name is still a change: the whole window rereads, as before, and the subscription stays up.
        Some("project_state") => Ok(Incoming::Forward(match project_named(event) {
            Some(project) => json!({"type":"project_state","projectId":project}),
            None => json!({"type":"refresh"}),
        })),
        Some(kind @ ("invalidation" | "public_events")) => {
            let project = project_named(event).ok_or(refused("invalid_project"))?;
            Ok(Incoming::Forward(if kind == "public_events" {
                json!({"type":"invalidated","projectId":project,"source":"runtime"})
            } else {
                json!({"type":"invalidated","projectId":project})
            }))
        }
        _ => Err(refused("unknown_event")),
    }
}
fn project_named(event: &Value) -> Option<String> {
    workspace_protocol::validate("Identity", &event["projectId"])
        .ok()
        .and_then(|()| event["projectId"].as_str().map(str::to_owned))
}
/// A bounded code for a failed frame read. Only the service's own short codes and I/O error
/// kinds are kept; nothing read from the socket is.
pub fn frame_failure(error: &std::io::Error) -> String {
    let text = error.to_string();
    if error.kind() == std::io::ErrorKind::Other
        && !text.is_empty()
        && text.len() <= 64
        && text.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
    {
        text
    } else {
        format!("{:?}", error.kind()).to_ascii_lowercase()
    }
}
/// One bounded diagnostic line on the desktop's stderr. Codes are fixed protocol words; no
/// project, task or message text is written.
fn log_line(what: &str, reason: &str, after: Option<Duration>) {
    let reason: String = reason
        .chars()
        .take(96)
        .map(|c| {
            if c.is_ascii_alphanumeric() || "_:-".contains(c) {
                c
            } else {
                '?'
            }
        })
        .collect();
    match after {
        Some(after) => eprintln!(
            "cortex-workspace-desktop: subscription {what} reason={reason} after_ms={}",
            after.as_millis()
        ),
        None => eprintln!("cortex-workspace-desktop: subscription {what} reason={reason}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{with_refusal_text, Bridge, REFUSED};
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn protocol_refusals_explain_how_to_recover() {
        for code in [
            "version_mismatch",
            "service_protocol_mismatch",
            "bridge_invalid_capabilities",
            "unsupported_protocol",
        ] {
            assert_eq!(
                with_refusal_text(code.into()),
                format!("{REFUSED}This app and the machine service are different versions. Reinstall the package so both are the same version.")
            );
        }
        #[cfg(feature = "remote")]
        assert_eq!(
            with_refusal_text(workspace_remote::INCOMPATIBLE_VERSION_CODE.into()),
            format!("{REFUSED}{}", crate::remote::UPDATE_NEEDED)
        );
        assert_eq!(
            with_refusal_text("service_timeout".into()),
            "service_timeout"
        );
    }

    // Keep each socket in its private fixture directory even if the caller's
    // TMPDIR makes the real pathname too long for sockaddr_un.
    fn socket_address(
        path: &std::path::Path,
    ) -> std::io::Result<(std::fs::File, std::path::PathBuf)> {
        use std::os::fd::AsRawFd;
        let dir = std::fs::File::open(path.parent().unwrap())?;
        let address = std::path::PathBuf::from(format!("/proc/self/fd/{}", dir.as_raw_fd()))
            .join(path.file_name().unwrap());
        Ok((dir, address))
    }

    fn bind_socket(path: &std::path::Path) -> tokio::net::UnixListener {
        let (_dir, address) = socket_address(path).unwrap();
        tokio::net::UnixListener::bind(address).unwrap()
    }

    /// A plain Unix-socket connector for the fake services below.
    struct Unix(std::path::PathBuf);
    impl super::Connector for Unix {
        fn connect(&self) -> super::BoxFuture<'_, Result<super::Stream, String>> {
            Box::pin(async move {
                let (_dir, address) =
                    socket_address(&self.0).map_err(|_| "service_unavailable".to_string())?;
                tokio::net::UnixStream::connect(address)
                    .await
                    .map(|s| Box::new(s) as super::Stream)
                    .map_err(|_| "service_unavailable".to_string())
            })
        }
    }
    fn bridge(path: std::path::PathBuf) -> Bridge {
        Bridge::new(std::sync::Arc::new(Unix(path)))
    }

    /// A fake service: every connection answers its first request; each subscription then
    /// receives the frames the script returns for its ordinal (0 for the first) and stays open.
    fn fake_service(
        name: &str,
        frames: impl Fn(usize) -> Vec<serde_json::Value> + Send + Sync + 'static,
    ) -> (std::path::PathBuf, tokio::task::JoinHandle<()>) {
        use serde_json::json;
        let root = std::env::temp_dir().join(format!("cw-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = root.join("service.sock");
        let listener = bind_socket(&path);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let frames = std::sync::Arc::new(frames);
        let subscriptions = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let server = tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let (frames, subscriptions) = (frames.clone(), subscriptions.clone());
                tokio::spawn(async move {
                    let request = crate::frames::read_frame(&mut stream).await.unwrap();
                    let result = match request["method"].as_str().unwrap() {
                        "projects.list" => json!([{"projectId":"p","activation":{}}]),
                        "service.subscribe.code" => json!({"subscribed":true}),
                        _ => panic!("unexpected method"),
                    };
                    let _ = crate::frames::write_frame(
                        &mut stream,
                        &json!({"jsonrpc":"2.0","id":request["id"],"result":result}),
                    )
                    .await;
                    if request["method"] == "service.subscribe.code" {
                        let ordinal =
                            subscriptions.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        // The subscriber resyncs before it reads events.
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                        for frame in frames(ordinal) {
                            let _ = crate::frames::write_frame(&mut stream, &frame).await;
                        }
                        std::future::pending::<()>().await;
                    }
                });
            }
        });
        (path, server)
    }
    fn event(params: serde_json::Value) -> serde_json::Value {
        serde_json::json!({"jsonrpc":"2.0","method":"service.event","params":params})
    }

    /// A machine that answers every request after `delay` (on a paused clock), or drops the
    /// connection without answering when `delay` is `None`.
    struct Slow(Option<std::time::Duration>);
    impl super::Connector for Slow {
        fn connect(&self) -> super::BoxFuture<'_, Result<super::Stream, String>> {
            let delay = self.0;
            Box::pin(async move {
                let (client, mut service) = tokio::io::duplex(1 << 16);
                tokio::spawn(async move {
                    let request = crate::frames::read_frame(&mut service).await.unwrap();
                    let Some(delay) = delay else { return };
                    tokio::time::sleep(delay).await;
                    let _ = crate::frames::write_frame(
                        &mut service,
                        &serde_json::json!({"jsonrpc":"2.0","id":request["id"],"result":{"ok":true,"result":{"tasks":{}}}}),
                    )
                    .await;
                });
                Ok(Box::new(client) as super::Stream)
            })
        }
    }

    /// A read that takes longer than any request deadline is waited for, never a timeout; an
    /// action keeps its deadline; a read whose connection is lost is still an error.
    #[tokio::test(start_paused = true)]
    async fn a_slow_read_waits_for_its_answer_and_only_a_disconnect_fails_it() {
        use serde_json::json;
        let slow = Bridge::new(std::sync::Arc::new(Slow(Some(std::time::Duration::from_secs(600)))));
        let read = slow
            .rpc("machine.call", json!({"op":"product.facts","projectId":"p","params":{}}))
            .await;
        assert_eq!(read.unwrap()["ok"], true);
        let history = slow.rpc("workspace", json!({"operation":"history.page","projectId":"p"})).await;
        assert!(history.is_ok(), "{history:?}");
        let action = slow
            .rpc("machine.call", json!({"op":"product.task","projectId":"p","params":{"action":"close"}}))
            .await;
        assert_eq!(action.unwrap_err(), "service_timeout");
        let lost = Bridge::new(std::sync::Arc::new(Slow(None)));
        let read = lost
            .rpc("machine.call", json!({"op":"product.facts","projectId":"p","params":{}}))
            .await;
        assert_eq!(read.unwrap_err(), "service_disconnected");
    }

    #[test]
    fn a_notification_click_carries_only_a_bounded_nonce_and_a_target() {
        use super::{incoming, Incoming};
        use serde_json::json;
        let click = json!({"type":"notification_open","nonce":"nc0a1b-2","target":{"kind":"task","project":"p","task":"t"}});
        assert!(matches!(incoming(&event(click.clone())), Ok(Incoming::Forward(e)) if e == click));
        for bad in [
            json!({"type":"notification_open","nonce":"a b","target":{}}),
            json!({"type":"notification_open","nonce":"","target":{}}),
            json!({"type":"notification_open","nonce":"n","target":"/m/x"}),
            json!({"type":"notification_open","nonce":"n","target":{},"token":"t"}),
        ] {
            assert_eq!(
                incoming(&event(bad)).unwrap_err().code,
                "invalid_notification"
            );
        }
    }

    #[test]
    fn a_refused_frame_names_a_fixed_code_and_only_its_own_valid_project() {
        use super::{incoming, Incoming, Refused};
        use serde_json::json;
        let unreadable = incoming(&event(
            json!({"type":"native_delta","projectId":"p","batch":{"unexpected":"private words"}}),
        ))
        .unwrap_err();
        assert_eq!(
            unreadable,
            Refused {
                code: "delta_unreadable",
                project: Some("p".into())
            }
        );
        let empty = incoming(&event(
            json!({"type":"native_delta","projectId":"p","batch":{
            "generation":"g","streamKey":{"scope":"project","project_id":"p"},
            "fromSequence":"1","throughSequence":"1","changes":[]}}),
        ))
        .unwrap_err();
        assert_eq!(empty.code, "invalid_delta");
        assert_eq!(
            incoming(&event(json!({"type":"brand_new_kind","projectId":"p"}))).unwrap_err(),
            Refused {
                code: "unknown_event",
                project: None
            }
        );
        assert_eq!(
            incoming(&json!({"jsonrpc":"2.0","id":"1","result":{}}))
                .unwrap_err()
                .code,
            "not_service_event"
        );
        assert!(matches!(
            incoming(&event(
                json!({"type":"resync_required","reason":"subscription_overflow"})
            )),
            Ok(Incoming::Resync)
        ));
        let Ok(Incoming::Forward(forwarded)) =
            incoming(&event(json!({"type":"public_events","projectId":"p"})))
        else {
            panic!("a valid invalidation is forwarded");
        };
        assert_eq!(
            forwarded,
            json!({"type":"invalidated","projectId":"p","source":"runtime"})
        );
    }

    #[tokio::test]
    async fn a_delta_the_window_cannot_apply_resyncs_its_project_and_keeps_the_subscription() {
        use serde_json::json;
        let (path, server) = fake_service("refused-delta", |ordinal| {
            assert_eq!(ordinal, 0, "the subscription was not ended");
            vec![
                event(json!({"type":"native_delta","projectId":"p","batch":{"unexpected":true}})),
                event(json!({"type":"invalidation","projectId":"p"})),
            ]
        });
        let bridge = bridge(path.clone());
        let events = std::sync::Mutex::new(Vec::new());
        let subscription = bridge.subscribe(|e| events.lock().unwrap().push(e));
        tokio::select! {
            _ = subscription => panic!("subscription ended"),
            _ = tokio::time::sleep(std::time::Duration::from_millis(400)) => {}
        }
        let seen = events.lock().unwrap();
        let resync = seen
            .iter()
            .position(|e| *e == json!({"type":"native_resync","projectId":"p"}))
            .expect("the refused delta becomes a resync of its project");
        assert!(seen[resync + 1..]
            .iter()
            .any(|e| *e == json!({"type":"invalidated","projectId":"p"})));
        assert_eq!(seen.iter().filter(|e| e["state"] == "connected").count(), 1);
        assert!(!seen.iter().any(|e| e["state"] == "disconnected"));
        server.abort();
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[tokio::test]
    async fn a_subscription_that_ends_and_resubscribes_quickly_never_shows_offline() {
        use serde_json::json;
        // The first subscription receives a frame that is not a service event at all.
        let (path, server) = fake_service("quick-resubscribe", |ordinal| {
            if ordinal == 0 {
                vec![json!({"jsonrpc":"2.0","method":"unexpected"})]
            } else {
                vec![]
            }
        });
        let bridge = bridge(path.clone());
        let events = std::sync::Mutex::new(Vec::new());
        let subscription = bridge.subscribe(|e| events.lock().unwrap().push(e));
        tokio::select! {
            _ = subscription => panic!("subscription ended"),
            _ = tokio::time::sleep(std::time::Duration::from_millis(1500)) => {}
        }
        let seen = events.lock().unwrap();
        assert_eq!(
            seen.iter().filter(|e| e["state"] == "connected").count(),
            2,
            "resubscribed"
        );
        assert!(!seen.iter().any(|e| e["state"] == "disconnected"));
        // The resubscribe re-reads what the lost connection may have missed.
        let second = seen
            .iter()
            .rposition(|e| e["state"] == "connected")
            .unwrap();
        assert!(
            seen[..second]
                .iter()
                .filter(|e| e["type"] == "refresh")
                .count()
                >= 2
        );
        server.abort();
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[tokio::test]
    async fn a_service_that_stays_away_past_the_grace_is_shown_offline() {
        let (path, server) = fake_service("stays-away", |_| {
            vec![serde_json::json!({"not":"an event"})]
        });
        let bridge = bridge(path.clone());
        let events = std::sync::Mutex::new(Vec::new());
        let first_offline = std::sync::Mutex::new(None);
        let started = tokio::time::Instant::now();
        let grace = std::time::Duration::from_millis(400);
        let subscription = bridge.subscribe_with(
            |e: serde_json::Value| {
                if e["state"] == "connected" {
                    // After the first subscription is up, the service goes away.
                    let _ = std::fs::remove_file(&path);
                }
                if e["state"] == "disconnected" {
                    first_offline
                        .lock()
                        .unwrap()
                        .get_or_insert(started.elapsed());
                }
                events.lock().unwrap().push(e);
            },
            grace,
        );
        tokio::select! {
            _ = subscription => panic!("subscription ended"),
            _ = tokio::time::sleep(std::time::Duration::from_millis(1500)) => {}
        }
        let offline = first_offline
            .lock()
            .unwrap()
            .expect("an outage longer than the grace is shown");
        assert!(offline >= grace, "shown offline after {offline:?}");
        server.abort();
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[tokio::test]
    async fn slow_ordinary_requests_never_crowd_out_the_status_check() {
        use serde_json::json;
        let root = std::env::temp_dir().join(format!("cw-lanes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = root.join("service.sock");
        let listener = bind_socket(&path);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let slow = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = slow.clone();
        // Ordinary requests are read and never answered; only the status check is.
        let server = tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let slow = seen.clone();
                tokio::spawn(async move {
                    let request = crate::frames::read_frame(&mut stream).await.unwrap();
                    if request["method"] != "service.status" {
                        slow.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        return std::future::pending::<()>().await;
                    }
                    crate::frames::write_frame(
                        &mut stream,
                        &json!({"jsonrpc":"2.0","id":request["id"],"result":{"channel":"dev"}}),
                    )
                    .await
                    .unwrap();
                });
            }
        });
        let bridge = std::sync::Arc::new(bridge(path));
        let mut waiting = Vec::new();
        for _ in 0..super::ORDINARY_REQUESTS + 8 {
            let bridge = bridge.clone();
            waiting.push(tokio::spawn(async move {
                bridge.rpc("projects.list", json!({})).await
            }));
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        // The window never opens more ordinary connections than its bound.
        assert_eq!(
            slow.load(std::sync::atomic::Ordering::SeqCst),
            super::ORDINARY_REQUESTS
        );
        let status = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            bridge.rpc("service.status", json!({})),
        )
        .await
        .expect("the status check does not wait behind ordinary requests")
        .unwrap();
        assert_eq!(status["channel"], "dev");
        for task in waiting {
            task.abort();
        }
        server.abort();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn many_slow_projects_do_not_delay_subscription_or_reconnect() {
        use crate::frames::{read_frame, write_frame};
        use serde_json::json;
        let root = std::env::temp_dir().join(format!("cw-sub-ready-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = root.join("service.sock");
        let listener = bind_socket(&path);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let server = tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let request = read_frame(&mut stream).await.unwrap();
                    let result = match request["method"].as_str().unwrap() {
                        "service.subscribe.code" => json!({"subscribed":true}),
                        "projects.list" => json!((0..50)
                            .map(|i| json!({"projectId":format!("p{i}"),"activation":{}}))
                            .collect::<Vec<_>>()),
                        _ => panic!("unexpected method"),
                    };
                    let _ = write_frame(
                        &mut stream,
                        &json!({"jsonrpc":"2.0","id":request["id"],"result":result}),
                    )
                    .await;
                    if request["method"] == "service.subscribe.code" {
                        std::future::pending::<()>().await;
                    }
                });
            }
        });
        let bridge = bridge(path);
        for _ in 0..2 {
            let events = std::sync::Mutex::new(Vec::new());
            let notify = |event| events.lock().unwrap().push(event);
            let subscription = bridge.subscription(&notify);
            tokio::pin!(subscription);
            tokio::select! {
                _ = &mut subscription => panic!("subscription ended"),
                _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {}
            }
            assert!(events
                .lock()
                .unwrap()
                .iter()
                .any(|e| e["state"] == "connected"));
        }
        server.abort();
        let _ = server.await;
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn unqualified_native_start_is_a_visible_refusal_not_a_disconnect() {
        use serde_json::json;
        let root = std::env::temp_dir().join(format!("cw-native-refusal-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = root.join("service.sock");
        let listener = bind_socket(&path);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let codes = [
            "native_route_authorization_unavailable",
            "native_control_outcome_unknown",
            "native_shutdown_unconfirmed",
            "native_execution_binding_invalid",
            "retry_not_expected",
            "portable_coordinator_unavailable",
            "native_route_executable_unavailable",
            "native_automatic_routing_unavailable",
            "native_route_settings_unreadable",
            "native_route_policy_unsupported",
            "native_task_route_unavailable",
            "native_task_settings_invalid",
            "native_worktree_unavailable",
        ];
        let serving = tokio::spawn(async move {
            for code in codes {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = crate::frames::read_frame(&mut stream).await.unwrap();
                crate::frames::write_frame(
                    &mut stream,
                    &json!({"jsonrpc":"2.0","id":request["id"],
                    "error":{"code":-32000,"message":code}}),
                )
                .await
                .unwrap();
            }
        });
        let bridge = bridge(path);
        for code in codes {
            let error = bridge.rpc("workspace", json!({})).await.unwrap_err();
            assert!(error.starts_with("refused:"), "{code} became a disconnect");
            if code == "native_route_authorization_unavailable" {
                assert!(error.starts_with("refused:Native Start is unavailable:"));
                assert!(error.ends_with("Nothing was started."));
            } else if code == "native_control_outcome_unknown"
                || code == "native_shutdown_unconfirmed"
            {
                assert!(error.contains("do not repeat the action"));
            } else if code == "native_worktree_unavailable" {
                assert!(error.contains("Edit the task to work in the project folder"));
                assert!(error.ends_with("Nothing was started."));
            }
        }
        serving.await.unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn voice_settings_error_is_preserved_only_for_a_matching_voice_reply() {
        use serde_json::json;

        let root = std::env::temp_dir().join(format!("cw-voice-error-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = root.join("service.sock");
        let listener = bind_socket(&path);
        let cases = [
            ("voice.start", true, "voice_settings_unavailable"),
            ("voice.settings", true, "voice_settings_unavailable"),
            ("voice.settings", false, "service_request_failed"),
            ("service.status", true, "service_request_failed"),
        ];
        let serving = tokio::spawn(async move {
            for (method, matching, _) in cases {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = crate::frames::read_frame(&mut stream).await.unwrap();
                assert_eq!(request["method"], method);
                let id = if matching {
                    request["id"].clone()
                } else {
                    json!(null)
                };
                crate::frames::write_frame(
                    &mut stream,
                    &json!({"jsonrpc":"2.0","id":id,
                        "error":{"code":-32000,"message":"voice_settings_unavailable"}}),
                )
                .await
                .unwrap();
            }
        });
        let bridge = bridge(path);
        for (method, _, expected) in cases {
            assert_eq!(bridge.rpc(method, json!({})).await.unwrap_err(), expected);
        }
        serving.await.unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn storage_refusal_carries_human_guidance_and_reason_across_the_bridge() {
        use serde_json::json;

        let root = std::env::temp_dir().join(format!("cw-storage-refusal-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = root.join("service.sock");
        let listener = bind_socket(&path);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let message = "This project uses format 10. Keep the original files.";
        let serving = tokio::spawn(async move {
            for (kind, mismatched_id) in [
                ("project_store", false),
                ("other", false),
                ("project_store", true),
            ] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = crate::frames::read_frame(&mut stream).await.unwrap();
                assert_eq!(request["method"], "projects.activate");
                let id = if mismatched_id {
                    json!(null)
                } else {
                    request["id"].clone()
                };
                crate::frames::write_frame(
                    &mut stream,
                    &json!({"jsonrpc":"2.0","id":id,
                        "error":{"code":-32000,"message":message,
                            "data":{"kind":kind,"reason":"unsupported_storage","retryable":false}}}),
                ).await.unwrap();
            }
        });
        let bridge = bridge(path);
        for expected in [
            format!("refused:project_store:unsupported_storage: {message}"),
            "service_request_failed".into(),
            "service_request_failed".into(),
        ] {
            assert_eq!(
                bridge
                    .rpc("projects.activate", json!({"projectId":"p"}))
                    .await
                    .unwrap_err(),
                expected
            );
        }
        serving.await.unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn native_query_routes_directly_and_validates_its_reply() {
        use serde_json::json;
        let root = std::env::temp_dir().join(format!(
            "cw-native-{}-{}",
            std::process::id(),
            "x".repeat(110)
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = root.join("service.sock");
        let listener = bind_socket(&path);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let serving = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = crate::frames::read_frame(&mut stream).await.unwrap();
            assert_eq!(request["method"], "native.query");
            crate::frames::write_frame(&mut stream,&json!({"jsonrpc":"2.0","id":request["id"],
                "result":{"protocol":{"major":2,"mapping":2},"projectId":"p","generation":"g","version":"1","highWatermark":"0",
                "data":{"type":"tasks","value":{"items":[],"nextCursor":null,"highWatermark":"0"}}}})).await.unwrap();
        });
        let bridge = bridge(path);
        let call=crate::policy::authorize("main","tauri://localhost/",json!({"operation":"native.query","message":{
            "protocol":{"major":2,"mapping":2},"projectId":"p","requestId":"read","operation":"tasks.page","payload":{"target":"p","cursor":null,"limit":50}}})).unwrap();
        assert_eq!(bridge.request(call).await.unwrap()["generation"], "g");
        serving.await.unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_remote_access_change_refreshes_the_window() {
        // Neither is a whole-window refresh: remote access rereads its own page, a project rereads only itself.
        let forwarded = |raw: serde_json::Value| match super::incoming(&event(raw)) {
            Ok(super::Incoming::Forward(forwarded)) => forwarded,
            _ => panic!("forwarded"),
        };
        assert_eq!(
            forwarded(serde_json::json!({"type":"remote_state"})),
            serde_json::json!({"type":"remote_state"})
        );
        assert_eq!(
            forwarded(serde_json::json!({"type":"project_state","projectId":"p"})),
            serde_json::json!({"type":"project_state","projectId":"p"})
        );
        // One it cannot name still rereads the window, and keeps the subscription.
        assert_eq!(
            forwarded(serde_json::json!({"type":"project_state"})),
            serde_json::json!({"type":"refresh"})
        );
    }

    /// A connector that refuses until the owner acts: shown as `unavailable` at once and
    /// once, never as a silent startup wait or an offline machine.
    #[tokio::test]
    async fn a_permanently_refused_connection_is_shown_unavailable_once() {
        struct Revoked;
        impl super::Connector for Revoked {
            fn connect(&self) -> super::BoxFuture<'_, Result<super::Stream, String>> {
                Box::pin(async { Err("device_rejected".to_string()) })
            }
            fn unavailable(&self) -> Option<String> {
                Some("revoked".into())
            }
        }
        let bridge = Bridge::phone(std::sync::Arc::new(Revoked));
        let events = std::sync::Mutex::new(Vec::new());
        let subscription = bridge.subscribe(|e| events.lock().unwrap().push(e));
        tokio::select! {
            _ = subscription => panic!("subscription ended"),
            _ = tokio::time::sleep(std::time::Duration::from_millis(2500)) => {}
        }
        let seen = events.lock().unwrap();
        assert_eq!(seen[0]["state"], "connecting");
        assert_eq!(
            seen.iter()
                .filter(|e| e["state"] == "unavailable")
                .collect::<Vec<_>>(),
            vec![
                &serde_json::json!({"type":"connection","state":"unavailable","reason":"revoked"})
            ]
        );
        assert!(!seen.iter().any(|e| e["state"] == "disconnected"));
    }
}
