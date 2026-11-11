use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Call {
    pub operation: String,
    pub message: Value,
}

/// Linux production origin only; no development server or remote content.
pub fn local_url(url: &str) -> bool {
    // Tauri 2.10 loads index.html at the bare Linux custom-protocol origin.
    url == "tauri://localhost"
        || url
            .strip_prefix("tauri://localhost/")
            .is_some_and(|tail| !tail.contains('\\'))
}
fn refused() -> String {
    "bridge_request_refused".into()
}

/// The closed set the machine helper answers. A new action is added here on purpose.
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
    "project.config.read",
    "project.config.save",
    "repo.snapshot",
    "repo.file",
    "repo.file.save",
    "git",
    "workers",
    "product.read",
    "product.facts",
    "product.task",
    "product.integrate",
    "product.sprint",
    "product.message",
    "product.comment",
];

fn project_identity(message: &Value) -> Result<(), String> {
    if let Some(id) = message.get("projectId") {
        let id = id.as_str().ok_or_else(refused)?;
        if id.is_empty()
            || !id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-:.".contains(&c))
            || matches!(id, "." | "..")
        {
            return Err(refused());
        }
    }
    Ok(())
}

fn forbidden(value: &Value) -> bool {
    match value {
        Value::Object(map) => map.iter().any(|(key, value)| {
            let key = key.to_ascii_lowercase().replace(['_', '-'], "");
            key.contains("path")
                || key.contains("launch")
                || key.contains("command")
                || key.contains("credential")
                || key.ends_with("directory")
                || matches!(
                    key.as_str(),
                    "path"
                        | "root"
                        | "cwd"
                        | "socketpath"
                        | "command"
                        | "argv"
                        | "args"
                        | "executable"
                        | "launch"
                        | "credentials"
                        | "env"
                        | "environment"
                )
                || forbidden(value)
        }),
        Value::Array(items) => items.iter().any(forbidden),
        _ => false,
    }
}

