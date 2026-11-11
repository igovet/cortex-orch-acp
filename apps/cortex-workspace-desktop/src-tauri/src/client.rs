use crate::opener;
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use tokio::{net::UnixStream, time::timeout};
use workspace_service::{connect, read_frame, write_frame};

/// What the owner is told for a refusal. A refusal changed nothing and the service is still there:
/// it is reported as itself, never as a lost service or an unknown outcome.
fn refusal(code: &str) -> Option<&'static str> {
    Some(match code {
        "project_folder_not_directory" => "This path is a file, not a folder.",
        "project_folder_unreadable" => "Cortex has no permission to open this folder.",
        "project_folder_not_writable" => "You cannot write to this folder, so it cannot hold a project.",
        "project_folder_exists" => "A folder with this name is already there.",
        "project_path_refused" => "This folder is private to Cortex or outside what this machine allows.",
        "absolute_path_required" => "Enter the full path of the folder, starting with / or ~/.",
        "invalid_path" | "symlink_refused" => "This path cannot be used. Enter the folder's real path.",
        "invalid_display_name" => "Enter a project name of up to 256 characters.",
        "project_activation_running" => "Cortex is already starting for this project.",
        "project_not_found" => "This project is no longer on this machine.",
        "project_inactive" | "resync_required" | "child_unavailable" => "Cortex is restarting for this project. Try again in a moment.",
        "machine_busy" | "machine_tool_timeout" => "The machine is busy and did not answer in time. Try again.",
        "machine_tool_failed" => "The machine could not finish this request.",
        "directory_search_timeout" => "The search took too long. Type more of the path.",
        "directory_search_failed" => "This machine could not search its folders.",
        "stale_version" => "This task changed on the machine. Refresh it before trying again.",
        "stale_epoch" | "execution_session_unavailable" => "This session no longer owns the task. Reopen its current state.",
        "task_paused" => "The task is paused. Resume it before sending or answering.",
        "pause_settling" => "The interrupted turn is still stopping. Wait before resuming.",
        "task_not_paused" => "This task is no longer paused. Refresh its current state.",
        "task_not_resumable" => "This task has ended and cannot be resumed.",
        "report_unavailable" => "No published worker report is available for this task.",
        "service_stopping" => "The machine service is stopping. Try again in a moment.",
        _ => return None,
    })
}
/// Marks an error as the machine's own answer, so a caller can tell it from a reply that never came.
const REFUSED: &str = "refused:";
/// Starting a child includes engine and store setup, which can outlast an ordinary request.
const ACTIVATION_DEADLINE: Duration = Duration::from_secs(120);
const REQUEST_DEADLINE: Duration = Duration::from_secs(35);
const RESYNC_DEADLINE: Duration = Duration::from_secs(5);

