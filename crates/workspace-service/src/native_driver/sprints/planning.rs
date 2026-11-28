//! Sprint plan intake rules and planning checkpoint interpretation.
use crate::native_runtime::Runtime;
use serde_json::{json, Value};
use std::{collections::BTreeSet, io};

// A plan has no item count limit; it arrives in parts. Each
// `sprint_plan_submit` call carries at most 128 KiB of UTF-8 JSON, and the
// whole assembled plan at most 512 KiB: about a thousand items of 500
// characters, kept in one bounded review prompt and well inside the 8 MiB
// Workspace frame limit (shared with history and other state).
pub(crate) const MAX_PART_BYTES: usize = 128 * 1024;
const MAX_PLAN_BYTES: usize = 512 * 1024;
/// The draft description limit of `tasks.create`.
const MAX_REQUEST_CHARS: usize = 32_000;

/// A rejected plan: a stable `code` plus a short owner-safe `detail`
/// naming the failing task (1-based index and sanitized key) and rule. The
/// detail never contains raw model prose.
#[derive(Debug, PartialEq, Clone)]
pub(crate) struct PlanError {
    pub(crate) code: &'static str,
    pub(crate) detail: String,
}
fn fail(code: &'static str, detail: impl Into<String>) -> PlanError {
    PlanError {
        code,
        detail: detail.into(),
    }
}
/// Model-chosen keys are shown only in a short, restricted form.
fn safe_key(key: &str) -> String {
    key.chars()
        .filter(|c| c.is_alphanumeric() || "-_.:".contains(*c))
        .take(40)
        .collect()
}
fn task_label(index: usize, item: &Value) -> String {
    match item["key"].as_str().map(safe_key).filter(|k| !k.is_empty()) {
        Some(key) => format!("task {index} ({key})"),
        None => format!("task {index}"),
    }
}

