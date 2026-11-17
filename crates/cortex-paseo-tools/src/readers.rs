//! The narrow store readers: `task-id`, `worker-results`, `running-commands`
//! and `pipeline`, ported from the Python scripts of
//! `integrations/paseo/server/{worker-history,worker-results,command-stop,pipeline}.ts`
//! with the same argv, queries and output bytes. Python's uncaught exceptions
//! (including `sqlite3.Error`) exit 1 with no output, as the scripts do.
use crate::common::{clip_scrubbed, dict, printed, result_checks, sum_tokens};
use crate::py::{self, Py};
use crate::scrub::scrub;
use crate::store::{self, placeholders, query, query_one, Fail, Sv};
use regex::Regex;
use rusqlite::Connection;
use std::sync::OnceLock;

fn finish(outcome: Result<String, Fail>) -> (String, i32) {
    match outcome {
        Ok(text) => (text, 0),
        Err(_) => (String::new(), 1),
    }
}

fn has(tables: &[String], name: &str) -> bool {
    tables.iter().any(|t| t == name)
}

/// `task-id <root> <session>`: the task bound to one ACP host session.
pub fn task_id(argv: &[String]) -> (String, i32) {
    let (Some(root), Some(session)) = (argv.first(), argv.get(1)) else {
        return (String::new(), 1);
    };
    finish(store::read(root, |db| {
        let row = query_one(
            db,
            "select task_id from acp_host_sessions where session_id=? and project_root=?",
            &[Sv::text(session.clone()), Sv::text(root.clone())],
        )?;
        let value = match &row {
            Some(row) if row.at(0)?.truthy() => row.at(0)?.to_str(),
            _ => String::new(),
        };
        Ok(format!("{value}\n"))
    }))
}

/// `worker-results <root> <task> <refs-json>`: the terminal `worker_result`
/// rows for the exact requested assignment refs.
pub fn worker_results(argv: &[String]) -> (String, i32) {
    let (Some(root), Some(task), Some(refs_json)) = (argv.first(), argv.get(1), argv.get(2)) else {
        return (String::new(), 1);
    };
    finish((|| {
        let parsed = py::loads(refs_json).ok_or(Fail::Crash)?;
        let refs = parsed.iter_strings()?;
        store::read(root, |db| {
            let mut results = Py::dict();
            for r in &refs {
                if !results.contains(r) {
                    results.set(r, Py::Null);
                }
            }
            let tables = store::tables(db)?;
            if !refs.is_empty()
                && has(&tables, "delegation_provisioning")
                && has(&tables, "task_events")
            {
                let mut params = vec![Sv::text(task.clone())];
                params.extend(refs.iter().map(|r| Sv::text(r.clone())));
                for row in query(
                    db,
                    &format!(
                        "select p.idempotency_key ref, e.payload_json from task_events e
            join delegation_provisioning p on p.task_id=e.task_id and p.attempt_id=e.assignment_ref
            where e.task_id=? and e.event_type='worker_result' and p.idempotency_key in ({})
            order by e.id",
                        placeholders(refs.len())
                    ),
                    &params,
                )? {
                    let Some(reference) = row.get("ref")?.as_text().map(str::to_owned) else {
                        continue;
                    };
                    if !results.contains(&reference) {
                        continue;
                    }
                    let payload = row
                        .get("payload_json")?
                        .loads()
                        .unwrap_or_else(|_| Py::dict());
                    let payload = if payload.is_dict() {
                        payload
                    } else {
                        Py::dict()
                    };
                    let checks = result_checks(payload.get("checks"), false)?;
                    results.set(
                        &reference,
                        dict(vec![
                            ("summary", clip_scrubbed(payload.get("summary"), 600)),
                            ("checks", Py::List(checks)),
                        ]),
                    );
                }
            }
            Ok(printed(&dict(vec![
                ("available", Py::Bool(true)),
                ("results", results),
            ]))?)
        })
    })())
}

fn gateway_ref() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^gateway:[0-9a-f]{32}$").expect("gateway pattern"))
}

