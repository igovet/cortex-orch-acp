//! Runtime choices from verified plans and review evidence. Lifecycle code owns
//! authorization, scheduling, event emission and state transitions.
use crate::{lifecycle::MAX_REVIEW_CYCLES, CoreResult};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};

pub mod memory;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Mode {
    Direct,
    Fast,
    Collaborative,
    Deep,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Fast => "fast",
            Self::Collaborative => "collaborative",
            Self::Deep => "deep",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Risk {
    Low,
    Standard,
    High,
}

impl Risk {
    pub fn name(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Standard => "standard",
            Self::High => "high",
        }
    }

    fn from_value(value: Option<&str>) -> Option<Self> {
        match value? {
            "low" => Some(Self::Low),
            "standard" => Some(Self::Standard),
            "high" => Some(Self::High),
            _ => None,
        }
    }
}

fn sensitive_path(path: &str) -> bool {
    path.split('/').any(|part| {
        let part = part.to_ascii_lowercase();
        [
            "auth",
            "credential",
            "permission",
            "access",
            "policy",
            "security",
            "sandbox",
            "migration",
            "schema",
            "storage",
            "store",
            "concurren",
            "scheduler",
            "ownership",
            "transaction",
            "mutex",
            "semaphore",
            "lock",
        ]
        .iter()
        .any(|marker| {
            part.split(|c: char| !c.is_ascii_alphanumeric())
                .any(|component| component.starts_with(marker))
        })
    })
}

/// A test file wherever it lives: under a `tests`/`test` folder, or named like a
/// test (`*.test.*`, `*.spec.*`, `*_test.*`, `test_*`).
pub(crate) fn test_file(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    path.split('/')
        .rev()
        .skip(1)
        .any(|d| d == "tests" || d == "test" || d == "__tests__")
        || name.contains(".test.")
        || name.contains(".spec.")
        || name.contains("_test.")
        || name.starts_with("test_")
}

/// The developer for product code a review finding names: frontend sources to the
/// frontend developer, scripts and CI to DevOps, a mix of frontend and other code
/// to the full-stack developer, anything else to the backend developer.
pub(crate) fn fix_role(files: &[String]) -> &'static str {
    let ext = |p: &str| p.rsplit_once('.').map_or("", |(_, e)| e).to_owned();
    let frontend = |p: &String| {
        [
            "ts", "tsx", "js", "jsx", "mjs", "css", "scss", "html", "vue", "svelte",
        ]
        .contains(&ext(p).as_str())
    };
    let ops = |p: &String| {
        p.starts_with("scripts/")
            || p.starts_with(".github/")
            || p.rsplit('/')
                .next()
                .is_some_and(|n| n.starts_with("Dockerfile"))
            || ext(p) == "sh"
    };
    let code: Vec<&String> = files
        .iter()
        .filter(|p| !p.ends_with(".md") && !p.starts_with("docs/") && !test_file(p))
        .collect();
    if !code.is_empty() && code.iter().all(|p| ops(p)) {
        "worker-devops-engineer"
    } else if !code.is_empty() && code.iter().all(|p| frontend(p)) {
        "worker-frontend-dev"
    } else if code.iter().any(|p| frontend(p)) {
        "worker-fullstack-dev"
    } else {
        "worker-backend-dev"
    }
}

fn documentation_or_test(path: &str) -> bool {
    path.ends_with(".md") || test_file(path)
}

fn high_work(node: &Value) -> bool {
    let role = node["role"].as_str().map(|role| role.trim_start_matches("worker-"));
    node["complexity"] == "high"
        || matches!(role, Some("security-auditor" | "database-architect"))
        || matches!(role, Some("explorer" | "general"))
            && node["competencies"].as_array().is_some_and(|ids| {
                ids.iter().any(|id| matches!(id.as_str(), Some("security" | "database")))
            })
        || node["scope"]
            .as_array()
            .is_some_and(|scopes| scopes.iter().filter_map(Value::as_str).any(sensitive_path))
}

fn planned_risk(plan: &Value) -> Risk {
    if plan["tasks"]
        .as_array()
        .is_some_and(|nodes| nodes.len() >= 6 || nodes.iter().any(high_work))
    {
        Risk::High
    } else {
        Risk::Low
    }
}