/// The sprint plan rules, shared by `sprint_plan_submit` and the sprint. Only
/// the plan's content fields survive.
pub(crate) fn validate_tasks(value: &Value) -> Result<Vec<Value>, PlanError> {
    check_tasks(value, true)
}
/// The same rules over a plan assembled so far: every item's own rules and
/// unique keys always; dependencies, cycles and the whole-plan size only once
/// the plan is `complete` (a later part may define a key an earlier one names).
pub(crate) fn check_tasks(value: &Value, complete: bool) -> Result<Vec<Value>, PlanError> {
    let items = value
        .as_array()
        .ok_or_else(|| fail("tasks_required", "tasks: missing or not a list"))?;
    if items.is_empty() {
        return Err(fail("tasks_required", "tasks: the list is empty"));
    }
    let mut keys = BTreeSet::new();
    let mut kept = Vec::new();
    for (n, item) in items.iter().enumerate() {
        let label = task_label(n + 1, item);
        if !item.is_object() {
            return Err(fail("invalid_task", format!("{label}: not an object")));
        }
        let key = match item.get("key").and_then(Value::as_str) {
            Some(k) if !k.trim().is_empty() && k.chars().count() <= 100 => k,
            _ => {
                return Err(fail(
                    "invalid_task",
                    format!("{label}: key must be non-blank text of 1 to 100 characters"),
                ))
            }
        };
        if !keys.insert(key.to_owned()) {
            return Err(fail("invalid_task", format!("{label}: duplicate key")));
        }
        match item.get("description").and_then(Value::as_str) {
            Some(d) if d.trim().is_empty() => {
                return Err(fail(
                    "invalid_task",
                    format!("{label}: description is empty"),
                ))
            }
            Some(d) if d.chars().count() > MAX_REQUEST_CHARS => {
                return Err(fail(
                    "invalid_task",
                    format!("{label}: description is longer than 32000 characters"),
                ))
            }
            Some(_) => {}
            None => {
                return Err(fail(
                    "invalid_task",
                    format!("{label}: description is missing or not text"),
                ))
            }
        }
        let deps: Vec<Value> = match item.get("dependsOn") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::String(s)) => vec![Value::String(s.clone())],
            Some(Value::Array(a)) if a.iter().all(Value::is_string) => a.clone(),
            Some(_) => {
                return Err(fail(
                    "invalid_task",
                    format!("{label}: dependsOn must be a list of task keys"),
                ))
            }
        };
        let mut dependencies = BTreeSet::new();
        if deps.iter().any(|d| !dependencies.insert(d.as_str().unwrap())) {
            return Err(fail(
                "invalid_dependencies",
                format!("{label} repeats the same dependency"),
            ));
        }
        let title = match item.get("title") {
            None | Some(Value::Null) => None,
            Some(Value::String(t)) if t.chars().count() > 200 => {
                let mut cut: String = t.chars().take(199).collect();
                cut.push('…');
                Some(Value::String(cut))
            }
            Some(t @ Value::String(_)) => Some(t.clone()),
            Some(_) => {
                return Err(fail("invalid_task", format!("{label}: title must be text")))
            }
        };
        let complexity = match item.get("complexity") {
            None | Some(Value::Null) => None,
            Some(Value::String(c)) if ["low", "normal", "high"].contains(&c.as_str()) => {
                Some(json!(c))
            }
            Some(_) => {
                return Err(fail(
                    "invalid_task",
                    format!("{label}: complexity must be low, normal or high"),
                ))
            }
        };
        let mut clean = serde_json::Map::new();
        clean.insert("key".into(), json!(key));
        if let Some(title) = title {
            clean.insert("title".into(), title);
        }
        clean.insert("description".into(), item["description"].clone());
        clean.insert("dependsOn".into(), Value::Array(deps));
        if let Some(complexity) = complexity {
            clean.insert("complexity".into(), complexity);
        }
        for (field, valid) in [
            ("produces", cortex_core::deliverables::outputs(item).is_some()),
            ("consumes", cortex_core::deliverables::inputs(item).is_some()),
        ] {
            if !valid {
                return Err(fail("invalid_deliverable_contract", format!("{label}: invalid {field}")));
            }
            if let Some(value) = item.get(field) {
                clean.insert(field.into(), value.clone());
            }
        }
        kept.push(Value::Object(clean));
    }
    if value.to_string().len() > MAX_PLAN_BYTES || json!(kept).to_string().len() > MAX_PLAN_BYTES {
        return Err(fail(
            "plan_too_large",
            format!("the whole plan exceeds {MAX_PLAN_BYTES} UTF-8 bytes; use shorter task descriptions without dropping required scope, acceptance criteria or owner decisions, or split the work into smaller sprints"),
        ));
    }
    if !complete {
        return Ok(kept);
    }
    check_deliverables(&kept)?;
    for (n, item) in kept.iter().enumerate() {
        for d in item["dependsOn"].as_array().unwrap() {
            let d = d.as_str().unwrap();
            if !keys.contains(d) {
                return Err(fail(
                    "invalid_dependencies",
                    format!(
                        "{} depends on unknown key {}",
                        task_label(n + 1, item),
                        safe_key(d)
                    ),
                ));
            }
        }
    }
    let mut settled = BTreeSet::new();
    let mut pending: Vec<(usize, &Value)> = kept.iter().enumerate().collect();
    while !pending.is_empty() {
        let (ready, rest): (Vec<_>, Vec<_>) = pending.into_iter().partition(|(_, i)| {
            i["dependsOn"]
                .as_array()
                .unwrap()
                .iter()
                .all(|d| settled.contains(d.as_str().unwrap()))
        });
        if ready.is_empty() {
            let (n, item) = rest[0];
            return Err(fail(
                "invalid_dependencies",
                format!(
                    "{} is part of or blocked by a dependency cycle",
                    task_label(n + 1, item)
                ),
            ));
        }
        for (_, item) in ready {
            settled.insert(item["key"].as_str().unwrap().to_owned());
        }
        pending = rest;
    }
    Ok(kept)
}

