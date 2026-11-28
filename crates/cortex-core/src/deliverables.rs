//! Structural validation for named sprint outputs and report provenance.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

pub const MAX_ITEMS: usize = 16;
const MAX_JSON_BYTES: usize = 8192;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Markdown,
    Json,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Output {
    pub name: String,
    pub version: String,
    pub kind: Kind,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub from: String,
    pub name: String,
    pub version: String,
    pub kind: Kind,
}

impl Input {
    pub fn output(&self) -> Output {
        Output {
            name: self.name.clone(),
            version: self.version.clone(),
            kind: self.kind.clone(),
        }
    }
}

fn name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.as_bytes()[0].is_ascii_lowercase()
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_.-".contains(&b))
}

fn version(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && value.as_bytes()[0].is_ascii_digit()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.+-".contains(&b))
}

pub fn outputs(record: &Value) -> Option<Vec<Output>> {
    let values: Vec<Output> = match record.get("produces") {
        Some(value) => serde_json::from_value(value.clone()).ok()?,
        None => return Some(Vec::new()),
    };
    let mut unique = HashSet::new();
    (values.len() <= MAX_ITEMS
        && values
            .iter()
            .all(|v| name(&v.name) && version(&v.version) && unique.insert((&v.name, &v.version))))
    .then_some(values)
}

pub fn inputs(record: &Value) -> Option<Vec<Input>> {
    let values: Vec<Input> = match record.get("consumes") {
        Some(value) => serde_json::from_value(value.clone()).ok()?,
        None => return Some(Vec::new()),
    };
    let mut unique = HashSet::new();
    (values.len() <= MAX_ITEMS
        && values.iter().all(|v| {
            !v.from.trim().is_empty()
                && v.from.chars().count() <= 128
                && name(&v.name)
                && version(&v.version)
                && unique.insert((&v.from, &v.name, &v.version))
        }))
    .then_some(values)
}

/// A typed entry identifies the report itself or the complete JSON object stored
/// with it. Existing reference/version entries are descriptive provenance.
pub fn artifact(entry: &Value) -> Option<Output> {
    let mut metadata = entry.as_object()?.clone();
    let data = metadata.remove("data");
    let output: Output = serde_json::from_value(Value::Object(metadata)).ok()?;
    if !name(&output.name) || !version(&output.version) {
        return None;
    }
    match (output.kind.clone(), data) {
        (Kind::Markdown, None) => Some(output),
        (Kind::Json, Some(data))
            if data.is_object() && data.to_string().len() <= MAX_JSON_BYTES =>
        {
            Some(output)
        }
        _ => None,
    }
}

use crate::{store::documents::Publisher, CoreError, CoreResult, Store};
use rusqlite::{Connection, OptionalExtension};
use serde_json::json;

#[derive(Debug)]
pub struct Issue {
    pub code: &'static str,
    pub requirement: Option<Input>,
}

type Readiness<T> = CoreResult<Result<T, Issue>>;

fn blocked<T>(code: &'static str, requirement: Option<&Input>) -> Readiness<T> {
    Ok(Err(Issue {
        code,
        requirement: requirement.cloned(),
    }))
}

pub fn contract(db: &Connection, project: &str, task: &str) -> CoreResult<Option<Value>> {
    let raw: Option<String> = db.query_row(
        "SELECT o.options_json FROM workspace_tasks w JOIN workspace_task_options o USING(task_id)
         WHERE w.project_id=? AND w.task_id=?",
        (project, task), |r| r.get(0),
    ).optional()?;
    raw.map(|raw| {
        let mut options: Value = serde_json::from_str(&raw)?;
        Ok(options
            .as_object_mut()
            .and_then(|v| v.remove("sprintDeliverables")))
    })
    .transpose()
    .map(Option::flatten)
}

