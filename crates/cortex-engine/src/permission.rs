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
/// whose requests the runtime answers itself: claude-agent-acp in `default` and
/// codex-acp in `read-only`/`workspace-write`, which asks only to leave its
/// sandbox. Other engines (OpenCode) keep the client route outside Auto.
pub struct Facts<'a> {
    pub access: &'a str,
    pub read_only: bool,
    pub consultant: bool,
    pub runtime_answers_native: bool,
    pub root: &'a Path,
    /// The engine's `HOME`, for `~` paths; `None` leaves them unclassified.
    pub home: Option<&'a Path>,
    /// Names of the external MCP servers Cortex attached to this session.
    pub mcp_servers: &'a [String],
    /// The class of a resolved absolute path (see [`PathClass`]).
    pub class: &'a dyn Fn(&Path) -> PathClass,
}
/// What a native request's path reaches, strongest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PathClass {
    /// An ordinary path (inside or outside the project).
    Ordinary,
    /// Git control files of the project, engine and Cortex configuration.
    Control,
    /// Credential and key material.
    Credential,
    /// Cortex private state: refused in every mode.
    Private,
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
    let refuse = if facts.runtime_answers_native {
        Action::Refuse
    } else {
        Action::Cancel
    };
    let ask = Action::Ask {
        deletion: false,
        fallback: if facts.runtime_answers_native {
            Fallback::Refuse
        } else {
            Fallback::Cancel
        },
    };
    let read = matches!(kind, Some("read" | "search"));
    let targets = targets(request, facts);
    // Cortex private state is refused to every native request in every mode.
    if targets.paths.iter().any(|(_, c)| *c == PathClass::Private) {
        return verdict(refuse, "cortex_private");
    }
    // Read-only channels never write, delete or execute natively, in any mode. A web
    // fetch only reads (the session keeps WebFetch for exactly that), so it is not
    // refused here: it follows the ordinary fetch rule below (Auto and Development
    // allow it, Guarded asks the owner). Refusing it made every read-only fetch fail
    // before any owner could be asked.
    let fetch = kind == Some("fetch");
    if facts.read_only && !read && !fetch {
        return verdict(refuse, "read_only");
    }
    let auto = facts.access == "auto";
    let deletion = is_deletion(request);
    let credential = targets
        .paths
        .iter()
        .any(|(_, c)| *c == PathClass::Credential);
    let control = !read && targets.paths.iter().any(|(_, c)| *c == PathClass::Control);
    let outside = targets.unclassified || targets.paths.iter().any(|(inside, _)| !inside);
    // Auto allows everything else; the rule names what was allowed.
    if auto {
        let rule = if credential {
            "auto_credential"
        } else if control {
            "auto_control"
        } else if deletion {
            "auto_deletion"
        } else if outside && read {
            "auto_outside_read"
        } else if outside && !targets.paths.is_empty() {
            "auto_outside_write"
        } else {
            "auto"
        };
        return verdict(Action::Allow, rule);
    }
    // Credential and key material, Git controls and engine/Cortex configuration
    // (written) and a path that cannot be resolved ask the owner.
    if credential {
        return verdict(ask, "credential");
    }
    if control {
        return verdict(ask, "control_target");
    }
    if targets.unclassified {
        return verdict(ask, "unclassified_path");
    }
    if !facts.runtime_answers_native {
        return verdict(ask, "client");
    }
    if deletion {
        return verdict(
            Action::Ask {
                deletion: true,
                fallback: Fallback::Refuse,
            },
            "deletion",
        );
    }
    let development = facts.access == "development";
    if read {
        return match (outside, development) {
            (false, _) => verdict(Action::Allow, "native_read"),
            (true, true) => verdict(Action::Allow, "outside_read"),
            (true, false) => verdict(ask, "outside_read"),
        };
    }
    if matches!(kind, Some("think" | "switch_mode")) {
        return verdict(Action::Allow, "native_read");
    }
    if kind == Some("fetch") {
        return if development {
            verdict(Action::Allow, "native_fetch")
        } else {
            verdict(ask, "native_fetch")
        };
    }
    // A write names its targets (an edit's locations, a sandbox escalation's
    // paths); outside the project it asks in Guarded and Development.
    if !targets.paths.is_empty() {
        return match (outside, development) {
            (true, _) => verdict(ask, "outside_write"),
            (false, true) => verdict(Action::Allow, "native_development"),
            (false, false) => verdict(Action::Allow, "native_project"),
        };
    }
    // An edit without a declared target cannot be shown to stay in the project.
    if kind == Some("edit") {
        return verdict(ask, "outside_write");
    }
    if development {
        verdict(Action::Allow, "native_development")
    } else {
        verdict(ask, "native_uncontained")
    }
}
/// The resolved targets of a native request: each with whether it lies in the
/// project and its class. `unclassified` marks a named path that cannot be
/// resolved (a `~` path without a known home, a non-string path field).
struct Targets {
    paths: Vec<(bool, PathClass)>,
    unclassified: bool,
}
/// Paths a native request names: ACP `locations`, the tool's
/// `path`/`file_path`/`notebook_path`, and the literal leading directories of a
/// Glob `pattern` or Grep `glob`. A relative path is taken from the project
/// folder; `~` from the engine's home. A request naming no path names none.
fn targets(request: &Value, facts: &Facts<'_>) -> Targets {
    let mut out = Targets {
        paths: Vec::new(),
        unclassified: false,
    };
    let Some(root) = resolved(facts.root) else {
        out.unclassified = true;
        return out;
    };
    let call = &request["toolCall"];
    let raw = &call["rawInput"];
    let mut names: Vec<String> = Vec::new();
    if let Some(locations) = call["locations"].as_array() {
        for location in locations {
            match location["path"].as_str() {
                Some(p) => names.push(p.to_owned()),
                None => out.unclassified = true,
            }
        }
    }
    for key in ["path", "file_path", "notebook_path"] {
        match &raw[key] {
            Value::Null => {}
            Value::String(p) => names.push(p.clone()),
            _ => out.unclassified = true,
        }
    }
    let pattern = match tool_name(request) {
        Some("Glob") => raw["pattern"].as_str(),
        Some("Grep") => raw["glob"].as_str(),
        _ => None,
    };
    if let Some(pattern) = pattern {
        if pattern.starts_with('/')
            || pattern.starts_with('~')
            || pattern.split('/').any(|p| p == "..")
        {
            let literal: Vec<&str> = pattern
                .split('/')
                .take_while(|part| !part.contains(['*', '?', '[', '{']))
                .collect();
            let prefix = literal.join("/");
            names.push(if prefix.is_empty() {
                "/".to_owned()
            } else {
                prefix
            });
        }
    }
    for name in names {
        let path = if let Some(rest) = name.strip_prefix('~') {
            match (
                facts.home,
                rest.strip_prefix('/').or((rest.is_empty()).then_some("")),
            ) {
                (Some(home), Some(rest)) => home.join(rest),
                _ => {
                    out.unclassified = true;
                    continue;
                }
            }
        } else if name.is_empty() {
            out.unclassified = true;
            continue;
        } else if Path::new(&name).is_absolute() {
            PathBuf::from(&name)
        } else {
            facts.root.join(&name)
        };
        match resolved(&path) {
            Some(real) => {
                // Preserve private names along the declared path as well as
                // the resolved target when a symlink changes classification.
                let class = (facts.class)(&real).max((facts.class)(&path));
                out.paths.push((real.starts_with(&root), class));
            }
            None => out.unclassified = true,
        }
    }
    out
}
/// Why an asked request is outside the normal bounds, for the owner card.
pub fn ask_reason(rule: &str, ru: bool) -> &'static str {
    if ru {
        return match rule {
            "credential" => "запрос затрагивает учётные данные или ключи",
            "control_target" => "запрос изменяет служебные файлы Git или настройки движка или Cortex",
            "unclassified_path" => "не удалось определить точный путь",
            "outside_read" => "запрос читает данные за пределами проекта",
            "outside_write" => "запрос записывает данные за пределами проекта",
            "outside_command" => "команда запускается вне песочницы",
            "native_fetch" => "запрос обращается к сети",
            "deletion" => "запрос удаляет или перемещает файлы",
            _ => "не удалось подтвердить, что запрос ограничен проектом",
        };
    }
    match rule {
        "credential" => "it reaches credential or key material",
        "control_target" => "it changes Git control files or engine/Cortex configuration",
        "unclassified_path" => "its path cannot be resolved",
        "outside_read" => "it reads outside the project",
        "outside_write" => "it writes outside the project",
        "outside_command" => "it runs outside the sandbox",
        "native_fetch" => "it fetches from the network",
        "deletion" => "it deletes or moves files",
        _ => "it is not provably contained in the project",
    }
}
/// The stable reason code of an asked rule.
pub fn reason_code(rule: &str) -> &'static str {
    match rule {
        "credential" => "credential",
        "control_target" => "control",
        "unclassified_path" => "unresolvable_path",
        "outside_read" | "outside_write" => "outside_project",
        "native_fetch" => "network",
        "deletion" => "deletion",
        _ => "uncontained",
    }
}
/// The operation kind of a native request: read, write, delete, run or fetch.
pub fn operation(request: &Value) -> &'static str {
    let call = &request["toolCall"];
    match call["kind"].as_str() {
        Some("read" | "search") => "read",
        Some("delete" | "move") => "delete",
        _ if is_deletion(request) => "delete",
        Some("edit") => "write",
        Some("fetch") => "fetch",
        Some("execute") => "run",
        _ if call["locations"].as_array().is_some_and(|l| !l.is_empty()) => "write",
        _ => "run",
    }
}
/// The exact paths (or URL) a native request names, as the engine declared them.
pub fn paths(request: &Value) -> Vec<String> {
    let call = &request["toolCall"];
    let mut paths: Vec<String> = call["locations"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|l| l["path"].as_str().map(str::to_owned))
        .collect();
    for key in ["path", "file_path", "notebook_path", "url", "cwd"] {
        if let Some(p) = call["rawInput"][key].as_str() {
            if !paths.iter().any(|q| q == p) {
                paths.push(p.to_owned());
            }
        }
    }
    paths
}
/// The owner card for an asked native request: the role, the operation kind,
/// the exact paths and why it is outside the normal bounds lead the title, and
/// only one-shot Allow once / Deny options are offered. A deletion keeps its
/// `Removal blocked pending confirmation: ...` title.
pub fn owner_card(request: &Value, role: &str, rule: &str, ru: bool) -> Value {
    let mut card = if rule == "deletion" {
        deletion_card(request, ru)
    } else {
        let mut card = request.clone();
        let call = &request["toolCall"];
        let paths = paths(request);
        let subject = if paths.is_empty() {
            let command = command_text(call);
            let title = call["title"].as_str().unwrap_or("");
            if command.trim().is_empty() {
                title.to_owned()
            } else {
                command.trim().to_owned()
            }
        } else {
            paths.join(", ")
        };
        let subject: String = subject.chars().take(300).collect();
        let operation = operation(request);
        let reason = ask_reason(rule, ru);
        card["toolCall"]["title"] = json!(if ru {
            let verb = match operation {
                "read" => "прочитать",
                "write" => "записать",
                "delete" => "удалить",
                "fetch" => "загрузить",
                _ => "запустить",
            };
            format!("Требуется ваше решение: {role} запрашивает разрешение {verb} {subject}. Причина: {reason}.")
        } else {
            format!("Owner decision: {role} wants to {operation} {subject} because {reason}")
        });
        card
    };
    if let Some(options) = request["options"].as_array() {
        card["options"] = json!(options
            .iter()
            .filter_map(|option| {
                let allow = match option["kind"].as_str()? {
                    "allow_once" => true,
                    "reject_once" => false,
                    _ => return None,
                };
                let mut option = option.clone();
                if ru {
                    option["name"] = json!(one_shot_label(allow, true));
                }
                Some(option)
            })
            .collect::<Vec<_>>());
    }
    card
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
/// Display text only: option IDs and one-shot semantics are independent of language.
pub fn one_shot_label(allow: bool, ru: bool) -> &'static str {
    match (allow, ru) {
        (true, false) => "Allow once",
        (false, false) => "Deny",
        (true, true) => "Разрешить один раз",
        (false, true) => "Отклонить",
    }
}