fn plan_mode(plan: &Value) -> Mode {
    let Some(nodes) = plan["tasks"].as_array() else {
        return Mode::Fast;
    };
    if nodes
        .iter()
        .any(|n| n["role"] == "worker-senior-consultant")
        || planned_risk(plan) == Risk::High
    {
        Mode::Deep
    } else if nodes.len() > 1 {
        Mode::Collaborative
    } else {
        Mode::Fast
    }
}

pub fn current_mode(db: &Connection, task: &str) -> CoreResult<Option<Mode>> {
    let name: Option<String> = db.query_row(
        "SELECT json_extract(payload_json,'$.strategy.mode') FROM runtime_events
         WHERE task_id=? AND type IN ('PLAN_ACCEPTED','PLAN_AMENDED','STRATEGY_ESCALATED','ANSWER_SUBMITTED')
         AND json_type(payload_json,'$.strategy.mode')='text' ORDER BY id DESC LIMIT 1",
        [task], |r| r.get(0),
    ).optional()?;
    Ok(match name.as_deref() {
        Some("direct") => Some(Mode::Direct),
        Some("fast") => Some(Mode::Fast),
        Some("collaborative") => Some(Mode::Collaborative),
        Some("deep") => Some(Mode::Deep),
        _ => None,
    })
}

pub fn select_plan(db: &Connection, task: &str, plan: &Value) -> CoreResult<Value> {
    let selected = plan_mode(plan);
    let previous = current_mode(db, task)?;
    let mode = previous.map_or(selected, |prior| prior.max(selected));
    Ok(
        json!({"mode":mode.name(),"required_by_plan":selected.name(),"planned_risk":planned_risk(plan).name(),
        "previous_mode":previous.map(Mode::name)}),
    )
}

pub fn path_risk<'a>(paths: impl Iterator<Item = &'a str>) -> Risk {
    let mut risk = Risk::Low;
    for (index, path) in paths.enumerate() {
        if index >= 20 || sensitive_path(path) {
            return Risk::High;
        }
        if index >= 4 || !documentation_or_test(path) {
            risk = Risk::Standard;
        }
    }
    risk
}

fn review_risk_floor(db: &Connection, task: &str) -> CoreResult<Risk> {
    // Across replans and rebindings, neither a model hint nor a smaller later patch
    // can downgrade an earlier risk classification.
    let seen: Option<i64> = db.query_row(
        "SELECT max(CASE coalesce(json_extract(payload_json,'$.packet.review_strategy.risk'),
                                 json_extract(payload_json,'$.strategy.planned_risk'))
          WHEN 'high' THEN 2 WHEN 'standard' THEN 1 WHEN 'low' THEN 0 END)
         FROM runtime_events WHERE task_id=? AND type IN ('REVIEW_PACKET','PLAN_ACCEPTED','PLAN_AMENDED')",
        [task],
        |r| r.get(0),
    )?;
    let raw: Option<String> = db.query_row(
        "SELECT plan_json FROM runtime_tasks WHERE task_id=?",
        [task],
        |r| r.get(0),
    )?;
    let plan = serde_json::from_str::<Value>(raw.as_deref().unwrap_or("{}"))?;
    let planned = planned_risk(&plan);
    Ok(match seen {
        Some(2) => Risk::High,
        Some(1) => planned.max(Risk::Standard),
        _ => planned,
    })
}