/// `shlex.quote(part)`.
fn shell_quote(part: &str) -> String {
    if part.is_empty() {
        return "''".into();
    }
    let safe = part
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&b));
    if safe {
        part.to_owned()
    } else {
        format!("'{}'", part.replace('\'', "'\"'\"'"))
    }
}

/// `datetime.now(timezone.utc).isoformat(timespec='microseconds')`.
fn utc_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let seconds = now.as_secs() as i64;
    let (days, rest) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:06}+00:00",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60,
        now.subsec_micros()
    )
}

/// `running-commands <root> <task> <assignment-ref>`: a read-only mirror of
/// `command_control._live_commands`.
pub fn running_commands(argv: &[String]) -> (String, i32) {
    let (Some(root), Some(task), Some(assignment)) = (argv.first(), argv.get(1), argv.get(2))
    else {
        return (String::new(), 1);
    };
    let assignment_ref = (!assignment.is_empty()).then(|| assignment.clone());
    finish(store::read(root, |db| {
        read_running(db, task, assignment_ref.as_deref())
    }))
}

fn read_running(db: &Connection, task: &str, assignment_ref: Option<&str>) -> Result<String, Fail> {
    let tables = store::tables(db)?;
    if ![
        "runtime_events",
        "trusted_channels",
        "task_owners",
        "runtime_tasks",
    ]
    .iter()
    .all(|t| has(&tables, t))
    {
        return Ok("{\"available\": false, \"commands\": [], \"message\": \
\"Running commands are unavailable in this workspace store.\"}\n"
            .into());
    }
    let provisioning = has(&tables, "delegation_provisioning")
        && store::columns(db, "delegation_provisioning")?
            .iter()
            .any(|c| c == "channel_id");
    let assignment = if provisioning {
        "p.idempotency_key"
    } else {
        "null"
    };
    let join = if provisioning {
        "LEFT JOIN delegation_provisioning p ON p.channel_id=c.channel_id AND p.task_id=c.task_id "
    } else {
        ""
    };
    let rows = query(
        db,
        &format!(
            "SELECT e.payload_json,e.created_at,c.attempt_id,{assignment} AS assignment_ref FROM runtime_events e \
             JOIN trusted_channels c ON c.channel_id=e.actor AND c.task_id=e.task_id \
             JOIN task_owners o ON o.task_id=c.task_id AND o.epoch=c.owner_epoch \
             JOIN runtime_tasks t ON t.task_id=e.task_id {join}\
             WHERE e.task_id=? AND e.type='COMMAND_PROCESS_STARTED' \
             AND c.role='worker' AND c.revoked_at IS NULL \
             AND (c.expires_at IS NULL OR c.expires_at>?) \
             AND t.state NOT IN ('COMPLETED','FAILED','CANCELLED') \
             AND NOT EXISTS (SELECT 1 FROM runtime_events x WHERE x.task_id=e.task_id \
             AND x.type='COMMAND_PROCESS_EXITED' AND json_extract(x.payload_json,'$.tool_call_ref')=\
             json_extract(e.payload_json,'$.tool_call_ref')) ORDER BY e.id"
        ),
        &[Sv::text(task), Sv::text(utc_now())],
    )?;
    let mut commands = Vec::new();
    for row in &rows {
        let Ok(data) = row.get("payload_json")?.loads() else {
            continue;
        };
        let reference = if data.is_dict() {
            data.get("tool_call_ref")
        } else {
            &py::NONE
        };
        let Some(reference) = reference.as_str().filter(|r| gateway_ref().is_match(r)) else {
            continue;
        };
        let row_assignment = row.get("assignment_ref")?;
        if let Some(wanted) = assignment_ref {
            if !row_assignment.eq_str(wanted) {
                continue;
            }
        }
        let summary = match data.get("argv") {
            Py::List(parts) => py::ellipsis(
                &scrub(
                    &parts
                        .iter()
                        .map(|p| shell_quote(&py::to_str(p)))
                        .collect::<Vec<_>>()
                        .join(" "),
                ),
                500,
            ),
            _ => String::new(),
        };
        let stop = query_one(
            db,
            "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='COMMAND_STOP_REQUESTED' \
             AND json_extract(payload_json,'$.tool_call_ref')=? ORDER BY id LIMIT 1",
            &[Sv::text(task), Sv::text(reference)],
        )?;
        let mut stop_request = Py::Null;
        if let Some(stop) = stop {
            let requested = stop
                .get("payload_json")?
                .loads()
                .unwrap_or_else(|_| Py::dict());
            if let Some(reason) = requested
                .is_dict()
                .then(|| requested.get("reason").as_str())
                .flatten()
            {
                let origin = requested.get("origin");
                stop_request = dict(vec![
                    ("reason", Py::str(py::ellipsis(reason, 2000))),
                    (
                        "origin",
                        if origin.as_str().is_some() {
                            origin.clone()
                        } else {
                            Py::Null
                        },
                    ),
                ]);
            }
        }
        let operation = data.get("operation");
        commands.push(dict(vec![
            ("toolCallRef", Py::str(reference)),
            (
                "operation",
                Py::str(if operation.truthy() {
                    py::to_str(operation)
                } else {
                    "command_run".into()
                }),
            ),
            ("assignmentRef", row_assignment.to_py()?),
            ("attemptId", row.get("attempt_id")?.to_py()?),
            ("startedAt", row.get("created_at")?.to_py()?),
            ("argvSummary", Py::str(summary)),
            ("stopRequest", stop_request),
        ]));
    }
    commands.truncate(50);
    Ok(printed(&dict(vec![
        ("available", Py::Bool(true)),
        ("commands", Py::List(commands)),
    ]))?)
}

