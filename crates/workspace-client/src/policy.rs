//! The closed set of requests a shell's web interface may send, checked before anything
//! reaches the machine service.
//!
//! Two profiles share one check: [`authorize`] for the desktop window and
//! [`authorize_phone`] for the phone app, which additionally refuses what a phone must not
//! even send (see [`phone_allows`]). The machine service applies its own allowlist to
//! every request that arrives through the remote tunnel; this one is the phone's first,
//! not its only, gate.
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
    // One level of named checkout folders for Code's tree; the service confines every name.
    "repo.list",
    // Names and descriptions of the project's own skills, for the composer's `/` list.
    "repo.skills",
    "git",
    "workers",
    "product.read",
    "product.board",
    "product.facts",
    "product.task",
    "product.integrate",
    "product.sprint",
    "product.message",
    "product.comment",
    // Uploads an owner attachment once and reads one body back by digest.
    "product.attachment",
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

/// Owner files travel as references `{name, mimeType, size, sha256}`; the service checks each one and
/// reads its body. A reference is checked for shape here and then stands in as an empty inline body, so
/// the rest of the message gets the same schema check. Inline bytes are refused: they go only to
/// `product.attachment` upload.
fn references_as_bodies(value: &Value) -> Result<Value, String> {
    Ok(match value {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, v) in map {
                let v = if key == "attachments" && v.is_array() {
                    let items = v.as_array().unwrap();
                    let mut bodies = Vec::with_capacity(items.len());
                    for item in items {
                        let ok = item.as_object().is_some_and(|m| {
                            m.len() == 4
                                && m.get("name").is_some_and(Value::is_string)
                                && m.get("mimeType").is_some_and(Value::is_string)
                                && m.get("size").and_then(Value::as_u64).is_some_and(|s| {
                                    s <= workspace_protocol::ATTACHMENT_BYTES as u64
                                })
                                && m.get("sha256").and_then(Value::as_str).is_some_and(|s| {
                                    s.len() == 64
                                        && s.bytes().all(|b| {
                                            b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
                                        })
                                })
                        });
                        if !ok {
                            return Err(refused());
                        }
                        bodies.push(serde_json::json!({"name":item["name"],"mimeType":item["mimeType"],"data":""}));
                    }
                    Value::Array(bodies)
                } else {
                    references_as_bodies(v)?
                };
                out.insert(key.clone(), v);
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(references_as_bodies)
                .collect::<Result<_, _>>()?,
        ),
        other => other.clone(),
    })
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

/// Bound on a pairing request or device id the owner's window sends back.
const REMOTE_ID_BYTES: usize = 128;
/// Bound on a relay URL the owner types.
const REMOTE_RELAY_BYTES: usize = 2048;

/// The owner's remote-access settings: exact shapes, nothing else.
fn remote(call: &Call) -> Result<(), String> {
    let map = call.message.as_object().ok_or_else(refused)?;
    let id = |key: &str| {
        map.get(key).and_then(Value::as_str).is_some_and(|text| {
            !text.is_empty()
                && text.len() <= REMOTE_ID_BYTES
                && text.bytes().all(|b| b.is_ascii_graphic())
        })
    };
    let ok = match call.operation.as_str() {
        "remote.status" | "remote.pairing.start" | "remote.pairing.cancel" => map.is_empty(),
        "remote.enable" => map.len() == 1 && map.get("enabled").is_some_and(Value::is_boolean),
        "remote.sudo.set" => map.len() == 1 && map.get("allowed").is_some_and(Value::is_boolean),
        "remote.pairing.decide" => {
            map.len() == 2 && id("requestId") && map.get("allow").is_some_and(Value::is_boolean)
        }
        "remote.devices.revoke" => map.len() == 1 && id("deviceId"),
        "remote.relay.set" => {
            map.len() == 1
                && map.get("url").is_some_and(|url| {
                    url.is_null()
                        || url.as_str().is_some_and(|text| {
                            !text.is_empty()
                                && text.len() <= REMOTE_RELAY_BYTES
                                && !text.chars().any(char::is_control)
                        })
                })
        }
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(refused())
    }
}

/// Longest sudo password a window sends (the service applies the same bound).
const SUDO_PASSWORD_BYTES: usize = 1024;

