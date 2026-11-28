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
        let mut templates: BTreeMap<String, String> = doc["profiles"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| {
                Some((
                    format!("worker-{}", p["name"].as_str()?.replace('_', "-")),
                    p["report_template"].as_str()?.to_owned(),
                ))
            })
            .collect();
        if let Some(legacy) = doc["legacy_report_templates"].as_object() {
            templates.extend(legacy.iter().filter_map(|(role, template)| {
                Some((role.clone(), template.as_str()?.to_owned()))
            }));
        }
        templates
    })
}

/// The report template the runtime selects for a worker profile and stage.
pub fn template_for(profile: &str, stage: &str) -> &'static str {
    if stage == "review" {
        return "general";
    }
    let name = format!("worker-{}", profile.trim_start_matches("worker-"));
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

/// Optional reports a review brief lists, newest first; the rest are counted.
pub const OPTIONAL_REVIEW_REPORTS: usize = 24;
/// How a reviewer uses the split, carried in its review packet.
pub const REVIEW_READING_RULE: &str = "Read every predecessor_reports entry to its last page with read_report before result_submit; a review that has not is refused with the unread ids. optional_reports (earlier fix rounds and reviews, work without recorded writes, earlier plans) are context: read one only when it bears on what you judge. unavailable_reports cannot be read and are not required.";

