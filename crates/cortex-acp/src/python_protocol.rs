//! Python-entry (`cortex_runtime.acp.protocol`/`agent`) request validation and
//! error wording. Only the protocol envelope is shared: native semantics and
//! later refusals still apply, and no check here admits work.
use serde_json::{json, Value};
use std::io;

const MAX_IMAGE_ENCODED: usize = (10usize * 1024 * 1024).div_ceil(3) * 4;
const MODES: &str = "['development', 'guarded', 'auto']";

fn refuse(message: impl Into<String>) -> io::Error {
    io::Error::other(message.into())
}
fn nonempty(p: &Value, key: &str) -> bool {
    p[key].as_str().is_some_and(|s| !s.is_empty())
}
fn need(method: &str, p: &Value, key: &str) -> io::Result<()> {
    if nonempty(p, key) {
        Ok(())
    } else {
        Err(refuse(format!(
            "{method} requires a non-empty string {key}."
        )))
    }
}
fn mcp_servers(p: &Value) -> io::Result<()> {
    let servers = match p.get("mcpServers") {
        None | Some(Value::Null) => return Ok(()),
        Some(Value::Array(servers)) => servers,
        Some(_) => return Err(refuse("mcpServers must be an array.")),
    };
    for entry in servers {
        if !nonempty(entry, "name") {
            return Err(refuse(
                "Each mcpServers entry needs a non-empty string name.",
            ));
        }
        if entry.get("command").is_some_and(|c| !c.is_string()) {
            return Err(refuse("An mcpServers entry's command must be a string."));
        }
    }
    Ok(())
}
/// Python `base64.b64decode(data, validate=True)`.
fn standard_base64(data: &str) -> bool {
    let body = data.trim_end_matches('=');
    data.len() - body.len() <= 2
        && data.len() % 4 == 0
        && body
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
}
/// Python `content_blocks_to_text` validation, in its order and wording.
fn content_blocks(blocks: &Value) -> io::Result<()> {
    let Some(blocks) = blocks.as_array().filter(|a| !a.is_empty()) else {
        return Err(refuse(
            "prompt must be a non-empty array of content blocks.",
        ));
    };
    for block in blocks {
        let Some(kind) = block["type"].as_str() else {
            return Err(refuse("Each prompt content block needs a string type."));
        };
        match kind {
            "text" if !block["text"].is_string() => {
                return Err(refuse("A text content block needs a string text field."))
            }
            "resource_link" if !block["name"].is_string() || !block["uri"].is_string() => {
                return Err(refuse(
                    "A resource_link content block needs string name and uri fields.",
                ))
            }
            "resource" => {
                let resource = &block["resource"];
                if !resource.is_object() || !resource["uri"].is_string() {
                    return Err(refuse("A resource content block needs an embedded resource object with a string uri field."));
                }
                if !resource["text"].is_string() && !resource["blob"].is_string() {
                    return Err(refuse(
                        "A resource content block needs an embedded 'text' or 'blob' field.",
                    ));
                }
            }
            "image" => {
                let Some(data) = block["data"].as_str().filter(|d| !d.is_empty()) else {
                    return Err(refuse(
                        "An image content block needs a non-empty base64 data string.",
                    ));
                };
                if !block["mimeType"]
                    .as_str()
                    .is_some_and(|m| m.starts_with("image/"))
                {
                    return Err(refuse("An image content block needs an image/* mimeType."));
                }
                // Python measures the decoded str length in code points.
                if data.chars().count() > MAX_IMAGE_ENCODED {
                    return Err(refuse(
                        "An image content block exceeds Cortex's 10 MiB limit.",
                    ));
                }
                if !standard_base64(data) {
                    return Err(refuse(
                        "An image content block data field must be standard base64.",
                    ));
                }
            }
            "text" | "resource_link" => {}
            other => {
                return Err(refuse(format!(
                    "Content block type '{other}' is not accepted; only text, resource_link, resource, and image are supported; audio is not supported."
                )))
            }
        }
    }
    Ok(())
}
/// Parameter checks the Python entry performs before any session lookup.
pub(crate) fn validate(method: &str, p: &Value) -> io::Result<()> {
    match method {
        "session/new" => {
            need(method, p, "cwd")?;
            mcp_servers(p)
        }
        "session/load" => {
            need(method, p, "sessionId")?;
            need(method, p, "cwd")?;
            mcp_servers(p)
        }
        "session/prompt" => {
            need(method, p, "sessionId")?;
            content_blocks(&p["prompt"])
        }
        "session/cancel" | "session/set_mode" => {
            need(method, p, "sessionId")?;
            if method == "session/set_mode" {
                need(method, p, "modeId")?;
            }
            Ok(())
        }
        "session/set_config_option" => {
            need(method, p, "sessionId")?;
            need(method, p, "configId")?;
            if p["type"] == "boolean" {
                if !p["value"].is_boolean() {
                    return Err(refuse(
                        "A type: 'boolean' set_config_option request requires a boolean value.",
                    ));
                }
            } else if !nonempty(p, "value") {
                return Err(refuse(
                    "set_config_option requires a non-empty string value (or type: 'boolean').",
                ));
            }
            Ok(())
        }
        _ => Ok(()),
    }
}
/// Python `repr()` of a string.
fn repr(s: &str) -> String {
    crate::startup::repr(&json!(s))
}
fn storage_error(id: &Value, code: i64, error: &io::Error) -> Option<Value> {
    let diagnostic = workspace_service::incoming::storage_diagnostic(error)?;
    Some(json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":diagnostic.message,
        "data":{"reason":diagnostic.reason,"retryable":diagnostic.retryable}}}))
}