fn runtime_binding(db: &Connection, runtime: &str) -> CoreResult<Option<(String, String)>> {
    Ok(db
        .query_row(
            "SELECT w.project_id,w.task_id FROM workspace_tasks w JOIN workspace_executions e
         ON e.execution_id=w.execution_id AND e.task_id=w.task_id WHERE e.runtime_task_id=?",
            [runtime],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
}

pub fn runtime_contract(db: &Connection, runtime: &str) -> CoreResult<Option<Value>> {
    match runtime_binding(db, runtime)? {
        Some((project, task)) => contract(db, &project, &task),
        None => Ok(None),
    }
}

fn since(db: &Connection, runtime: &str) -> CoreResult<i64> {
    Ok(db.query_row(
        "SELECT coalesce(max(id),0) FROM runtime_events WHERE task_id=?
         AND type IN ('PLAN_ACCEPTED','PLAN_AMENDED','TASK_REOPENED')",
        [runtime],
        |r| r.get(0),
    )?)
}

fn resolve(
    store: &Store,
    db: &Connection,
    project: &str,
    sprint: &str,
    input: &Input,
    completing: bool,
) -> Readiness<(Value, Value)> {
    let Some(producer) = contract(db, project, &input.from)? else {
        return blocked("deliverable_not_declared", Some(input));
    };
    let required = input.output();
    if producer["sprintId"] != sprint
        || !outputs(&producer).is_some_and(|items| items.contains(&required))
    {
        return blocked("deliverable_not_declared", Some(input));
    }
    let execution: Option<(String,String,String,i64)> = db.query_row(
        "SELECT e.execution_id,e.runtime_task_id,t.state,t.revision
         FROM workspace_tasks w JOIN workspace_executions e ON e.execution_id=w.execution_id AND e.task_id=w.task_id
         JOIN runtime_tasks t ON t.task_id=e.runtime_task_id WHERE w.project_id=? AND w.task_id=?",
        (project, &input.from), |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)),
    ).optional()?;
    let Some((execution, runtime, state, revision)) = execution else {
        return blocked("deliverable_producer_unsettled", Some(input));
    };
    if if completing { !matches!(state.as_str(), "RECONCILING" | "ANSWER_READY") } else { state != "COMPLETED" } {
        return blocked("deliverable_producer_unsettled", Some(input));
    }
    let since = since(db, &runtime)?;
    let latest: Option<(i64, String, String)> = db
        .query_row(
            "SELECT id,type,payload_json FROM runtime_events WHERE task_id=? AND id>? AND type IN
         ('VERIFICATION_PASSED','VERIFICATION_FAILED','VERIFICATION_REVISED')
         ORDER BY id DESC LIMIT 1",
            (&runtime, since),
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((verification_id, status, details)) = latest else {
        return blocked("deliverable_unverified", Some(input));
    };
    let verification: Value = serde_json::from_str(&details)?;
    let review_event = verification["review_event"].as_i64().unwrap_or_default();
    let reviewed = verification["revision_digest"]
        .as_str()
        .filter(|v| v.len() == 64);
    if status != "VERIFICATION_PASSED"
        || verification["revision"] != revision
        || review_event <= since
        || review_event >= verification_id
        || reviewed.is_none()
        || !crate::lifecycle::completion::retained_verification_pass(
            db,
            &runtime,
            verification_id,
            &verification,
        )?
    {
        return blocked("deliverable_unverified", Some(input));
    }
    let completed: bool = db.query_row(
        "SELECT (?4 OR EXISTS(SELECT 1 FROM runtime_events WHERE task_id=?1 AND id>?2 AND type='TASK_STATE_CHANGED'
             AND json_extract(payload_json,'$.state')='COMPLETED'))
         AND NOT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=?1 AND id>?3
             AND type IN ('OWNER_TASK_RESOLVED','USER_FORCE_FINISHED'))",
        (&runtime, verification_id, since, completing), |r| r.get(0),
    )?;
    let independent: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_events e
         JOIN runtime_nodes n ON n.task_id=e.task_id AND n.assignment_ref=json_extract(e.payload_json,'$.assignment_ref')
         WHERE e.task_id=?1 AND e.id=?2 AND e.type='WORKER_COMPLETED' AND n.revision=?3
          AND n.stage='review' AND n.status='completed' AND n.result_json=json_extract(e.payload_json,'$.result')
          AND json_extract(n.result_json,'$.verdict')='APPROVE'
          AND json_extract(n.result_json,'$.review_independence.status')='independent'
          AND json_extract(n.result_json,'$.review_digest')=?4
          AND NOT EXISTS(SELECT 1 FROM runtime_events a
           LEFT JOIN runtime_nodes replacement ON replacement.task_id=a.task_id
            AND replacement.assignment_ref=json_extract(a.payload_json,'$.assignment_ref')
           WHERE a.task_id=?1 AND a.type='REVIEW_STARTED' AND a.id>?2
            AND coalesce(substr(replacement.node_id,1,24),'')!='_sprint_revision_review_'))",
        (&runtime,review_event,revision,reviewed), |r| r.get(0),
    )?;
    if !completed || !independent {
        return blocked("deliverable_unverified", Some(input));
    }
    let publication = match accepted_publication(store, db, &runtime, revision, since, review_event, input)? {
        Ok(publication) => publication,
        Err(issue) => return Ok(Err(issue)),
    };
    let Publication { assignment, attempt, request, event: publication, result, report, sha256, entry } = publication;
    let mut pages = db.prepare(
        "SELECT json_extract(p.payload_json,'$.cursor'), json_extract(p.payload_json,'$.next_cursor')
         FROM runtime_events p JOIN runtime_events r ON r.task_id=p.task_id
         JOIN trusted_channels c ON c.task_id=p.task_id AND c.channel_id=json_extract(p.payload_json,'$.channel_id')
         WHERE p.task_id=?1 AND p.type='REPORT_PAGE_READ' AND p.id<?2 AND p.id>?3 AND r.id=?2
          AND c.assignment_id=json_extract(r.payload_json,'$.assignment_ref')
          AND c.attempt_id=json_extract(r.payload_json,'$.attempt_id')
          AND json_extract(p.payload_json,'$.report_id')=?4
          AND json_extract(p.payload_json,'$.sha256')=?5
          AND json_type(p.payload_json,'$.next_cursor') IN ('null','text')
         ORDER BY p.id LIMIT 2048",
    )?;
    let mut reads = pages.query((&runtime, review_event, result, &report, &sha256))?;
    let mut reachable = HashSet::new();
    let mut read_by_reviewer = false;
    while let Some(read) = reads.next()? {
        let cursor: Option<String> = read.get(0)?;
        if cursor
            .as_ref()
            .is_some_and(|cursor| !reachable.contains(cursor))
        {
            continue;
        }
        let next: Option<String> = read.get(1)?;
        if let Some(next) = next {
            reachable.insert(next);
        } else {
            read_by_reviewer = true;
            break;
        }
    }
    if !read_by_reviewer {
        return blocked("deliverable_unreviewed", Some(input));
    }
    Ok(Ok((
        json!({"requirement":input,"execution_id":execution,"runtime_id":runtime,"revision":revision,
        "report_id":report,"sha256":sha256,"verification_event":verification_id,"review_event":review_event,
        "publication":{"assignment":assignment,"attempt":attempt,"request":request,
            "event":publication,"accepted_result_event":result},
        "artifact_sha256":crate::digest(entry.to_string().as_bytes())}),
        entry,
    )))
}

