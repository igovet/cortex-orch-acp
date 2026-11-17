//! Engine `session/request_permission` classification: the runtime's final fence.
//! Python oracle: src/cortex_runtime/engine/permissions.py:300-539 and
//! acp/orchestrating_runtime.py:7248-7276. Identity comes only from what the
//! engine declares (tool name, kind, locations and command text), never from
//! titles. Core supplies authenticated channel facts and answers the request;
//! this module performs no I/O beyond resolving declared paths.
use regex::Regex;
use serde_json::{json, Value};
use std::{
    path::{Component, Path, PathBuf},
    sync::LazyLock,
};

/// How claude-agent-acp names a tool of the Cortex MCP server.
pub const CORTEX_MCP_TOOL_PREFIX: &str = "mcp__cortex__";
/// Claude's subagent tools and Codex's collaboration (multi-agent) tools.
pub const SUBAGENT_TOOLS: [&str; 16] = [
    "Agent",
    "Task",
    "Workflow",
    "TeamCreate",
    "TeamDelete",
    "SendMessage",
    "ScheduleWakeup",
    "Monitor",
    "CronCreate",
    "CronDelete",
    "CronList",
    "RemoteTrigger",
    "spawn_agent",
    "send_input",
    "close_agent",
    "resume_agent",
];
pub const NATIVE_QUESTION_TOOLS: [&str; 3] = [
    "AskUserQuestion",
    "request_user_input",
    "experimental_request_user_input",
];
const DELETION_TITLE: &str = "Removal blocked pending confirmation: ";