/// `load(text)`: a JSON object, or `{}`.
fn load(value: &Sv) -> Py {
    match value.loads() {
        Ok(v) if v.is_dict() => v,
        _ => Py::dict(),
    }
}

/// `pipeline <root> <task>`: one task's durable lifecycle projection.
pub fn pipeline(argv: &[String]) -> (String, i32) {
    let (Some(root), Some(task)) = (argv.first(), argv.get(1)) else {
        return (String::new(), 1);
    };
    finish(store::read(root, |db| read_pipeline(db, root, task)))
}

fn read_pipeline(db: &Connection, root: &str, task_id: &str) -> Result<String, Fail> {
    let tables = store::tables(db)?;
    let task = query_one(
        db,
        "select id, created_at from tasks where id=? and project_root=?",
        &[Sv::text(task_id), Sv::text(root)],
    )?;
    let id = Sv::text(task_id);
    let mut out = dict(vec![
        ("available", Py::Bool(task.is_some())),
        (
            "tokens",
            dict(vec![("total", Py::Null), ("coordinator", Py::Null)]),
        ),
        ("task", Py::Null),
        ("messages", Py::List(Vec::new())),
        ("requirements", Py::List(Vec::new())),
        ("delegations", Py::dict()),
        ("plan", Py::Null),
    ]);
    let Some(task) = task else {
        return Ok(printed(&out)?);
    };
    let mut task_out = dict(vec![
        ("id", task.get("id")?.to_py()?),
        ("createdAt", task.get("created_at")?.to_py()?),
        ("state", Py::Null),
        ("waitReason", Py::Null),
        ("waitDetail", Py::Null),
    ]);
    if has(&tables, "task_orchestration") {
        if let Some(row) = query_one(
            db,
            "select state, wait_reason, wait_detail_json from task_orchestration where task_id=?",
            &[id.clone()],
        )? {
            task_out.set("state", row.get("state")?.to_py()?);
            task_out.set("waitReason", row.get("wait_reason")?.to_py()?);
            task_out.set(
                "waitDetail",
                clip_scrubbed(load(row.get("wait_detail_json")?).get("explanation"), 1200),
            );
        }
    }
    out.set("task", task_out);
    if has(&tables, "work_requirements") {
        let mut requirements = Vec::new();
        for row in query(
            db,
            "select r.requirement_id, v.statement, v.criteria_json from work_requirements r
                join work_requirement_revisions v on v.requirement_id=r.requirement_id and v.revision=r.current_revision
                where r.task_id=? order by r.created_at limit 50",
            &[id.clone()],
        )? {
            let criteria: Vec<Py> = match row.get("criteria_json")?.loads() {
                Ok(parsed) => match parsed.iterate() {
                    Ok(items) => items
                        .iter()
                        .filter(|c| c.as_str().is_some())
                        .map(|c| clip_scrubbed(c, 400))
                        .collect(),
                    Err(_) => Vec::new(),
                },
                Err(_) => Vec::new(),
            };
            let statement = clip_scrubbed(&row.get("statement")?.to_py()?, 1200);
            requirements.push(dict(vec![
                ("id", row.get("requirement_id")?.to_py()?),
                ("statement", if statement.truthy() { statement } else { Py::str("") }),
                ("criteria", Py::List(criteria.into_iter().take(30).collect())),
            ]));
        }
        out.set("requirements", Py::List(requirements));
    }
    if has(&tables, "task_events") {
        let mut messages = Vec::new();
        for row in query(
            db,
            "select payload_json, created_at from task_events where task_id=? and event_type='user_message' order by id",
            &[id.clone()],
        )? {
            let text = clip_scrubbed(load(row.get("payload_json")?).get("content"), 2000);
            if text.truthy() {
                messages.push(dict(vec![("text", text), ("createdAt", row.get("created_at")?.to_py()?)]));
            }
        }
        let mut kept: Vec<Py> = messages.iter().take(1).cloned().collect();
        let rest = messages.len().saturating_sub(1);
        kept.extend(messages.into_iter().skip(1).skip(rest.saturating_sub(19)));
        out.set("messages", Py::List(kept));
        let mut delegations = Py::dict();
        for row in query(
            db,
            "select id, assignment_ref from task_events where task_id=? and event_type='delegation_requested' \
             and assignment_ref is not null order by id",
            &[id.clone()],
        )? {
            let reference = row.get("assignment_ref")?.as_text().ok_or(Fail::Crash)?.to_owned();
            if !delegations.contains(&reference) {
                delegations.set(
                    &reference,
                    dict(vec![("order", row.get("id")?.to_py()?), ("result", Py::Null), ("decision", Py::Null)]),
                );
            }
        }
        if has(&tables, "delegation_provisioning") {
            for row in query(
                db,
                "select p.idempotency_key ref, e.payload_json from task_events e
                    join delegation_provisioning p on p.task_id=e.task_id and p.attempt_id=e.assignment_ref
                    where e.task_id=? and e.event_type='worker_result' order by e.id",
                &[id.clone()],
            )? {
                let Some(reference) = row.get("ref")?.as_text().map(str::to_owned) else {
                    continue;
                };
                if !delegations.contains(&reference) {
                    continue;
                }
                let payload = load(row.get("payload_json")?);
                let checks = result_checks(payload.get("checks"), true)?;
                let result = dict(vec![
                    ("summary", clip_scrubbed(payload.get("summary"), 600)),
                    ("checks", Py::List(checks)),
                ]);
                if let Py::Dict(entries) = &mut delegations {
                    if let Some((_, entry)) = entries.iter_mut().find(|(k, _)| *k == reference) {
                        entry.set("result", result);
                    }
                }
            }
        }
        for row in query(
            db,
            "select assignment_ref, payload_json from task_events where task_id=? and event_type='coordinator_decision' \
             and assignment_ref is not null order by id",
            &[id.clone()],
        )? {
            let Some(reference) = row.get("assignment_ref")?.as_text().map(str::to_owned) else {
                continue;
            };
            if !delegations.contains(&reference) {
                continue;
            }
            let payload = load(row.get("payload_json")?);
            if let Some(decision) = payload.get("decision").as_str() {
                let value = dict(vec![
                    ("decision", Py::str(decision)),
                    ("rationale", clip_scrubbed(payload.get("rationale"), 600)),
                ]);
                if let Py::Dict(entries) = &mut delegations {
                    if let Some((_, entry)) = entries.iter_mut().find(|(k, _)| *k == reference) {
                        entry.set("decision", value);
                    }
                }
            }
        }
        out.set("delegations", delegations);
    }
    if has(&tables, "runtime_tasks") && has(&tables, "runtime_nodes") {
        if let Some(runtime) = query_one(
            db,
            "select state, revision, blocker from runtime_tasks where task_id=?",
            &[id.clone()],
        )? {
            let rows = query(
                db,
                "select node_id, role, objective, stage, status, assignment_ref, dependencies_json, result_json
                from runtime_nodes where task_id=? and revision=? order by rowid",
                &[id.clone(), runtime.get("revision")?.clone()],
            )?;
            let mut finished: Vec<String> = Vec::new();
            for r in &rows {
                if r.get("status")?.eq_str("completed") {
                    finished.push(r.get("node_id")?.to_str());
                }
            }
            let mut nodes = Vec::new();
            for r in &rows {
                let dependencies: Vec<String> = match r.get("dependencies_json")?.loads() {
                    Ok(parsed) => parsed.iter_strings().unwrap_or_default(),
                    Err(_) => Vec::new(),
                };
                let waiting = if r.get("status")?.eq_str("pending") {
                    let unmet: Vec<&str> = dependencies
                        .iter()
                        .filter(|d| !finished.contains(d))
                        .map(String::as_str)
                        .collect();
                    Py::str(if unmet.is_empty() {
                        "Waiting to start".to_owned()
                    } else {
                        format!("Waiting for {}", unmet.join(", "))
                    })
                } else {
                    Py::Null
                };
                let result = load(r.get("result_json")?);
                let checks = result_checks(result.get("checks"), true)?;
                let node_result = if r.get("result_json")?.is_null() {
                    Py::Null
                } else {
                    dict(vec![
                        ("summary", clip_scrubbed(result.get("summary"), 600)),
                        ("checks", Py::List(checks)),
                    ])
                };
                let or_default = |value: &Sv, default: &str| -> Result<Py, Fail> {
                    Ok(if value.truthy() {
                        value.to_py()?
                    } else {
                        Py::str(default)
                    })
                };
                let objective = clip_scrubbed(&r.get("objective")?.to_py()?, 2000);
                nodes.push(dict(vec![
                    ("id", r.get("node_id")?.to_py()?),
                    ("role", or_default(r.get("role")?, "")?),
                    (
                        "objective",
                        if objective.truthy() {
                            objective
                        } else {
                            Py::str("")
                        },
                    ),
                    ("stage", or_default(r.get("stage")?, "")?),
                    ("status", or_default(r.get("status")?, "pending")?),
                    ("assignmentRef", r.get("assignment_ref")?.to_py()?),
                    ("waiting", waiting),
                    ("result", node_result),
                ]));
            }
            out.set(
                "plan",
                dict(vec![
                    ("state", runtime.get("state")?.to_py()?),
                    (
                        "blocker",
                        clip_scrubbed(&runtime.get("blocker")?.to_py()?, 600),
                    ),
                    ("nodes", Py::List(nodes)),
                ]),
            );
        }
    }
    if has(&tables, "usage_records") {
        let rows = query(
            db,
            "select attempt_id, basis, tokens_json, stop_reason, failed from usage_records where task_id=?",
            &[id.clone()],
        )?;
        let mut coordinator = Vec::new();
        for row in &rows {
            let attempt = row.get("attempt_id")?;
            let text = if attempt.truthy() {
                attempt.as_text().ok_or(Fail::Crash)?
            } else {
                ""
            };
            if text.starts_with("co_") {
                coordinator.push(row.clone());
            }
        }
        out.set(
            "tokens",
            dict(vec![
                ("total", sum_tokens(&rows)?.unwrap_or(Py::Null)),
                ("coordinator", sum_tokens(&coordinator)?.unwrap_or(Py::Null)),
            ]),
        );
    }
    Ok(printed(&out)?)
}
