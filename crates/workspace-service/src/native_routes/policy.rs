//! Settings-only route menu and deterministic policy ranking (Python rank v15).
use super::{contains, read, refuse, Candidate, RouteResolver, SelectedRoute};
use cortex_core::{routing::Evidence, CoreError, CoreResult};
use serde_json::{json, Value};
use std::collections::BTreeSet;

const TIERS: [&str; 4] = ["flagship", "deep", "standard", "light"];
const EFFORTS: [&str; 6] = ["low", "medium", "high", "xhigh", "max", "ultra"];
const KINDS: [&str; 12] = [
    "implementation_backend",
    "implementation_frontend",
    "implementation_frontend_design",
    "operations",
    "debugging",
    "testing",
    "investigation",
    "review",
    "design",
    "writing",
    "planning",
    "analysis",
];
const ROLES: [&str; 23] = [
    "accessibility-auditor",
    "accessibility-fixer",
    "architect",
    "backend-dev",
    "build-verification",
    "code-reviewer",
    "data-engineer",
    "database-architect",
    "debugger",
    "devops-engineer",
    "explorer",
    "frontend-dev",
    "fullstack-dev",
    "general",
    "mobile-dev",
    "performance-engineer",
    "planner",
    "qa-engineer",
    "refactorer",
    "security-auditor",
    "senior-consultant",
    "technical-writer",
    "ux-designer",
];
pub(super) fn role_kind(role: &str) -> &'static str {
    match role.trim_start_matches("worker-") {
        "accessibility-auditor" | "security-auditor" | "code-reviewer" => "review",
        "architect" | "database-architect" | "ux-designer" | "senior-consultant" => "design",
        "accessibility-fixer" | "frontend-dev" | "mobile-dev" => "implementation_frontend",
        "devops-engineer" => "operations",
        "explorer" => "investigation",
        "technical-writer" => "writing",
        "planner" => "planning",
        "qa-engineer" | "build-verification" => "testing",
        "debugger" | "performance-engineer" => "debugging",
        _ => "implementation_backend",
    }
}
/// Review route selection over the usable candidates (already filtered for owner
/// permission, automatic use, blocking and exhausted quota). `node["avoid_routes"]`
/// names each reviewed node's author route with its recorded changed-file count;
/// `node["previous_reviewers"]` the routes of earlier reviews of this revision.
///
/// 1. When any model the owner marked `preferred_for: review` is usable, only those
///    are kept. A model not preferred for review reviews only when none is usable
///    (`no_review_preferred_model`), never because of independence.
/// 2. Among them, independence in this order: (a) a model different from every
///    author model (`different_from_all_authors`); (b) a model different from the
///    dominant author's model, preferring another engine than that author's
///    (`different_from_dominant_author`); the dominant author is the reviewed node with
///    the most recorded changed files, ties going to the first in plan order; (c) the
///    authors' own review-preferred models, in a fresh session
///    (`same_model_fresh_session`). Without review-preferred models, the former filter
///    (another model, then another engine) applies to all candidates.
/// 3. A rerun prefers, within the chosen set, a model no earlier review of this
///    revision used. Ranking among the rest (quota, usage) is unchanged.
///
/// Returns the audit record, including the review's effort target: `high`, or
/// `xhigh` for high-complexity work, raised one step per rerun.
fn review_candidates<'a>(
    usable: &mut Vec<(&'a Candidate, Option<f64>)>,
    node: &Value,
    preferred: impl Fn(&Candidate) -> bool,
) -> Value {
    let authors: Vec<&Value> = node["avoid_routes"]
        .as_array()
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    let models: BTreeSet<&str> = authors.iter().filter_map(|a| a["model"].as_str()).collect();
    let engines: BTreeSet<&str> = authors
        .iter()
        .filter_map(|a| a["engine"].as_str())
        .collect();
    // The first author with the most changed files (`max_by_key` keeps the last of equals).
    let dominant = authors
        .iter()
        .enumerate()
        .max_by_key(|(i, a)| (a["changes"].as_i64().unwrap_or(0), usize::MAX - i))
        .map(|(_, a)| *a);
    let previous: BTreeSet<&str> = node["previous_reviewers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| r["model"].as_str())
        .collect();
    let review: Vec<_> = usable
        .iter()
        .filter(|(c, _)| preferred(c))
        .cloned()
        .collect();
    let rule = if review.is_empty() {
        if !models.is_empty() && usable.iter().any(|(c, _)| !models.contains(c.model())) {
            usable.retain(|(c, _)| !models.contains(c.model()));
            if usable.iter().any(|(c, _)| !engines.contains(c.engine())) {
                usable.retain(|(c, _)| !engines.contains(c.engine()));
            }
        }
        "no_review_preferred_model"
    } else {
        *usable = review;
        if usable.iter().any(|(c, _)| !models.contains(c.model())) {
            usable.retain(|(c, _)| !models.contains(c.model()));
            if models.is_empty() {
                "no_authors"
            } else {
                "different_from_all_authors"
            }
        } else if let Some(d) = dominant.filter(|d| {
            usable
                .iter()
                .any(|(c, _)| Some(c.model()) != d["model"].as_str())
        }) {
            usable.retain(|(c, _)| Some(c.model()) != d["model"].as_str());
            if usable
                .iter()
                .any(|(c, _)| Some(c.engine()) != d["engine"].as_str())
            {
                usable.retain(|(c, _)| Some(c.engine()) != d["engine"].as_str());
            }
            "different_from_dominant_author"
        } else {
            "same_model_fresh_session"
        }
    };
    let avoided_previous = !previous.is_empty()
        && usable.iter().any(|(c, _)| !previous.contains(c.model()))
        && usable.iter().any(|(c, _)| previous.contains(c.model()));
    if avoided_previous {
        usable.retain(|(c, _)| !previous.contains(c.model()));
    }
    let rerun = node["review_rerun"].as_u64().unwrap_or(0) as usize;
    let base = if node["complexity"] == "high" { 3 } else { 2 };
    json!({
        "rule": rule,
        "authors": authors.iter().map(|a| json!({"node":a["node"],"engine":a["engine"],"model":a["model"],"changes":a["changes"]})).collect::<Vec<_>>(),
        "dominant_author": dominant.map(|d| json!({"node":d["node"],"engine":d["engine"],"model":d["model"]})),
        "kept": usable.iter().map(|(c, _)| format!("{}/{}", c.engine(), c.model())).collect::<Vec<_>>(),
        "rerun": rerun,
        "avoided_previous_reviewer": avoided_previous,
        "effort_target": EFFORTS[(base + rerun).min(EFFORTS.len() - 1)],
    })
}
fn base_tier(role: &str) -> usize {
    match role.trim_start_matches("worker-") {
        "architect" | "database-architect" | "security-auditor" | "senior-consultant" => 1,
        "explorer" | "general" | "planner" | "technical-writer" => 3,
        _ => 2,
    }
}
fn role_tier(role: &str, complexity: &str, escalated: bool) -> usize {
    if role == "worker-explorer" {
        return 3;
    }
    let shift = match complexity {
        "low" => 1,
        "high" => -1,
        _ => 0,
    };
    let i = base_tier(role) as i32 + shift - i32::from(escalated);
    i.clamp(if escalated { 0 } else { 1 }, 3) as usize
}
fn kind_in(v: &Value, kind: &str) -> bool {
    contains(v, kind) || (kind.starts_with("implementation_") && contains(v, "implementation"))
}
fn kinds(v: &Value) -> Vec<String> {
    let mut result = BTreeSet::new();
    for k in v.as_array().into_iter().flatten().filter_map(Value::as_str) {
        if k == "implementation" {
            result.extend(KINDS[..3].iter().map(|s| s.to_string()));
        } else {
            result.insert(k.into());
        }
    }
    result.into_iter().collect()
}
fn label(c: &Candidate) -> String {
    format!("{}/{}", c.engine(), c.model())
}
fn tiers(v: &Value) -> Vec<&str> {
    TIERS
        .into_iter()
        .filter(|t| v.as_str() == Some(t) || contains(v, t))
        .collect()
}
fn hot(used: Option<f64>) -> bool {
    used.is_some_and(|n| n >= 95.0)
}
fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}
pub(super) fn validate_policy(r: &Value) -> CoreResult<()> {
    if let Some(engines) = r["models"].as_object() {
        for engine in engines.values() {
            if let Some(policy) = engine.get("policy").filter(|v| !v.is_null()) {
                let entries = policy
                    .as_object()
                    .ok_or(CoreError::Refused("invalid_routing_overrides"))?;
                for p in entries.values() {
                    let attrs = p
                        .as_object()
                        .ok_or(CoreError::Refused("invalid_routing_overrides"))?;
                    if attrs.keys().any(|k| {
                        ![
                            "effort_floor",
                            "max_effort",
                            "effort_ladder",
                            "also_standard_for",
                            "preferred_for",
                            "preferred_when",
                            "request_names",
                        ]
                        .contains(&k.as_str())
                    }) {
                        return refuse("invalid_routing_overrides");
                    }
                    for key in ["effort_floor", "max_effort"] {
                        if !p[key].is_null()
                            && !p[key].as_str().is_some_and(|s| EFFORTS[..5].contains(&s))
                        {
                            return refuse("invalid_routing_overrides");
                        }
                    }
                    for key in [
                        "preferred_for",
                        "also_standard_for",
                        "effort_ladder",
                        "request_names",
                    ] {
                        if p[key].is_null() {
                            continue;
                        }
                        let a = super::strings(&p[key])?;
                        if a.len() > 1000 || a.iter().collect::<BTreeSet<_>>().len() != a.len() {
                            return refuse("invalid_routing_overrides");
                        }
                        if ["preferred_for", "also_standard_for"].contains(&key)
                            && a.iter()
                                .any(|s| s != "implementation" && !KINDS.contains(&s.as_str()))
                        {
                            return refuse("invalid_routing_overrides");
                        }
                        if key == "effort_ladder"
                            && (a.iter().any(|s| !EFFORTS[..5].contains(&s.as_str()))
                                || a.windows(2).any(|w| {
                                    EFFORTS.iter().position(|s| *s == w[0])
                                        >= EFFORTS.iter().position(|s| *s == w[1])
                                }))
                        {
                            return refuse("invalid_routing_overrides");
                        }
                        if key == "request_names"
                            && (a.len() > 16
                                || a.iter().any(|s| {
                                    s.chars().count() < 2
                                        || s.chars().count() > 32
                                        || !s
                                            .chars()
                                            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
                                }))
                        {
                            return refuse("invalid_routing_overrides");
                        }
                    }
                    if !p["preferred_when"].is_null()
                        && !p["preferred_when"].as_str().is_some_and(|s| {
                            !s.trim().is_empty()
                                && s.chars().count() <= 500
                                && !s.chars().any(char::is_control)
                        })
                    {
                        return refuse("invalid_routing_overrides");
                    }
                    if let (Some(f), Some(m)) =
                        (p["effort_floor"].as_str(), p["max_effort"].as_str())
                    {
                        if EFFORTS.iter().position(|s| *s == f)
                            > EFFORTS.iter().position(|s| *s == m)
                        {
                            return refuse("invalid_routing_overrides");
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
impl RouteResolver {
    fn config(&self, c: &Candidate) -> &Value {
        &self.rules["models"][c.engine()]
    }
    fn policy(&self, c: &Candidate) -> &Value {
        &self.config(c)["policy"][c.model()]
    }
    pub(super) fn placed(&self, c: &Candidate) -> Vec<&str> {
        tiers(&self.config(c)["tiers"][c.model()])
    }
    pub(super) fn automatic(&self, c: &Candidate) -> Option<&'static str> {
        if !contains(&self.config(c)["auto"], c.model()) {
            Some("not_selected")
        } else if self.placed(c).is_empty() {
            Some("policy_missing")
        } else {
            None
        }
    }
    fn affinity(&self, kind: &str) -> (Option<&str>, bool) {
        let a = &self.rules["engine_affinity"];
        let configured = a
            .get(kind)
            .or_else(|| {
                if kind.starts_with("implementation_") {
                    a.get("implementation")
                } else {
                    None
                }
            })
            .or_else(|| a.get("*"));
        if let Some(value) = configured {
            return (value.as_str(), true);
        }
        (
            match kind {
                "implementation_backend" | "implementation_frontend" | "operations" => {
                    Some("codex-acp")
                }
                "debugging" | "testing" => None,
                _ => Some("claude-agent-acp"),
            },
            false,
        )
    }
    fn permitted(&self, c: &Candidate, tier: &str) -> CoreResult<Vec<String>> {
        let p = self.policy(c);
        let offered: BTreeSet<_> = c.entry["route"]["supported_parameters"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|p| p[0] == "thought_level")
            .filter_map(|p| p[1].as_str())
            .collect();
        let floor = p["effort_floor"].as_str();
        let ladder: Vec<_> = p["effort_ladder"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        if floor.is_some_and(|f| !offered.contains(f))
            || ladder.iter().any(|e| !offered.contains(e))
        {
            return refuse("model_effort_policy_unsupported");
        }
        let max = p["max_effort"].as_str().or(if tier == "flagship" {
            Some("high")
        } else {
            None
        });
        let mut result = Vec::new();
        for (i, e) in EFFORTS.iter().enumerate() {
            if offered.contains(e)
                && *e != "ultra"
                && (tier == "light" || *e != "max" || ladder.contains(e))
                && max.is_none_or(|m| i <= EFFORTS.iter().position(|v| *v == m).unwrap_or(0))
                && floor.is_none_or(|f| i >= EFFORTS.iter().position(|v| *v == f).unwrap_or(6))
            {
                result.push(e.to_string());
            }
        }
        if result.is_empty()
            && (floor.is_some()
                || max.is_some() && !p["max_effort"].is_null()
                || !ladder.is_empty())
        {
            return refuse("model_effort_policy_unsupported");
        }
        Ok(result)
    }
    fn efforts_any(&self, c: &Candidate) -> CoreResult<Vec<String>> {
        let mut result = BTreeSet::new();
        for tier in self.placed(c) {
            result.extend(self.permitted(c, tier)?);
        }
        Ok(EFFORTS
            .iter()
            .filter(|e| result.contains(**e))
            .map(|s| s.to_string())
            .collect())
    }
    pub(super) fn usage(&self, c: &Candidate) -> CoreResult<Option<f64>> {
        let cached = if c.engine() == "codex-acp" {
            c.connection["credential_ref"]["path"]
                .as_str()
                .map(std::path::Path::new)
                .map(super::quota::codex)
                .unwrap_or(json!({}))
        } else {
            read(
                &self
                    .environment
                    .state
                    .join("cortex/quota")
                    .join(format!("{}.json", c.connection_id())),
                true,
            )?
            .unwrap_or(json!({}))
        };
        let mut used: Option<f64> = None;
        if let Some(windows) = cached.as_object() {
            for (name, w) in windows {
                if name.contains("overage")
                    || c.engine() == "claude-agent-acp" && name == "five_hour"
                {
                    continue;
                }
                if w["resets_at"].as_f64().is_some_and(|t| t <= now()) {
                    continue;
                }
                let association = self.rules["quota_windows"][c.engine()]
                    .as_array()
                    .and_then(|a| a.iter().find(|v| v["window"] == *name))
                    .map(|v| &v["models"]);
                if association.is_some_and(|a| a.is_array() && !contains(a, c.model())) {
                    continue;
                }
                if let Some(n) = w["used"]
                    .as_f64()
                    .filter(|n| n.is_finite() && *n >= 0.0 && *n <= 100.0)
                {
                    used = Some(used.map_or(n, |old| old.max(n)));
                }
            }
        }
        Ok(used)
    }
    fn blocked<'a>(&self, c: &Candidate, evidence: &'a Evidence) -> Option<&'a Value> {
        evidence.unavailable.iter().find(|v| {
            v["connection_id"] == c.connection_id()
                && (v["model"].is_null() || v["model"] == c.model())
        })
    }
    fn attrs(&self, c: &Candidate) -> Value {
        let p = self.policy(c);
        let mut sources = json!({});
        for key in [
            "effort_floor",
            "max_effort",
            "effort_ladder",
            "also_standard_for",
            "preferred_for",
            "preferred_when",
            "request_names",
        ] {
            if !p[key].is_null() {
                sources[key] = json!("configured");
            }
        }
        json!({"effort_floor":p["effort_floor"],"max_effort":p["max_effort"],
            "effort_ladder":p["effort_ladder"].as_array().cloned().unwrap_or_default(),
            "also_standard_for":kinds(&p["also_standard_for"]),"preferred_for":kinds(&p["preferred_for"]),
            "preferred_when":p["preferred_when"],"request_names":p["request_names"].as_array().cloned().unwrap_or_default(),
            "attribute_sources":sources})
    }
    pub fn previews(&self, plan: &Value, evidence: &Evidence) -> CoreResult<Value> {
        let mut result = Vec::new();
        for n in plan["tasks"]
            .as_array()
            .ok_or(CoreError::Refused("invalid_plan"))?
        {
            let role = n["role"]
                .as_str()
                .ok_or(CoreError::Refused("invalid_plan"))?;
            let role = if role.starts_with("worker-") {
                role.into()
            } else {
                format!("worker-{role}")
            };
            let hint = n["model"].as_str().unwrap_or("auto");
            let kind = n["work_kind"].as_str().unwrap_or(role_kind(&role));
            if hint.contains('/')
                && !self.candidates.iter().any(|c| {
                    label(c) == hint && self.allowed(c, true) && !self.placed(c).is_empty()
                })
            {
                return refuse("unknown_model_hint");
            }
            let (grant, changed, _) = self.consent(&role, kind, hint, evidence);
            let tier = TIERS[role_tier(&role, n["complexity"].as_str().unwrap_or("normal"), false)];
            let mut item = json!({"node":n["id"],"model":hint,"tier_now":tier,"work_kind":kind,
                "preview_status":if hint=="auto"{"auto"}else{"pending_dispatch"}});
            if let Some(change) = changed {
                item["project_instruction_note"]=json!(format!("{} changed after this task started, so its model requirement for this role's work does not apply to this task; it applies from the next task.",change["path"].as_str().unwrap_or("")));
                item["project_instruction"] = change;
            }
            if let Some(grant) = grant {
                if grant["source"] == "project_instruction" {
                    item["preview_status"] = json!("pending_dispatch:project_instruction");
                    item["project_instruction"] = json!({"route":grant["family"],"path":grant["path"],"line":grant["line"],"matched":grant["matched"]});
                    if hint != grant["family"].as_str().unwrap_or("") {
                        item["preview_note"] = json!("The project's own instruction file requires this route for this role's work. It counts as the owner's request: dispatch uses it instead of this hint, ahead of pins, owner preferred models and usage, and refuses visibly rather than substituting when it cannot run.");
                    }
                } else {
                    item["preview_status"] = json!("pending_dispatch:user_request");
                    item["user_request"] = grant["source_ref"].clone();
                }
            } else if hint.contains('/') {
                let chosen = self.choose(&role, n, evidence, false, None)?;
                item["preview_status"] = chosen.decision["model_hint_status"].clone();
                if item["preview_status"] == "honoured" {
                    item["preview_status"] = json!("pending_dispatch");
                }
                if item["preview_status"] == "ignored:owner_preferred" {
                    let mut preferred = chosen.decision["purpose"]["preferred"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default();
                    preferred.sort_by_key(Value::to_string);
                    item["owner_preferred"] = json!(preferred);
                    item["preview_note"]=json!("The owner's preferred models for this work outrank this hint, so dispatch will choose one of owner_preferred; that is a legitimate route, not a mismatch to block on. Only a recorded user request for this exact route changes it.");
                }
            }
            result.push(item);
        }
        Ok(json!(result))
    }
    pub fn menu(&self, evidence: &Evidence) -> CoreResult<Value> {
        let mut menu = json!({"tiers":{"flagship":[],"deep":[],"standard":[],"light":[]},"roles":{},"role_work_kinds":{},
            "quota_window_policy":{"unmapped":"conservative_shared","rules":self.rules.get("quota_windows").cloned().unwrap_or(json!({}))},
            "affinity":{},"affinity_not_eligible":{},"affinity_source":{},"preferred":{},"preferred_when":{},
            "flagship":{"routes":[],"request_names":{},"rule":"escalation, or an exact route the user explicitly asked for in this task"},"stretch":[]});
        let mut eligible = BTreeSet::new();
        for c in self.candidates.iter().filter(|c| self.allowed(c, true)) {
            let label = label(c);
            let p = self.policy(c);
            let placed = self.placed(c);
            let reason = self.automatic(c);
            let usage = self.usage(c)?;
            for tier in &placed {
                let state = self
                    .blocked(c, evidence)
                    .map(|v| {
                        format!(
                            "unavailable_until:{}",
                            v["expires_at"].as_str().unwrap_or("")
                        )
                    })
                    .unwrap_or_else(|| {
                        if usage.is_some_and(|v| v >= 100.0) {
                            "exhausted".into()
                        } else {
                            "available".into()
                        }
                    });
                menu["tiers"][*tier].as_array_mut().unwrap().push(json!({"route":label,"engine":c.engine(),"model":c.model(),
                    "connection_id":c.connection_id(),"efforts":self.permitted(c,tier)?,"state":state,
                    "auto_eligible":reason.is_none(),"auto_reason":reason.unwrap_or("eligible"),"superseded_by":null,
                    "tier_source":"configured","tiers":placed,"selection":"listed","policy":self.attrs(c),
                    "quota":{"band":if usage.is_none(){"unknown"}else if hot(usage){"hot"}else{"cool"},"used_percent":usage}}));
            }
            if placed.contains(&"flagship") {
                menu["flagship"]["routes"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!(label));
                if p["request_names"].as_array().is_some_and(|a| !a.is_empty()) {
                    menu["flagship"]["request_names"][&label] = p["request_names"].clone();
                }
            }
            if reason.is_none() {
                eligible.insert(c.engine());
                if !p["preferred_when"].is_null() {
                    menu["preferred_when"][&label] =
                        json!({"when":p["preferred_when"],"work_kinds":kinds(&p["preferred_for"])});
                } else {
                    for kind in kinds(&p["preferred_for"]) {
                        if menu["preferred"].get(&kind).is_none() {
                            menu["preferred"][&kind] = json!([]);
                        }
                        menu["preferred"][&kind]
                            .as_array_mut()
                            .unwrap()
                            .push(json!(label));
                    }
                }
            }
            let ladder = p["effort_ladder"].as_array().cloned().unwrap_or_default();
            if placed.contains(&"light")
                && (!ladder.is_empty() || !kinds(&p["also_standard_for"]).is_empty())
            {
                let efforts = if ladder.is_empty() {
                    Value::Null
                } else {
                    json!({"light":ladder[0],"standard":ladder[1.min(ladder.len()-1)],"standard_escalated":ladder[2.min(ladder.len()-1)]})
                };
                menu["stretch"].as_array_mut().unwrap().push(json!({"route":label,"auto_eligible":reason.is_none(),
                    "effort_floor":p["effort_floor"],"efforts":efforts,"standard_work_kinds":kinds(&p["also_standard_for"]),
                    "standard_candidate_roles":ROLES.iter().map(|r|format!("worker-{r}")).filter(|r|!["worker-code-reviewer","worker-build-verification"].contains(&r.as_str())
                        && base_tier(r)>=2 && kind_in(&p["also_standard_for"],role_kind(r))).collect::<Vec<_>>(),
                    "never":"complexity high, deep or flagship work, deep roles"}));
            }
        }
        for kind in KINDS {
            let (a, configured) = self.affinity(kind);
            menu["affinity"][kind] = json!(a.filter(|a| eligible.contains(a)));
            if let Some(a) = a.filter(|a| !eligible.contains(a)) {
                menu["affinity_not_eligible"][kind] = json!(a);
            }
            menu["affinity_source"][kind] = json!(if configured {
                "routing.json"
            } else {
                "builtin"
            });
        }
        for role in ROLES
            .iter()
            .filter(|r| !["code-reviewer", "build-verification"].contains(r))
        {
            let role = format!("worker-{role}");
            menu["role_work_kinds"][&role] = json!(role_kind(&role));
            menu["roles"][&role] = json!({"low":TIERS[role_tier(&role,"low",false)],"normal":TIERS[role_tier(&role,"normal",false)],"high":TIERS[role_tier(&role,"high",false)]});
        }
        Ok(menu)
    }
    fn resolved(
        &self,
        c: &Candidate,
        tier: &str,
        effort: Option<&str>,
        basis: &'static str,
        mut decision: Value,
        escalated: bool,
        kind: &str,
    ) -> CoreResult<SelectedRoute> {
        let permitted = if effort.is_some() {
            self.efforts_any(c)?
        } else {
            self.permitted(c, tier)?
        };
        if effort.is_some_and(|e| !permitted.iter().any(|v| v == e)) {
            return refuse(if basis == "auto:user_request" {
                "user_requested_effort_unavailable"
            } else {
                "pinned_effort_unavailable"
            });
        }
        let ladder = self.policy(c)["effort_ladder"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let target = if effort.is_some() {
            effort.unwrap()
        } else if !ladder.is_empty()
            && (tier == "light" || kind_in(&self.policy(c)["also_standard_for"], kind))
        {
            ladder[(usize::from(tier != "light") + usize::from(escalated)).min(ladder.len() - 1)]
                .as_str()
                .unwrap()
        } else {
            match tier {
                "light" => "low",
                "standard" => "medium",
                _ => "high",
            }
        };
        // A review asks for at least its own effort target (`review_effort`), within
        // what the model permits (its floor up to its maximum).
        let position = |e: &str| EFFORTS.iter().position(|v| *v == e).unwrap_or(0);
        let target = match decision["review_route"]["effort_target"].as_str() {
            Some(review)
                if effort.is_none() && kind == "review" && position(review) > position(target) =>
            {
                EFFORTS[position(review)]
            }
            _ => target,
        };
        let selected_effort = effort.map(str::to_owned).or_else(|| {
            permitted
                .iter()
                .min_by_key(|e| {
                    let i = EFFORTS.iter().position(|v| v == &e.as_str()).unwrap_or(0);
                    let t = EFFORTS.iter().position(|v| v == &target).unwrap_or(0);
                    i.abs_diff(t)
                })
                .cloned()
        });
        let mut selected = self.selected(
            c,
            selected_effort.map_or(json!({}), |e| json!({"thought_level":e})),
            basis,
        )?;
        decision["chosen"] = json!(label(c));
        decision["chosen_connection_id"] = json!(c.connection_id());
        decision["tier"] = json!(tier);
        decision["rank_version"] = json!(15);
        selected.decision = decision;
        Ok(selected)
    }
    pub fn choose(
        &self,
        role: &str,
        node: &Value,
        evidence: &Evidence,
        escalated: bool,
        previous_engine: Option<&str>,
    ) -> CoreResult<SelectedRoute> {
        if !ROLES.contains(&role.trim_start_matches("worker-")) {
            return refuse("unknown_session_role");
        }
        let kind = node["work_kind"].as_str().unwrap_or(role_kind(role));
        if !KINDS.contains(&kind) {
            return refuse("invalid_work_kind");
        }
        let complexity = node["complexity"].as_str().unwrap_or("normal");
        if !["low", "normal", "high"].contains(&complexity) {
            return refuse("invalid_complexity");
        }
        let hint = node["model"].as_str().unwrap_or("auto");
        let (grant, changed, revocation) = self.consent(role, kind, hint, evidence);
        let mut decision = json!({"role":role,"model_hint":hint,"model_hint_reason":node["model_reason"],
            "role_tier":TIERS[role_tier(role,complexity,escalated)],"escalated":escalated});
        if let Some(changed) = changed {
            decision["project_instruction"] = changed;
        }
        if let Some(revocation) = revocation {
            decision["user_model_request"] = revocation;
        }
        if let Some(mut grant) = grant {
            if grant["effort_ambiguous"] == true {
                return refuse("user_requested_effort_ambiguous");
            }
            let route = grant["family"].as_str().unwrap();
            // Purpose requirements override a hint; non-purpose user consent is exact-hint only.
            let candidates: Vec<_> = self
                .candidates
                .iter()
                .filter(|c| {
                    label(c) == route
                        && self.allowed(c, true)
                        && self.blocked(c, evidence).is_none()
                })
                .collect();
            let mut matches = Vec::new();
            for c in candidates {
                let u = self.usage(c)?;
                if u.is_none_or(|v| v < 100.0) {
                    matches.push((c, u));
                }
            }
            matches.sort_by(|a, b| {
                hot(a.1)
                    .cmp(&hot(b.1))
                    .then_with(|| a.1.unwrap_or(0.0).total_cmp(&b.1.unwrap_or(0.0)))
            });
            let (c, _) = matches
                .first()
                .ok_or(CoreError::Refused("user_requested_route_unavailable"))?;
            let placed = self.placed(c);
            let tier = placed
                .first()
                .ok_or(CoreError::Refused("user_requested_route_unavailable"))?;
            let source = if grant["source"] == "project_instruction" {
                "project_instruction"
            } else {
                "user_request"
            };
            grant["status"] = json!("honoured");
            decision["basis"] = json!(source);
            decision["decided_by"] = json!(source);
            decision["model_hint_status"] = json!("honoured");
            decision["user_model_request"] = grant.clone();
            if source == "project_instruction" {
                decision["project_instruction"] = grant.clone();
            }
            return self.resolved(
                c,
                tier,
                grant["effort"].as_str(),
                "auto:user_request",
                decision,
                escalated,
                kind,
            );
        }
        let (affinity, configured) = self.affinity(kind);
        if kind == "implementation_frontend_design" && affinity.is_none() {
            return refuse("no_available_route");
        }
        let pins = &self.rules["pin"];
        if let Some(pin) = pins
            .get(role)
            .or_else(|| pins.get(role.trim_start_matches("worker-")))
            .or_else(|| pins.get("*"))
            .and_then(Value::as_str)
        {
            let (route, effort) = pin
                .rsplit_once(':')
                .map_or((pin, None), |(r, e)| (r, Some(e)));
            let mut candidates = Vec::new();
            for c in self.candidates.iter().filter(|c| {
                label(c) == route
                    && self.allowed(c, true)
                    && self.blocked(c, evidence).is_none()
                    && (kind != "implementation_frontend_design" || Some(c.engine()) == affinity)
            }) {
                let usage = self.usage(c)?;
                if usage.is_none_or(|v| v < 100.0) {
                    candidates.push((c, usage));
                }
            }
            candidates.sort_by(|a, b| {
                hot(a.1)
                    .cmp(&hot(b.1))
                    .then_with(|| a.1.unwrap_or(0.0).total_cmp(&b.1.unwrap_or(0.0)))
            });
            let (c, _) = candidates
                .first()
                .ok_or(CoreError::Refused("pinned_route_unavailable"))?;
            let placed = self.placed(c);
            let tier = placed.first().ok_or(CoreError::Refused("policy_missing"))?;
            if *tier == "flagship" && !escalated {
                return refuse("pinned_route_unavailable");
            }
            decision["basis"] = json!("pin");
            decision["decided_by"] = json!("pin");
            decision["model_hint_status"] = json!(if hint == "auto" {
                "auto"
            } else if escalated {
                "ignored:escalated"
            } else {
                "ignored:pinned"
            });
            let permitted = self.efforts_any(c)?;
            let automatic_tier = TIERS[role_tier(role, complexity, escalated)];
            let ladder = self.policy(c)["effort_ladder"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let target = if !ladder.is_empty() && placed.contains(&"light") {
                ladder[(usize::from(automatic_tier != "light") + usize::from(escalated))
                    .min(ladder.len() - 1)]
                .as_str()
                .unwrap()
            } else {
                match automatic_tier {
                    "light" => "low",
                    "standard" => "medium",
                    _ => "high",
                }
            };
            let automatic = permitted
                .iter()
                .min_by_key(|e| {
                    EFFORTS
                        .iter()
                        .position(|v| v == &e.as_str())
                        .unwrap_or(0)
                        .abs_diff(EFFORTS.iter().position(|v| *v == target).unwrap_or(0))
                })
                .map(String::as_str);
            if effort.is_none() && !ladder.is_empty() && !permitted.iter().any(|e| e == target) {
                return refuse("model_effort_policy_unsupported");
            }
            decision["effort_basis"] = json!(if effort.is_some() {
                "pin:explicit_effort"
            } else {
                "pin"
            });
            return self.resolved(
                c,
                tier,
                effort.or(automatic),
                "auto:pin",
                decision,
                escalated,
                kind,
            );
        }
        let base_role_tier = role_tier(role, complexity, false);
        let mut role_tier = role_tier(role, complexity, escalated);
        let stretch_allowed = base_tier(role) >= 2 && complexity != "high" && base_role_tier == 2;
        if escalated
            && stretch_allowed
            && self.candidates.iter().any(|c| {
                self.placed(c).contains(&"light")
                    && kind_in(&self.policy(c)["also_standard_for"], kind)
            })
        {
            role_tier = 2;
        }
        let order: &[&str] = match role_tier {
            0 => &TIERS,
            1 => &["deep", "standard", "light"],
            2 => &["standard", "deep", "light"],
            _ => &["light", "standard", "deep"],
        };
        let serves = |c: &Candidate, tier: &str| {
            self.placed(c).contains(&tier)
                || tier == "standard"
                    && stretch_allowed
                    && self.placed(c).contains(&"light")
                    && kind_in(&self.policy(c)["also_standard_for"], kind)
        };
        let purpose = |c: &Candidate| {
            let p = self.policy(c);
            if p["preferred_when"].is_null() || kind == "implementation_frontend_design" {
                kind_in(&p["preferred_for"], kind)
            } else {
                !escalated
                    && hint == label(c)
                    && (kinds(&p["preferred_for"]).is_empty() || kind_in(&p["preferred_for"], kind))
            }
        };
        let mut usable = Vec::new();
        for c in self.candidates.iter().filter(|c| {
            self.allowed(c, true)
                && self.automatic(c).is_none()
                && self.blocked(c, evidence).is_none()
        }) {
            let usage = self.usage(c)?;
            if usage.is_none_or(|v| v < 100.0) {
                usable.push((c, usage));
            }
        }
        // Review routing. A model the owner prefers for review always reviews when one is
        // usable; independence then chooses among those models only, never by falling
        // back to a model the owner did not mark for review. See `review_candidates`.
        let mut review_hint_dropped = None;
        if kind == "review" {
            let matches_hint = |c: &Candidate| hint == label(c) || hint == c.engine();
            let hinted_before: Vec<bool> = usable
                .iter()
                .filter(|(c, _)| matches_hint(c))
                .map(|(c, _)| purpose(c))
                .collect();
            let rule = review_candidates(&mut usable, node, |c| {
                purpose(c) && order.iter().any(|t| serves(c, t))
            });
            decision["review_route"] = rule;
            // A hinted model that review routing set aside says why.
            if !hinted_before.is_empty() && !usable.iter().any(|(c, _)| matches_hint(c)) {
                review_hint_dropped = Some(if hinted_before.iter().any(|p| *p) {
                    "ignored:review_independence"
                } else {
                    "ignored:owner_preferred"
                });
            }
        } else if let Some(avoid) = node["avoid_routes"].as_array().filter(|a| !a.is_empty()) {
            // Other independence-constrained work: leave only routes on another model when
            // one exists, then prefer another engine among them.
            let models: BTreeSet<&str> = avoid.iter().filter_map(|a| a["model"].as_str()).collect();
            let engines: BTreeSet<&str> =
                avoid.iter().filter_map(|a| a["engine"].as_str()).collect();
            if usable.iter().any(|(c, _)| !models.contains(c.model())) {
                usable.retain(|(c, _)| !models.contains(c.model()));
                if usable.iter().any(|(c, _)| !engines.contains(c.engine())) {
                    usable.retain(|(c, _)| !engines.contains(c.engine()));
                }
            }
        }
        let meaningfully_worse =
            |preferred: &Vec<(&Candidate, Option<f64>)>, all: &Vec<(&Candidate, Option<f64>)>| {
                let p = preferred
                    .iter()
                    .map(|(_, v)| *v)
                    .collect::<Option<Vec<_>>>();
                let all = all.iter().map(|(_, v)| *v).collect::<Option<Vec<_>>>();
                match (p, all) {
                    (Some(p), Some(all)) if !p.is_empty() && !all.is_empty() => {
                        let p = p.into_iter().fold(f64::INFINITY, f64::min);
                        let best = all.into_iter().fold(f64::INFINITY, f64::min);
                        p >= 95.0 && p - best > 10.0
                    }
                    _ => false,
                }
            };
        if kind == "implementation_frontend_design" {
            if usable
                .iter()
                .any(|(c, _)| Some(c.engine()) == affinity && order.iter().any(|t| serves(c, t)))
            {
                usable.retain(|(c, _)| Some(c.engine()) == affinity);
            }
            let preferred: Vec<_> = usable
                .iter()
                .filter(|(c, _)| purpose(c) && order.iter().any(|t| serves(c, t)))
                .cloned()
                .collect();
            if !preferred.is_empty() && !meaningfully_worse(&preferred, &usable) {
                usable = preferred;
            }
        }
        for tier in order {
            let mut candidates: Vec<_> = usable
                .iter()
                .filter(|(c, _)| serves(c, tier))
                .cloned()
                .collect();
            if candidates.is_empty() {
                continue;
            }
            let preferred: Vec<_> = candidates
                .iter()
                .filter(|(c, _)| purpose(c))
                .cloned()
                .collect();
            let purpose_active =
                !preferred.is_empty() && !meaningfully_worse(&preferred, &candidates);
            let hinted: Vec<_> = candidates
                .iter()
                .filter(|(c, _)| hint == label(c) || hint == c.engine())
                .cloned()
                .collect();
            let hint_active = !escalated
                && !hinted.is_empty()
                && !(hinted.iter().all(|(_, u)| hot(*u))
                    && candidates.iter().any(|(_, u)| !hot(*u)));
            let provider = node["provider"].as_str().filter(|s| *s != "auto");
            let provider_candidates: Vec<_> = candidates
                .iter()
                .filter(|(c, _)| Some(c.engine()) == provider)
                .cloned()
                .collect();
            let provider_active = provider.is_some()
                && !provider_candidates.is_empty()
                && !meaningfully_worse(&provider_candidates, &candidates);
            let configured_preferred = self.rules["models"].as_object().is_some_and(|engines| {
                engines.iter().any(|(engine, config)| {
                    config["tiers"].as_object().is_some_and(|models| {
                        models.iter().any(|(model, ts)| {
                            let p = &config["policy"][model];
                            tiers(ts).contains(tier)
                                && if p["preferred_when"].is_null()
                                    || kind == "implementation_frontend_design"
                                {
                                    kind_in(&p["preferred_for"], kind)
                                } else {
                                    !escalated
                                        && hint == format!("{engine}/{model}")
                                        && (kinds(&p["preferred_for"]).is_empty()
                                            || kind_in(&p["preferred_for"], kind))
                                }
                        })
                    })
                })
            });
            let usage_scope = if purpose_active {
                preferred.clone()
            } else if configured_preferred {
                Vec::new()
            } else {
                candidates.clone()
            };
            let usage_active =
                !usage_scope.is_empty() && usage_scope.iter().all(|(_, u)| u.is_some());
            let key = |(c, u): &(&Candidate, Option<f64>)| {
                (
                    u8::from(!(provider_active && Some(c.engine()) == provider)),
                    u8::from(!(purpose_active && purpose(c))),
                    u8::from(!(hint_active && (hint == label(c) || hint == c.engine()))),
                    u8::from(hot(*u)),
                    u8::from(!(escalated && previous_engine.is_some_and(|e| e != c.engine()))),
                    u8::from(Some(c.engine()) != affinity),
                )
            };
            candidates.sort_by(|a, b| {
                key(a)
                    .cmp(&key(b))
                    .then_with(|| {
                        let usage = |x: &(&Candidate, Option<f64>)| {
                            if usage_active
                                && usage_scope
                                    .iter()
                                    .any(|(c, _)| c.route_id() == x.0.route_id())
                            {
                                x.1.unwrap_or(0.0)
                            } else {
                                0.0
                            }
                        };
                        usage(a).total_cmp(&usage(b))
                    })
                    .then_with(|| {
                        self.candidates
                            .iter()
                            .position(|c| c.route_id() == a.0.route_id())
                            .cmp(
                                &self
                                    .candidates
                                    .iter()
                                    .position(|c| c.route_id() == b.0.route_id()),
                            )
                    })
            });
            let c = candidates[0].0;
            let decided_by = if candidates.len() == 1 {
                "only_candidate"
            } else {
                let a = key(&candidates[0]);
                let b = key(&candidates[1]);
                if a.0 != b.0 {
                    "preference"
                } else if a.1 != b.1 {
                    "purpose"
                } else if a.2 != b.2 {
                    "hint"
                } else if a.3 != b.3 {
                    "quota"
                } else if a.4 != b.4 {
                    "diversify"
                } else if a.5 != b.5 {
                    "affinity"
                } else if usage_active && candidates[0].1 != candidates[1].1 {
                    "used_percent"
                } else {
                    "engine_order"
                }
            };
            decision["decided_by"] = json!(decided_by);
            decision["used_percent_compared"] = json!(usage_active);
            decision["role_tier"] = json!(TIERS[role_tier]);
            if hint != "auto"
                && self
                    .candidates
                    .iter()
                    .any(|c| label(c) == hint && self.placed(c).contains(&"flagship"))
                && decision.get("user_model_request").is_none()
            {
                decision["user_model_request"] = json!({"status":"not_requested_by_user"});
            }
            let hint_status = if hint == "auto" {
                "auto"
            } else if escalated {
                "ignored:escalated"
            } else if purpose_active
                && !hinted.is_empty()
                && !hinted.iter().any(|(c, _)| purpose(c))
            {
                "ignored:owner_preferred"
            } else if let Some(reason) = review_hint_dropped.filter(|_| hinted.is_empty()) {
                reason
            } else if hint_active {
                "honoured"
            } else if !hinted.is_empty() {
                "ignored:near_quota_limit"
            } else {
                "ignored:tier_mismatch"
            };
            decision["model_hint_status"] = json!(hint_status);
            decision["basis"] = json!("rank");
            decision["affinity"] = json!({"work_kind":kind,"engine":affinity,"source":if configured{"routing.json"}else{"default"}});
            decision["purpose"] = json!({"work_kind":kind,"status":if purpose_active{"honoured"}else if preferred.is_empty(){"none"}else{"ignored:near_quota_limit"},
                "preferred":preferred.iter().map(|(c,_)|label(c)).collect::<Vec<_>>()});
            decision["usage"] = json!({"compared":usage_active,"active":usage_active,"status":if usage_active{"active"}else if configured_preferred{"not_applied:owner_preferred_unavailable"}else{"unavailable"},
                "scope":if purpose_active{"preferred"}else if configured_preferred{"none"}else{"all"},"threshold":95,"role":"tie_breaker"});
            decision["candidates"]=json!(candidates.iter().map(|(c,u)|json!({"route":label(c),"connection_id":c.connection_id(),"used_percent":u})).collect::<Vec<_>>());
            return self.resolved(c, tier, None, "auto", decision, escalated, kind);
        }
        refuse("no_available_route")
    }
}