pub fn review_strategy(
    db: &Connection,
    task: &str,
    packet: &mut Value,
    observed_bytes: Option<usize>,
) -> CoreResult<Risk> {
    let changed = &packet["changed_files"];
    let path_risk = packet["review_strategy"]["path_risk"].clone();
    let mut risk = Risk::from_value(path_risk.as_str())
        .unwrap_or(Risk::Standard)
        .max(review_risk_floor(db, task)?);
    if changed["complete"] != true || changed["attribution_complete"] != true {
        risk = Risk::High;
    }
    risk = risk.max(match observed_bytes {
        Some(bytes) if bytes > 64 * 1024 => Risk::High,
        Some(bytes) if bytes > 8 * 1024 => Risk::Standard,
        Some(_) => Risk::Low,
        None => Risk::Standard,
    });
    let policy = ReviewPolicy::for_risk(risk);
    packet["review_strategy"] = json!({"risk":risk.name(),"path_risk":path_risk,"max_cycles":policy.cycles,
        "max_lineage_cycles":policy.lineage_cycles});
    if risk == Risk::Low {
        if let Some(files) = packet["changed_files"]["files"].as_array_mut() {
            files.retain(|file| file["required"] == true);
            packet["changed_files"]["total"] = json!(files.len());
        }
        if let Some(changed) = packet["changed_files"].as_object_mut() {
            changed.remove("more");
        }
    }
    if risk == Risk::High {
        packet["review_strategy"]["focus"] = json!([
            "Inspect relevant callers, configuration and boundary conditions in addition to every required file.",
            "Check the applicable permission boundaries, failure and rollback paths, and concurrent behavior separately.",
            "Cross-check the verification commands and identify any applicable check without evidence."
        ]);
        let checks: Option<String> = db.query_row(
            "SELECT json_extract(plan_json,'$.verification') FROM runtime_tasks WHERE task_id=?",
            [task],
            |r| r.get(0),
        )?;
        if let Some(checks) = checks {
            packet["review_strategy"]["planned_verification"] = serde_json::from_str(&checks)?;
        }
    }
    Ok(risk)
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ReviewPolicy {
    pub cycles: usize,
    pub lineage_cycles: usize,
}

impl ReviewPolicy {
    fn for_risk(risk: Risk) -> Self {
        match risk {
            Risk::Low => Self {
                cycles: 2,
                lineage_cycles: 2,
            },
            Risk::Standard => Self {
                cycles: MAX_REVIEW_CYCLES,
                lineage_cycles: 12,
            },
            Risk::High => Self {
                cycles: MAX_REVIEW_CYCLES,
                lineage_cycles: 12,
            },
        }
    }
}

pub(crate) fn review_policy(db: &Connection, task: &str) -> CoreResult<ReviewPolicy> {
    let observed: Option<String> = db.query_row(
        "SELECT json_extract(payload_json,'$.packet.review_strategy.risk')
         FROM runtime_events WHERE task_id=? AND type='REVIEW_PACKET'
         AND json_type(payload_json,'$.packet.review_strategy.risk')='text' ORDER BY id DESC LIMIT 1",
        [task], |r| r.get(0),
    ).optional()?;
    let risk = Risk::from_value(observed.as_deref()).unwrap_or(Risk::Standard);
    Ok(ReviewPolicy::for_risk(
        risk.max(review_risk_floor(db, task)?),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE runtime_events (id INTEGER PRIMARY KEY, task_id TEXT, type TEXT, payload_json TEXT);
             CREATE TABLE runtime_tasks (task_id TEXT PRIMARY KEY, plan_json TEXT);
             INSERT INTO runtime_tasks VALUES ('t', '{\"tasks\":[],\"verification\":[[\"cargo\",\"test\"]]}');",
        ).unwrap();
        db
    }

    fn record(db: &Connection, event: &str, payload: Value) {
        db.execute(
            "INSERT INTO runtime_events (task_id,type,payload_json) VALUES ('t',?,?)",
            (event, payload.to_string()),
        )
        .unwrap();
    }

    #[test]
    fn chooses_depth_from_the_actual_graph_and_risk() {
        let node = json!({"role":"worker-frontend-dev","scope":["ui/button.ts"]});
        assert_eq!(plan_mode(&json!({"tasks":[node]})), Mode::Fast);
        assert_eq!(
            plan_mode(&json!({"tasks":[node,node]})),
            Mode::Collaborative
        );
        let deep = json!({"tasks":[node,{"role":"worker-backend-dev","scope":["crates/core/storage/migrate.rs"]}]});
        assert_eq!(plan_mode(&deep), Mode::Deep);
    }

    #[test]
    fn canonical_modules_keep_the_same_upward_risk_as_their_legacy_roles() {
        for (legacy, module) in [("security-auditor", "security"), ("database-architect", "database")] {
            for role in ["worker-explorer", "worker-general"] {
                for complexity in ["low", "normal", "high"] {
                    let prior = json!({"role":legacy,"scope":["."],"complexity":complexity});
                    let canonical = json!({"role":role,"competencies":[module],"scope":["."],"complexity":complexity});
                    assert_eq!(high_work(&canonical), high_work(&prior));
                    assert!(high_work(&canonical));
                }
            }
        }
        for modules in [json!([]), json!(["data","backend","testing"]), json!(["security-adjacent"])] {
            assert!(!high_work(&json!({"role":"worker-general","competencies":modules,"scope":["."]})));
        }
        assert!(!high_work(&json!({"role":"worker-backend-dev","competencies":["security"],"scope":["."]})));
    }

    #[test]
    fn bounds_small_review_without_weakening_high_risk_review() {
        assert_eq!(
            path_risk(["guide.md", "ui/tests/button.test.ts"].into_iter()),
            Risk::Low
        );
        assert_eq!(path_risk(["ui/button.ts"].into_iter()), Risk::Standard);
        assert_eq!(path_risk(["docs/permissions.md"].into_iter()), Risk::High);
        assert_eq!(path_risk(std::iter::repeat_n("guide.md", 21)), Risk::High);
        assert_eq!(
            ReviewPolicy::for_risk(Risk::Low),
            ReviewPolicy {
                cycles: 2,
                lineage_cycles: 2
            }
        );
        assert_eq!(ReviewPolicy::for_risk(Risk::High).cycles, MAX_REVIEW_CYCLES);
    }

    #[test]
    fn retained_plan_mode_can_increase_and_cannot_decrease() {
        let db = db();
        let single = json!({"tasks":[{"role":"backend-dev","scope":["src/a.rs"]}]});
        let multiple =
            json!({"tasks":[single["tasks"][0],{"role":"frontend-dev","scope":["ui/panel.ts"]}]});
        for (plan, expected) in [(&single, "fast"), (&multiple, "collaborative")] {
            let strategy = select_plan(&db, "t", plan).unwrap();
            assert_eq!(strategy["mode"], expected);
            record(&db, "PLAN_ACCEPTED", json!({"strategy":strategy}));
        }
        let smaller = select_plan(&db, "t", &single).unwrap();
        assert_eq!(smaller["required_by_plan"], "fast");
        assert_eq!(smaller["mode"], "collaborative");
        record(
            &db,
            "STRATEGY_ESCALATED",
            json!({"strategy":{"mode":"deep"}}),
        );
        assert_eq!(select_plan(&db, "t", &single).unwrap()["mode"], "deep");
    }

    #[test]
    fn low_risk_packet_keeps_required_files_and_later_observation_can_raise_risk() {
        let db = db();
        let original = json!({"review_strategy":{"path_risk":"low"},"changed_files":{
            "complete":true,"attribution_complete":true,"required":1,"total":2,
            "files":[{"path":"guide.md","required":true},{"path":"unrelated.rs","required":false}]
        }});
        let mut packet = original.clone();
        assert_eq!(
            review_strategy(&db, "t", &mut packet, Some(1024)).unwrap(),
            Risk::Low
        );
        assert_eq!(
            packet["changed_files"]["files"].as_array().unwrap().len(),
            1
        );
        record(&db, "REVIEW_PACKET", json!({"packet":packet}));
        assert_eq!(review_policy(&db, "t").unwrap().cycles, 2);

        packet = original.clone();
        assert_eq!(
            review_strategy(&db, "t", &mut packet, Some(65537)).unwrap(),
            Risk::High
        );
        assert_eq!(
            packet["changed_files"]["files"].as_array().unwrap().len(),
            2
        );
        assert_eq!(
            packet["review_strategy"]["planned_verification"],
            json!([["cargo", "test"]])
        );
        record(&db, "REVIEW_PACKET", json!({"packet":packet}));
        assert_eq!(review_policy(&db, "t").unwrap().cycles, MAX_REVIEW_CYCLES);
        assert_eq!(
            review_strategy(&db, "t", &mut original.clone(), Some(1024)).unwrap(),
            Risk::High
        );
    }
}