pub(crate) fn check_deliverables(items: &[Value]) -> Result<(), PlanError> {
    use cortex_core::deliverables::{inputs, outputs};
    let mut declared = std::collections::HashMap::new();
    for (index, item) in items.iter().enumerate() {
        let produced = outputs(item).ok_or_else(|| fail(
            "invalid_deliverable_contract", format!("{} has invalid outputs", task_label(index + 1, item)),
        ))?;
        if let Some(key) = item["key"].as_str() {
            declared.insert(key, produced);
        }
    }
    for (index, item) in items.iter().enumerate() {
        let consumed = inputs(item).ok_or_else(|| fail(
            "invalid_deliverable_contract", format!("{} has invalid inputs", task_label(index + 1, item)),
        ))?;
        for input in consumed {
            let direct = item["dependsOn"].as_array().is_some_and(|dependencies| {
                dependencies.iter().any(|dep| dep == &input.from)
            });
            if !direct || !declared.get(input.from.as_str()).is_some_and(|outputs| outputs.contains(&input.output())) {
                return Err(fail(
                    "deliverable_not_declared",
                    format!("{} requires {} {} from {}; declare the exact output and include its producer in dependsOn",
                        task_label(index + 1, item), input.name, input.version, safe_key(&input.from)),
                ));
            }
        }
    }
    Ok(())
}
/// Owner-visible text: a stable code and the short precise reason.
fn parse_failure(error: &PlanError) -> String {
    match error.code {
        "requirements_too_large" => "The plan and original owner requirements exceed the task description limit. Shorten the sprint input without dropping required decisions.".to_owned(),
        "plan_too_large" => "The plan was too long. If this repeats, split the work into smaller sprints.".to_owned(),
        code => format!("The planning task submitted an invalid plan ({code}): {}.", error.detail),
    }
}

/// What the planning task is asked to do. The sprint description and the
/// conversation are the owner's request; the plan's shape is the advertised
/// `sprint_plan_submit` schema, never restated here.
pub(crate) fn planning_request(
    sprint: &Value,
    retry: Option<PlanningFailure>,
) -> Result<String, String> {
    let language = match sprint["settings"]["language"].as_str() {
        Some("ru") => "Russian",
        Some("en") => "English",
        _ => "the language of the description",
    };
    let mut text = format!(
        "Sprint planning (read-only). Study this project and plan the sprint described below.\n\
         - This task is read-only: read and inspect the project, run read-only discovery and, \
         when the study needs it, plan only read-only specialists such as explorer, architect, \
         planner or auditors. Change no file and implement nothing.\n\
         - Ask the owner with your question tool when an essential decision is missing, and \
         always when the description explicitly requires a question before implementation. \
         Never decide on the owner's behalf.\n\
         - Base the plan on what you found and submit it with sprint_plan_submit. For a large \
         plan, submit small ordered parts as you work (part 1, 2, and so on). Use final:false \
         on each intermediate part and final:true on the last. Follow the tool's advertised \
         limits. Keep each item compact while \
         preserving scope, dependencies, owner decisions and acceptance criteria. Do not echo \
         the full plan in prose.\n\
         - The runtime verifies and independently reviews the plan; after approval finish the \
         task. The owner then approves the plan and creates the tasks.\n\
         - Write in {language}."
    );
    if let Some(limit) = retry {
        text.push_str(&format!(
            "\n- Retry strategy: {} Start with small ordered sprint_plan_submit parts from \
             the outset, even if the whole plan might fit in one call. Submit the first part \
             as soon as its items are ready; do not wait to draft the whole plan. Keep each \
             item compact without dropping requirements.",
            limit.reason()
        ));
    }
    text.push_str("\n\nSprint description:\n");
    text.push_str(sprint["description"].as_str().unwrap_or(""));
    let earlier: Vec<String> = sprint["conversation"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| {
            let who = match c["kind"].as_str() {
                Some("question") => "Question",
                Some("answer") => "Owner answer",
                Some("message") => "Owner message",
                _ => return None,
            };
            c["text"].as_str().map(|t| format!("{who}: {t}"))
        })
        .collect();
    if !earlier.is_empty() {
        text.push_str("\n\nEarlier sprint conversation:\n");
        text.push_str(&earlier.join("\n"));
    }
    let length = text.chars().count();
    if length > MAX_REQUEST_CHARS {
        return Err(format!("The planning request is too long ({length} characters; limit {MAX_REQUEST_CHARS}, including instructions and saved conversation). Shorten the sprint description before retrying."));
    }
    Ok(text)
}
/// The sprint's saved files, as the draft's own attachments.
pub(super) fn planning_files(sprint: &Value) -> Vec<Value> {
    sprint["attachments"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(
            sprint["conversation"]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|c| c["attachments"].as_array().into_iter().flatten()),
        )
        .filter_map(|a| {
            Some(
                json!({"name":a["name"].as_str()?,"mimeType":a["mimeType"].as_str()?,
                "data":a["data"].as_str()?}),
            )
        })
        .collect()
}
/// A sole planned task keeps the original owner requirements as a safeguard
/// against a shortened summary; oversized combined descriptions refuse.
pub(crate) fn finalize_items(
    sprint: &Value,
    mut items: Vec<Value>,
) -> Result<Vec<Value>, String> {
    if items.len() == 1 {
        let mut context = format!("\n\nOriginal owner requirements (take precedence over this task summary; never decide an explicitly requested owner question yourself; ask it before implementation unless the owner already answered):\n{}",
            sprint["description"].as_str().unwrap_or(""));
        let follow: Vec<&str> = sprint["conversation"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|c| c["kind"] == "answer" || c["kind"] == "message")
            .filter_map(|c| c["text"].as_str())
            .collect();
        if !follow.is_empty() {
            context.push_str(&format!("\nOwner follow-up:\n{}", follow.join("\n")));
        }
        let description = format!("{}{context}", items[0]["description"].as_str().unwrap());
        if description.chars().count() > MAX_REQUEST_CHARS {
            return Err(parse_failure(&fail("requirements_too_large", "")));
        }
        items[0]["description"] = json!(description);
    }
    Ok(items)
}

