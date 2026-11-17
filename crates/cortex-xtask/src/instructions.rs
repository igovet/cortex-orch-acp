//! Native instruction data (`generate-instructions`): the portable role and skill text the
//! Rust host embeds, assembled from the canonical `skills/` sources at build time.

use crate::fsutil::{py_strip, read_bytes, read_text, write_text};
use crate::json::{dumps, Format, Res, Value};
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub const TARGET: &str = "crates/workspace-service/schema/instructions.json";

const COORDINATOR_ROLE_IDS: &[&str] = &["orchestrator"];
const SHARED_ROLE_IDS: &[&str] = &[
    "cortex-control", "coordinator-communication", "content-safety", "tool-discipline", "knowledge-harvest",
    "documentation-sync", "context-compaction", "skill-management", "skill-authoring", "knowledge-harvest-scope",
    "tool-discipline-worker", "requirements-intake", "design-workflow", "design-frontend-design",
    "design-ui-ux-pro-max", "design-impeccable", "design-reference-analysis", "design-visual-qa",
];
const COORDINATOR_ONLY_SHARED: &[&str] = &[
    "cortex-control", "coordinator-communication", "skill-management", "knowledge-harvest-scope",
    "requirements-intake", "design-workflow",
];
const WORKER_ONLY_SHARED: &[&str] = &[
    "tool-discipline-worker", "skill-authoring", "knowledge-harvest", "design-frontend-design",
    "design-ui-ux-pro-max", "design-impeccable", "design-reference-analysis", "design-visual-qa",
];
const MUTATION_ONLY_SHARED: &[&str] = &["skill-authoring", "knowledge-harvest"];
const DESIGN_SKILL_ROLES: &[(&str, &[&str])] = &[
    ("design-frontend-design", &["worker-frontend-dev", "worker-mobile-dev"]),
    ("design-ui-ux-pro-max", &["worker-ux-designer", "worker-mobile-dev"]),
    ("design-impeccable", &["worker-ux-designer", "worker-accessibility-auditor", "worker-frontend-dev"]),
    ("design-reference-analysis", &["worker-ux-designer"]),
    (
        "design-visual-qa",
        &[
            "worker-ux-designer", "worker-accessibility-auditor", "worker-frontend-dev", "worker-mobile-dev",
            "worker-qa-engineer",
        ],
    ),
];
const READ_ONLY: &[&str] = &[
    "worker-architect", "worker-ux-designer", "worker-code-reviewer", "worker-senior-consultant",
    "senior-consultant", "worker-explorer", "worker-planner", "worker-security-auditor",
    "worker-accessibility-auditor", "worker-database-architect", "worker-performance-engineer",
    "worker-build-verification",
];
const CONSULTANT_ROLE_ID: &str = "worker-senior-consultant";
const CONSULTANT_EXCLUDED_NAMED: &[&str] = &[
    "code-and-evidence", "interactive-resources", "documentation-sync", "knowledge-harvest",
    "tool-discipline-worker", "skill-authoring",
];
const WORKER_REFERENCE_IDS: &[&str] = &["report-publication", "code-and-evidence", "interactive-resources"];
const COORDINATOR_REFERENCE_IDS: &[&str] = &[
    "cortex-release", "governance", "host-compatibility", "incident", "live-qualification",
    "pipeline-publication", "senior-consultation", "verification-coverage", "worker-routing", "question-banks",
    "agreed-spec", "design-intake", "PRODUCT", "DESIGN", "SCREENS", "visual-review",
];
const COORDINATOR_REFERENCE_PATHS: &[(&str, &str)] = &[
    ("question-banks", "requirements-intake/references/question-banks.md"),
    ("agreed-spec", "requirements-intake/references/agreed-spec.md"),
    ("design-intake", "design-workflow/references/design-intake.md"),
    ("PRODUCT", "design-workflow/references/PRODUCT.md"),
    ("DESIGN", "design-workflow/references/DESIGN.md"),
    ("SCREENS", "design-workflow/references/SCREENS.md"),
    ("visual-review", "design-workflow/references/visual-review.md"),
];
const EXAMPLE_REFERENCE_IDS: &[&str] = &[
    "index", "documentation-example", "implementation-example", "investigation-example", "plan-example",
    "report-example", "verification-example",
];
const DESIGN_REFERENCE_PATHS: &[(&str, &str, &str)] = &[
    ("design-colors", "design-ui-ux-pro-max", "references/colors.csv"),
    ("design-typography", "design-ui-ux-pro-max", "references/typography.csv"),
    ("design-ux-guidelines", "design-ui-ux-pro-max", "references/ux-guidelines.csv"),
    ("design-pro-rules", "design-ui-ux-pro-max", "references/pro-rules.md"),
    ("design-critique", "design-impeccable", "references/critique.md"),
    ("design-refinement", "design-impeccable", "references/refinement.md"),
    ("design-defect-checklist", "design-visual-qa", "references/defect-checklist.md"),
    ("design-banned-patterns", "design-visual-qa", "references/banned-patterns.md"),
];
const NAMED_COORDINATOR: &[&str] = &[
    "tool-discipline", "coordinator-communication", "content-safety", "context-compaction", "documentation-sync",
    "knowledge-harvest-scope", "cortex-control", "skill-management", "requirements-intake", "design-workflow",
    "cortex-release", "governance", "host-compatibility", "incident", "live-qualification",
    "pipeline-publication", "senior-consultation", "verification-coverage", "worker-routing",
];
const NAMED_WORKER: &[&str] = &[
    "tool-discipline", "tool-discipline-worker", "content-safety", "report-publication", "code-and-evidence",
    "interactive-resources", "index", "context-compaction", "documentation-sync", "knowledge-harvest",
    "skill-authoring", "design-frontend-design", "design-ui-ux-pro-max", "design-impeccable",
    "design-reference-analysis", "design-visual-qa",
];
const INLINED_COORDINATOR: &[&str] = &["tool-discipline", "content-safety", "coordinator-communication"];
const INLINED_WORKER: &[&str] =
    &["tool-discipline", "tool-discipline-worker", "content-safety", "report-publication", "code-and-evidence"];