/// Authenticated channel facts. `runtime_answers_native` is true for engines
/// whose requests are ordinary per-tool approvals (claude-agent-acp in its
/// `default` floor). Other engines ask only to leave their own sandbox, so
/// their native requests keep the client route outside Auto.
pub struct Facts<'a> {
    pub access: &'a str,
    pub read_only: bool,
    pub consultant: bool,
    pub runtime_answers_native: bool,
    pub root: &'a Path,
    /// Names of the external MCP servers Cortex attached to this session.
    pub mcp_servers: &'a [String],
    /// Runtime/control-plane targets no native edit may reach outside Auto.
    pub protected: &'a dyn Fn(&Path) -> bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Select an offered allow option (one-shot preferred), else refuse.
    Allow,
    /// Select an offered reject option, else cancel.
    Refuse,
    /// Legacy refusal shape kept for engines the runtime does not answer.
    Cancel,
    /// Ask the connected owner client. A deletion is retitled and accepts
    /// only an offered allow option; without a client, `fallback` applies.
    Ask { deletion: bool, fallback: Fallback },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fallback {
    Refuse,
    Cancel,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Verdict {
    pub action: Action,
    /// Stable audit label; never model or engine text.
    pub rule: &'static str,
}
fn verdict(action: Action, rule: &'static str) -> Verdict {
    Verdict { action, rule }
}

/// The engine-declared tool name (`toolCall.name` or claude-agent-acp's
/// `_meta.claudeCode.toolName`), if any.
pub fn tool_name(request: &Value) -> Option<&str> {
    let call = &request["toolCall"];
    call["name"]
        .as_str()
        .or_else(|| call["_meta"]["claudeCode"]["toolName"].as_str())
}
pub fn tool_kind(request: &Value) -> Option<&str> {
    request["toolCall"]["kind"].as_str()
}
/// Claude Code's MCP tool prefix normalization.
fn normalized(server: &str) -> String {
    server
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}
/// A tool of the channel-bound Cortex gateway or of an attached external
/// server; both re-check the channel and operation before any effect.
fn channel_mcp(name: &str, servers: &[String]) -> bool {
    name.starts_with(CORTEX_MCP_TOOL_PREFIX)
        || servers
            .iter()
            .any(|s| !s.is_empty() && name.starts_with(&format!("mcp__{}__", normalized(s))))
}

pub fn classify(request: &Value, facts: &Facts<'_>) -> Verdict {
    let name = tool_name(request).unwrap_or("");
    let kind = tool_kind(request);
    if NATIVE_QUESTION_TOOLS.contains(&name) {
        return verdict(Action::Refuse, "native_question");
    }
    // The consultant works only from its packet: only Cortex MCP calls pass.
    if facts.consultant && !name.starts_with(CORTEX_MCP_TOOL_PREFIX) {
        return verdict(Action::Refuse, "consultant_native");
    }
    if SUBAGENT_TOOLS.contains(&name) {
        return verdict(Action::Refuse, "native_subagent");
    }
    // Gateway-owned consent (project_delete, command deletion, outside routes)
    // happens inside the call; the engine prompt is not a second card.
    if channel_mcp(name, facts.mcp_servers) {
        return verdict(Action::Allow, "channel_mcp");
    }
    // No engine jail bounds native tools here, so read-only channels keep the
    // read/search-only fence in every mode.
    if facts.read_only && !matches!(kind, Some("read" | "search")) {
        return verdict(
            if facts.runtime_answers_native {
                Action::Refuse
            } else {
                Action::Cancel
            },
            "read_only",
        );
    }
    if facts.access == "auto" {
        return verdict(Action::Allow, "auto");
    }
    if !facts.runtime_answers_native {
        return verdict(
            Action::Ask {
                deletion: false,
                fallback: Fallback::Cancel,
            },
            "client",
        );
    }
    if is_deletion(request) {
        return verdict(
            Action::Ask {
                deletion: true,
                fallback: Fallback::Refuse,
            },
            "deletion",
        );
    }
    let effect = match kind {
        Some("read" | "search" | "fetch" | "think" | "switch_mode") => Effect::None,
        Some("edit") => edit_effect(request, facts),
        _ => Effect::Uncontained,
    };
    match (facts.access, effect) {
        (_, Effect::None) => verdict(Action::Allow, "native_read"),
        (_, Effect::Protected) => verdict(Action::Refuse, "protected_target"),
        ("development", _) => verdict(Action::Allow, "native_development"),
        (_, Effect::Project) => verdict(Action::Allow, "native_project"),
        _ => verdict(
            Action::Ask {
                deletion: false,
                fallback: Fallback::Refuse,
            },
            "native_uncontained",
        ),
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Effect {
    None,
    Project,
    Protected,
    Uncontained,
}
fn edit_effect(request: &Value, facts: &Facts<'_>) -> Effect {
    let Some(locations) = request["toolCall"]["locations"].as_array() else {
        return Effect::Uncontained;
    };
    if locations.is_empty() {
        return Effect::Uncontained;
    }
    let Some(root) = resolved(facts.root) else {
        return Effect::Uncontained;
    };
    let mut effect = Effect::Project;
    for location in locations {
        let Some(path) = location["path"].as_str().and_then(resolved_str) else {
            return Effect::Uncontained;
        };
        if (facts.protected)(&path) {
            return Effect::Protected;
        }
        if !path.starts_with(&root) {
            effect = Effect::Uncontained;
        }
    }
    effect
}
fn resolved_str(path: &str) -> Option<PathBuf> {
    resolved(Path::new(path))
}
/// Lexically normalized absolute path whose deepest existing ancestor is
/// canonicalized, so a symlinked parent cannot hide its real target.
fn resolved(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let mut lexical = PathBuf::new();
    for part in path.components() {
        match part {
            Component::RootDir | Component::Prefix(_) => lexical.push(part.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !lexical.pop() {
                    return None;
                }
            }
            Component::Normal(p) => lexical.push(p),
        }
    }
    let mut existing = lexical.as_path();
    let mut rest = Vec::new();
    loop {
        if let Ok(real) = existing.canonicalize() {
            let mut out = real;
            for p in rest.iter().rev() {
                out.push(p);
            }
            return Some(out);
        }
        rest.push(existing.file_name()?.to_owned());
        existing = existing.parent()?;
    }
}

static DELETION_COMMAND: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|[;&|\n]\s*|\b(?:sudo|xargs|exec)\s+(?:-\S+\s+)*)(?:\S*/)?rm\b").unwrap()
});
static FIND_EXEC_RM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bfind\b[^;&|]*\s-exec(?:dir)?\s+(?:\S*/)?rm\b").unwrap());
static SHELL_SCRIPT_ARGUMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\b(?:sh|bash)\s+-[A-Za-z]*c[A-Za-z]*\s+(?:'([^']*)'|"([^"]*)"|([^\s;&|]+))"#)
        .unwrap()
});
fn command_text(call: &Value) -> String {
    let raw = &call["rawInput"];
    let mut parts = Vec::new();
    if let Some(object) = raw.as_object() {
        for key in ["command", "cmd", "script", "input"] {
            match object.get(key) {
                Some(Value::String(s)) => parts.push(s.clone()),
                Some(Value::Array(items)) => parts.push(
                    items
                        .iter()
                        .map(|v| {
                            v.as_str()
                                .map(str::to_owned)
                                .unwrap_or_else(|| v.to_string())
                        })
                        .collect::<Vec<_>>()
                        .join(" "),
                ),
                Some(Value::Object(inner)) => {
                    for k in ["cmd", "command"] {
                        if let Some(s) = inner.get(k).and_then(Value::as_str) {
                            parts.push(s.to_owned());
                        }
                    }
                }
                _ => {}
            }
        }
    } else if let Some(s) = raw.as_str() {
        parts.push(s.to_owned());
    }
    parts.join("\n")
}
/// ACP `delete`/`move` kinds or a declared `rm` invocation. Lexical only:
/// code that hides an `rm` from the declared text is outside this fence.
pub fn is_deletion(request: &Value) -> bool {
    let call = &request["toolCall"];
    if matches!(call["kind"].as_str(), Some("delete" | "move")) {
        return true;
    }
    let command = command_text(call);
    if DELETION_COMMAND.is_match(&command) || FIND_EXEC_RM.is_match(&command) {
        return true;
    }
    SHELL_SCRIPT_ARGUMENT.captures_iter(&command).any(|c| {
        (1..=3)
            .find_map(|i| c.get(i))
            .is_some_and(|script| DELETION_COMMAND.is_match(script.as_str()))
    })
}
/// The offered allow option: `allow_once` preferred, then `allow_always`.
pub fn allow_option(request: &Value) -> Option<String> {
    option_of(request, &["allow_once", "allow_always"])
}
/// The offered reject option: `reject_once` preferred, then `reject_always`.
pub fn reject_option(request: &Value) -> Option<String> {
    option_of(request, &["reject_once", "reject_always"])
}
fn option_of(request: &Value, kinds: &[&str]) -> Option<String> {
    let options = request["options"].as_array()?;
    kinds.iter().find_map(|kind| {
        options
            .iter()
            .find(|o| o["kind"] == *kind && o["optionId"].as_str().is_some_and(|s| !s.is_empty()))
            .and_then(|o| o["optionId"].as_str().map(str::to_owned))
    })
}
/// Whether `option` is one of the request's own allow options.
pub fn offered_allow(request: &Value, option: &str) -> bool {
    request["options"].as_array().is_some_and(|options| {
        options.iter().any(|o| {
            o["optionId"] == option && o["kind"].as_str().is_some_and(|k| k.starts_with("allow"))
        })
    })
}
/// The owner card for a deletion: the declared command (or tool title) leads.
pub fn deletion_card(request: &Value) -> Value {
    let mut card = request.clone();
    let command = command_text(&request["toolCall"]);
    let command = command.trim();
    let title = if command.is_empty() {
        let title = request["toolCall"]["title"].as_str().unwrap_or("");
        format!(
            "{DELETION_TITLE}{}",
            if title.is_empty() { "file" } else { title }
        )
    } else {
        format!(
            "{DELETION_TITLE}{}",
            command.chars().take(300).collect::<String>()
        )
    };
    card["toolCall"]["title"] = json!(title);
    card
}

