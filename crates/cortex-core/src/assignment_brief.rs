//! What the runtime hands a worker with its assignment, and the report structure it
//! checks on the way back.
//!
//! The runtime, never the model, chooses the report template: the profile's
//! `report_template` (`profiles.json`) for an execution assignment, and for a review
//! the reviewer's `general` template with the three review sections the review
//! packet and the verdict need. The brief carries that skeleton (every `##` heading
//! in order, one line on what belongs under it, and whether it is required);
//! `result_submit` checks only that the required headings are present.
//!
//! Heading match is by the exact English heading text, tolerant to letter case and
//! white space, at Markdown level two (`## `), outside fenced code. Content quality
//! is never judged here.
use crate::CoreResult;
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// Structural refusals one attempt may receive before its report is accepted as is,
/// marked [`STRUCTURE_INCOMPLETE`], so the worker's result is never lost.
pub const STRUCTURE_REFUSALS: i64 = 2;
/// Marker recorded on a result whose report was accepted without every required section.
pub const STRUCTURE_INCOMPLETE: &str = "REPORT_STRUCTURE_INCOMPLETE";
/// The refusal code naming missing required sections.
pub const SECTIONS_MISSING: &str = "report_sections_missing";
/// `read_report` page size, in characters.
pub const PAGE_CHARACTERS: i64 = 4000;
/// Owner messages carried in a brief, newest kept; each text bounded below.
const OWNER_MESSAGES: usize = 10;
const MESSAGE_CHARS: usize = 1500;
const DENIAL_CHARS: usize = 4000;

/// One `##` section of a report structure.
pub struct Section {
    pub heading: &'static str,
    pub holds: &'static str,
    pub required: bool,
}
const fn s(heading: &'static str, holds: &'static str, required: bool) -> Section {
    Section {
        heading,
        holds,
        required,
    }
}

const GENERAL: &[Section] = &[
    s(
        "Evidence and work",
        "What you did and observed, with the exact paths read or changed.",
        true,
    ),
    s(
        "Verification",
        "Each check run with its command, cwd and outcome; checks not run.",
        true,
    ),
    s(
        "Risks and remaining work",
        "What is not verified and why, risks and next steps.",
        true,
    ),
];
const IMPLEMENTATION: &[Section] = &[
    s(
        "Inputs consumed",
        "Predecessor reports, requirements and constraints you used.",
        false,
    ),
    s(
        "Changes",
        "Every changed path and what changed in behaviour, data or interface.",
        true,
    ),
    s(
        "Verification",
        "Each check run with its command, cwd and outcome.",
        true,
    ),
    s(
        "Evidence links and delivery state",
        "Related reports and artifacts; what is delivered.",
        false,
    ),
    s(
        "Risks and remaining work",
        "What is not verified and why, risks and follow-up.",
        true,
    ),
];
const INVESTIGATION: &[Section] = &[
    s(
        "Question and scope",
        "The question answered and the scope examined.",
        false,
    ),
    s(
        "Evidence examined",
        "The exact sources, paths and commands examined.",
        true,
    ),
    s(
        "Findings",
        "The conclusions, each tied to its evidence.",
        true,
    ),
    s(
        "Implications",
        "What the findings mean for the work.",
        false,
    ),
    s(
        "Gaps and next steps",
        "What is not verified and why, and what to do next.",
        true,
    ),
];
const VERIFICATION: &[Section] = &[
    s(
        "Scope and expected behavior",
        "What was verified and the expected behaviour.",
        false,
    ),
    s(
        "Executed checks",
        "Each check run with its command, cwd and outcome.",
        true,
    ),
    s(
        "Evidence links and delivery state",
        "Related reports and artifacts.",
        false,
    ),
    s(
        "Failures and gaps",
        "Failures, and what is not verified and why.",
        true,
    ),
    s("Assessment", "The conclusion the evidence supports.", true),
];
const PLANNING: &[Section] = &[
    s(
        "Requirements and constraints",
        "The requirements and constraints the plan meets.",
        false,
    ),
    s(
        "Evidence reviewed",
        "The exact sources and paths reviewed.",
        true,
    ),
    s(
        "Proposed work graph",
        "The proposed design or work, with the paths it touches.",
        true,
    ),
    s(
        "Verification strategy",
        "How the result can be checked.",
        false,
    ),
    s(
        "Risks and open questions",
        "What is not verified and why, risks and open questions.",
        true,
    ),
];
const DOCUMENTATION: &[Section] = &[
    s(
        "Verified source behavior",
        "The source behaviour the documentation states.",
        false,
    ),
    s(
        "Documentation changes",
        "Every changed documentation path and what changed.",
        true,
    ),
    s(
        "Validation",
        "Each check run (links, commands) with its outcome.",
        true,
    ),
    s(
        "Limits and follow-up",
        "What is not verified and why, and follow-up.",
        true,
    ),
];
const SYNTHESIS: &[Section] = &[
    s(
        "Requirements and delivered results",
        "Each requirement and what was delivered.",
        true,
    ),
    s(
        "Evidence and verification",
        "The checks and evidence behind the results.",
        true,
    ),
    s(
        "Changes and documentation",
        "The changed paths and documentation.",
        false,
    ),
    s(
        "Risks and unresolved items",
        "What is not verified and why.",
        true,
    ),
];
/// A review: the verdict, its findings and its coverage, then the general sections.
const REVIEW: &[Section] = &[
    s(
        "Verdict",
        "APPROVE, REQUEST_CHANGES, BLOCK or INCOMPLETE (the verdict field) and why.",
        true,
    ),
    s(
        "Findings",
        "Each confirmed problem with its node, files and fix, or None.",
        true,
    ),
    s(
        "Coverage",
        "What you reviewed (files, reports, checks) and what you did not cover and why.",
        true,
    ),
    s(
        "Verification",
        "Each check run with its command, cwd and outcome.",
        false,
    ),
    s(
        "Risks and remaining work",
        "Residual risk and next steps.",
        false,
    ),
];