/// `askpass.list` with no parameters, or `askpass.answer` naming one prompt with either a
/// password or `cancel: true`.
fn sudo_prompt(call: &Call) -> Result<(), String> {
    let map = call.message.as_object().ok_or_else(refused)?;
    let id = map
        .get("promptId")
        .and_then(Value::as_str)
        .is_some_and(|id| {
            !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_hexdigit())
        });
    let ok = match call.operation.as_str() {
        "askpass.list" => map.is_empty(),
        "askpass.answer" => {
            id && map.len() == 2
                && (map.get("cancel") == Some(&Value::Bool(true))
                    || map
                        .get("password")
                        .and_then(Value::as_str)
                        .is_some_and(|p| {
                            !p.is_empty()
                                && p.len() <= SUDO_PASSWORD_BYTES
                                && !p.contains(['\n', '\r', '\0'])
                        }))
        }
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(refused())
    }
}

/// Longest sign-in token a window sends (the service applies the same bound).
const OWNER_TOKEN_BYTES: usize = 4096;

/// `owner_actions.list` with no parameters, or `owner_actions.act` naming one project and
/// action with `recheck`, `terminal` or `dismiss`, or `token` with the pasted token.
fn owner_action(call: &Call) -> Result<(), String> {
    let map = call.message.as_object().ok_or_else(refused)?;
    let id = |key: &str| {
        map.get(key).and_then(Value::as_str).is_some_and(|t| {
            !t.is_empty() && t.len() <= 96 && t.bytes().all(|b| b.is_ascii_graphic())
        })
    };
    let ok = match call.operation.as_str() {
        "owner_actions.list" => map.is_empty(),
        "owner_actions.act" => {
            id("projectId")
                && id("actionId")
                && match (map.len(), map.get("action").and_then(Value::as_str)) {
                    (3, Some("recheck" | "terminal" | "dismiss")) => true,
                    (4, Some("token")) => {
                        map.get("token").and_then(Value::as_str).is_some_and(|t| {
                            !t.is_empty()
                                && t.len() <= OWNER_TOKEN_BYTES
                                && !t.contains(['\n', '\r', '\0'])
                        })
                    }
                    _ => false,
                }
        }
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(refused())
    }
}

/// The desktop window's requests: only the main window at the bundled origin.
pub fn authorize(window: &str, url: &str, body: Value) -> Result<Call, String> {
    if window != "main" || !local_url(url) {
        return Err(refused());
    }
    authorize_body(body, false)
}

/// The phone app's requests. The phone shell checks its own web view's origin before it
/// calls this. On top of the desktop rules, everything [`phone_allows`] refuses is
/// refused here, before a byte is sent.
pub fn authorize_phone(body: Value) -> Result<Call, String> {
    authorize_body(body, true)
}

/// `machine.call` operations a phone never sends, beyond those the desktop list lacks.
fn phone_machine_operation(op: &str, params: &Value) -> bool {
    let action = params.get("action").and_then(Value::as_str);
    match op {
        // Code is read-only from the phone; every Git action is a write; the logs folder
        // exists for the desktop's own opener.
        "repo.file.save" | "git" | "logs.folder" => false,
        // A native login opens a terminal on the machine.
        "engine.connection" => matches!(action, Some("recheck" | "add_key")),
        // Installing or rolling back a package raises a privilege prompt on the machine,
        // and an unconfirmed install is acknowledged by the owner at the machine.
        "maintenance" => match action {
            Some("voice.status" | "update.status" | "update.check" | "read" | "cancel") => true,
            Some("start") => matches!(
                params.get("kind").and_then(Value::as_str),
                Some("clone" | "download")
            ),
            _ => false,
        },
        _ => true,
    }
}

/// Whether a phone may send this call at all: no terminal, voice or `remote.*` operation,
/// no desktop-only `native.*` operation (folder picker, attachment save dialog,
/// clipboard image, dropped files, engine login, editor and log openers, notifications),
/// and none of the `machine.call` operations a phone must not send (file saves, Git
/// writes, native logins, privilege prompts).
pub fn phone_allows(call: &Call) -> bool {
    if !phone_method(&call.operation) {
        return false;
    }
    if call.operation == "machine.call" {
        let Some(op) = call.message.get("op").and_then(Value::as_str) else {
            return false;
        };
        return phone_machine_operation(op, call.message.get("params").unwrap_or(&Value::Null));
    }
    // An owner action's terminal login opens in the desktop's own terminal.
    if call.operation == "owner_actions.act"
        && call.message.get("action") == Some(&Value::from("terminal"))
    {
        return false;
    }
    true
}