pub fn one_shot_options(ru: bool) -> Value {
    json!([
        {"optionId":"allow","name":one_shot_label(true, ru),"kind":"allow_once"},
        {"optionId":"deny","name":one_shot_label(false, ru),"kind":"reject_once"}
    ])
}

/// The owner card for a deletion: the declared command (or tool title) leads.
fn deletion_card(request: &Value, ru: bool) -> Value {
    let mut card = request.clone();
    let prefix = if ru { "Удаление требует подтверждения: " } else { DELETION_TITLE };
    let command = command_text(&request["toolCall"]);
    let command = command.trim();
    let title = if command.is_empty() {
        let title = request["toolCall"]["title"].as_str().unwrap_or("");
        format!(
            "{prefix}{}",
            if title.is_empty() {
                if ru { "файл" } else { "file" }
            } else {
                title
            }
        )
    } else {
        format!(
            "{prefix}{}",
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
            home: None,
            mcp_servers: &[],
            class: &|_| PathClass::Ordinary,
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
    fn names_kinds_and_mcp() {
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
        assert_eq!(classify(&external, &f).rule, "read_only");
        f.mcp_servers = &servers;
        assert_eq!(classify(&external, &f).rule, "channel_mcp");
        f.consultant = true;
        assert_eq!(classify(&external, &f).rule, "consultant_native");
        let bash = command("ls");
        assert_eq!(
            classify(&bash, &facts("development", false, &root)).action,
            Action::Allow
        );
        assert_eq!(
            classify(&bash, &facts("guarded", false, &root)).action,
            Action::Ask {
                deletion: false,
                fallback: Fallback::Refuse
            }
        );
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
    }
    /// Read-only channels never write, delete or execute natively, in any mode,
    /// while reads follow the access-mode table.
    #[test]
    fn read_only_channels_never_get_native_edit_tools() {
        let root = std::env::temp_dir();
        let edit = json!({"toolCall":{"kind":"edit","locations":[{"path":root.join("a")}]}});
        for access in ["auto", "development", "guarded"] {
            for answers in [true, false] {
                let mut f = facts(access, true, &root);
                f.runtime_answers_native = answers;
                let v = classify(&edit, &f);
                assert_eq!(v.rule, "read_only");
                assert_eq!(
                    v.action,
                    if answers {
                        Action::Refuse
                    } else {
                        Action::Cancel
                    }
                );
                for kind in ["execute", "delete", "move"] {
                    assert_eq!(
                        classify(&json!({"toolCall":{"kind":kind}}), &f).rule,
                        "read_only"
                    );
                }
                // A web fetch only reads: it follows the fetch rule of the mode.
                let fetch = json!({"toolCall":{"kind":"fetch","rawInput":{"url":"https://github.com/x/y"}}});
                let v = classify(&fetch, &f);
                assert_ne!(v.rule, "read_only", "{access}");
                if answers {
                    assert_eq!(
                        v.action,
                        match access {
                            "guarded" => Action::Ask {
                                deletion: false,
                                fallback: Fallback::Refuse
                            },
                            _ => Action::Allow,
                        },
                        "{access}"
                    );
                }
                assert_ne!(
                    classify(&json!({"toolCall":{"kind":"read"}}), &f).rule,
                    "read_only"
                );
            }
        }
    }
    /// Every cell of the access-mode decision table.
    #[test]
    fn decision_table() {
        let base = std::env::temp_dir().join(format!("permission-table-{}", std::process::id()));
        let root = base.join("project");
        let home = base.join("home");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        let class = |p: &Path| {
            if p.starts_with(root.join(".cortex")) || p.starts_with(root.join(".cortex-dev")) {
                PathClass::Private
            } else if p.starts_with(home.join(".ssh")) || p.to_string_lossy().ends_with(".pem") {
                PathClass::Credential
            } else if p.starts_with(root.join(".git")) || p.starts_with(home.join(".claude")) {
                PathClass::Control
            } else {
                PathClass::Ordinary
            }
        };
        let read = |p: &Path| {
            json!({"toolCall":{"kind":"read","locations":[{"path":p}],
            "_meta":{"claudeCode":{"toolName":"Read"}},"rawInput":{"file_path":p}}})
        };
        let edit = |p: &Path| json!({"toolCall":{"kind":"edit","locations":[{"path":p}]}});
        let ask = Action::Ask {
            deletion: false,
            fallback: Fallback::Refuse,
        };
        let allow = Action::Allow;
        let refuse = Action::Refuse;
        // (request, [guarded, development, auto] as (action, rule)).
        let cases: Vec<(Value, [(Action, &str); 3])> = vec![
            (
                read(&root.join("src/a.rs")),
                [
                    (allow, "native_read"),
                    (allow, "native_read"),
                    (allow, "auto"),
                ],
            ),
            (
                read(&base.join("other/b.rs")),
                [
                    (ask, "outside_read"),
                    (allow, "outside_read"),
                    (allow, "auto_outside_read"),
                ],
            ),
            (
                json!({"toolCall":{"kind":"search","_meta":{"claudeCode":{"toolName":"Glob"}},"rawInput":{"pattern":"/etc/**"}}}),
                [
                    (ask, "outside_read"),
                    (allow, "outside_read"),
                    (allow, "auto_outside_read"),
                ],
            ),
            (
                read(Path::new("~/notes.txt")),
                [
                    (ask, "outside_read"),
                    (allow, "outside_read"),
                    (allow, "auto_outside_read"),
                ],
            ),
            (
                edit(&root.join("src/a.rs")),
                [
                    (allow, "native_project"),
                    (allow, "native_development"),
                    (allow, "auto"),
                ],
            ),
            (
                edit(&base.join("other/b.rs")),
                [
                    (ask, "outside_write"),
                    (ask, "outside_write"),
                    (allow, "auto_outside_write"),
                ],
            ),
            (
                json!({"toolCall":{"kind":"edit"}}),
                [
                    (ask, "outside_write"),
                    (ask, "outside_write"),
                    (allow, "auto"),
                ],
            ),
            (
                read(&home.join(".ssh/id_ed25519")),
                [
                    (ask, "credential"),
                    (ask, "credential"),
                    (allow, "auto_credential"),
                ],
            ),
            (
                read(&root.join("src/server.pem")),
                [
                    (ask, "credential"),
                    (ask, "credential"),
                    (allow, "auto_credential"),
                ],
            ),
            (
                edit(&home.join(".ssh/config")),
                [
                    (ask, "credential"),
                    (ask, "credential"),
                    (allow, "auto_credential"),
                ],
            ),
            (
                edit(&root.join(".git/config")),
                [
                    (ask, "control_target"),
                    (ask, "control_target"),
                    (allow, "auto_control"),
                ],
            ),
            (
                edit(&home.join(".claude/settings.json")),
                [
                    (ask, "control_target"),
                    (ask, "control_target"),
                    (allow, "auto_control"),
                ],
            ),
            (
                read(&root.join(".git/HEAD")),
                [
                    (allow, "native_read"),
                    (allow, "native_read"),
                    (allow, "auto"),
                ],
            ),
            (
                read(&root.join(".cortex/cortex.sqlite3")),
                [
                    (refuse, "cortex_private"),
                    (refuse, "cortex_private"),
                    (refuse, "cortex_private"),
                ],
            ),
            (
                edit(&root.join(".cortex-dev/x")),
                [
                    (refuse, "cortex_private"),
                    (refuse, "cortex_private"),
                    (refuse, "cortex_private"),
                ],
            ),
            (
                command("rm -rf build"),
                [
                    (
                        Action::Ask {
                            deletion: true,
                            fallback: Fallback::Refuse,
                        },
                        "deletion",
                    ),
                    (
                        Action::Ask {
                            deletion: true,
                            fallback: Fallback::Refuse,
                        },
                        "deletion",
                    ),
                    (allow, "auto_deletion"),
                ],
            ),
            (
                json!({"toolCall":{"kind":"fetch","rawInput":{"url":"https://example.invalid"}}}),
                [
                    (ask, "native_fetch"),
                    (allow, "native_fetch"),
                    (allow, "auto"),
                ],
            ),
            (
                json!({"toolCall":{"kind":"other","locations":[{"path":base.join("other")}]}}),
                [
                    (ask, "outside_write"),
                    (ask, "outside_write"),
                    (allow, "auto_outside_write"),
                ],
            ),
        ];
        for (request, expected) in cases {
            for (access, (action, rule)) in
                ["guarded", "development", "auto"].into_iter().zip(expected)
            {
                let mut f = facts(access, false, &root);
                f.home = Some(&home);
                f.class = &class;
                let v = classify(&request, &f);
                assert_eq!((v.action, v.rule), (action, rule), "{access} {request}");
            }
        }
        // Without a known home a `~` path is unclassified and asks outside Auto.
        let v = classify(&read(Path::new("~/x")), &facts("development", false, &root));
        assert_eq!(
            (v.action, v.rule),
            (
                Action::Ask {
                    deletion: false,
                    fallback: Fallback::Refuse
                },
                "unclassified_path"
            )
        );
        // A read-only channel's reads follow the same table; its private reads are refused.
        let mut f = facts("auto", true, &root);
        f.home = Some(&home);
        f.class = &class;
        assert_eq!(
            classify(&read(&home.join(".ssh/id_ed25519")), &f).rule,
            "auto_credential"
        );
        assert_eq!(
            classify(&read(&root.join(".cortex/x")), &f).rule,
            "cortex_private"
        );
        // The owner card names role, operation, path and reason with one-shot options.
        let request = json!({"toolCall":{"kind":"read","locations":[{"path":"/etc/hosts"}]},
            "options":[{"optionId":"a","kind":"allow_once"},{"optionId":"aa","kind":"allow_always"},{"optionId":"r","kind":"reject_once"}]});
        let card = owner_card(&request, "worker-explorer", "outside_read", false);
        assert_eq!(card["toolCall"]["title"],
            "Owner decision: worker-explorer wants to read /etc/hosts because it reads outside the project");
        assert_eq!(card["options"].as_array().unwrap().len(), 2);
        std::fs::remove_dir_all(&base).unwrap();
    }
    #[test]
    fn owner_permission_language_preserves_one_shot_ids_and_targets() {
        let request = json!({
            "toolCall":{"kind":"read","locations":[{"path":"/tmp/exact path"}]},
            "options":[
                {"optionId":"provider-allow","kind":"allow_once","name":"Allow once"},
                {"optionId":"provider-always","kind":"allow_always","name":"Always allow"},
                {"optionId":"provider-deny","kind":"reject_once","name":"Deny"}
            ]
        });
        let en = owner_card(&request, "worker-explorer", "outside_read", false);
        let ru = owner_card(&request, "worker-explorer", "outside_read", true);
        assert_eq!(en["toolCall"]["title"],
            "Owner decision: worker-explorer wants to read /tmp/exact path because it reads outside the project");
        assert_eq!(ru["toolCall"]["title"],
            "Требуется ваше решение: worker-explorer запрашивает разрешение прочитать /tmp/exact path. Причина: запрос читает данные за пределами проекта.");
        assert_eq!(ru["toolCall"]["locations"], en["toolCall"]["locations"]);
        for (card, labels) in [(&en, ["Allow once", "Deny"]), (&ru, ["Разрешить один раз", "Отклонить"])] {
            assert_eq!(card["options"].as_array().unwrap().len(), 2);
            assert_eq!(card["options"][0]["name"], labels[0]);
            assert_eq!(card["options"][1]["name"], labels[1]);
            assert_eq!(allow_option(card).as_deref(), Some("provider-allow"));
            assert_eq!(reject_option(card).as_deref(), Some("provider-deny"));
        }
        let deletion = json!({"toolCall":{"title":"EXACT TOOL TITLE"}});
        assert_eq!(
            owner_card(&deletion, "worker-explorer", "deletion", true)["toolCall"]["title"],
            "Удаление требует подтверждения: EXACT TOOL TITLE",
        );
        for ru in [false, true] {
            let options = one_shot_options(ru);
            assert_eq!(options[0]["optionId"], "allow");
            assert_eq!(options[1]["optionId"], "deny");
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
        let card = deletion_card(&json!({"toolCall":{"title":"t","rawInput":{"command":"rm a"}}}), false);
        assert_eq!(
            card["toolCall"]["title"],
            "Removal blocked pending confirmation: rm a"
        );
    }
}