const SKILL_TRIGGERS: &[(&str, &str)] = &[
    ("context-compaction", "before continuing after context is compacted or runs out mid-work"),
    ("documentation-sync", "before assessing documentation impact for a plan or updating docs affected by a behavior or contract change"),
    ("knowledge-harvest", "before work on an explicit /harvest or /harvest-refresh assignment"),
    ("knowledge-harvest-scope", "before scoping an explicit harvest or harvest-refresh request"),
    ("skill-authoring", "before creating, revising or importing an authorized project skill"),
    ("requirements-intake", "before planning a new request whose goal lacks information that would materially change the result"),
    ("design-workflow", "the user requests new UI/UX design, a visual redesign, a design system or significant visual improvement"),
    ("design-frontend-design", "you build or reshape a screen and must make visual design choices"),
    ("design-ui-ux-pro-max", "you choose a visual system or check UX rules for a product UI"),
    ("design-impeccable", "you critique or polish an already-rendered interface"),
    ("design-reference-analysis", "you derive design principles from a reference site or screenshot"),
    ("design-visual-qa", "you capture or judge a rendered UI across target viewports"),
    ("skill-management", "the user invoked /skill-management or approved a skill proposal, or retained reports repeat a procedure"),
    ("cortex-control", "the user asks about /cortex-clear chat retention"),
    ("interactive-resources", "before starting or using a browser, device, port or server session"),
    ("index", "you want a full-report example"),
    ("cortex-release", "before release-sensitive package, profile-generation or live-qualification work"),
    ("governance", "the plan carries user constraints, risk tradeoffs or governance wording to state"),
    ("host-compatibility", "before describing what the installed ACP client or engine actually supports"),
    ("incident", "the user reports a live incident or defect to diagnose and repair"),
    ("live-qualification", "before planning or reporting a live/real-client qualification check"),
    ("pipeline-publication", "before submitting the final synthesis"),
    ("senior-consultation", "before requesting a senior consultation (escalation_request) for a material uncertainty"),
    ("verification-coverage", "before choosing or writing the plan's required verification commands"),
    ("worker-routing", "before proposing assignment nodes, roles, dependencies or scopes in a plan"),
];

type Set = BTreeSet<String>;

fn set(items: &[&str]) -> Set {
    items.iter().map(|s| s.to_string()).collect()
}

fn contains(items: &[&str], id: &str) -> bool {
    items.contains(&id)
}

struct Roles {
    skills_root: PathBuf,
    native: Regex,
    frontmatter: Regex,
    footer: Regex,
}

impl Roles {
    fn new(root: &Path) -> Roles {
        Roles {
            skills_root: root.join("skills"),
            native: Regex::new(r"(?s)<!--\s*cortex:native\s+[a-z0-9_-]+\s*-->.*?<!--\s*/cortex:native\s*-->").unwrap(),
            frontmatter: Regex::new(r"(?s)\A---\n.*?\n---\n+").unwrap(),
            footer: Regex::new(r"\n*<!--\s*END OF COMPLETE CORTEX[^>]*-->\s*\z").unwrap(),
        }
    }