fn profiles() -> &'static BTreeMap<String, String> {
    static DATA: std::sync::OnceLock<BTreeMap<String, String>> = std::sync::OnceLock::new();
    DATA.get_or_init(|| {
        let doc: Value =
            serde_json::from_str(include_str!("../../../profiles.json")).unwrap_or(Value::Null);
        doc["profiles"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| {
                Some((
                    p["name"].as_str()?.to_owned(),
                    p["report_template"].as_str()?.to_owned(),
                ))
            })
            .collect()
    })
}

/// The report template the runtime selects for a worker profile and stage.
pub fn template_for(profile: &str, stage: &str) -> &'static str {
    if stage == "review" {
        return "general";
    }
    let name = profile.trim_start_matches("worker-").replace('-', "_");
    match profiles().get(&name).map(String::as_str) {
        Some("implementation") => "implementation",
        Some("investigation") => "investigation",
        Some("verification") => "verification",
        Some("planning") => "planning",
        Some("documentation") => "documentation",
        Some("synthesis") => "synthesis",
        _ => "general",
    }
}

/// The section structure of `template` at `stage`.
pub fn sections(template: &str, stage: &str) -> &'static [Section] {
    if stage == "review" {
        return REVIEW;
    }
    match template {
        "implementation" => IMPLEMENTATION,
        "investigation" => INVESTIGATION,
        "verification" => VERIFICATION,
        "planning" => PLANNING,
        "documentation" => DOCUMENTATION,
        "synthesis" => SYNTHESIS,
        _ => GENERAL,
    }
}

/// The brief's report object: template, opening and every section in order.
pub fn skeleton(profile: &str, stage: &str) -> Value {
    let template = template_for(profile, stage);
    json!({"template":template,
        "opening":"Start with the title and a short decision brief: conclusion, evidence, checks, limits, next action.",
        "sections":sections(template, stage).iter().map(|s| json!({"heading":format!("## {}",s.heading),
            "holds":s.holds,"required":s.required})).collect::<Vec<_>>()})
}

