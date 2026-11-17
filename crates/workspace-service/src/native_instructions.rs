//! Canonical portable instructions embedded at build time; no Python at session time.
//! Regenerate/check with scripts/generate_native_instructions.py.
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
                || crate::native_commands::supports(name)
        }
    }
}
pub(crate) fn read_skill(name: &str, skill: &str) -> CoreResult<&'static str> {
    let p = profile(name)?;
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
    if engine == EngineKind::Codex {
        text.push_str("\n# Cortex tool cheat sheet (code mode)\n\nDerived from this channel's served schemas. Use actual arguments, not placeholders. Call tools as `await tools.mcp__cortex__<name>({...})`.\n");
        for tool in tools {
            text.push_str(&format!(
                "\n- {}: {}\n",
                tool["name"].as_str().unwrap(),
                signature(&tool["inputSchema"])
            ));
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
            // Full field schemas preserve enums, nested fields and numeric/string bounds.
            format!("{name}{mark}={field}")
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
        assert!(s.contains("n=") && s.contains("\"minimum\":1") && s.contains("choice?="));
    }
}
