//! Canonical portable instructions embedded at build time; no Python at session time.
//! Regenerate/check with `scripts/cortex-xtask.sh check-generated` (crates/cortex-xtask).
use cortex_core::{CoreError, CoreResult};
use cortex_engine::{environment::EngineKind, schema};
use serde::Deserialize;
use serde_json::Value;
use std::{collections::BTreeMap, sync::OnceLock};

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
        | "escalation_request" => true,
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
    let mut text = p.body.clone();
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
            let kind = field["type"].as_str().unwrap_or("value");
            let detail = if let Some(items) = field.get("items") {
                if items.get("properties").is_some() {
                    format!("[{{{}}}]", signature(items))
                } else {
                    format!("[{}]", items["type"].as_str().unwrap_or("value"))
                }
            } else if let Some(values) = field.get("enum") {
                values.to_string()
            } else {
                kind.to_owned()
            };
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

#[cfg(test)]
mod tests {
    use super::*;
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