/// Whether a method or operation name is one a phone may use at all (a `machine.call`'s
/// own operation is decided by [`phone_allows`]).
pub fn phone_method(operation: &str) -> bool {
    !(operation.starts_with("terminal.")
        || operation.starts_with("voice.")
        || operation.starts_with("remote.")
        || (operation.starts_with("native.")
            && !matches!(operation, "native.query" | "native.board"))
        || operation.starts_with("notifications.")
        || operation == "service.stop")
}

fn authorize_body(body: Value, phone: bool) -> Result<Call, String> {
    if serde_json::to_vec(&body).map_err(|_| refused())?.len() > workspace_protocol::FRAME_BYTES {
        return Err(refused());
    }
    let call: Call = serde_json::from_value(body).map_err(|_| refused())?;
    if phone && !phone_allows(&call) {
        return Err(refused());
    }
    if call.operation.starts_with("remote.") {
        remote(&call)?;
        return Ok(call);
    }
    // The owner's sudo password prompts: list them, or answer one with a password or a
    // cancel. Exact shapes; the password is passed through, never kept or logged here.
    if call.operation.starts_with("askpass.") {
        return sudo_prompt(&call).map(|()| call);
    }
    // Owner actions: list them, or act on one. Exact shapes; a pasted token is passed
    // through once, never kept or logged here.
    if call.operation.starts_with("owner_actions.") {
        return owner_action(&call).map(|()| call);
    }
    // Desktop notifications: what the focused window shows and the kinds the owner wants, and
    // the target kept for a window the service started from a click. Exact shapes; the
    // service checks every id again.
    if call.operation == "notifications.take" && call.message == serde_json::json!({}) {
        return Ok(call);
    }
    if call.operation == "notifications.report" {
        let map = call.message.as_object().ok_or_else(refused)?;
        let id = |v: Option<&Value>| {
            v.and_then(Value::as_str).is_some_and(|t| {
                !t.is_empty()
                    && t.len() <= 256
                    && !t.chars().any(|c| c.is_control() || c.is_whitespace())
            })
        };
        let focused = match map.get("focused") {
            Some(Value::Null) => true,
            Some(Value::Object(p)) => {
                id(p.get("project")) && p.len() == 2 && (id(p.get("task")) ^ id(p.get("sprint")))
            }
            _ => false,
        };
        let kinds = map.get("kinds").and_then(Value::as_array).is_some_and(|k| {
            k.len() <= 7
                && k.iter().all(|k| {
                    matches!(
                        k.as_str(),
                        Some(
                            "question"
                                | "stop"
                                | "result"
                                | "failed"
                                | "planning"
                                | "paused"
                                | "stalled"
                        )
                    )
                })
        });
        // The no-progress threshold in hours, when the window names one.
        let stall = match map.get("stallHours") {
            None => Some(2),
            Some(h) => matches!(h.as_u64(), Some(1 | 2 | 3 | 6)).then_some(3),
        };
        if stall != Some(map.len()) || !focused || !kinds {
            return Err(refused());
        }
        return Ok(call);
    }
    if call.operation == "native.engine_login" {
        let map = call.message.as_object().ok_or_else(refused)?;
        if map.len() != 1
            || !matches!(
                call.message["engine"].as_str(),
                Some("codex-acp" | "claude-agent-acp")
            )
        {
            return Err(refused());
        }
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
        workspace_protocol::validate("Identity", &call.message["projectId"])
            .map_err(|_| refused())?;
        let name = map
            .get("displayName")
            .and_then(Value::as_str)
            .ok_or_else(refused)?;
        if map.len() != 2 || name.trim().is_empty() || name.len() > 1024 {
            return Err(refused());
        }
        return Ok(call);
    }
    if matches!(
        call.operation.as_str(),
        "projects.remove" | "native.open_editor"
    ) {
        let map = call.message.as_object().ok_or_else(refused)?;
        workspace_protocol::validate("Identity", &call.message["projectId"])
            .map_err(|_| refused())?;
        if call.operation == "projects.remove" {
            if map.len() != 2 || call.message["confirmed"] != true {
                return Err(refused());
            }
        } else {
            // A checkout, file and line only name what to open. The service still resolves and confines the file.
            let bounded = |key: &str, limit: usize| {
                map.get(key).is_none_or(|v| {
                    v.as_str().is_some_and(|text| {
                        !text.is_empty() && text.len() <= limit && !text.contains('\0')
                    })
                })
            };
            if map
                .keys()
                .any(|key| !matches!(key.as_str(), "projectId" | "checkoutId" | "file" | "line"))
                || !bounded("checkoutId", 256)
                || !bounded("file", 4096)
                || map.get("line").is_some_and(|line| {
                    !line.as_u64().is_some_and(|n| (1..=10_000_000).contains(&n))
                })
            {
                return Err(refused());
            }
        }
        return Ok(call);
    }
    // A task's recorded pull request, opened in the system browser. Only the project and the
    // task are named; the service reads and checks the address, the shell never takes one.
    if call.operation == "native.open_pull_request" {
        let map = call.message.as_object().ok_or_else(refused)?;
        workspace_protocol::validate("Identity", &call.message["projectId"])
            .map_err(|_| refused())?;
        workspace_protocol::validate("Identity", &call.message["taskId"]).map_err(|_| refused())?;
        let task = call.message["taskId"].as_str().unwrap_or("");
        if map.len() != 2
            || !task
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_:.-".contains(&b))
        {
            return Err(refused());
        }
        return Ok(call);
    }
    if call.operation == "native.save_attachment" {
        // The name is only a suggestion for the save dialog. The owner still chooses where it goes.
        let map = call.message.as_object().ok_or_else(refused)?;
        let name = map
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(refused)?;
        let data = map
            .get("data")
            .and_then(Value::as_str)
            .ok_or_else(refused)?;
        if map.len() != 2
            || name.trim().is_empty()
            || name.chars().count() > 255
            || name
                .chars()
                .any(|c| c.is_control() || c == '/' || c == '\\')
            || data.len() > (workspace_protocol::ATTACHMENT_BYTES + 2) / 3 * 4
        {
            return Err(refused());
        }
        return Ok(call);
    }
    if matches!(
        call.operation.as_str(),
        "native.open_logs"
            | "native.clipboard_image"
            | "native.take_dropped_files"
            | "native.pick_folder"
    ) && call.message == serde_json::json!({})
    {
        return Ok(call);
    }
    // The owner's typed text is the whole request: the service chooses every root and limit.
    if call.operation.starts_with("directories.") {
        workspace_protocol::directories::parse(&call.operation, call.message.clone())
            .map_err(|_| refused())?;
        return Ok(call);
    }
    if call.operation.starts_with("voice.") {
        workspace_protocol::voice::parse(&call.operation, call.message.clone())
            .map_err(|_| refused())?;
        return Ok(call);
    }
    if call.operation.starts_with("terminal.") {
        workspace_protocol::terminal::parse(&call.operation, call.message.clone())
            .map_err(|_| refused())?;
        return Ok(call);
    }
    // The owner lifts one of their own write denials: the rule's text as recorded, which the
    // runtime matches exactly against its own denials. It names no location to open. A phone
    // sends it too: it already answers permission requests and writes `allow-write:` in a
    // message, so the control grants it nothing more.
    if call.operation == "executions.control" && call.message["action"] == "allow_write" {
        let map = call.message.as_object().ok_or_else(refused)?;
        if map.len() != 5
            || map.keys().any(|key| {
                !matches!(
                    key.as_str(),
                    "projectId" | "executionId" | "requestId" | "action" | "path"
                )
            })
            || !call.message["path"].as_str().is_some_and(|text| {
                !text.trim().is_empty() && text.len() <= 4096 && !text.chars().any(char::is_control)
            })
        {
            return Err(refused());
        }
        for key in ["projectId", "executionId", "requestId"] {
            workspace_protocol::validate("Identity", &call.message[key]).map_err(|_| refused())?;
        }
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
        "projects.status" | "projects.activate" => {
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
            if map.keys().any(|key| {
                !matches!(
                    key.as_str(),
                    "projectId"
                        | "executionId"
                        | "requestId"
                        | "action"
                        | "workerId"
                        | "text"
                        | "reason"
                        | "toolCallRef"
                        | "settings"
                        | "attachments"
                        | "expectedVersion"
                        | "confirmed"
                ) && !(rerun
                    && matches!(
                        key.as_str(),
                        "checkIndex" | "checkName" | "digest" | "resultVersion"
                    ))
            }) {
                return Err(refused());
            }
            if rerun {
                if !call.message["checkIndex"]
                    .as_u64()
                    .is_some_and(|index| index < 100)
                    || !call.message["checkName"].as_str().is_some_and(|text| {
                        !text.trim().is_empty() && text.len() <= 1024 && !text.contains('\0')
                    })
                    || !call.message["digest"].as_str().is_some_and(|text| {
                        !text.is_empty() && text.len() <= 256 && !text.contains('\0')
                    })
                {
                    return Err(refused());
                }
                workspace_protocol::validate("PositiveDecimal", &call.message["resultVersion"])
                    .map_err(|_| refused())?;
            }
            for key in ["projectId", "executionId", "requestId"] {
                workspace_protocol::validate("Identity", &call.message[key])
                    .map_err(|_| refused())?;
            }
            if let Some(items) = call.message.get("attachments") {
                workspace_protocol::validate(
                    "Attachments",
                    &references_as_bodies(&serde_json::json!({"attachments":items}))?
                        ["attachments"],
                )
                .map_err(|_| refused())?;
            }
            if matches!(call.message["action"].as_str(), Some("pause" | "resume")) {
                workspace_protocol::validate("PositiveDecimal", &call.message["expectedVersion"])
                    .map_err(|_| refused())?;
            }
            if call.message["action"] == "task_settings" {
                workspace_protocol::validate("CreatePayload", &serde_json::json!({"description":"settings","settings":call.message["settings"]})).map_err(|_| refused())?;
            }
            if !matches!(
                call.message["action"].as_str(),
                Some(
                    "worker_message"
                        | "worker_stop"
                        | "command_stop"
                        | "task_settings"
                        | "pause"
                        | "resume"
                        | "close_task"
                        | "mark_resolved"
                        | "retry"
                        | "rerun_check"
                )
            ) {
                return Err(refused());
            }
        }
        "executions.cancel" | "executions.prompt" => {
            let map = call.message.as_object().ok_or_else(refused)?;
            let allowed = if call.operation == "executions.prompt" {
                &[
                    "projectId",
                    "executionId",
                    "text",
                    "requestId",
                    "attachments",
                ][..]
            } else {
                &["projectId", "executionId"][..]
            };
            if map.keys().any(|key| !allowed.contains(&key.as_str())) {
                return Err(refused());
            }
            workspace_protocol::validate("Identity", &call.message["projectId"])
                .map_err(|_| refused())?;
            workspace_protocol::validate("Identity", &call.message["executionId"])
                .map_err(|_| refused())?;
            if let Some(items) = call.message.get("attachments") {
                workspace_protocol::validate(
                    "Attachments",
                    &references_as_bodies(&serde_json::json!({"attachments":items}))?
                        ["attachments"],
                )
                .map_err(|_| refused())?;
            }
            if call.operation == "executions.prompt"
                && !call.message["text"]
                    .as_str()
                    .is_some_and(|text| !text.is_empty())
            {
                return Err(refused());
            }
        }
        "capabilities.get" => {
            let map = call.message.as_object().ok_or_else(refused)?;
            if map.len() != 2
                || !map.contains_key("requestId")
                || !map.contains_key("protocol")
                || call.message["protocol"]
                    != serde_json::json!({"major":workspace_protocol::WIRE_MAJOR,"minor":workspace_protocol::WIRE_MINOR})
            {
                return Err(refused());
            }
            workspace_protocol::validate("Identity", &call.message["requestId"])
                .map_err(|_| refused())?;
        }
        "native.query" | "native.board" => {
            workspace_protocol::core::decode_native_query(
                &serde_json::to_vec(&call.message).map_err(|_| refused())?,
            )
            .map_err(|_| refused())?;
        }
        "project.snapshot" | "tasks.list" | "tasks.get" | "executions.get" | "decisions.list"
        | "decisions.get" | "results.get" | "reports.read" | "history.page" | "events.page"
        | "requests.get" | "tasks.create" | "tasks.start" | "decisions.answer" => {
            let command = matches!(
                call.operation.as_str(),
                "tasks.create" | "tasks.start" | "decisions.answer"
            );
            workspace_protocol::validate(
                if command { "Command" } else { "Query" },
                &references_as_bodies(&call.message)?,
            )
            .map_err(|_| refused())?;
            if call.message["operation"] != call.operation
                || call.message["protocol"]
                    != serde_json::json!({"major":workspace_protocol::WIRE_MAJOR,"minor":workspace_protocol::WIRE_MINOR})
            {
                return Err(refused());
            }
        }
        _ => return Err(refused()),
    }
    Ok(call)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ORIGIN: &str = "tauri://localhost/";

    fn desktop(operation: &str, message: Value) -> bool {
        authorize(
            "main",
            ORIGIN,
            json!({"operation":operation,"message":message}),
        )
        .is_ok()
    }
    fn phone(operation: &str, message: Value) -> bool {
        authorize_phone(json!({"operation":operation,"message":message})).is_ok()
    }

    /// A conversation read from its end goes through `native.query` with the history page's bounds, from the desktop
    /// and from a phone alike; an unknown field or an oversized page is refused.
    #[test]
    fn a_backward_history_read_passes_as_a_bounded_native_query() {
        let read = |payload: Value| json!({"protocol":{"major":2,"mapping":2},"requestId":"r","projectId":"p","operation":"history.earlier","payload":payload});
        for allowed in [desktop, phone] {
            assert!(allowed(
                "native.query",
                read(json!({"target":"t","cursor":null,"limit":100}))
            ));
            assert!(!allowed(
                "native.query",
                read(json!({"target":"t","cursor":null,"limit":101}))
            ));
            assert!(!allowed(
                "native.query",
                read(json!({"target":"t","cursor":null,"limit":100,"before":"x"}))
            ));
        }
    }

    #[test]
    fn the_desktop_owner_sends_exact_remote_settings_requests() {
        for operation in [
            "remote.status",
            "remote.pairing.start",
            "remote.pairing.cancel",
        ] {
            assert!(desktop(operation, json!({})), "{operation}");
            assert!(!desktop(operation, json!({"x":1})), "{operation}");
        }
        assert!(desktop("remote.enable", json!({"enabled":true})));
        assert!(!desktop("remote.enable", json!({"enabled":1})));
        assert!(!desktop("remote.enable", json!({"enabled":true,"x":1})));
        assert!(desktop(
            "remote.pairing.decide",
            json!({"requestId":"r1","allow":false})
        ));
        assert!(!desktop("remote.pairing.decide", json!({"requestId":"r1"})));
        assert!(!desktop(
            "remote.pairing.decide",
            json!({"requestId":"","allow":true})
        ));
        assert!(!desktop(
            "remote.pairing.decide",
            json!({"requestId":"r".repeat(129),"allow":true})
        ));
        assert!(desktop("remote.devices.revoke", json!({"deviceId":"abc"})));
        assert!(!desktop("remote.devices.revoke", json!({"deviceId":"a b"})));
        assert!(desktop("remote.relay.set", json!({"url":null})));
        assert!(desktop(
            "remote.relay.set",
            json!({"url":"https://relay.example"})
        ));
        assert!(!desktop("remote.relay.set", json!({})));
        assert!(!desktop("remote.relay.set", json!({"url":""})));
        assert!(!desktop("remote.relay.set", json!({"url":7})));
        assert!(!desktop("remote.unknown", json!({})));
        assert!(desktop("remote.sudo.set", json!({"allowed":false})));
        assert!(!desktop("remote.sudo.set", json!({"allowed":"no"})));
        assert!(!phone("remote.sudo.set", json!({"allowed":true})));
        // The origin rule applies to remote settings too.
        assert!(authorize(
            "other",
            ORIGIN,
            json!({"operation":"remote.status","message":{}})
        )
        .is_err());
    }

    #[test]
    fn the_desktop_keeps_its_machine_bound_operations() {
        assert!(desktop("native.pick_folder", json!({})));
        // A pull request is named by its task only; an address from the page is refused.
        assert!(desktop(
            "native.open_pull_request",
            json!({"projectId":"p","taskId":"t"})
        ));
        for message in [
            json!({"projectId":"p","taskId":"t","url":"https://example.invalid/"}),
            json!({"projectId":"p"}),
            json!({"projectId":"p","taskId":"../t"}),
        ] {
            assert!(
                !desktop("native.open_pull_request", message.clone()),
                "{message}"
            );
        }
        assert!(desktop(
            "native.engine_login",
            json!({"engine":"codex-acp"})
        ));
        assert!(desktop(
            "machine.call",
            json!({"op":"repo.file.save","projectId":"p","params":{}})
        ));
        assert!(desktop(
            "machine.call",
            json!({"op":"git","projectId":"p","params":{"action":"commit"}})
        ));
    }

    #[test]
    fn the_desktop_owner_lifts_a_write_denial_in_an_exact_shape() {
        let lift = json!({"projectId":"p","executionId":"e","requestId":"r","action":"allow_write","path":"test/run.js"});
        assert!(desktop("executions.control", lift.clone()));
        for (key, value) in [
            ("path", json!("")),
            ("path", json!(" ")),
            ("path", json!("a\nb")),
            ("path", json!(7)),
            ("path", json!("x".repeat(4097))),
            ("executionId", json!("")),
            ("cwd", json!("/tmp")),
            ("text", json!("hello")),
        ] {
            let mut bad = lift.clone();
            bad[key] = value;
            assert!(!desktop("executions.control", bad), "{key}");
        }
        let mut missing = lift.clone();
        missing.as_object_mut().unwrap().remove("path");
        assert!(!desktop("executions.control", missing));
        // Another action never carries a path.
        let mut other = lift.clone();
        other["action"] = json!("worker_stop");
        assert!(!desktop("executions.control", other));
        // The phone sends the same exact shape, and nothing looser.
        assert!(phone("executions.control", lift.clone()));
        let mut loose = lift;
        loose["cwd"] = json!("/tmp");
        assert!(!phone("executions.control", loose));
    }

    #[test]
    fn a_phone_never_sends_machine_bound_or_owner_only_operations() {
        for (operation, message) in [
            ("remote.status", json!({})),
            ("remote.enable", json!({"enabled":true})),
            ("remote.devices.revoke", json!({"deviceId":"abc"})),
            (
                "terminal.open",
                json!({"projectId":"p","requestId":"r","cols":80,"rows":24}),
            ),
            ("voice.start", json!({"requestId":"voice-1"})),
            ("native.pick_folder", json!({})),
            ("native.save_attachment", json!({"name":"a.txt","data":""})),
            ("native.clipboard_image", json!({})),
            ("native.take_dropped_files", json!({})),
            ("native.engine_login", json!({"engine":"codex-acp"})),
            ("native.open_logs", json!({})),
            ("native.open_editor", json!({"projectId":"p"})),
            (
                "native.open_pull_request",
                json!({"projectId":"p","taskId":"t"}),
            ),
            ("native.notify", json!({})),
            ("service.stop", json!({})),
        ] {
            assert!(!phone(operation, message), "{operation}");
        }
        let machine = |op: &str, params: Value| {
            phone(
                "machine.call",
                json!({"op":op,"projectId":"p","params":params}),
            )
        };
        assert!(!machine(
            "repo.file.save",
            json!({"file":"a","text":"","revision":null})
        ));
        for action in [
            "commit",
            "stage",
            "revert",
            "push",
            "pull",
            "fetch",
            "branch",
            "draft",
            "delete_worktree",
        ] {
            assert!(!machine("git", json!({"action":action})), "{action}");
        }
        assert!(!machine("logs.folder", json!({})));
        assert!(!machine(
            "engine.connection",
            json!({"action":"login_command","engine":"codex-acp"})
        ));
        assert!(!machine(
            "maintenance",
            json!({"action":"start","kind":"install","id":"a"})
        ));
        assert!(!machine(
            "maintenance",
            json!({"action":"start","kind":"rollback","id":"a"})
        ));
        assert!(!machine(
            "maintenance",
            json!({"action":"acknowledge","id":"a","confirmed":true})
        ));
        assert!(!phone("machine.call", json!({"projectId":"p"})));
        assert!(!phone("brand.new", json!({})));
    }

    #[test]
    fn a_phone_reads_and_steers_work() {
        assert!(phone("service.status", json!({})));
        assert!(phone("service.capabilities", json!({})));
        assert!(phone("projects.list", json!({})));
        assert!(phone(
            "projects.register",
            json!({"root":"~/code/x","displayName":"x"})
        ));
        assert!(phone(
            "directories.search",
            json!({"session":"s","query":"code"})
        ));
        assert!(phone(
            "executions.prompt",
            json!({"projectId":"p","executionId":"e","text":"go"})
        ));
        let machine = |op: &str, params: Value| {
            phone(
                "machine.call",
                json!({"op":op,"projectId":"p","params":params}),
            )
        };
        for op in [
            "overview",
            "config.read",
            "config.save",
            "repo.snapshot",
            "repo.file",
            "repo.list",
            "product.read",
            "product.task",
            "product.integrate",
            "product.attachment",
        ] {
            assert!(machine(op, json!({})), "{op}");
        }
        assert!(machine(
            "engine.connection",
            json!({"action":"recheck","engine":"codex-acp"})
        ));
        assert!(machine("maintenance", json!({"action":"update.check"})));
        assert!(phone_method("machine.call"));
        assert!(!phone_method("terminal.open"));
        assert!(!phone_method("remote.status"));
    }

    /// A sudo password prompt is listed and answered from the desktop and the phone in
    /// exact shapes only.
    #[test]
    fn sudo_prompts_are_listed_and_answered_in_exact_shapes() {
        for allowed in [desktop, phone] {
            assert!(allowed("askpass.list", json!({})));
            assert!(!allowed("askpass.list", json!({"x":1})));
            assert!(allowed(
                "askpass.answer",
                json!({"promptId":"ab12","password":"pw with spaces"})
            ));
            assert!(allowed(
                "askpass.answer",
                json!({"promptId":"ab12","cancel":true})
            ));
            for bad in [
                json!({"promptId":"ab12"}),
                json!({"promptId":"ab12","cancel":false}),
                json!({"promptId":"ab12","password":""}),
                json!({"promptId":"ab12","password":"a\nb"}),
                json!({"promptId":"ab12","password":"x".repeat(1025)}),
                json!({"promptId":"zz","password":"x"}),
                json!({"promptId":"ab12","password":"x","cancel":true}),
            ] {
                assert!(!allowed("askpass.answer", bad.clone()), "{bad}");
            }
            assert!(!allowed("askpass.other", json!({})));
        }
    }

    /// Owner actions are listed and acted on in exact shapes; a phone never asks for the
    /// desktop terminal login.
    #[test]
    fn owner_actions_are_listed_and_acted_on_in_exact_shapes() {
        let act = |action: &str| json!({"projectId":"wp_1","actionId":"oa_1","action":action});
        let token =
            |t: &str| json!({"projectId":"wp_1","actionId":"oa_1","action":"token","token":t});
        for allowed in [desktop, phone] {
            assert!(allowed("owner_actions.list", json!({})));
            assert!(!allowed("owner_actions.list", json!({"x":1})));
            for action in ["recheck", "dismiss"] {
                assert!(allowed("owner_actions.act", act(action)), "{action}");
            }
            assert!(allowed("owner_actions.act", token("ghp_x")));
            for bad in [
                act("other"),
                token(""),
                token("a\nb"),
                token(&"x".repeat(4097)),
                json!({"projectId":"wp_1","actionId":"oa_1","action":"recheck","token":"t"}),
                json!({"projectId":"wp_1","action":"recheck"}),
                json!({"projectId":"wp 1","actionId":"oa_1","action":"recheck"}),
            ] {
                assert!(!allowed("owner_actions.act", bad.clone()), "{bad}");
            }
            assert!(!allowed("owner_actions.other", json!({})));
        }
        assert!(desktop("owner_actions.act", act("terminal")));
        assert!(!phone("owner_actions.act", act("terminal")));
    }
}