fn normal(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// The level-two headings of a Markdown text outside fenced code, normalized.
fn headings(report: &str) -> Vec<String> {
    let mut fenced: Option<&str> = None;
    let mut out = Vec::new();
    for line in report.lines() {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        for fence in ["```", "~~~"] {
            if indent <= 3 && trimmed.starts_with(fence) {
                fenced = match fenced {
                    None => Some(fence),
                    Some(open) if open == fence => None,
                    other => other,
                };
            }
        }
        if fenced.is_some() || indent > 3 {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("##") {
            if rest.starts_with(char::is_whitespace) {
                out.push(normal(rest.trim().trim_end_matches('#')));
            }
        }
    }
    out
}

/// The required `## ` headings of the structure that `report` lacks, in order.
pub fn missing_sections(report: &str, profile: &str, stage: &str) -> Vec<String> {
    let present = headings(report);
    sections(template_for(profile, stage), stage)
        .iter()
        .filter(|s| s.required && !present.contains(&normal(s.heading)))
        .map(|s| format!("## {}", s.heading))
        .collect()
}

/// Whether `report` still contains a report template's guidance comment line.
/// Other HTML comments are ordinary report content.
pub fn guidance_remaining(report: &str) -> bool {
    [
        include_str!("../../../report-templates/general.md"),
        include_str!("../../../report-templates/implementation.md"),
        include_str!("../../../report-templates/investigation.md"),
        include_str!("../../../report-templates/verification.md"),
        include_str!("../../../report-templates/documentation.md"),
        include_str!("../../../report-templates/planning.md"),
        include_str!("../../../report-templates/synthesis.md"),
    ]
    .iter()
    .flat_map(|t| t.lines())
    .any(|line| line.starts_with("<!--") && line.ends_with("-->") && report.contains(line))
}

fn bounded(text: &str, cap: usize) -> Value {
    if text.chars().count() <= cap {
        return json!(text);
    }
    json!({"preview":text.chars().take(cap).collect::<String>(),"size_chars":text.chars().count(),
        "sha256":crate::digest(text.as_bytes())})
}

/// One report a worker should read: identity, role, title, size and page count.
fn report_entry(
    db: &Connection,
    task: &str,
    report: &str,
    node: Option<&str>,
    role: Option<&str>,
) -> CoreResult<Value> {
    let row: Option<(String, i64)> = db
        .query_row(
            "SELECT e.title,e.size_bytes FROM reports r JOIN editions e ON e.report_id=r.id
             WHERE r.task_id=? AND r.id=? ORDER BY e.sequence DESC LIMIT 1",
            (task, report),
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let mut entry = json!({"report_id":report});
    if let Some(node) = node {
        entry["node"] = json!(node);
    }
    if let Some(role) = role {
        entry["role"] = json!(role.trim_start_matches("worker-"));
    }
    match row {
        Some((title, bytes)) => {
            entry["title"] = bounded(&title, 200);
            entry["size_bytes"] = json!(bytes);
            entry["pages_at_most"] =
                json!(((bytes + PAGE_CHARACTERS - 1) / PAGE_CHARACTERS).max(1));
        }
        None => entry["availability"] = json!("unavailable"),
    }
    Ok(entry)
}

/// Reports the assignment of `node` (current revision) builds on: each dependency's
/// report, and for the runtime-inserted consultation every report its packet names.
pub fn predecessor_reports(db: &Connection, task: &str, node: &str) -> CoreResult<Vec<Value>> {
    let revision: i64 = db.query_row(
        "SELECT revision FROM runtime_tasks WHERE task_id=?",
        [task],
        |r| r.get(0),
    )?;
    let mut out = Vec::new();
    if node == "_consultation" {
        let packet: Option<String> = db
            .query_row(
                "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='CONSULTATION_REQUIRED'
                 AND json_extract(payload_json,'$.revision')=? ORDER BY id DESC LIMIT 1",
                (task, revision),
                |r| r.get(0),
            )
            .optional()?;
        let packet: Value = packet
            .map(|p| serde_json::from_str(&p))
            .transpose()?
            .unwrap_or(Value::Null);
        for item in packet["dependency_reports"]
            .as_array()
            .into_iter()
            .flatten()
        {
            if let Some(id) = item["report_id"].as_str() {
                let origin: Option<(String, String)> = db
                    .query_row(
                        "SELECT node_id,role FROM runtime_nodes WHERE task_id=? AND json_extract(result_json,'$.report_ref')=? ORDER BY revision DESC LIMIT 1",
                        (task, id),
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?;
                out.push(report_entry(
                    db,
                    task,
                    id,
                    origin.as_ref().map(|o| o.0.as_str()),
                    origin.as_ref().map(|o| o.1.as_str()),
                )?);
            }
        }
        return Ok(out);
    }
    let deps: Option<String> = db
        .query_row(
            "SELECT dependencies_json FROM runtime_nodes WHERE task_id=? AND revision=? AND node_id=?",
            (task, revision, node),
            |r| r.get(0),
        )
        .optional()?;
    let deps: Vec<String> = deps
        .map(|d| serde_json::from_str(&d))
        .transpose()?
        .unwrap_or_default();
    for dep in deps {
        let row: Option<(String, Option<String>, Option<String>)> = db
            .query_row(
                "SELECT role,json_extract(result_json,'$.report_ref'),json_extract(result_json,'$.report_structure.status')
                 FROM runtime_nodes WHERE task_id=? AND revision=? AND node_id=?",
                (task, revision, &dep),
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((role, Some(report), marker)) = row else {
            continue;
        };
        let mut entry = report_entry(db, task, &report, Some(&dep), Some(&role))?;
        if let Some(marker) = marker {
            entry["report_structure"] = json!(marker);
        }
        out.push(entry);
    }
    Ok(out)
}

/// The owner's constraints in force: the owner's messages to this task (newest kept,
/// each text bounded with its size and sha256), the write denials and the access mode.
pub fn constraints(db: &Connection, task: &str) -> CoreResult<Value> {
    let rows = db
        .prepare(
            "SELECT id,json_extract(payload_json,'$.text') FROM runtime_events WHERE task_id=? AND type='USER_STEERING'
             AND json_type(payload_json,'$.text')='text' ORDER BY id DESC LIMIT ?",
        )?
        .query_map((task, OWNER_MESSAGES as i64 + 1), |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    let more = rows.len() > OWNER_MESSAGES;
    let messages: Vec<Value> = rows
        .iter()
        .take(OWNER_MESSAGES)
        .rev()
        .map(|(id, text)| json!({"event_id":id,"text":bounded(text, MESSAGE_CHARS)}))
        .collect();
    let all = crate::lifecycle::steering::denials(db, task)?;
    let mut paths = Vec::new();
    let mut used = 0;
    for path in &all {
        used += path.chars().count() + 4;
        if used > DENIAL_CHARS {
            break;
        }
        paths.push(path.clone());
    }
    let mut value = json!({"owner_messages":messages,
        "write_denials":{"paths":paths,"complete":paths.len()==all.len(),"total":all.len()},
        "access_mode":crate::access::current(db, task)?});
    if more {
        value["older_owner_messages"] = json!("retained in the capsule's retained_user_messages");
    }
    Ok(value)
}

/// What the plan requires for acceptance: the runtime's own verification commands.
fn acceptance(db: &Connection, task: &str) -> CoreResult<Value> {
    let plan: Option<String> = db.query_row(
        "SELECT plan_json FROM runtime_tasks WHERE task_id=?",
        [task],
        |r| r.get(0),
    )?;
    let plan: Value = plan
        .map(|p| serde_json::from_str(&p))
        .transpose()?
        .unwrap_or(Value::Null);
    let commands: Vec<Value> = plan["verification"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| {
            if c.to_string().chars().count() <= 600 {
                c.clone()
            } else {
                bounded(&c.to_string(), 600)
            }
        })
        .collect();
    Ok(json!({"deliver":"the objective, within the scope",
        "verification_commands":commands,
        "accepted_when":"these commands pass when the runtime runs them after the independent review approves"}))
}

/// Resolve the task's owner language (`domain::task_language_tx`) and keep the
/// `task_language` row in step with it, for every reader of that table.
pub fn record_language_tx(db: &rusqlite::Transaction<'_>, task: &str) -> CoreResult<&'static str> {
    let language = crate::lifecycle::domain::task_language_tx(db, task)?;
    db.execute(
        "INSERT INTO task_language(task_id,language) SELECT ?1,?2 WHERE EXISTS(SELECT 1 FROM tasks WHERE id=?1)
         ON CONFLICT(task_id) DO UPDATE SET language=excluded.language WHERE language IS NOT excluded.language",
        (task, language),
    )?;
    Ok(language)
}

/// The brief fields the runtime adds to an assignment: acceptance, constraints,
/// predecessor reports, the report skeleton and the owner's question language.
pub fn extras(
    db: &Connection,
    task: &str,
    node: &str,
    role: &str,
    stage: &str,
    language: &str,
) -> CoreResult<Value> {
    let mut v = json!({"acceptance":acceptance(db, task)?,"constraints":constraints(db, task)?,
        "report":skeleton(role, stage),"user_question_language":language});
    if stage != "review" {
        let reports = predecessor_reports(db, task, node)?;
        if !reports.is_empty() {
            v["predecessor_reports"] = json!(reports);
        }
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn file_headings(template: &str) -> Vec<String> {
        let text = std::fs::read_to_string(format!(
            "{}/../../report-templates/{template}.md",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        text.lines()
            .filter_map(|l| l.strip_prefix("## "))
            .map(str::to_owned)
            .collect()
    }
    #[test]
    fn every_structure_follows_its_template_file_in_order() {
        for template in [
            "general",
            "implementation",
            "investigation",
            "verification",
            "planning",
            "documentation",
            "synthesis",
        ] {
            let table: Vec<String> = sections(template, "execution")
                .iter()
                .map(|s| s.heading.to_owned())
                .collect();
            assert_eq!(table, file_headings(template), "{template}");
            assert!(sections(template, "execution").iter().any(|s| s.required));
        }
        let review: Vec<_> = sections("general", "review")
            .iter()
            .filter(|s| s.required)
            .map(|s| s.heading)
            .collect();
        assert_eq!(review, ["Verdict", "Findings", "Coverage"]);
    }
    #[test]
    fn the_runtime_chooses_the_template_from_profile_and_stage() {
        assert_eq!(
            template_for("worker-backend-dev", "execution"),
            "implementation"
        );
        assert_eq!(
            template_for("worker-explorer", "execution"),
            "investigation"
        );
        assert_eq!(
            template_for("worker-technical-writer", "execution"),
            "documentation"
        );
        assert_eq!(template_for("worker-architect", "execution"), "planning");
        assert_eq!(
            template_for("worker-qa-engineer", "execution"),
            "verification"
        );
        assert_eq!(
            template_for("worker-senior-consultant", "execution"),
            "general"
        );
        assert_eq!(template_for("worker-code-reviewer", "review"), "general");
        assert_eq!(template_for("worker-backend-dev", "review"), "general");
        assert_eq!(template_for("unknown", "execution"), "general");
    }
    #[test]
    fn heading_presence_is_case_and_space_tolerant_and_ignores_code() {
        let report = "# T\n\n##   changes \n\nx\n\n## VERIFICATION ##\n\n```\n## Risks and remaining work\n```\n";
        assert_eq!(
            missing_sections(report, "worker-backend-dev", "execution"),
            ["## Risks and remaining work"]
        );
        let fixed = format!("{report}\n## Risks  and Remaining work\n");
        assert!(missing_sections(&fixed, "worker-backend-dev", "execution").is_empty());
        // Level three or a different wording is not the section.
        assert_eq!(
            missing_sections(
                "### Verdict\n## Findings\n## Scope\n",
                "worker-code-reviewer",
                "review"
            ),
            ["## Verdict", "## Coverage"]
        );
    }
    #[test]
    fn only_template_guidance_comments_count_as_left_over_guidance() {
        assert!(!guidance_remaining(
            "# Report\n\nThe page uses <!-- a comment --> in its HTML.\n"
        ));
        let line = include_str!("../../../report-templates/implementation.md")
            .lines()
            .find(|l| l.starts_with("<!--"))
            .unwrap();
        assert!(guidance_remaining(&format!("# Report\n\n{line}\n")));
    }
}