struct Publication {
    assignment: String,
    attempt: String,
    request: String,
    event: i64,
    result: i64,
    report: String,
    sha256: String,
    entry: Value,
}

/// The exact accepted result from this generation, linked to one authenticated
/// immutable publication. Additional unaccepted editions never count.
fn accepted_publication(
    store: &Store,
    db: &Connection,
    runtime: &str,
    revision: i64,
    since: i64,
    before: i64,
    input: &Input,
) -> Readiness<Publication> {
    let mut stmt = db.prepare(
        "SELECT n.assignment_ref, r.id,e.digest,a.value,p.artifacts
         FROM runtime_nodes n
         JOIN reports r ON r.task_id=n.task_id AND r.kind='report' AND r.id=json_extract(n.result_json,'$.report_ref')
         JOIN editions e ON e.report_id=r.id AND e.digest=json_extract(n.result_json,'$.report_digest')
         JOIN report_provenance p ON p.edition_sequence=e.sequence
         JOIN json_each(p.artifacts) a
         WHERE n.task_id=?1 AND n.revision=?2 AND n.stage='execution' AND n.status='completed'
          AND json_extract(a.value,'$.name')=?3 AND json_extract(a.value,'$.version')=?4
         LIMIT 2",
    )?;
    let candidates = stmt
        .query_map((runtime, revision, &input.name, &input.version), |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    if candidates.len() > 1 {
        return blocked("deliverable_ambiguous", Some(input));
    }
    let Some((assignment, report, sha256, entry, artifacts)) = candidates.into_iter().next() else {
        return blocked("deliverable_missing", Some(input));
    };
    let entry: Value = serde_json::from_str(&entry)?;
    if artifact(&entry) != Some(input.output()) {
        return blocked("deliverable_invalid_evidence", Some(input));
    }
    // Require the committed document response from the execution channel of the
    // exact accepted result. Generic author labels and a copied report cannot bind.
    let mut writers = db.prepare(
        "SELECT c.attempt_id,q.request_key,w.id,done.id FROM runtime_requests q
         JOIN trusted_channels c ON c.channel_id=q.actor AND c.task_id=q.task_id
         JOIN delegation_provisioning d ON d.task_id=c.task_id AND d.attempt_id=c.attempt_id
         JOIN runtime_events w ON w.task_id=q.task_id AND w.type='NATIVE_DOCUMENT_TOOL'
         JOIN runtime_events done ON done.task_id=q.task_id AND done.type='WORKER_COMPLETED'
         WHERE q.task_id=?1 AND q.operation='write_report' AND d.idempotency_key=?2 AND c.role='worker'
          AND json_extract(q.result_json,'$.complete')=1
          AND json_extract(q.result_json,'$.result.report_id')=?3
          AND json_extract(q.result_json,'$.result.sha256')=?4
          AND json(json_extract(q.result_json,'$.result.artifacts'))=json(?5)
          AND json_extract(w.payload_json,'$.name')='write_report'
          AND json_extract(w.payload_json,'$.channel_id')=q.actor
          AND json_extract(w.payload_json,'$.request_key')=q.request_key
          AND w.id>?6 AND w.id<?7
            AND done.id>w.id AND done.id<?7
            AND json_extract(done.payload_json,'$.assignment_ref')=?2
            AND json_extract(done.payload_json,'$.attempt_id')=c.attempt_id
            AND json_extract(done.payload_json,'$.result.report_ref')=?3
            AND json_extract(done.payload_json,'$.result.report_digest')=?4
         LIMIT 2",
    )?;
    let publications = writers
        .query_map(
            (
                runtime,
                &assignment,
                &report,
                &sha256,
                &artifacts,
                since,
                before,
            ),
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;
    if publications.len() > 1 {
        return blocked("deliverable_ambiguous", Some(input));
    }
    let Some((attempt, request, publication, result)) = publications.into_iter().next() else {
        return blocked("deliverable_stale_evidence", Some(input));
    };
    if Publisher::new(store)
        .verify_immutable_report_on(db, runtime, &report, &sha256)
        .is_err()
    {
        return blocked("deliverable_invalid_evidence", Some(input));
    }
    Ok(Ok(Publication { assignment, attempt, request, event: publication, result, report, sha256, entry }))
}

/// Review and verification require every declared output in the current
/// generation. Before completion, also check the consumer's full proof while
/// the producer is still in its last state transition.
pub fn producer_issue(
    store: &Store,
    db: &Connection,
    runtime: &str,
    completing: bool,
) -> CoreResult<Option<Value>> {
    let Some((project, draft)) = runtime_binding(db, runtime)? else { return Ok(None) };
    let Some(contract) = contract(db, &project, &draft)? else { return Ok(None) };
    let (Some(sprint), Some(outputs)) = (contract["sprintId"].as_str(), outputs(&contract)) else {
        return Ok(Some(json!({"code":"invalid_deliverable_contract"})));
    };
    if outputs.is_empty() {
        return Ok(None);
    }
    let revision = crate::lifecycle::domain::task_tx(db, runtime)?.1;
    let since = since(db, runtime)?;
    let mut issues = Vec::new();
    for output in outputs {
        let input = Input { from: draft.clone(), name: output.name, version: output.version, kind: output.kind };
        let readiness = if completing {
            resolve(store, db, &project, sprint, &input, true)?.map(|_| ())
        } else {
            accepted_publication(store, db, runtime, revision, since, i64::MAX, &input)?.map(|_| ())
        };
        if let Err(issue) = readiness {
            issues.push(json!({"code":issue.code,"output":input.output()}));
        }
    }
    Ok((!issues.is_empty()).then(|| json!({"revision":revision,"issues":issues})))
}

pub fn ready(store: &Store, db: &Connection, project: &str, task: &str) -> Readiness<Vec<Value>> {
    let Some(contract) = contract(db, project, task)? else {
        return Ok(Ok(Vec::new()));
    };
    let (Some(sprint), Some(inputs)) = (contract["sprintId"].as_str(), inputs(&contract)) else {
        return blocked("invalid_deliverable_contract", None);
    };
    let mut bindings = Vec::with_capacity(inputs.len());
    for input in inputs {
        match resolve(store, db, project, sprint, &input, false)? {
            Ok((binding, _)) => bindings.push(binding),
            Err(issue) => return Ok(Err(issue)),
        }
    }
    Ok(Ok(bindings))
}

fn last_binding(db: &Connection, runtime: &str) -> CoreResult<Value> {
    let raw: Option<String> = db
        .query_row(
            "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='SPRINT_INPUTS_BOUND'
         ORDER BY id DESC LIMIT 1",
            [runtime],
            |r| r.get(0),
        )
        .optional()?;
    serde_json::from_str(&raw.ok_or(CoreError::Refused("deliverable_binding_missing"))?)
        .map_err(Into::into)
}

fn token(input: &Value) -> String {
    crate::digest(input.to_string().as_bytes())
}

fn prospective_read(
    db: &Connection,
    runtime: &str,
    revision: i64,
    input: &Value,
) -> CoreResult<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_events p JOIN runtime_events r ON r.task_id=p.task_id
         WHERE p.task_id=?1 AND p.type='PLAN_ACCEPTED' AND json_extract(p.payload_json,'$.revision')=?2
          AND r.type='SPRINT_INPUT_PROSPECTIVE_READ' AND r.id<p.id AND r.id>?3
          AND r.id>coalesce((SELECT max(b.id) FROM runtime_events b WHERE b.task_id=p.task_id
           AND b.type IN ('TASK_REOPENED','PLAN_ACCEPTED','PLAN_AMENDED') AND b.id<p.id),0)
          AND json_extract(r.payload_json,'$.binding_token')=?4
          AND json_extract(r.payload_json,'$.channel_id')=json_extract(p.payload_json,'$.sprint_consumer_channel'))",
        (runtime,revision,input["verification_event"].as_i64(),token(input)), |r|r.get(0),
    )?)
}