/// The predecessor reports of review node `review` (current revision), bounded to
/// what that review judges.
///
/// `required` is the work of this review's round and nothing older: after a
/// completed review of the same revision, the fixes scheduled since (every one,
/// with or without writes) and that review's own report, so its findings are
/// neither re-raised nor lost; for the first review of a revision, the work nodes
/// whose writes or changes the runtime recorded (all of the round's work when none
/// recorded any). Only nodes the review depends on count. `optional` lists every
/// other report it builds on (earlier rounds, work without recorded writes) and the
/// reports of earlier plans, newest first and at most [`OPTIONAL_REVIEW_REPORTS`];
/// `optional_more` counts the rest. A required report without a published edition
/// is moved to `unavailable` and never required.
pub fn review_reports(db: &Connection, task: &str, review: &str) -> CoreResult<Value> {
    let revision: i64 = db.query_row(
        "SELECT revision FROM runtime_tasks WHERE task_id=?",
        [task],
        |r| r.get(0),
    )?;
    let own: Option<(i64, String)> = db
        .query_row(
            "SELECT rowid,dependencies_json FROM runtime_nodes WHERE task_id=? AND revision=? AND node_id=?",
            (task, revision, review),
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let (own_row, deps) = own.unwrap_or((i64::MAX, "[]".into()));
    let deps: std::collections::BTreeSet<String> = serde_json::from_str::<Vec<String>>(&deps)?
        .into_iter()
        .collect();
    struct Node {
        row: i64,
        id: String,
        role: String,
        stage: String,
        report: String,
        marker: Option<String>,
        wrote: bool,
        typed_output: bool,
    }
    let nodes: Vec<Node> = db
        .prepare(
            "SELECT n.rowid,n.node_id,n.role,n.stage,json_extract(n.result_json,'$.report_ref'),
                json_extract(n.result_json,'$.report_structure.status'),
                EXISTS(SELECT 1 FROM runtime_events e JOIN trusted_channels c ON c.task_id=e.task_id
                  AND c.channel_id=json_extract(e.payload_json,'$.channel_id')
                  WHERE e.task_id=n.task_id AND e.type IN ('NATIVE_PROJECT_WRITTEN','NATIVE_PROJECT_CHANGED')
                  AND c.assignment_id=n.assignment_ref),
                EXISTS(SELECT 1 FROM reports r JOIN editions d ON d.report_id=r.id
                  JOIN report_provenance p ON p.edition_sequence=d.sequence JOIN json_each(p.artifacts) a
                  WHERE r.task_id=n.task_id AND r.id=json_extract(n.result_json,'$.report_ref')
                    AND json_extract(a.value,'$.kind') IN ('json','markdown'))
             FROM runtime_nodes n WHERE n.task_id=? AND n.revision=? AND n.rowid!=?
             AND json_type(n.result_json,'$.report_ref')='text' ORDER BY n.rowid",
        )?
        .query_map((task, revision, own_row), |r| {
            Ok(Node {
                row: r.get(0)?,
                id: r.get(1)?,
                role: r.get(2)?,
                stage: r.get(3)?,
                report: r.get(4)?,
                marker: r.get(5)?,
                wrote: r.get(6)?,
                typed_output: r.get(7)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|n| deps.contains(&n.id))
        .collect();
    let previous = nodes
        .iter()
        .filter(|n| {
            n.stage == "review"
                && n.row < own_row
                && !crate::lifecycle::sprint_planning::is_plan_review(&n.id)
        })
        .max_by_key(|n| n.row);
    let after = previous.map_or(i64::MIN, |p| p.row);
    let round: Vec<&Node> = nodes
        .iter()
        .filter(|n| n.stage == "execution" && n.row > after)
        .collect();
    let mut chosen: Vec<&Node> = if previous.is_some() || !round.iter().any(|n| n.wrote) {
        round.clone()
    } else {
        round.iter().copied().filter(|n| n.wrote).collect()
    };
    chosen.extend(previous);
    for node in &nodes {
        if node.stage == "execution" && node.typed_output && !chosen.iter().any(|n| n.row == node.row) {
            chosen.push(node);
        }
    }
    let entry = |n: &Node, revision: Option<i64>| -> CoreResult<Value> {
        let mut e = report_entry(db, task, &n.report, Some(&n.id), Some(&n.role))?;
        e["page_characters"] = json!(PAGE_CHARACTERS);
        if let Some(marker) = &n.marker {
            e["report_structure"] = json!(marker);
        }
        if let Some(revision) = revision {
            e["revision"] = json!(revision);
        }
        Ok(e)
    };
    let mut seen = std::collections::BTreeSet::new();
    let (mut required, mut unavailable) = (Vec::new(), Vec::new());
    for n in nodes
        .iter()
        .filter(|n| chosen.iter().any(|c| c.row == n.row))
    {
        if !seen.insert(n.report.clone()) {
            continue;
        }
        let e = entry(n, None)?;
        if e["availability"] == "unavailable" {
            unavailable.push(e);
        } else {
            required.push(e);
        }
    }
    let mut optional = Vec::new();
    let mut more = 0usize;
    for n in nodes.iter().rev() {
        if !seen.insert(n.report.clone()) {
            continue;
        }
        if optional.len() >= OPTIONAL_REVIEW_REPORTS {
            more += 1;
            continue;
        }
        let mut e = entry(n, None)?;
        e["why_optional"] = json!(if n.row < after {
            "earlier_round"
        } else {
            "no_recorded_writes"
        });
        optional.push(e);
    }
    // Earlier plans: bounded read, newest first; the remainder is only counted.
    let earlier: Vec<Node> = db
        .prepare(
            "SELECT revision,node_id,role,stage,json_extract(result_json,'$.report_ref'),
                json_extract(result_json,'$.report_structure.status')
             FROM runtime_nodes WHERE task_id=? AND revision<?
             AND json_type(result_json,'$.report_ref')='text' ORDER BY revision DESC,rowid DESC LIMIT ?",
        )?
        .query_map(
            (task, revision, (OPTIONAL_REVIEW_REPORTS * 2) as i64),
            |r| {
                Ok(Node {
                    row: r.get(0)?,
                    id: r.get(1)?,
                    role: r.get(2)?,
                    stage: r.get(3)?,
                    report: r.get(4)?,
                    marker: r.get(5)?,
                    wrote: false,
                    typed_output: false,
                })
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;
    let earlier_total: i64 = db.query_row(
        "SELECT count(*) FROM runtime_nodes WHERE task_id=? AND revision<? AND json_type(result_json,'$.report_ref')='text'",
        (task, revision),
        |r| r.get(0),
    )?;
    let mut counted = 0i64;
    for n in &earlier {
        counted += 1;
        if !seen.insert(n.report.clone()) {
            continue;
        }
        if optional.len() >= OPTIONAL_REVIEW_REPORTS {
            more += 1;
            continue;
        }
        let mut e = entry(n, Some(n.row))?;
        e["why_optional"] = json!("earlier_plan");
        optional.push(e);
    }
    more += (earlier_total - counted).max(0) as usize;
    let mut out = json!({"required":required,"optional":optional,"unavailable":unavailable});
    if more > 0 {
        out["optional_more"] = json!(more);
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
        if let Some(plan) = crate::lifecycle::plan::plan_in_force(db, task)? {
            if let Some(choice) = crate::lifecycle::plan::competency_decision(
                &crate::lifecycle::plan::node_routing(&plan, node),
            ) {
                v["competency_decision"] = choice;
            }
        }
        let reports = predecessor_reports(db, task, node)?;
        if !reports.is_empty() {
            v["predecessor_reports"] = json!(reports);
        }
        let required = crate::required_reports::selected(db, task, node)?;
        if !required.is_empty() {
            v["required_reports"] = json!(required);
            v["required_reports_guidance"] = json!("Use read_report on this assignment to read each required report or its specified section completely before submitting your result. Other predecessor reports are available when needed.");
        }
        if node != "_consultation" {
            let revision: Option<i64> = db.query_row(
                "SELECT revision FROM runtime_tasks WHERE task_id=?",
                [task],
                |r| r.get(0),
            ).optional()?;
            if let Some(revision) = revision {
                if let Some(advice) = crate::lifecycle::plan::previous_consultation(db, task, revision)? {
                    v["previous_consultation"] = advice;
                }
            }
        }
    }
    if node == "_consultation" {
        let plan: Option<String> = db.query_row(
            "SELECT plan_json FROM runtime_tasks WHERE task_id=?",
            [task], |row| row.get(0),
        )?;
        if let Some(plan) = plan {
            let plan: Value = serde_json::from_str(&plan)?;
            let proposed = plan["tasks"].as_array().into_iter().flatten()
                .filter(|item| item["id"] != "_consultation");
            let total = proposed.clone().count();
            let nodes = proposed.take(12).map(|item| json!({
                "node":item["id"],"role":item["role"],"dependencies":item["dependencies"],
                "objective":item["objective"].as_str().map(|s| s.chars().take(500).collect::<String>()),
                "scope":item["scope"].as_array().into_iter().flatten().take(8).collect::<Vec<_>>(),
            })).collect::<Vec<_>>();
            v["consultation_plan"] = json!({"nodes":nodes,"nodes_total":total,
                "verification":plan["verification"],
                "competency_decisions":crate::lifecycle::plan::plan_competency_decisions(&plan)});
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
    /// Three replans (revisions 1 to 4) and six fix rounds (2, 2, 0 and 2), each work
    /// and fix node writing, each with a published report. Returns the report id of
    /// every node by `(revision, node)`; the last review of revision 4 is pending.
    fn long_history(
        db: &rusqlite::Transaction<'_>,
    ) -> crate::CoreResult<BTreeMap<(i64, String), String>> {
        let mut reports = BTreeMap::new();
        let mut counter = 0u32;
        for (revision, rounds) in [(1i64, 2), (2, 2), (3, 0), (4, 2)] {
            let mut ids: Vec<String> = Vec::new();
            let mut add = |id: &str,
                           role: &str,
                           stage: &str,
                           deps: &[String],
                           wrote: bool,
                           done: bool|
             -> crate::CoreResult<()> {
                counter += 1;
                let assignment = format!("a{counter}");
                let report = format!("r_{counter:012x}");
                let result = done.then(|| json!({"report_ref":report}).to_string());
                db.execute(
                    "INSERT INTO runtime_nodes VALUES ('t',?,?,?,'o',?,'[\"src\"]',?,?,?,?)",
                    (
                        revision,
                        id,
                        role,
                        json!(deps).to_string(),
                        stage,
                        if done { "completed" } else { "pending" },
                        &assignment,
                        result,
                    ),
                )?;
                if done {
                    db.execute("INSERT INTO reports(id,task_id,kind,filename,created_at) VALUES (?,'t','report',?,'0')", (&report, format!("{report}.md")))?;
                    db.execute("INSERT INTO editions(report_id,title,summary,author,updated_at,digest,size_bytes) VALUES (?,'T','S','x','0','d',4100)", [&report])?;
                    reports.insert((revision, id.to_owned()), report);
                }
                if wrote {
                    let channel = format!("ch{counter}");
                    db.execute("INSERT INTO trusted_channels(channel_id,secret_hash,task_id,principal_thread_id,role,assignment_id,attempt_id,owner_epoch,issued_at)
                        VALUES (?,'x','t','p','worker',?,'x',1,'0')", (&channel, &assignment))?;
                    crate::lifecycle::event_tx(
                        db,
                        "t",
                        "NATIVE_PROJECT_WRITTEN",
                        json!({"channel_id":channel,"path":"src/a.rs"}),
                    )?;
                }
                Ok(())
            };
            add("work", "worker-backend-dev", "execution", &[], true, true)?;
            ids.push("work".into());
            add("scan", "worker-explorer", "execution", &[], false, true)?;
            ids.push("scan".into());
            add(
                "docs",
                "worker-technical-writer",
                "execution",
                &[],
                false,
                true,
            )?;
            ids.push("docs".into());
            if revision > 1 {
                add(
                    "_consultation",
                    "worker-senior-consultant",
                    "execution",
                    &[],
                    false,
                    true,
                )?;
                ids.push("_consultation".into());
            }
            for k in 1..=rounds {
                let review = format!("_review_{k}");
                add(
                    &review,
                    "worker-code-reviewer",
                    "review",
                    &ids.clone(),
                    false,
                    true,
                )?;
                ids.push(review.clone());
                let fix = format!("_fix_{k}_work");
                add(
                    &fix,
                    "worker-backend-dev",
                    "execution",
                    &[review],
                    true,
                    true,
                )?;
                ids.push(fix);
            }
            let last = format!("_review_{}", rounds + 1);
            add(
                &last,
                "worker-code-reviewer",
                "review",
                &ids.clone(),
                false,
                revision < 4,
            )?;
        }
        db.execute("UPDATE runtime_tasks SET revision=4 WHERE task_id='t'", [])?;
        Ok(reports)
    }
    fn ids(list: &Value) -> Vec<String> {
        list.as_array()
            .unwrap()
            .iter()
            .map(|e| e["report_id"].as_str().unwrap().to_owned())
            .collect()
    }
    /// After three replans and six fix rounds the current review requires only its
    /// round: the last fix and the review that asked for it. Everything older,
    /// across revisions, is optional and bounded; the first review of a revision
    /// requires the writing work only.
    #[test]
    fn review_reading_is_bounded_to_the_current_round() {
        use crate::{lifecycle::TaskMachine, OpenMode, ProjectKey, StoreChannel};
        let root = std::env::temp_dir().join(format!("review-reading-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let store = crate::Store::open(
            ProjectKey::new(&root, StoreChannel::Dev).unwrap(),
            OpenMode::Initialize,
        )
        .unwrap();
        TaskMachine::new(&store)
            .create("t", "Goal", "owner")
            .unwrap();
        let reports = store.write(|db| long_history(db)).unwrap();
        let r = |rev: i64, id: &str| reports[&(rev, id.to_owned())].clone();
        let split = store
            .read(|db| review_reports(db, "t", "_review_3"))
            .unwrap();
        assert_eq!(
            ids(&split["required"]),
            [r(4, "_review_2"), r(4, "_fix_2_work")]
        );
        let required = &split["required"][0];
        assert_eq!(required["node"], "_review_2");
        assert_eq!(required["pages_at_most"], 2);
        assert_eq!(required["page_characters"], PAGE_CHARACTERS);
        let optional = split["optional"].as_array().unwrap();
        assert_eq!(optional.len(), OPTIONAL_REVIEW_REPORTS);
        // 6 other reports of revision 4 plus 8, 9 and 5 of revisions 1 to 3.
        assert_eq!(
            split["optional_more"],
            6 + 8 + 9 + 5 - OPTIONAL_REVIEW_REPORTS
        );
        assert_eq!(optional[0]["report_id"], r(4, "_fix_1_work"));
        assert_eq!(optional[0]["why_optional"], "earlier_round");
        assert!(optional
            .iter()
            .any(|e| e["why_optional"] == "earlier_plan" && e["revision"] == 3));
        assert!(split["unavailable"].as_array().unwrap().is_empty());
        // The first review of a revision requires the work that wrote, not the
        // explorer or the consultation (both optional).
        let first = store
            .read(|db| review_reports(db, "t", "_review_1"))
            .unwrap();
        assert_eq!(ids(&first["required"]), [r(4, "work")]);
        let optional = ids(&first["optional"]);
        assert!(optional.contains(&r(4, "scan")) && optional.contains(&r(4, "_consultation")));
        // A required report without a published edition is unavailable, not required.
        store
            .write(|db| {
                db.execute("UPDATE runtime_nodes SET result_json=? WHERE task_id='t' AND revision=4 AND node_id='_fix_2_work'",
                    [json!({"report_ref":"r_ffffffffffff"}).to_string()])?;
                Ok(())
            })
            .unwrap();
        let split = store
            .read(|db| review_reports(db, "t", "_review_3"))
            .unwrap();
        assert_eq!(ids(&split["required"]), [r(4, "_review_2")]);
        assert_eq!(ids(&split["unavailable"]), ["r_ffffffffffff"]);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}