    fn known_role_ids(&self) -> Res<Vec<String>> {
        let mut ids = set(COORDINATOR_ROLE_IDS);
        ids.extend(set(SHARED_ROLE_IDS));
        let entries = std::fs::read_dir(&self.skills_root).map_err(|e| format!("cannot read skills: {e}"))?;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("worker-") && entry.path().join("SKILL.md").is_file() {
                ids.insert(name);
            }
        }
        Ok(ids.into_iter().collect())
    }

    fn role_kind(&self, id: &str) -> Res<&'static str> {
        if contains(COORDINATOR_ROLE_IDS, id) {
            Ok("coordinator")
        } else if contains(SHARED_ROLE_IDS, id) {
            Ok("shared")
        } else if id.starts_with("worker-") {
            Ok("worker")
        } else {
            Err(format!("unknown_role: '{id}' is not one of the known role ids"))
        }
    }

    fn shared_skill_ids(&self, kind: &str, role_id: Option<&str>) -> Set {
        if kind == "coordinator" {
            return &set(SHARED_ROLE_IDS) - &set(WORKER_ONLY_SHARED);
        }
        let mut ids = &set(SHARED_ROLE_IDS) - &set(COORDINATOR_ONLY_SHARED);
        for (skill, roles) in DESIGN_SKILL_ROLES {
            if !role_id.map(|r| roles.contains(&r)).unwrap_or(false) {
                ids.remove(*skill);
            }
        }
        if role_id == Some(CONSULTANT_ROLE_ID) {
            ids = &ids - &set(WORKER_ONLY_SHARED);
        } else if let Some(role) = role_id {
            if contains(READ_ONLY, role) {
                ids = &ids - &set(MUTATION_ONLY_SHARED);
            }
        }
        ids
    }

    fn reference_ids(&self, kind: &str, role_id: Option<&str>) -> Set {
        match kind {
            "coordinator" => &set(COORDINATOR_REFERENCE_IDS) | &set(EXAMPLE_REFERENCE_IDS),
            "worker" => {
                let mut ids = &set(WORKER_REFERENCE_IDS) | &set(EXAMPLE_REFERENCE_IDS);
                for (reference, skill, _) in DESIGN_REFERENCE_PATHS {
                    let allowed = DESIGN_SKILL_ROLES.iter().find(|(s, _)| s == skill).map(|(_, r)| *r).unwrap_or(&[]);
                    if role_id.map(|r| allowed.contains(&r)).unwrap_or(false) {
                        ids.insert(reference.to_string());
                    }
                }
                ids
            }
            _ => Set::new(),
        }
    }

    fn read_reference(&self, id: &str, kind: &str, role_id: Option<&str>) -> Res<String> {
        if !self.reference_ids(kind, role_id).contains(id) {
            return Err(format!("unknown_reference: {id:?} is unavailable for {kind}"));
        }
        let source = if let Some((_, skill, rel)) = DESIGN_REFERENCE_PATHS.iter().find(|(r, _, _)| *r == id) {
            self.skills_root.join(skill).join(rel)
        } else if let Some((_, rel)) = COORDINATOR_REFERENCE_PATHS.iter().find(|(r, _)| *r == id) {
            self.skills_root.join(rel)
        } else if contains(COORDINATOR_REFERENCE_IDS, id) {
            self.skills_root.join("orchestrator/references").join(format!("{id}.md"))
        } else if contains(WORKER_REFERENCE_IDS, id) {
            self.skills_root.join("worker-general/references").join(format!("{id}.md"))
        } else {
            self.skills_root.join("cortex-control/references").join(format!("{id}.md"))
        };
        read_text(&source).map_err(|e| format!("reference_source_unavailable: {id:?}: {e}"))
    }

    /// `load_role(id).body_portable`.
    fn body_portable(&self, role_id: &str) -> Res<String> {
        let path = self.skills_root.join(role_id).join("SKILL.md");
        let text = String::from_utf8(read_bytes(&path)?).map_err(|_| format!("not UTF-8: {}", path.display()))?;
        let text = self.native.replace_all(&text, "").into_owned();
        let text = self.frontmatter.replace(&text, "").into_owned();
        Ok(self.footer.replace(&text, "").into_owned())
    }

    fn situational_skill_triggers(&self, kind: &str, role_id: Option<&str>) -> Vec<(&'static str, &'static str)> {
        let mut allowed = self.shared_skill_ids(kind, role_id);
        allowed.extend(set(WORKER_REFERENCE_IDS));
        allowed.extend(set(COORDINATOR_REFERENCE_IDS));
        allowed.extend(set(EXAMPLE_REFERENCE_IDS));
        let named: &[&str] = match kind {
            "coordinator" => NAMED_COORDINATOR,
            "worker" => NAMED_WORKER,
            _ => &[],
        };
        let inlined = inlined_for(kind);
        named
            .iter()
            .filter(|id| allowed.contains(**id))
            .filter(|id| role_id != Some(CONSULTANT_ROLE_ID) || !contains(CONSULTANT_EXCLUDED_NAMED, id))
            .filter(|id| !contains(inlined, id))
            .filter_map(|id| SKILL_TRIGGERS.iter().find(|(s, _)| s == id).map(|(s, t)| (*s, *t)))
            .collect()
    }

    fn composed_instructions(&self, role_id: &str, kind: &str) -> Res<String> {
        let loadable = self.situational_skill_triggers(kind, Some(role_id));
        let mut parts = vec![self.body_portable(role_id)?];
        let mut inlined: Vec<(String, String)> = Vec::new();
        for skill_id in inlined_for(kind) {
            if role_id == CONSULTANT_ROLE_ID && contains(CONSULTANT_EXCLUDED_NAMED, skill_id) {
                continue;
            }
            let text = if contains(WORKER_REFERENCE_IDS, skill_id) || contains(COORDINATOR_REFERENCE_IDS, skill_id) {
                self.read_reference(skill_id, kind, None)
            } else {
                self.body_portable(skill_id)
            }
            .map_err(|e| {
                format!("required_skill_unavailable: cannot compose instructions for role '{role_id}': skill '{skill_id}' failed to load ({e})")
            })?;
            if py_strip(&text).is_empty() {
                return Err(format!("required_skill_empty: '{skill_id}' for role '{role_id}'"));
            }
            inlined.push((skill_id.to_string(), py_strip(&text).to_string()));
        }
        if !inlined.is_empty() {
            let body: Vec<String> = inlined.iter().map(|(id, text)| format!("### Skill `{id}`\n\n{text}")).collect();
            parts.push(format!(
                "## Skills already included\n\nThese skills apply to almost every session of your kind, so the runtime includes them here in full. Follow them from your first action; do not read them again with `read_skill`.\n\n{}",
                body.join("\n\n")
            ));
        }
        if !loadable.is_empty() {
            let lines: Vec<String> = loadable.iter().map(|(id, trigger)| format!("- `{id}` -- {trigger}")).collect();
            parts.push(format!(
                "## Skills, and when to read one\n\nThe `read_skill` tool on the `cortex` MCP server returns one skill by its exact name from the list below; use it, not an engine-native skill loader. When an action below becomes necessary, call Cortex MCP `read_skill` with that id before the action, then follow its body. Do not read skills whose trigger has not arisen or re-read skills already included above:\n{}",
                lines.join("\n")
            ));
        }
        let kept: Vec<&str> = parts.iter().map(|p| py_strip(p)).filter(|p| !p.is_empty()).collect();
        Ok(kept.join("\n\n") + "\n")
    }
}