pub(crate) fn request_error(id: Value, method: &str, error: &io::Error) -> Value {
    let reason = error.to_string();
    if reason == "legacy_session_read_only" {
        return json!({"jsonrpc":"2.0","id":id,"error":{
            "code":-32602,
            "message":"This chat was created by an earlier version of Cortex. It is read-only history and no longer supports new work. To continue, open a new chat and create a new task there.",
            "data":{"reason":reason}
        }});
    }
    let (code, message) = envelope(method, &reason);
    storage_error(&id, code, error).unwrap_or_else(|| crate::error(id, code, &message))
}

/// Translate a native synchronous refusal into Python's `(code, message)`.
fn envelope(method: &str, native: &str) -> (i64, String) {
    match native {
        "method_not_found" => (-32601, format!("Unknown method '{method}'.")),
        "Method 'authenticate' is not supported." => (-32601, native.into()),
        "not_initialized" => (-32600, "Call initialize before any other method.".into()),
        "workspace_not_activated" => (-32000, native.into()),
        "unknown_session" => (-32602, "Unknown sessionId.".into()),
        "invalid_cwd" => (
            -32602,
            "cwd must be an absolute, canonical, existing directory.".into(),
        ),
        other => (-32602, other.into()),
    }
}
/// A saved portable coordinator that cannot bind refuses a route choice with
/// Python's `PortableError` envelope: `(code, remediation)`.
pub(crate) fn portable_selection(native: &str) -> Option<(&str, &'static str)> {
    let code = native.strip_prefix("portable_selection:")?;
    Some((
        code,
        match code {
            "portable_coordinator_ambiguous" => {
                "Multiple current connections/routes match; explicitly select one."
            }
            "unsupported_dependent_setting" => {
                "The configured coordinator parameters are not all observed on this route."
            }
            _ => "The configured coordinator is not currently observed/selectable.",
        },
    ))
}
/// Translate a configuration refusal, which Python words with its inputs.
pub(crate) fn config_message(native: &str, set_mode: bool, key: &str, value: &str) -> String {
    let known = matches!(
        key,
        "provider" | "model" | "thought_level" | "language" | "access_mode"
    );
    match native {
        "unknown_session" => "Unknown sessionId.".into(),
        "selected_route_unavailable" => "The selected route is no longer selectable.".into(),
        "unknown_config_option" => format!("Unknown configId {}.", repr(key)),
        "unsupported_config_value" if set_mode => format!(
            "{} is not a valid mode; expected one of {MODES}.",
            repr(value)
        ),
        "unsupported_config_value" if known => format!(
            "{} is not a valid value for configId {}.",
            repr(value),
            repr(key)
        ),
        other => other.into(),
    }
}
/// Human-readable configuration detail for a slash-command chat reply only.
pub(crate) fn command_config_message(native: &str, key: &str, value: &str, ru: bool) -> String {
    if !ru {
        return config_message(native, false, key, value);
    }
    match native {
        "unknown_session" => "Сеанс не найден.".into(),
        "selected_route_unavailable" => "Выбранный маршрут больше недоступен.".into(),
        "unknown_config_option" => format!("Неизвестный configId {}.", repr(key)),
        "unsupported_config_value" => format!(
            "{} — недопустимое значение для configId {}.",
            repr(value),
            repr(key)
        ),
        other => other.into(),
    }
}
/// Python `_run_prompt_body`/`_run_cortex_new_task` error envelopes.
///
/// A prompt no native route can serve is the state Python reports as
/// `no_configuration_revision`: no route was applied to the task before
/// orchestration (Python's first prompt and `/cortex-new-task` both reach it
/// when nothing is selectable). The native refusal code is not on the wire.
pub(crate) fn prompt_error(id: Value, error: &io::Error) -> Value {
    if let Some(error) = storage_error(&id, -32603, error) {
        return error;
    }
    let native = error.to_string();
    let error = if native.starts_with("orchestration_unavailable:") {
        json!({"code":-32001,"message":"orchestration_unavailable","data":{"retryable":false,
            "reason":"no_configuration_revision",
            "remediation":"This task has no current configuration revision; select a route before starting orchestration."}})
    } else if native == "new_task_unavailable" {
        json!({"code":-32001,"message":"orchestration_unavailable",
            "data":{"retryable":false,"reason":"no_configuration_revision"}})
    } else if let Some(reason) = native.strip_prefix("image_capture_rejected:") {
        // Refused before any task, source or transcript row exists.
        json!({"code":-32602,"message":"Image content could not be captured within Cortex limits.",
            "data":{"reason":reason,"retryable":false}})
    } else if native == "orchestration_unavailable" {
        json!({"code":-32001,"message":native})
    } else if native == crate::task_commands::BUSY {
        json!({"code":-32602,"message":native})
    } else {
        json!({"code":-32603,"message":native})
    };
    json!({"jsonrpc":"2.0","id":id,"error":error})
}
/// Python `protocol.content_blocks_to_text`: the exact user text Python
/// records for a prompt, with markers for non-text blocks.
pub(crate) fn prompt_text(blocks: &Value) -> String {
    blocks
        .as_array()
        .into_iter()
        .flatten()
        .map(|b| {
            let s = |v: &Value| v.as_str().unwrap_or_default().to_owned();
            match b["type"].as_str() {
                Some("resource_link") => {
                    format!("[resource_link: {} ({})]", s(&b["name"]), s(&b["uri"]))
                }
                Some("resource") => format!("[resource: {}]", s(&b["resource"]["uri"])),
                Some("image") => format!("[image: {}]", s(&b["mimeType"])),
                _ => s(&b["text"]),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}
/// Native route-resolution refusals that Python reports as orchestration_unavailable.
pub(crate) fn route_unavailable(native: &str) -> bool {
    native.contains("route")
        || native.contains("policy")
        || matches!(
            native,
            "connection_unavailable" | "portable_coordinator_unavailable"
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(method: &str, p: Value) -> String {
        validate(method, &p).unwrap_err().to_string()
    }

    #[test]
    fn python_prompt_validation_order_and_wording() {
        assert_eq!(
            message(
                "session/prompt",
                json!({"prompt":[{"type":"text","text":"x"}]})
            ),
            "session/prompt requires a non-empty string sessionId."
        );
        assert_eq!(
            message("session/prompt", json!({"sessionId":"s","prompt":[]})),
            "prompt must be a non-empty array of content blocks."
        );
        assert!(message(
            "session/prompt",
            json!({"sessionId":"s","prompt":[{"type":"audio"}]})
        )
        .starts_with("Content block type 'audio' is not accepted"));
        assert_eq!(
            message(
                "session/prompt",
                json!({"sessionId":"s","prompt":[{"type":"image","data":"AA=","mimeType":"image/png"}]})
            ),
            "An image content block data field must be standard base64."
        );
        assert!(validate(
            "session/prompt",
            &json!({"sessionId":"s","prompt":[{"type":"image","data":"AAA=","mimeType":"image/png"}]})
        )
        .is_ok());
    }

    #[test]
    fn python_config_and_envelope_wording() {
        assert_eq!(
            message(
                "session/set_config_option",
                json!({"sessionId":"s","configId":"language"})
            ),
            "set_config_option requires a non-empty string value (or type: 'boolean')."
        );
        assert_eq!(
            config_message("unsupported_config_value", false, "language", "xx"),
            "'xx' is not a valid value for configId 'language'."
        );
        assert_eq!(
            config_message("unsupported_config_value", true, "access_mode", "bogus"),
            "'bogus' is not a valid mode; expected one of ['development', 'guarded', 'auto']."
        );
        assert_eq!(
            envelope("nope/x", "method_not_found").1,
            "Unknown method 'nope/x'."
        );
        assert_eq!(envelope("session/new", "not_initialized").0, -32600);
        assert!(
            route_unavailable("connection_unavailable") && !route_unavailable("invalid_prompt")
        );
    }
}