pub fn require_available(store: &Store, db: &Connection, runtime: &str) -> CoreResult<()> {
    if let Some((project, task)) = runtime_binding(db, runtime)? {
        ready(store, db, &project, &task)?.map_err(|issue| CoreError::Refused(issue.code))?;
    }
    Ok(())
}

/// Recheck at a driver boundary. A newer consumer revision may bind repaired
/// inputs; a live revision cannot silently switch to a different producer result.
pub fn continue_tx(store: &Store, db: &rusqlite::Transaction<'_>, runtime: &str) -> CoreResult<()> {
    let Some((project, task)) = runtime_binding(db, runtime)? else {
        return Ok(());
    };
    let bindings =
        ready(store, db, &project, &task)?.map_err(|issue| CoreError::Refused(issue.code))?;
    if bindings.is_empty() {
        return Ok(());
    }
    let (state, revision) = crate::lifecycle::domain::task_tx(db, runtime)?;
    let current = last_binding(db, runtime)?;
    let new_plan = current["revision"]
        .as_i64()
        .is_some_and(|old| revision > old)
        && state == crate::TaskState::PlanReady;
    let unchanged = current["inputs"] == json!(bindings);
    if !new_plan && unchanged {
        return Ok(());
    }
    if !unchanged {
        let plan = since(db, runtime)?;
        if !new_plan
            || bindings.iter().any(|input| {
                input["verification_event"]
                    .as_i64()
                    .is_none_or(|verified| verified >= plan)
            })
        {
            return Err(CoreError::Refused("deliverable_binding_changed"));
        }
        let prior = current["inputs"]
            .as_array()
            .ok_or(CoreError::Refused("deliverable_binding_missing"))?;
        for input in bindings.iter().filter(|input| !prior.contains(input)) {
            if !prospective_read(db, runtime, revision, input)? {
                return Err(CoreError::Refused("deliverable_prospective_read_required"));
            }
        }
    }
    crate::lifecycle::event_tx(
        db,
        runtime,
        "SPRINT_INPUTS_BOUND",
        json!({"revision":revision,"inputs":bindings}),
    )?;
    Ok(())
}