pub struct Bridge {
    socket: PathBuf,
    sequence: AtomicU64,
    start_unit: bool,
    starting: tokio::sync::Mutex<()>,
}
impl Bridge {
    /// The socket is resolved by trusted native startup, never a JavaScript argument.
    pub fn for_user() -> Result<Self, String> {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").ok_or("service_unavailable")?;
        if !PathBuf::from(&runtime).is_absolute() {
            return Err("service_unavailable".into());
        }
        let mut bridge = Self::new(
            workspace_protocol::channel::current()
                .runtime_dir(&PathBuf::from(runtime))
                .join("service.sock"),
        );
        bridge.start_unit = true;
        Ok(bridge)
    }
    pub fn new(socket: PathBuf) -> Self {
        Self {
            socket,
            sequence: AtomicU64::new(1),
            start_unit: false,
            starting: tokio::sync::Mutex::new(()),
        }
    }
    async fn connection(&self) -> Result<UnixStream, String> {
        if let Ok(stream) = connect(&self.socket).await {
            return Ok(stream);
        }
        if !self.start_unit {
            return Err("service_unavailable".into());
        }
        // Serialize startup, never replay the caller's RPC or expose a shell to JS.
        let _guard = self.starting.lock().await;
        if let Ok(stream) = connect(&self.socket).await {
            return Ok(stream);
        }
        Self::start_service(
            "/usr/bin/systemctl",
            &workspace_protocol::channel::current().unit,
        )
        .await?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
        while tokio::time::Instant::now() < deadline {
            if let Ok(stream) = connect(&self.socket).await {
                return Ok(stream);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Err("service_start_timeout".into())
    }
    async fn start_service(systemctl: &str, unit: &str) -> Result<(), String> {
        use std::process::Stdio;
        // Reload also handles first launch in a manager that predates package install.
        for args in [
            vec!["--user", "daemon-reload"],
            vec!["--user", "start", unit],
        ] {
            let mut child = tokio::process::Command::new(systemctl)
                .args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .map_err(|_| "service_manager_unavailable")?;
            let status = timeout(Duration::from_secs(8), child.wait())
                .await
                .map_err(|_| "service_manager_timeout")?
                .map_err(|_| "service_manager_unavailable")?;
            if !status.success() {
                return Err("service_start_failed".into());
            }
        }
        Ok(())
    }
    async fn send(
        &self,
        stream: &mut UnixStream,
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
        if method.starts_with("voice.") && reply["jsonrpc"] == "2.0" && reply["id"] == id {
            if let Some(code @ ("voice_busy" | "voice_unavailable" | "voice_session_unknown" | "voice_request_expired")) = reply["error"]["message"].as_str() {
                return Err(code.into());
            }
        }
        if method.starts_with("terminal.") && reply["jsonrpc"] == "2.0" && reply["id"] == id {
            let message = match reply["error"]["message"].as_str() {
                Some("terminal_session_limit") => Some("Eight project shells are already open. Close a shell first."),
                Some("terminal_session_unknown") => Some("This terminal session is gone. Reconnect to open a new shell."),
                Some("terminal_shell_unavailable" | "terminal_unavailable") => Some("Your interactive shell could not be opened."),
                Some("terminal_exited") => Some("The shell exited. Close it before opening a new shell."),
                Some("terminal_input_refused") => Some("Terminal input was refused. Nothing was resent."),
                Some("terminal_resize_failed") => Some("The terminal could not be resized."),
                Some("terminal_project_changed" | "project_not_found") => Some("This terminal's project is no longer available."),
                _ => None,
            };
            if let Some(message) = message { return Err(message.into()); }
        }
        if reply["error"]["message"]
            .as_str()
            .is_some_and(|message| message.starts_with("dev_project_refused"))
        {
            return Err(format!("{REFUSED}Cortex Workspace Dev did not open this folder: its .cortex-dev store was not created by Dev on this machine, so it was left untouched. Released Cortex data in .cortex is never the reason."));
        }
        if reply["jsonrpc"] == "2.0" && reply["id"] == id {
            match reply["error"]["message"].as_str() {
                // These two steer the caller, so they stay codes.
                Some(code @ ("project_folder_missing" | "unknown_outcome")) => return Err(code.into()),
                Some(code) => if let Some(text) = refusal(code) { return Err(format!("{REFUSED}{text}")); },
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
        timeout(if method == "projects.activate" { ACTIVATION_DEADLINE } else { REQUEST_DEADLINE }, async {
            let mut stream = self.connection().await?;
            self.send(&mut stream, method, params).await
        })
        .await
        .map_err(|_| "service_timeout")?
    }
    pub async fn request(&self, call: crate::policy::Call) -> Result<Value, String> {
        match call.operation.as_str() {
            "service.status"
            | "projects.register"
            | "projects.remove"
            | "projects.rename"
            | "projects.list"
            | "projects.status"
            | "projects.activate"
            | "projects.resynchronize"
            | "executions.cancel"
            | "executions.control"
            | "executions.prompt"
            | "machine.call"
            | "directories.search"
            | "directories.cancel"
            | "voice.start"
            | "voice.read"
            | "voice.stop"
            | "voice.cancel"
            | "terminal.open"
            | "terminal.read"
            | "terminal.write"
            | "terminal.resize"
            | "terminal.close" => self.rpc(&call.operation, call.message).await,
            "service.capabilities" => {
                let mut value = self.rpc("service.capabilities", call.message).await?;
                if let Some(methods) = value["methods"].as_array_mut() { methods.push(json!("native.clipboard_image")); methods.push(json!("native.take_dropped_files")); methods.push(json!("native.pick_folder")); methods.push(json!("native.save_attachment")); }
                let vs_code = opener::vs_code().is_some();
                let system = opener::system_opener().is_some();
                if let Some(methods) = value["methods"].as_array_mut() {
                    if vs_code { methods.push(json!("native.open_editor.vscode")); }
                    if vs_code || system { methods.push(json!("native.open_editor")); }
                    if system { methods.push(json!("native.open_logs")); }
                }
                Ok(value)
            }
            "native.engine_login" => {
                let reply = self.rpc("machine.call", json!({"op":"engine.connection","params":{"action":"login_command","engine":call.message["engine"]}})).await?;
                if reply["ok"] != true { return Err(format!("{REFUSED}{}", reply["message"].as_str().unwrap_or("Native login is unavailable."))); }
                let program = reply["result"]["program"].as_str().ok_or("Native login is unavailable.")?;
                let args: Vec<std::ffi::OsString> = reply["result"]["args"].as_array().ok_or("Native login is unavailable.")?.iter().map(|v| v.as_str().unwrap_or("").into()).collect();
                opener::engine_login(program, &args).await.map_err(|s| format!("{REFUSED}{s}"))?;
                Ok(json!({"message":"Native login opened in a terminal. Finish there, then recheck here."}))
            },
            "native.open_editor" => self.open_editor(&call.message).await.map_err(|text| format!("{REFUSED}{text}")),
            "native.open_logs" => self.open_logs().await.map_err(|text| format!("{REFUSED}{text}")),
            "capabilities.get" => {
                let mut value = self.capabilities().await?;
                if call.message["protocol"]["minor"].as_u64().is_some_and(|minor| minor <= 6) {
                    value["limits"]["frameBytes"] = json!(1024 * 1024);
                }
                Ok(value)
            },
            _ => self.rpc("workspace", call.message).await,
        }
    }
    /// Where to open, as the service resolves it. A plain project opens without Git; a checkout or file needs its answer.
    async fn located(&self, message: &Value) -> Result<(String, Option<String>), String> {
        if message.get("checkoutId").is_none() && message.get("file").is_none() {
            let projects = self.rpc("projects.list", json!({})).await?;
            let project = projects.as_array().and_then(|rows| rows.iter().find(|p| p["projectId"] == message["projectId"])).ok_or("This project is no longer on this machine.")?;
            return Ok((project["canonicalRoot"].as_str().ok_or("This project has no folder.")?.to_owned(), None));
        }
        let mut params = serde_json::Map::new();
        for key in ["checkoutId", "file"] {
            if let Some(value) = message.get(key) { params.insert(key.to_owned(), value.clone()); }
        }
        let reply = self.rpc("machine.call", json!({"op":"repo.locate","projectId":message["projectId"],"params":params})).await?;
        if reply["ok"] != true { return Err(reply["message"].as_str().unwrap_or("This file could not be found.").to_owned()); }
        let root = reply["result"]["root"].as_str().ok_or("This file could not be found.")?.to_owned();
        Ok((root, reply["result"]["path"].as_str().map(str::to_owned)))
    }
    async fn open_editor(&self, message: &Value) -> Result<Value, String> {
        let (root, file) = self.located(message).await?;
        if let Some(editor) = opener::vs_code() {
            let args = opener::vs_code_args(&root, file.as_deref(), message["line"].as_u64().unwrap_or(1));
            opener::launch(&editor, &args, "VS Code could not open this.").await?;
        } else {
            let (program, args) = opener::system_opener().ok_or("No external editor or file opener is installed on this machine.")?;
            let mut args: Vec<std::ffi::OsString> = args.iter().map(Into::into).collect();
            args.push(file.unwrap_or(root).into());
            opener::launch(&program, &args, "The default application could not open this.").await?;
        }
        Ok(json!({"outcome":"opened"}))
    }
    async fn open_logs(&self) -> Result<Value, String> {
        let reply = self.rpc("machine.call", json!({"op":"logs.folder"})).await?;
        if reply["ok"] != true { return Err("Private service logs are unavailable.".into()); }
        let folder = reply["result"]["folder"].as_str().ok_or("Private service logs are unavailable.")?;
        let (program, args) = opener::system_opener().ok_or("No folder opener is installed on this machine.")?;
        let mut args: Vec<std::ffi::OsString> = args.iter().map(Into::into).collect();
        args.push(folder.into());
        opener::launch(&program, &args, "The folder opener could not open logs.").await?;
        Ok(json!({"outcome":"opened"}))
    }
    async fn capabilities(&self) -> Result<Value, String> {
        let projects = self.rpc("projects.list", json!({})).await?;
        let projects = projects.as_array().ok_or("service_invalid_response")?;
        // projects.status carries a negotiated subset, not a full adapter Capabilities.
        // Publish the native shell's own identity with only that observed intersection.
        let mut shared = workspace_protocol::capabilities(
            &format!(
                "workspace-desktop-1.0.6-{}",
                workspace_protocol::channel::current().name
            ),
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
        let ack = timeout(
            Duration::from_secs(5),
            self.send(&mut stream, "service.subscribe", json!({})),
        )
        .await
        .map_err(|_| "service_timeout")??;
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
                .map_err(|_| "service_disconnected")?;
            if message["jsonrpc"] != "2.0" || message["method"] != "service.event" {
                return Err("service_invalid_event".into());
            }
            let event = &message["params"];
            match event["type"].as_str() {
                Some("resync_required") => self.resync(notify).await?,
                Some("project_state") => notify(json!({"type":"refresh"})),
                Some("invalidation" | "public_events") => {
                    workspace_protocol::validate("Identity", &event["projectId"])
                        .map_err(|_| "service_invalid_event")?;
                    notify(json!({"type":"invalidated","projectId":event["projectId"]}));
                }
                _ => return Err("service_invalid_event".into()),
            }
        }
    }
    async fn resync(&self, notify: &impl Fn(Value)) -> Result<(), String> {
        let projects = self.rpc("projects.list", json!({})).await?;
        for project in projects
            .as_array()
            .ok_or("service_invalid_response")?
            .iter()
            .filter(|p| !p["activation"].is_null())
        {
            // One project's child being away is that project's state, not a lost machine. The
            // service brings it back and announces it. A stalled child does not hold this loop
            // either: the service resnapshots by itself once the child answers.
            let _ = timeout(
                RESYNC_DEADLINE,
                self.rpc("projects.resynchronize", json!({"projectId":project["projectId"]})),
            )
            .await;
            notify(json!({"type":"invalidated","projectId":project["projectId"]}));
        }
        Ok(())
    }
    pub async fn subscribe(&self, notify: impl Fn(Value)) {
        loop {
            let _ = self.subscription(&notify).await;
            notify(
                json!({"type":"connection","state":"disconnected","reason":"Machine service disconnected"}),
            );
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Bridge;
    use std::os::unix::fs::PermissionsExt;

    #[tokio::test]
    async fn startup_reloads_and_starts_only_the_fixed_unit_and_retries_after_failure() {
        let root = std::env::temp_dir().join(format!("cw-start-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let script = root.join("systemctl");
        let trace = root.join("trace");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexit 1\n",
                trace.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            Bridge::start_service(script.to_str().unwrap(), "fixture.service")
                .await
                .is_err()
        );
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n",
                trace.display()
            ),
        )
        .unwrap();
        Bridge::start_service(script.to_str().unwrap(), "fixture.service")
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(&trace).unwrap(),
            "--user daemon-reload\n--user daemon-reload\n--user start fixture.service\n"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
