//! Latest rerun outcomes, including source/receipt-bound literal-gap closures.
use cortex_core::{CoreError, CoreResult, Store};
use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;
use std::collections::BTreeMap;

fn pending<T>() -> CoreResult<T> {
    super::task_facts_work::degraded()
}
pub(super) fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(v) => *v,
        Value::Number(v) => v.as_f64() != Some(0.0),
        Value::String(v) => !v.is_empty(),
        Value::Array(v) => !v.is_empty(),
        Value::Object(v) => !v.is_empty(),
    }
}
pub(super) struct Attempt {
    pub id: i64,
    pub outcome: &'static str,
    pub closed: bool,
}
#[derive(Default)]
pub(super) struct Attempts(BTreeMap<(Option<String>, i64), Attempt>);
impl Attempts {
    pub fn get(&self, assignment: Option<&str>, index: usize) -> Option<&Attempt> {
        self.0.get(&(assignment.map(str::to_owned), index as i64))
    }
    pub fn read(db: &Connection, store: &Store, task: &str, revision: i64) -> CoreResult<Self> {
        let mut q = db.prepare("SELECT id,type,payload_json FROM runtime_events WHERE task_id=? AND actor='runtime' AND type IN ('WORKER_CHECK_REQUESTED','WORKER_CHECK_FINISHED') ORDER BY id LIMIT 10001")?;
        let rows = q
            .query_map([task], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if rows.len() > 10000 || rows.iter().map(|r| r.2.len()).sum::<usize>() > 8 * 1024 * 1024 {
            return pending();
        }
        let mut latest = BTreeMap::new();
        for (id, kind, raw) in rows {
            let value: Value = serde_json::from_str(&raw)?;
            let Some(old) = value["binding"]["revision"].as_i64() else {
                return pending();
            };
            if old != revision {
                continue;
            }
            let target = &value["binding"]["target"];
            let assignment = match &target["assignment"] {
                Value::String(s) => Some(s.clone()),
                Value::Null if target.get("assignment").is_some() => None,
                _ => return pending(),
            };
            let Some(index) = target["index"].as_i64() else {
                return pending();
            };
            latest.insert((assignment, index), (id, kind, value));
        }
        if latest.is_empty() {
            return Ok(Self::default());
        }
        // closures() cannot close anything without reviewed_digest(). Do not use
        // native workspace hashes in place of the retained observer's digest.
        let review: Option<Option<String>> = db.query_row(
            "SELECT result_json FROM runtime_nodes WHERE task_id=? AND revision=? AND stage='review' ORDER BY rowid DESC LIMIT 1",
            (task,revision), |r| r.get(0)).optional()?;
        let digest = if let Some(raw) = review {
            let value: Value =
                serde_json::from_str(raw.as_deref().filter(|s| !s.is_empty()).unwrap_or("{}"))?;
            if !value.is_object() {
                return pending();
            }
            value["project_digest"].clone()
        } else {
            let waiver: Option<String> = db.query_row(
                "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='REVIEW_NOT_REQUIRED' AND json_extract(payload_json,'$.revision')=? ORDER BY id DESC LIMIT 1",
                (task,revision), |r| r.get(0)).optional()?;
            match waiver {
                Some(raw) => {
                    let value: Value = serde_json::from_str(&raw)?;
                    let Some(digest) = value.get("revision_digest") else {
                        return pending();
                    };
                    digest.clone()
                }
                None => Value::Null,
            }
        };
        let owner: Option<i64> = db
            .query_row(
                "SELECT epoch FROM task_owners WHERE task_id=?",
                [task],
                |r| r.get(0),
            )
            .optional()?;
        let mut result = Self::default();
        let mut current_source = None;
        for (key, (id, kind, value)) in latest {
            let binding = &value["binding"];
            let observed = &value["result"];
            if value.get("result").is_some() && !observed.is_object() {
                return pending();
            }
            let reason = observed["blocker"].as_str().unwrap_or("");
            let unavailable = matches!(
                reason,
                "verification_command_unavailable" | "verification_executable_unavailable"
            );
            let epoch = &binding["owner_epoch"];
            if !epoch.is_null() && epoch.as_i64().is_none() {
                return pending();
            }
            let same_owner = owner.is_some() && epoch.as_i64() == owner;
            let stale = truthy(&observed["stale"]);
            if truthy(&digest)
                && same_owner
                && kind == "WORKER_CHECK_FINISHED"
                && binding["digest"] == digest
                && observed["digest"] == digest
                && !stale
                && !truthy(&observed["blocker"])
            {
                // These are necessary, not sufficient, closure conditions. Reject
                // non-passes without walking source; use only the exact legacy
                // observer before validating a potentially closing success.
                let checks = observed["checks"]
                    .as_array()
                    .ok_or(CoreError::Refused(super::task_facts_work::DEGRADED))?;
                if checks.len() == 1 {
                    let check = &checks[0];
                    if check["complete"] == true
                        && check["exit_code"].as_i64() == Some(0)
                        && check["revision_digest"] == digest
                        && !truthy(&check["failure"])
                        && !truthy(&check["truncated"])
                    {
                        let current = match &current_source {
                            Some(value) => value,
                            None => {
                                current_source.insert(super::task_facts_rerun::source(db, task)?)
                            }
                        };
                        if current.as_deref().is_some_and(|value| digest == value)
                            && super::task_facts_rerun::closed(
                                db, store, task, revision, id, &value, check,
                            )?
                        {
                            result.0.insert(
                                key,
                                Attempt {
                                    id,
                                    outcome: "passed",
                                    closed: true,
                                },
                            );
                            continue;
                        }
                    }
                }
            }
            let outcome = if !same_owner {
                // Only unchanged literal targets can carry a missing-tool
                // blocker across owners. Permissions/errors/passes never carry.
                let carry = if owner.is_some()
                    && kind == "WORKER_CHECK_FINISHED"
                    && !stale
                    && unavailable
                {
                    match super::task_facts_rerun::target_identity(
                        db,
                        task,
                        revision,
                        &binding["target"],
                    )? {
                        Some((target, _)) if target == binding["target"] => {
                            let current = match &current_source {
                                Some(value) => value,
                                None => current_source
                                    .insert(super::task_facts_rerun::source(db, task)?),
                            };
                            current
                                .as_deref()
                                .is_some_and(|value| binding["digest"] == value)
                        }
                        _ => false,
                    }
                } else {
                    false
                };
                if carry {
                    "not_run"
                } else {
                    "unverified"
                }
            } else if kind == "WORKER_CHECK_FINISHED"
                && !stale
                && (unavailable
                    || reason.starts_with("verification_execution_error:")
                        && !reason.ends_with("StoreError"))
            {
                "not_run"
            } else {
                let checks = &observed["checks"];
                if observed.get("checks").is_some() && !checks.is_array() {
                    return pending();
                }
                match checks.as_array().and_then(|a| a.first()) {
                    Some(check) if !check.is_object() => return pending(),
                    Some(check)
                        if check["failure"] == "timeout"
                            || check["exit_code"].as_i64().is_some_and(|code| code != 0)
                                && check["complete"] == true =>
                    {
                        "failed"
                    }
                    _ => "unverified",
                }
            };
            result.0.insert(
                key,
                Attempt {
                    id,
                    outcome,
                    closed: false,
                },
            );
        }
        Ok(result)
    }
}