pub fn read_for_consumer(
    store: &Store,
    db: &Connection,
    runtime: &str,
    producer: &str,
    name: &str,
    version: &str,
) -> CoreResult<Value> {
    let refused = CoreError::Refused("deliverable_not_required");
    let (project, task) = runtime_binding(db, runtime)?.ok_or(refused)?;
    let contract =
        contract(db, &project, &task)?.ok_or(CoreError::Refused("deliverable_not_required"))?;
    let sprint = contract["sprintId"]
        .as_str()
        .ok_or(CoreError::Refused("invalid_deliverable_contract"))?;
    let input = inputs(&contract)
        .ok_or(CoreError::Refused("invalid_deliverable_contract"))?
        .into_iter()
        .find(|i| i.from == producer && i.name == name && i.version == version)
        .ok_or(CoreError::Refused("deliverable_not_required"))?;
    let (mut binding, entry) = resolve(store, db, &project, sprint, &input, false)?
        .map_err(|issue| CoreError::Refused(issue.code))?;
    let retained = last_binding(db, runtime)?;
    let is_bound = retained["inputs"]
        .as_array()
        .is_some_and(|inputs| inputs.contains(&binding));
    if !is_bound {
        let state: String = db.query_row(
            "SELECT state FROM runtime_tasks WHERE task_id=?",
            [runtime],
            |r| r.get(0),
        )?;
        if state != "PLANNING" {
            return Err(CoreError::Refused("deliverable_binding_changed"));
        }
    }
    let binding_token = token(&binding);
    let public = binding.as_object_mut().expect("binding object");
    public.remove("runtime_id");
    public.remove("publication");
    Ok(json!({"deliverable":entry,"binding":binding,
        "binding_token":binding_token,
        "binding_status":if is_bound {"bound"} else {"prospective_replan"}}))
}
