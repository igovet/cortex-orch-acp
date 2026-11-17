//! Python `acp.commands` recognition and reply text for the local commands
//! this host implements. A command never dispatches an engine or a task.
use serde_json::Value;

/// Python `_TRIGGERS`, in recognition priority order.
const TRIGGERS: [(&str, &str); 14] = [
    ("/cortex-clear", "cortex-clear"),
    ("/cortex-status", "cortex-status"),
    ("/cortex-new-task", "cortex-new-task"),
    ("/cortex-language", "cortex-language"),
    ("/skill-management", "skill-management"),
    ("/harvest-refresh", "harvest-refresh"),
    ("/harvest", "harvest"),
    ("/cortex-model", "cortex-model"),
    ("/cortex-effort", "cortex-effort"),
    ("/cortex-setup", "cortex-setup"),
    ("/cortex-stop", "cortex-stop"),
    ("/cortex-kill", "cortex-kill"),
    ("/cortex-finish", "cortex-finish"),
    ("/cortex-answer", "cortex-answer"),
];

/// Python `strip()`/`isspace()` whitespace, not only ASCII.
fn python_space(c: char) -> bool {
    c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)
}
/// Python `recognize`: exactly one text block whose trimmed text is a trigger,
/// alone or followed by a space, tab or newline. Anything else is a prompt.
pub(crate) fn recognize(blocks: &Value) -> Option<(&'static str, String)> {
    let [block] = blocks.as_array()?.as_slice() else {
        return None;
    };
    if block["type"] != "text" {
        return None;
    }
    let stripped = block["text"].as_str()?.trim_matches(python_space);
    for (trigger, id) in TRIGGERS {
        if stripped == trigger {
            return Some((id, String::new()));
        }
        if let Some(rest) = stripped.strip_prefix(trigger) {
            if rest.starts_with([' ', '\t', '\n']) {
                return Some((id, rest.trim_matches(python_space).to_owned()));
            }
        }
    }
    None
}
/// Python `resolve_language_argument`.
pub(crate) fn language(argument: &str) -> Option<String> {
    let value = argument.trim_matches(python_space).to_lowercase();
    matches!(value.as_str(), "auto" | "en" | "ru").then_some(value)
}
/// Python `resolve_model_argument`: an exact route id, else one case-insensitive
/// label match, else one engine model id. Never a best-effort guess.
pub(crate) fn model(argument: &str, entries: &[Value]) -> Option<String> {
    let argument = argument.trim_matches(python_space);
    if argument.is_empty() {
        return None;
    }
    if let Some(e) = entries.iter().find(|e| e["route"] == argument) {
        return e["route"].as_str().map(str::to_owned);
    }
    let lowered = argument.to_lowercase();
    let lower = |v: &Value| v.as_str().map(str::to_lowercase);
    let exact: Vec<_> = entries
        .iter()
        .filter(|e| {
            lower(&e["disambiguated_label"]).as_deref() == Some(&lowered)
                || lower(&e["label"]).as_deref() == Some(&lowered)
        })
        .collect();
    if exact.len() == 1 {
        return exact[0]["route"].as_str().map(str::to_owned);
    }
    let models: Vec<_> = entries
        .iter()
        .filter(|e| lower(&e["model"]).as_deref() == Some(&lowered))
        .collect();
    (models.len() == 1)
        .then(|| models[0]["route"].as_str().map(str::to_owned))
        .flatten()
}
fn count(states: &[String], state: &str) -> usize {
    states.iter().filter(|s| *s == state).count()
}
/// Python `_connection_setup_summary`.
pub(crate) fn connection_summary(states: &[String]) -> String {
    if states.is_empty() {
        return "no connection registered yet; see /cortex-setup".into();
    }
    let mut details = vec![format!(
        "{}/{} connection(s) configured",
        count(states, "configured"),
        states.len()
    )];
    for (state, text) in [
        ("authorization_required", "awaiting authorization"),
        ("unavailable", "unavailable"),
        ("invalid", "invalid"),
    ] {
        let n = count(states, state);
        if n > 0 {
            details.push(format!("{n} {text}"));
        }
    }
    details.join("; ")
}
/// Python `build_status_text`. The execution line states this host truthfully:
/// it does not run Python's engine jail.
pub(crate) fn status(options: &Value, summary: &str) -> String {
    let mut lines = vec!["Cortex configuration status:".to_owned()];
    for option in options.as_array().into_iter().flatten() {
        let current = &option["currentValue"];
        let label = option["options"]
            .as_array()
            .and_then(|a| a.iter().find(|c| c["value"] == *current))
            .map(|c| c["name"].clone())
            .unwrap_or_else(|| current.clone());
        let text = |v: &Value| {
            v.as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| v.to_string())
        };
        let note = option["description"].as_str().filter(|s| !s.is_empty());
        lines.push(format!(
            "- {}: {}{}",
            text(&option["id"]),
            text(&label),
            note.map(|n| format!(" {n}")).unwrap_or_default()
        ));
    }
    lines.push(format!("- connections: {summary}"));
    lines.push("- execution: Rust native host; engine processes are not jailed (not hostile-code isolation)".into());
    lines.join("\n")
}
const SETUP_GUIDANCE: &str = "No model is selectable yet for this project. To connect an engine:\n1. Install the pinned engine adapter with `cortex-paseo-tools install-engine --engine ENGINE`, then register its native account with `cortex-paseo-tools add-connection --engine ENGINE`.\n2. Complete the engine's native login in the isolated directory shown by setup.\n3. Run `cortex-paseo-tools validate-connection --connection ID` using the ID returned by setup. Once it reports 'configured', start a new Cortex session to refresh models.\nThis message never contains a secret and never mutates any connection.";
/// Python `build_setup_text`.
pub(crate) fn setup(states: &[String]) -> &'static str {
    if count(states, "authorization_required") > 0 {
        return "A connection is already registered but is awaiting authorization.\n1. Complete that engine's native login in the isolated engine directory shown during connection setup.\n2. Run `cortex-paseo-tools validate-connection --connection ID` using its existing ID until it reports 'configured'.\n3. Once configured, its models will start appearing in the model selector (or /cortex-model).\nDo not register another connection for the same engine and account. This message never contains a secret and never mutates any connection.";
    }
    if count(states, "unavailable") > 0 {
        return "A registered connection is currently unavailable.\nCheck that the configured engine is installed and reachable, then run `cortex-paseo-tools validate-connection --connection ID`. If the connection is no longer usable, repair or remove that registration before adding a replacement.\nThis message never contains a secret and never mutates any connection.";
    }
    if count(states, "invalid") > 0 {
        return "A registered connection has invalid configuration.\nReview its engine, provider, and authentication settings, repair the existing registration, then run `cortex-paseo-tools validate-connection --connection ID` again.\nThis message never contains a secret and never mutates any connection.";
    }
    SETUP_GUIDANCE
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn recognition_matches_python_shapes() {
        let one = |t: &str| json!([{"type":"text","text":t}]);
        assert_eq!(
            recognize(&one(" /cortex-status ")),
            Some(("cortex-status", String::new()))
        );
        assert_eq!(
            recognize(&one("/cortex-model\tfoo bar ")),
            Some(("cortex-model", "foo bar".into()))
        );
        assert_eq!(
            recognize(&one("/harvest-refresh x")),
            Some(("harvest-refresh", "x".into()))
        );
        assert_eq!(recognize(&one("/cortex-modelx")), None);
        assert_eq!(recognize(&one("/cortex-foo")), None);
        assert_eq!(
            recognize(&json!([{"type":"text","text":"/cortex-status"},{"type":"text","text":"x"}])),
            None
        );
        assert_eq!(language(" RU "), Some("ru".into()));
        assert_eq!(language("de"), None);
    }
    #[test]
    fn model_resolution_never_guesses() {
        let entries = vec![
            json!({"route":"xr_a","model":"m","label":"claude · a · m","disambiguated_label":"claude · a · m"}),
            json!({"route":"xr_b","model":"m","label":"codex · b · m","disambiguated_label":"codex · b · m"}),
            json!({"route":"xr_c","model":"Other","label":"x","disambiguated_label":"x"}),
        ];
        assert_eq!(model("xr_b", &entries).as_deref(), Some("xr_b"));
        assert_eq!(model("CODEX · B · M", &entries).as_deref(), Some("xr_b"));
        assert_eq!(model("m", &entries), None);
        assert_eq!(model("other", &entries).as_deref(), Some("xr_c"));
        assert_eq!(model("", &entries), None);
        assert_eq!(
            connection_summary(&[]),
            "no connection registered yet; see /cortex-setup"
        );
        assert_eq!(
            connection_summary(&["configured".into(), "invalid".into()]),
            "1/2 connection(s) configured; 1 invalid"
        );
    }
}
