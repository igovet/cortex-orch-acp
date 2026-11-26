//! Canonical portable instructions embedded at build time; no Python at session time.
//! Regenerate/check with `scripts/cortex-xtask.sh check-generated` (crates/cortex-xtask).
use cortex_core::{CoreError, CoreResult};
use cortex_engine::{environment::EngineKind, schema};
use serde::Deserialize;
use serde_json::Value;
use std::{collections::BTreeMap, sync::OnceLock};

/// Used both in the session instructions and, where those are not sent with
/// each request, in the text of the coordinator's turn.
pub(crate) fn coordinator_language_directive(language: &str) -> String {
    format!("Owner-selected response language: {language}. Write in it everything the owner reads directly: \
your messages and questions to the owner (question text and option labels), the final result text and its not_verified entries. \
Write plans, objectives and every instruction to workers in English; worker reports stay English, so compose the owner-facing text \
from them in {language}.")
}

#[derive(Deserialize)]
struct Profile {
    kind: String,
    body: String,
    skills: Vec<String>,
}
#[derive(Deserialize)]
struct Bundle {
    version: u32,
    profiles: BTreeMap<String, Profile>,
    skills: BTreeMap<String, String>,
}
fn bundle() -> &'static Bundle {
    static DATA: OnceLock<Bundle> = OnceLock::new();
    DATA.get_or_init(|| {
        let data: Bundle = serde_json::from_str(include_str!("../schema/instructions.json"))
            .expect("generated native instruction data");
        assert_eq!(data.version, 1);
        data
    })
}
fn profile(name: &str) -> CoreResult<&'static Profile> {
    bundle()
        .profiles
        .get(name)
        .ok_or(CoreError::Refused("native_profile_unavailable"))
}
/// One capability filter shared by the gateway and instruction renderer.
pub(crate) fn supports(stage: Option<&str>, name: &str) -> bool {
    if stage == Some(cortex_core::lifecycle::steering::ACTIVE_STAGE) {
        return matches!(name, "steer_plan" | "steer_worker") || schema::active_steering_read(name);
    }
    match name {
        "read_skill"
        | "read_batch"
        | "ask_question"
        | "user_question_request"
        | "answer_submit"
        | "plan_submit"
        | "list_route_options"
        | "synthesis_submit"
        | "verification_submit"
        | "replan_request"
        | "scope_request"
        | "escalation_request"
        | "sprint_plan_submit"
        | "sprint_revision_submit" => true,
        "result_submit" => matches!(stage, Some("execution" | "review")),
        _ => {
            crate::native_documents::supports(name)
                || crate::native_access::supports(name)
                || crate::native_sources::supports(name)
                || crate::native_project::supports(name)
                || crate::native_project_ops::supports(name)
                || crate::native_context_tools::supports(name)
                || crate::native_commands::supports(name)
        }
    }
}
pub(crate) fn read_skill(name: &str, skill: &str) -> CoreResult<&'static str> {
    let p = profile(name)?;
    // The role's own id is advertised in its read_skill enum: serve its own body.
    if skill == name {
        return Some(p.body.as_str())
            .filter(|s| !s.trim().is_empty())
            .ok_or(CoreError::Refused("native_skill_unavailable"));
    }
    if !p.skills.iter().any(|id| id == skill) {
        return Err(CoreError::Refused("native_skill_unavailable"));
    }
    bundle()
        .skills
        .get(skill)
        .map(String::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or(CoreError::Refused("native_skill_unavailable"))
}
/// Whether `skill` is a Cortex shared/reference id or a profile id of any role.
pub(crate) fn is_bundled(skill: &str) -> bool {
    bundle().skills.contains_key(skill) || bundle().profiles.contains_key(skill)
}
/// Cortex skill ids readable by this profile, for precise refusals.
pub(crate) fn skill_ids(name: &str) -> Vec<&'static str> {
    profile(name)
        .map(|p| p.skills.iter().map(String::as_str).collect())
        .unwrap_or_default()
}
/// Opening rule of a Claude execution or review worker prompt. Instruction only:
/// the installed engine offers no switch that stops assistant text, and the
/// runtime records whatever text is generated.
pub(crate) const TOOL_CALLS_ONLY: &str = "# Output rule: tool calls only\n\n\
This session is a delegated Cortex worker. While work remains, write no assistant text at all: every response consists only of tool calls. \
Do not write a plan, an announcement of the next step, a progress update, a status note, a finding, a summary or an acknowledgement as assistant text, \
not before the first tool call, not between tool calls and not after a tool result. \
Nobody reads that text as part of your work; the runtime records any text you generate in the worker's activity. \
Put everything you need to communicate into tool arguments: findings, decisions, assumptions, risks and the report go into the structured result \
(`result_submit` or an owned report draft), and a genuine owner question goes through `ask_question`. \
Wherever the instructions below say to explain, state, describe, say or report something, they mean the content of those tool arguments, never assistant text. \
The only exception is the supporting detail the `ask_question` description tells you to write right before that call. \
After the final tool call, end the turn without text, or with at most one short sentence; it never replaces the structured result.\n\n";
/// Closing restatement of [`TOOL_CALLS_ONLY`], the last text of the prompt.
pub(crate) const TOOL_CALLS_ONLY_AGAIN: &str = "\n# Output rule: tool calls only (repeated)\n\n\
While work remains, respond only with tool calls: no plan, announcement, progress update, finding or summary as assistant text before, between or after tool calls. \
Everything goes into tool arguments; the only text before a call is the detail `ask_question` asks for. \
After the final tool call, end without text, or with one short sentence at most.\n";
/// Adds a later prompt part before a closing [`TOOL_CALLS_ONLY_AGAIN`], which
/// stays the last text; without one, `part` is appended.
pub(crate) fn append(text: &mut String, part: &str) {
    let end = text
        .strip_suffix(TOOL_CALLS_ONLY_AGAIN)
        .map_or(text.len(), str::len);
    text.insert_str(end, part);
}
/// The role/stage comes from Core's authenticated channel, not an engine request.
/// Only the tools actually served on that channel appear in the Codex supplement.
pub(crate) fn compose(name: &str, stage: Option<&str>, engine: EngineKind) -> CoreResult<String> {
    let p = profile(name)?;
    if p.body.trim().is_empty() {
        return Err(CoreError::Refused("native_instructions_unavailable"));
    }
    let role = if p.kind == "coordinator" {
        "coordinator"
    } else {
        "worker"
    };
    let tools = schema::catalogue(role, (role == "worker").then_some(name), stage)
        .map_err(|_| CoreError::Refused("native_profile_unavailable"))?
        .into_iter()
        .filter(|t| supports(stage, t["name"].as_str().unwrap_or("")))
        .collect::<Vec<_>>();
    // Claude execution workers and reviewers get the no-text rule first and last.
    let silent = engine == EngineKind::Claude
        && role == "worker"
        && name != "worker-senior-consultant"
        && matches!(stage, Some("execution" | "review"));
    let mut text = String::new();
    if silent {
        text.push_str(TOOL_CALLS_ONLY);
    }
    text.push_str(&p.body);
    text.push_str("\n## Native capability boundary\n\nThis host is partially migrated. Only the tools advertised on this channel are available. Missing capabilities must be reported, never replaced with native engine mutations or invented receipts.\n");
    let discovery = if engine == EngineKind::Claude {
        "`mcp__cortex__tools_discover`"
    } else {
        "`tools_discover`"
    };
    text.push_str(&format!(
        "\nUse {discovery} only for the Cortex server's active tools and schemas. For external MCP tools, including code indexes, use the engine's tool list or search if offered. Read the tool's live description and schema and use the exact name advertised by the engine. Do not substitute the Cortex namespace or infer a tool name from a display title. If the external tool is not advertised, fall back to authorized project reads where available and report the limitation. Do not guess a tool name.\n\nAfter a JSON parsing error, split a large edit into smaller independent changes.\n"
    ));
    if engine == EngineKind::Codex {
        let start = text.len();
        text.push_str("\n# Cortex tool cheat sheet (code mode)\n\nDerived from this channel's served schemas. Use actual arguments, not placeholders. Call tools as `await tools.mcp__cortex__<name>({...})`.\n");
        let mut tools = tools;
        let priority = [
            "tools_discover",
            "result_submit",
            "command_run",
            "command_status",
            "project_read",
            "project_edit",
            "plan_submit",
            "read_skill",
        ];
        tools.sort_by_key(|t| {
            priority
                .iter()
                .position(|n| t["name"] == *n)
                .unwrap_or(priority.len())
        });
        for tool in tools {
            let mut entry = format!(
                "\n- {}: {}\n",
                tool["name"].as_str().unwrap(),
                signature(&tool["inputSchema"])
            );
            if let Some(example) = tool["inputSchema"]["examples"].get(0) {
                entry.push_str(&format!(
                    "  await tools.mcp__cortex__{}({example})\n",
                    tool["name"].as_str().unwrap()
                ));
            }
            if text.len() - start + entry.len() > 7850 {
                text.push_str("Further signatures are available through tools_discover.\n");
                break;
            }
            text.push_str(&entry);
        }
    }
    if silent {
        text.push_str(TOOL_CALLS_ONLY_AGAIN);
    }
    Ok(text)
}
fn signature(schema: &Value) -> String {
    let required = schema["required"].as_array().cloned().unwrap_or_default();
    let Some(properties) = schema["properties"].as_object() else {
        return "no fields".into();
    };
    properties
        .iter()
        .map(|(name, field)| {
            let mark = if required.iter().any(|v| v == name) {
                ""
            } else {
                "?"
            };
            let detail = field_kind(field);
            let min = field
                .get("minimum")
                .or(field.get("minLength"))
                .or(field.get("minItems"));
            let max = field
                .get("maximum")
                .or(field.get("maxLength"))
                .or(field.get("maxItems"));
            let bounds = match (min, max) {
                (Some(lo), Some(hi)) => format!(" {lo}..{hi}"),
                (Some(lo), None) => format!(" >= {lo}"),
                (None, Some(hi)) => format!(" <= {hi}"),
                _ => String::new(),
            };
            format!("{name}{mark}={detail}{bounds}")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn field_kind(field: &Value) -> String {
    if let Some(alternatives) = field["anyOf"].as_array() {
        alternatives
            .iter()
            .map(field_kind)
            .collect::<Vec<_>>()
            .join(" | ")
    } else if let Some(items) = field.get("items") {
        format!("[{}]", field_kind(items))
    } else if field.get("properties").is_some() {
        format!("{{{}}}", signature(field))
    } else if let Some(values) = field.get("enum") {
        values.to_string()
    } else {
        field["type"].as_str().unwrap_or("value").to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn question_option_details_are_derived_from_the_worker_tool_schema() {
        let tools = schema::catalogue("worker", Some("worker-backend-dev"), Some("execution"))
            .unwrap();
        let tool = tools.iter().find(|t| t["name"] == "ask_question").unwrap();
        let signature = signature(&tool["inputSchema"]);
        assert!(signature.contains("recommended?=integer 1..8"));
        assert!(signature.contains("recommended=integer 1..8"));
        assert_eq!(
            signature
                .matches("[string | {detail?=string 1..1000, label=string 1..1000}]")
                .count(),
            2
        );
    }
    #[test]
    fn every_profile_and_required_skill_is_embedded_and_scoped() {
        assert_eq!(bundle().profiles.len(), 24); // 22 specialists, consultant, coordinator
        for (name, p) in &bundle().profiles {
            let text = compose(name, Some("execution"), EngineKind::Codex).unwrap();
            assert!(text.starts_with(p.body.trim()));
            assert!(!text.contains("END OF COMPLETE CORTEX"));
            for skill in &p.skills {
                assert!(!read_skill(name, skill).unwrap().trim().is_empty());
            }
        }
        assert!(compose("../orchestrator", None, EngineKind::Codex).is_err());
        assert!(read_skill("worker-general", "coordinator-communication").is_err());
        assert!(read_skill("worker-general", "design-colors").is_err());
        assert!(read_skill("worker-ux-designer", "design-colors").is_ok());
        assert!(read_skill("worker-senior-consultant", "tool-discipline-worker").is_err());
        assert!(read_skill("worker-explorer", "skill-authoring").is_err());
    }
    #[test]
    fn owner_language_reminder_is_only_in_coordinator_instructions() {
        let reminder = "After submitting a plan or reading English tool output, do not recap the plan to the owner.\n\
                        Keep owner-facing messages, questions and option labels, final text and owner-facing report prose\n\
                        in the selected owner language, even when tools, objectives and worker reports are in English.";
        for engine in [EngineKind::Claude, EngineKind::Codex, EngineKind::OpenCode] {
            for (name, stage, expected) in [
                ("orchestrator", None, true),
                (
                    "orchestrator",
                    Some(cortex_core::lifecycle::steering::ACTIVE_STAGE),
                    true,
                ),
                ("worker-backend-dev", Some("execution"), false),
                ("worker-code-reviewer", Some("review"), false),
                ("worker-senior-consultant", Some("execution"), false),
            ] {
                let instructions = compose(name, stage, engine).unwrap();
                assert_eq!(
                    instructions.contains(reminder),
                    expected,
                    "{name} {stage:?} {engine:?}",
                );
            }
        }
    }
    #[test]
    fn only_claude_execution_and_review_specialists_receive_the_tool_calls_only_rule() {
        for (name, stage, engine, expected) in [
            (
                "worker-backend-dev",
                Some("execution"),
                EngineKind::Claude,
                true,
            ),
            (
                "worker-code-reviewer",
                Some("review"),
                EngineKind::Claude,
                true,
            ),
            ("orchestrator", None, EngineKind::Claude, false),
            (
                "worker-senior-consultant",
                Some("execution"),
                EngineKind::Claude,
                false,
            ),
            ("worker-backend-dev", None, EngineKind::Claude, false),
            (
                "worker-backend-dev",
                Some("execution"),
                EngineKind::Codex,
                false,
            ),
            (
                "worker-backend-dev",
                Some("execution"),
                EngineKind::OpenCode,
                false,
            ),
        ] {
            let text = compose(name, stage, engine).unwrap();
            let label = format!("{name} {stage:?} {engine:?}");
            // First and last text of the prompt, nowhere else.
            assert_eq!(text.starts_with(TOOL_CALLS_ONLY), expected, "{label}");
            assert_eq!(text.ends_with(TOOL_CALLS_ONLY_AGAIN), expected, "{label}");
            assert_eq!(text.matches("# Output rule: tool calls only").count(), if expected { 2 } else { 0 }, "{label}");
            // No retained instruction asks a silent worker for interim progress text.
            assert!(!expected || !text.contains("English: progress,"), "{label}");
            for preset in ["give short updates", "Brief is good", "say in a sentence what you're about to do"] {
                assert!(!text.contains(preset), "{label}: {preset}");
            }
            // Parts the session adds later keep the closing rule last.
            let mut later = text.clone();
            append(&mut later, "\n\nRoute note.");
            assert!(later.contains("Route note."), "{label}");
            assert_eq!(later.ends_with(TOOL_CALLS_ONLY_AGAIN), expected, "{label}");
        }
    }
    /// Every tool any catalogue advertises has a handler, and every Cortex tool
    /// name the shipped profiles and skills mention is one a role is served.
    #[test]
    fn advertised_tools_are_handled_and_instructions_name_only_served_tools() {
        let snapshot: Value =
            serde_json::from_str(include_str!("../../cortex-engine/schema/tools.json")).unwrap();
        // Every catalogued tool, read_batch included, has a handler: no exception.
        let mut served = std::collections::BTreeSet::new();
        for key in snapshot["catalogues"].as_object().unwrap().keys() {
            let p: Vec<_> = key.split('|').collect();
            let stage = if p[2].is_empty() { None } else { Some(p[2]) };
            for tool in schema::catalogue(p[0], (!p[1].is_empty()).then_some(p[1]), stage).unwrap()
            {
                let name = tool["name"].as_str().unwrap().to_owned();
                // result_submit is served only to execution and review stages.
                let handled = supports(stage.or(Some("execution")), &name);
                assert!(handled, "{key} advertises {name} without a handler");
                served.insert(name);
            }
        }
        // Names that were Cortex tools once and are no longer served to any role.
        let retired = ["read_diagnostics"];
        let names = regex::Regex::new(r"\b[a-z]+(?:_[a-z]+)+\b").unwrap();
        let mut texts: Vec<(String, &str)> = bundle()
            .profiles
            .iter()
            .map(|(n, p)| (format!("profile {n}"), p.body.as_str()))
            .collect();
        texts.extend(
            bundle()
                .skills
                .iter()
                .map(|(n, s)| (format!("skill {n}"), s.as_str())),
        );
        for (source, text) in texts {
            for found in names.find_iter(text) {
                assert!(
                    !retired.contains(&found.as_str()),
                    "{source} names retired tool {}",
                    found.as_str()
                );
            }
        }
        for name in [
            "task_get_context",
            "read_capsule",
            "project_skill_link",
            "check_preflight",
            "read_batch",
        ] {
            assert!(served.contains(name), "{name}");
        }
        assert!(!served.iter().any(|n| retired.contains(&n.as_str())));
    }
    #[test]
    fn cheat_sheet_matches_served_tools_and_only_codex_receives_it() {
        let coordinator = compose("orchestrator", None, EngineKind::Codex).unwrap();
        let sheet = coordinator
            .split("# Cortex tool cheat sheet")
            .nth(1)
            .unwrap();
        assert!(sheet.contains("- plan_submit:"));
        assert!(sheet.contains("- read_skill:"));
        assert!(!sheet.contains("- result_submit:"));
        assert!(!sheet.contains("- command_run:"));
        let live = compose(
            "orchestrator", Some(cortex_core::lifecycle::steering::ACTIVE_STAGE), EngineKind::Codex,
        ).unwrap();
        let live = live.split("# Cortex tool cheat sheet").nth(1).unwrap();
        assert!(live.contains("- steer_plan:") && live.contains("- steer_worker:"));
        assert!(!live.contains("- plan_submit:") && !live.contains("- replan_request:"));
        assert!(!live.contains("- command_run:") && !live.contains("- answer_submit:"));
        let worker = compose("worker-general", Some("execution"), EngineKind::Codex).unwrap();
        assert!(worker.contains("- result_submit:"));
        assert!(!compose("worker-general", None, EngineKind::Claude)
            .unwrap()
            .contains("# Cortex tool cheat sheet"));
        let s = signature(
            &serde_json::json!({"required":["n"],"properties":{"n":{"type":"integer","minimum":1,"maximum":3},"choice":{"enum":["a","b"]}}}),
        );
        assert!(s.contains("n=") && s.contains("1..3") && s.contains("choice?="));
    }
}