pub fn authorize(window: &str, url: &str, body: Value) -> Result<Call, String> {
    if window != "main" || !local_url(url) {
        return Err(refused());
    }
    if serde_json::to_vec(&body).map_err(|_| refused())?.len() > workspace_protocol::FRAME_BYTES {
        return Err(refused());
    }
    let call: Call = serde_json::from_value(body).map_err(|_| refused())?;
    if call.operation == "native.engine_login" {
        let map = call.message.as_object().ok_or_else(refused)?;
        if map.len() != 1 || !matches!(call.message["engine"].as_str(), Some("codex-acp" | "claude-agent-acp")) { return Err(refused()); }
        return Ok(call);
    }
    if call.operation == "projects.register" {
        let map = call.message.as_object().ok_or_else(refused)?;
        let root = map
            .get("root")
            .and_then(Value::as_str)
            .ok_or_else(refused)?;
        let name = map
            .get("displayName")
            .and_then(Value::as_str)
            .ok_or_else(refused)?;
        let create = map.get("create");
        let new_folder = map.get("newFolder");
        if create.is_some_and(|value| !value.is_boolean())
            || new_folder.is_some_and(|value| !value.is_boolean())
            || map.len() != 2 + usize::from(create.is_some()) + usize::from(new_folder.is_some())
            || !(root.starts_with('/') || root == "~" || root.starts_with("~/"))
            || root.len() > 4096
            || root.contains('\0')
            || name.trim().is_empty()
            || name.len() > 128
        {
            return Err(refused());
        }
        return Ok(call);
    }
    // Machine actions carry file paths inside a checkout by design. The service resolves every
    // root itself and the helper refuses anything outside it, so only the envelope is checked here.
    if call.operation == "machine.call" {
        let map = call.message.as_object().ok_or_else(refused)?;
        let op = map.get("op").and_then(Value::as_str).ok_or_else(refused)?;
        if !MACHINE_OPERATIONS.contains(&op)
            || map
                .keys()
                .any(|key| !matches!(key.as_str(), "op" | "projectId" | "params"))
        {
            return Err(refused());
        }
        project_identity(&call.message)?;
        return Ok(call);
    }
    if call.operation == "projects.rename" {
        let map = call.message.as_object().ok_or_else(refused)?;
        workspace_protocol::validate("Identity", &call.message["projectId"]).map_err(|_| refused())?;
        let name = map.get("displayName").and_then(Value::as_str).ok_or_else(refused)?;
        if map.len() != 2 || name.trim().is_empty() || name.len() > 1024 { return Err(refused()); }
        return Ok(call);
    }
    if matches!(call.operation.as_str(), "projects.remove" | "native.open_editor") {
        let map = call.message.as_object().ok_or_else(refused)?;
        workspace_protocol::validate("Identity", &call.message["projectId"]).map_err(|_| refused())?;
        if call.operation == "projects.remove" {
            if map.len() != 2 || call.message["confirmed"] != true { return Err(refused()); }
        } else {
            // A checkout, file and line only name what to open. The service still resolves and confines the file.
            let bounded = |key: &str, limit: usize| map.get(key).is_none_or(|v| v.as_str().is_some_and(|text| !text.is_empty() && text.len() <= limit && !text.contains('\0')));
            if map.keys().any(|key| !matches!(key.as_str(), "projectId" | "checkoutId" | "file" | "line"))
                || !bounded("checkoutId", 256)
                || !bounded("file", 4096)
                || map.get("line").is_some_and(|line| !line.as_u64().is_some_and(|n| (1..=10_000_000).contains(&n)))
            {
                return Err(refused());
            }
        }
        return Ok(call);
    }
    if call.operation == "native.save_attachment" {
        // The name is only a suggestion for the save dialog. The owner still chooses where it goes.
        let map = call.message.as_object().ok_or_else(refused)?;
        let name = map.get("name").and_then(Value::as_str).ok_or_else(refused)?;
        let data = map.get("data").and_then(Value::as_str).ok_or_else(refused)?;
        if map.len() != 2 || name.trim().is_empty() || name.chars().count() > 255 || name.chars().any(|c| c.is_control() || c == '/' || c == '\\') || data.len() > (workspace_protocol::ATTACHMENT_BYTES + 2) / 3 * 4 {
            return Err(refused());
        }
        return Ok(call);
    }
    if matches!(call.operation.as_str(), "native.open_logs" | "native.clipboard_image" | "native.take_dropped_files" | "native.pick_folder") && call.message == serde_json::json!({}) { return Ok(call); }
    // The owner's typed text is the whole request: the service chooses every root and limit.
    if call.operation.starts_with("directories.") {
        workspace_protocol::directories::parse(&call.operation, call.message.clone()).map_err(|_| refused())?;
        return Ok(call);
    }
    if call.operation.starts_with("voice.") {
        workspace_protocol::voice::parse(&call.operation, call.message.clone()).map_err(|_| refused())?;
        return Ok(call);
    }
    if call.operation.starts_with("terminal.") {
        workspace_protocol::terminal::parse(&call.operation, call.message.clone()).map_err(|_| refused())?;
        return Ok(call);
    }
    if forbidden(&call.message) {
        return Err(refused());
    }
    project_identity(&call.message)?;
    match call.operation.as_str() {
        "service.status" | "service.capabilities" | "projects.list" => {
            if call.message != serde_json::json!({}) {
                return Err(refused());
            }
        }
        "projects.status" | "projects.activate" | "projects.resynchronize" => {
            let map = call.message.as_object().ok_or_else(refused)?;
            if map.len() != 1 {
                return Err(refused());
            }
            workspace_protocol::validate("Identity", &call.message["projectId"])
                .map_err(|_| refused())?;
        }
        "executions.control" => {
            let map = call.message.as_object().ok_or_else(refused)?;
            let rerun = call.message["action"] == "rerun_check";
            if map.keys().any(|key| !matches!(key.as_str(), "projectId" | "executionId" | "requestId" | "action" | "workerId" | "text" | "reason" | "toolCallRef" | "settings" | "attachments" | "expectedVersion" | "confirmed")
                && !(rerun && matches!(key.as_str(), "checkIndex" | "checkName" | "digest" | "resultVersion"))) {
                return Err(refused());
            }
            if rerun {
                if !call.message["checkIndex"].as_u64().is_some_and(|index| index < 100)
                    || !call.message["checkName"].as_str().is_some_and(|text| !text.trim().is_empty() && text.len() <= 1024 && !text.contains('\0'))
                    || !call.message["digest"].as_str().is_some_and(|text| !text.is_empty() && text.len() <= 256 && !text.contains('\0')) {
                    return Err(refused());
                }
                workspace_protocol::validate("PositiveDecimal", &call.message["resultVersion"]).map_err(|_| refused())?;
            }
            for key in ["projectId", "executionId", "requestId"] {
                workspace_protocol::validate("Identity", &call.message[key]).map_err(|_| refused())?;
            }
            if let Some(items) = call.message.get("attachments") {
                workspace_protocol::validate("Attachments", items).map_err(|_| refused())?;
            }
            if matches!(call.message["action"].as_str(), Some("pause" | "resume")) {
                workspace_protocol::validate("PositiveDecimal", &call.message["expectedVersion"]).map_err(|_| refused())?;
            }
            if call.message["action"] == "task_settings" {
                workspace_protocol::validate("CreatePayload", &serde_json::json!({"description":"settings","settings":call.message["settings"]})).map_err(|_| refused())?;
            }
            if !matches!(call.message["action"].as_str(), Some("worker_message" | "worker_stop" | "command_stop" | "task_settings" | "pause" | "resume" | "close_task" | "rerun_check")) {
                return Err(refused());
            }
        }
        "executions.cancel" | "executions.prompt" => {
            let map = call.message.as_object().ok_or_else(refused)?;
            let allowed = if call.operation == "executions.prompt" { &["projectId", "executionId", "text", "requestId", "attachments"][..] } else { &["projectId", "executionId"][..] };
            if map.keys().any(|key| !allowed.contains(&key.as_str())) {
                return Err(refused());
            }
            workspace_protocol::validate("Identity", &call.message["projectId"])
                .map_err(|_| refused())?;
            workspace_protocol::validate("Identity", &call.message["executionId"])
                .map_err(|_| refused())?;
            if let Some(items) = call.message.get("attachments") {
                workspace_protocol::validate("Attachments", items).map_err(|_| refused())?;
            }
            if call.operation == "executions.prompt"
                && !call.message["text"].as_str().is_some_and(|text| !text.is_empty())
            {
                return Err(refused());
            }
        }
        "capabilities.get" => {
            let map = call.message.as_object().ok_or_else(refused)?;
            if map.len() != 2
                || !map.contains_key("requestId")
                || !map.contains_key("protocol")
                || call.message["protocol"] != serde_json::json!({"major":workspace_protocol::WIRE_MAJOR,"minor":workspace_protocol::WIRE_MINOR})
            {
                return Err(refused());
            }
            workspace_protocol::validate("Identity", &call.message["requestId"])
                .map_err(|_| refused())?;
        }
        "project.snapshot" | "tasks.list" | "tasks.get" | "executions.get" | "decisions.list"
        | "decisions.get" | "results.get" | "reports.read" | "history.page" | "events.page" | "requests.get"
        | "tasks.create" | "tasks.start" | "decisions.answer" => {
            let command = matches!(
                call.operation.as_str(),
                "tasks.create" | "tasks.start" | "decisions.answer"
            );
            workspace_protocol::validate(if command { "Command" } else { "Query" }, &call.message)
                .map_err(|_| refused())?;
            if call.message["operation"] != call.operation
                || call.message["protocol"] != serde_json::json!({"major":workspace_protocol::WIRE_MAJOR,"minor":workspace_protocol::WIRE_MINOR})
            {
                return Err(refused());
            }
        }
        _ => return Err(refused()),
    }
    Ok(call)
}