/// Only exact runtime codes enter this allowlist; engine text and generic
/// cancellation/refusal codes never supply a cause.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum PlanningFailure {
    OwnerStopped,
    PartSize,
    TotalSize,
    CapacityTimeout,
    StepLimit,
    NoProgressLimit,
    #[default]
    Unknown,
}

impl PlanningFailure {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::OwnerStopped => "planning_stopped_by_owner",
            Self::PartSize => "plan_part_too_large",
            Self::TotalSize => "plan_too_large",
            Self::CapacityTimeout => "scheduler_capacity_timeout",
            Self::StepLimit => "native_progress_limit",
            Self::NoProgressLimit => "coordinator_no_progress_limit",
            Self::Unknown => "planning_task_stopped",
        }
    }

    pub(crate) fn kind(self) -> &'static str {
        match self {
            Self::OwnerStopped => "owner_stopped",
            Self::Unknown => "unknown",
            _ => "limit",
        }
    }

    pub(crate) fn reason(self) -> &'static str {
        match self {
            Self::OwnerStopped => "You stopped the planning task.",
            Self::PartSize => "The planning task ended after a plan part was rejected for exceeding the size limit.",
            Self::TotalSize => "The planning task ended after the assembled plan was rejected for exceeding the total size limit.",
            Self::CapacityTimeout => "The planning task timed out waiting for runtime capacity.",
            Self::StepLimit => "The planning task reached the runtime's step limit.",
            Self::NoProgressLimit => "The planning task reached the runtime's limit for turns without progress.",
            Self::Unknown => "The planning task ended without a known cause.",
        }
    }

    pub(crate) fn is_limit(self) -> bool {
        self.kind() == "limit"
    }
}

fn recorded_planning_failure(
    db: &rusqlite::Connection,
    task: &str,
    state: &str,
    blocker: Option<&str>,
) -> cortex_core::CoreResult<PlanningFailure> {
    use rusqlite::OptionalExtension;
    let transition: Option<(i64, Option<String>)> = db.query_row(
        "SELECT id,json_extract(payload_json,'$.blocker') FROM runtime_events
         WHERE id=(SELECT max(id) FROM runtime_events WHERE task_id=?1 AND type='TASK_STATE_CHANGED')
         AND actor='runtime' AND json_extract(payload_json,'$.state')=?2",
        (task, state),
        |r| Ok((r.get(0)?, r.get(1)?)),
    ).optional()?;
    let Some((event, recorded)) = transition.filter(|(_, code)| code.as_deref() == blocker) else {
        return Ok(PlanningFailure::Unknown);
    };
    Ok(match recorded.as_deref() {
        Some("owner_resolved_without_certification" | "user_force_finished") if state == "CANCELLED" => {
            PlanningFailure::OwnerStopped
        }
        Some("scheduler_capacity_timeout") => PlanningFailure::CapacityTimeout,
        Some("native_progress_limit") => PlanningFailure::StepLimit,
        Some("coordinator_no_progress_limit") => PlanningFailure::NoProgressLimit,
        // Generic cancellation carries no owner attribution. Only use a
        // size refusal that was not superseded in the final planning phase.
        None | Some("cancelled") => {
            let code: Option<String> = db.query_row(
                "SELECT CASE WHEN type=?3 THEN json_extract(payload_json,'$.code') END
                 FROM runtime_events
                 WHERE task_id=?1 AND id<?2 AND actor='runtime' AND type IN (
                     ?3,?4,?5,'TASK_STATE_CHANGED','NATIVE_TURN_DEGENERATE','USER_ANSWER_RECEIVED'
                 )
                 ORDER BY id DESC LIMIT 1",
                (task, event, crate::native_sprint_plan::REJECTED,
                    cortex_core::lifecycle::sprint_planning::PART,
                    cortex_core::lifecycle::sprint_planning::SUBMITTED),
                |r| r.get(0),
            ).optional()?.flatten();
            match code.as_deref() {
                Some("plan_part_too_large") => PlanningFailure::PartSize,
                Some("plan_too_large") => PlanningFailure::TotalSize,
                _ => PlanningFailure::Unknown,
            }
        }
        _ => PlanningFailure::Unknown,
    })
}