#[cfg(test)]
mod tests {
    use super::*;
    fn facts<'a>(access: &'a str, read_only: bool, root: &'a Path) -> Facts<'a> {
        Facts {
            access,
            read_only,
            consultant: false,
            runtime_answers_native: true,
            root,
            mcp_servers: &[],
            protected: &|_| false,
        }
    }
    fn command(cmd: &str) -> Value {
        json!({"toolCall":{"kind":"execute","rawInput":{"command":cmd}}})
    }
    #[test]
    fn deletion_matches_python_lexical_fence() {
        for yes in [
            "rm a",
            "/bin/rm -rf x",
            "ls; rm a",
            "cd s && rm -f b",
            "sudo -n rm x",
            "find . -name x -exec rm {} +",
            "bash -c 'rm -f a'",
            "sh -c \"echo; rm a\"",
        ] {
            assert!(is_deletion(&command(yes)), "{yes}");
        }
        for no in ["ls", "echo rm", "grep -r rmdir .", "git status", "perm x"] {
            assert!(!is_deletion(&command(no)), "{no}");
        }
        assert!(is_deletion(&json!({"toolCall":{"kind":"move"}})));
        assert!(is_deletion(
            &json!({"toolCall":{"rawInput":{"command":["rm","x"]}}})
        ));
    }
    #[test]
    fn names_kinds_and_modes() {
        let root = std::env::temp_dir();
        let mcp = json!({"toolCall":{"kind":"other","_meta":{"claudeCode":{"toolName":"mcp__cortex__plan_submit"}}}});
        assert_eq!(
            classify(&mcp, &facts("guarded", true, &root)).action,
            Action::Allow
        );
        let external =
            json!({"toolCall":{"_meta":{"claudeCode":{"toolName":"mcp__code_index__search"}}}});
        let servers = vec!["code.index".to_owned()];
        let mut f = facts("guarded", true, &root);
        assert_eq!(classify(&external, &f).action, Action::Refuse);
        f.mcp_servers = &servers;
        assert_eq!(classify(&external, &f).rule, "channel_mcp");
        f.consultant = true;
        assert_eq!(classify(&external, &f).rule, "consultant_native");
        let bash = command("ls");
        assert_eq!(
            classify(&bash, &facts("development", false, &root)).action,
            Action::Allow
        );
        assert!(matches!(
            classify(&bash, &facts("guarded", false, &root)).action,
            Action::Ask {
                deletion: false,
                fallback: Fallback::Refuse
            }
        ));
        let mut legacy = facts("development", false, &root);
        legacy.runtime_answers_native = false;
        assert_eq!(
            classify(&bash, &legacy).action,
            Action::Ask {
                deletion: false,
                fallback: Fallback::Cancel
            }
        );
        legacy.read_only = true;
        assert_eq!(classify(&bash, &legacy).action, Action::Cancel);
        let edit = |p: PathBuf| json!({"toolCall":{"kind":"edit","locations":[{"path":p}]}});
        assert_eq!(
            classify(&edit(root.join("a/b")), &facts("guarded", false, &root)).rule,
            "native_project"
        );
        assert_eq!(
            classify(
                &edit(root.join("a/../../x")),
                &facts("guarded", false, &root)
            )
            .rule,
            "native_uncontained"
        );
        assert_eq!(
            classify(
                &json!({"toolCall":{"kind":"edit"}}),
                &facts("guarded", false, &root)
            )
            .rule,
            "native_uncontained"
        );
    }
    /// A reviewer channel is read-only in every access mode: native edit, execute and
    /// delete tools are refused (Claude, which the runtime answers) or cancelled
    /// (other engines) before any access-mode shortcut, while reads still pass.
    #[test]
    fn read_only_channels_never_get_native_edit_tools() {
        let root = std::env::temp_dir();
        let edit = json!({"toolCall":{"kind":"edit","locations":[{"path":root.join("a")}]}});
        let read = json!({"toolCall":{"kind":"read"}});
        for access in ["auto", "development", "guarded"] {
            for answers in [true, false] {
                let mut f = facts(access, true, &root);
                f.runtime_answers_native = answers;
                let v = classify(&edit, &f);
                assert_eq!(v.rule, "read_only");
                assert_eq!(v.action, if answers { Action::Refuse } else { Action::Cancel });
                for kind in ["execute", "delete", "move"] {
                    let tool = json!({"toolCall":{"kind":kind}});
                    assert_eq!(classify(&tool, &f).rule, "read_only");
                }
                assert_ne!(classify(&read, &f).rule, "read_only");
            }
        }
    }
    #[test]
    fn options_are_only_offered_ones() {
        let request = json!({"options":[{"optionId":"r","kind":"reject_always"},{"optionId":"","kind":"allow_once"},
            {"optionId":"a","kind":"allow_always"}]});
        assert_eq!(allow_option(&request).as_deref(), Some("a"));
        assert_eq!(reject_option(&request).as_deref(), Some("r"));
        assert!(
            offered_allow(&request, "a")
                && !offered_allow(&request, "r")
                && !offered_allow(&request, "x")
        );
        let card = deletion_card(&json!({"toolCall":{"title":"t","rawInput":{"command":"rm a"}}}));
        assert_eq!(
            card["toolCall"]["title"],
            "Removal blocked pending confirmation: rm a"
        );
    }
}
