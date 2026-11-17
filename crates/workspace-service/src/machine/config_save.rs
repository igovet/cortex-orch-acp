//! Machine settings writer. Choose the native provider once, before any effects.
use super::{native::*, project_config_save};
use serde_json::Value;

pub fn save(config: &crate::Config, params: &Value) -> Result<Value> {
    if !params.is_object() {
        return Err(refuse("machine_tool_failed", "AttributeError"));
    }
    match params["surface"].as_str() {
        Some("routing") => {
            let environment = super::config_read::environment(config)?;
            project_config_save::user_routing::save(config, &environment, params)
        }
        Some("user_defaults") => project_config_save::user_defaults::save(config, params),
        Some("mcp_servers") => project_config_save::user_mcp::save(config, params),
        _ => Err(refuse(
            "unsupported_action",
            "This machine does not support that yet.",
        )),
    }
}