/// The runtime facts of a planning task's current execution.
#[derive(Debug, Default, Clone)]
pub(crate) struct PlanningFact {
    pub(crate) execution: Option<String>,
    pub(crate) runtime_task: Option<String>,
    pub(crate) state: Option<String>,
    pub(super) failure: PlanningFailure,
    /// The retained event that completed the current run.
    pub(crate) completion: Option<i64>,
    pub(crate) plan: Option<(i64, Value)>,
}
pub(crate) async fn planning_fact(runtime: &Runtime, task: &str) -> io::Result<PlanningFact> {
    let task = task.to_owned();
    runtime
        .host
        .executor
        .read(move |db| {
            use rusqlite::OptionalExtension;
            let row = db
                .query_row(
                    "SELECT w.execution_id,e.runtime_task_id,r.state,r.blocker FROM workspace_tasks w
                     LEFT JOIN workspace_executions e ON e.execution_id=w.execution_id
                     LEFT JOIN runtime_tasks r ON r.task_id=e.runtime_task_id WHERE w.task_id=?",
                    [&task],
                    |r| Ok((r.get(0)?, r.get::<_, Option<String>>(1)?,
                        r.get::<_, Option<String>>(2)?, r.get::<_, Option<String>>(3)?)),
                )
                .optional()?;
            let Some((execution, runtime_task, state, blocker)) = row else {
                return Ok(PlanningFact::default());
            };
            let mut fact = PlanningFact {
                execution,
                runtime_task: runtime_task.clone(),
                state: state.clone(),
                ..PlanningFact::default()
            };
            if let Some(rt) = runtime_task {
                if state.as_deref() == Some("COMPLETED") {
                    fact.completion = db.query_row(
                        "SELECT max(id) FROM runtime_events WHERE task_id=? AND actor='runtime'
                         AND type='TASK_STATE_CHANGED' AND json_extract(payload_json,'$.state')='COMPLETED'",
                        [&rt],
                        |r| r.get(0),
                    )?;
                }
                if let Some(state @ ("FAILED" | "CANCELLED")) = state.as_deref() {
                    fact.failure = recorded_planning_failure(db, &rt, state, blocker.as_deref())?;
                }
                if state.as_deref() == Some("COMPLETED") {
                    fact.plan = cortex_core::lifecycle::sprint_planning::verified_latest(db, &rt)?;
                }
            }
            Ok(fact)
        })
        .map_err(|e| crate::failure(e.code()))?
        .await
        .map_err(|_| crate::failure("native_host_closed"))?
        .map_err(|e| crate::failure(e.code()))
}
/// What the sprint does next with its planning task. Pure, so every branch is tested.
#[derive(Debug, PartialEq)]
pub(crate) enum PlanningStep {
    Wait,
    Ready {
        items: Vec<Value>,
        run: Value,
    },
    Correct {
        text: String,
        run: Value,
    },
    Cancel {
        note: String,
        code: &'static str,
        run: Value,
    },
    Continue {
        text: String,
        run: Value,
    },
    Terminal {
        failure: PlanningFailure,
        run: Value,
    },
    Replace {
        retry: Option<PlanningFailure>,
    },
    /// A finished turn without a new plan keeps the current proposal (the owner asked
    /// something and the coordinator answered).
    Keep {
        run: Value,
    },
}
pub(super) fn planning_start_reason(code: &str) -> String {
    match code {
        "maintenance_busy" => "Maintenance was running when the planning task was starting.",
        "service_stopping" | "native_host_closed" => {
            "The service was stopping when the planning task was starting."
        }
        "native_capacity_unavailable" => "Too many tasks are running to start planning now.",
        _ => "The planning task could not start. Check the selected model.",
    }
    .to_owned()
}
const CORRECTION: &str = "The sprint plan is still missing: this planning task finished without a plan recorded by sprint_plan_submit. Submit the plan with sprint_plan_submit based on what you studied (ask the owner first if an essential decision is missing), then finish the task again.";
const CONTINUE: &str = "Continue planning this sprint: submit the plan with sprint_plan_submit based on what you studied, then finish the task.";
pub(crate) fn planning_step(sprint: &Value, fact: &PlanningFact) -> PlanningStep {
    let Some(rt) = fact.runtime_task.as_deref() else {
        return PlanningStep::Wait;
    };
    let prior = &sprint["planningRun"];
    let fresh = prior["runtimeTask"] != rt;
    let handled = if fresh {
        Value::Null
    } else {
        prior["handled"].clone()
    };
    let outcome = if fresh {
        ""
    } else {
        prior["outcome"].as_str().unwrap_or("")
    };
    let corrections = if fresh {
        0
    } else {
        prior["corrections"].as_i64().unwrap_or(0)
    };
    let consumed = if fresh {
        0
    } else {
        prior["planEvent"].as_i64().unwrap_or(0)
    };
    let resumed = sprint["planning"] == "planning";
    let run = |handled: Value, outcome: &str, corrections: i64, plan: i64| {
        json!({"runtimeTask":rt,"handled":handled,"outcome":outcome,
            "corrections":corrections,"planEvent":plan})
    };
    match fact.state.as_deref() {
        Some("COMPLETED") => {
            let Some(completion) = fact.completion else {
                return PlanningStep::Wait;
            };
            let completion = json!(completion);
            if handled == completion {
                // Already answered: a plan waits for the owner or a correction for the task.
                // A cancelled run the owner resumed asks the same task to continue.
                return if outcome == "cancelled" && resumed {
                    PlanningStep::Continue {
                        text: CONTINUE.into(),
                        run: run(completion, "continued", 0, consumed),
                    }
                } else {
                    PlanningStep::Wait
                };
            }
            match &fact.plan {
                Some((event, tasks)) if *event > consumed => {
                    match validate_tasks(tasks)
                        .map_err(|e| parse_failure(&e))
                        .and_then(|items| finalize_items(sprint, items))
                    {
                        Ok(items) => PlanningStep::Ready {
                            items,
                            run: run(completion, "ready", 0, *event),
                        },
                        Err(note) => PlanningStep::Cancel {
                            note,
                            code: "plan_rejected",
                            run: run(completion, "cancelled", corrections, *event),
                        },
                    }
                }
                _ if !sprint["plan"].is_null() && sprint["plan"]["items"].is_array() => {
                    PlanningStep::Keep {
                        run: run(completion, "ready", 0, consumed),
                    }
                }
                _ if corrections < 1 => PlanningStep::Correct {
                    text: CORRECTION.into(),
                    run: run(completion, "corrected", corrections + 1, consumed),
                },
                _ => PlanningStep::Cancel {
                    note: "The planning task finished twice without a sprint plan.".into(),
                    code: "plan_missing",
                    run: run(completion, "cancelled", corrections, consumed),
                },
            }
        }
        Some("FAILED" | "CANCELLED") => {
            let stopped = json!(format!("stopped:{rt}"));
            if handled != stopped {
                PlanningStep::Terminal {
                    failure: fact.failure,
                    run: run(stopped, "cancelled", corrections, consumed),
                }
            } else if resumed {
                PlanningStep::Replace {
                    retry: fact.failure.is_limit().then_some(fact.failure),
                }
            } else {
                PlanningStep::Wait
            }
        }
        _ => PlanningStep::Wait,
    }
}