fn inlined_for(kind: &str) -> &'static [&'static str] {
    match kind {
        "coordinator" => INLINED_COORDINATOR,
        "worker" => INLINED_WORKER,
        _ => &[],
    }
}

pub fn expected(root: &Path) -> Res<String> {
    let roles = Roles::new(root);
    let mut profiles = Value::obj();
    let mut skills: BTreeMap<String, String> = BTreeMap::new();
    for name in roles.known_role_ids()? {
        let kind = roles.role_kind(&name)?;
        if kind != "coordinator" && kind != "worker" {
            continue;
        }
        let references = roles.reference_ids(kind, Some(&name));
        let mut allowed = roles.shared_skill_ids(kind, Some(&name));
        allowed.extend(references.iter().cloned());
        for skill in &allowed {
            let text = if references.contains(skill) {
                roles.read_reference(skill, kind, Some(&name))?
            } else {
                roles.body_portable(skill)?
            };
            if py_strip(&text).is_empty() {
                return Err("empty required instruction source".into());
            }
            if let Some(previous) = skills.get(skill) {
                if *previous != text {
                    return Err("inconsistent role-visible skill".into());
                }
            }
            skills.insert(skill.clone(), text);
        }
        let mut profile = Value::obj();
        profile.set("kind", Value::str(kind));
        profile.set("body", Value::Str(roles.composed_instructions(&name, kind)?));
        profile.set("skills", Value::Arr(allowed.iter().map(|s| Value::str(s.clone())).collect()));
        profiles.set(&name, profile);
    }
    let mut skill_map = Value::obj();
    for (id, text) in skills {
        skill_map.set(&id, Value::Str(text));
    }
    let mut out = Value::obj();
    out.set("version", Value::int(1));
    out.set("profiles", profiles);
    out.set("skills", skill_map);
    let mut text = dumps(&out, &Format { sort_keys: true, ensure_ascii: false, indent: Some(2), compact: false })?;
    text.push('\n');
    Ok(text)
}

pub fn run(root: &Path, write: bool) -> Res<()> {
    let text = expected(root)?;
    let target = root.join(TARGET);
    if write {
        write_text(&target, &text)?;
    } else if std::fs::read(&target).ok().as_deref() != Some(text.as_bytes()) {
        return Err("native instruction data is stale; regenerate with generate-instructions write".into());
    }
    println!("native instruction data: {} passed", if write { "write" } else { "check" });
    Ok(())
}
