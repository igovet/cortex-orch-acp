//! Bounded, typed memory derived from authenticated project observations.
use crate::{lifecycle::review, CoreResult};
use rusqlite::{Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const OBSERVATION_LIMIT: i64 = 32;

#[derive(Serialize, Deserialize)]
pub(crate) struct FailedCheck {
    event_id: i64,
    tool: String,
    action: Option<String>,
    exit_code: i64,
}

fn failed_check(
    db: &Transaction<'_>,
    task: &str,
    result: &Value,
) -> CoreResult<Option<FailedCheck>> {
    let Some(reference) = result["checks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|check| {
            check["outcome"] == "failed"
                && check["evidence_strength"] == "verified"
                && !crate::lifecycle::worker_checks::resolved(check)
        })
        .find_map(|check| check["tool_call_ref"].as_str())
    else {
        return Ok(None);
    };
    // The result validator already matched the check to this attempt's command
    // receipts. Bind the summary to one exact finished event; a duplicate or
    // incomplete receipt gives no claim about what the process did.
    let rows = db
        .prepare(
            "SELECT id,json_extract(payload_json,'$.receipt.argv[0]'),
                    json_extract(payload_json,'$.receipt.argv[1]'),
                    json_extract(payload_json,'$.receipt.observation.exit_code'),
                    json_extract(payload_json,'$.receipt.observation.complete')
             FROM runtime_events WHERE task_id=? AND type='NATIVE_COMMAND_FINISHED'
             AND json_extract(payload_json,'$.receipt.tool_call_ref')=? LIMIT 2",
        )?
        .query_map((task, reference), |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<i64>>(4)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let [(event_id, Some(executable), action, Some(exit_code), Some(1))] = rows.as_slice() else {
        return Ok(None);
    };
    if *exit_code == 0 {
        return Ok(None);
    }
    let tool = match executable.rsplit('/').next() {
        Some("cargo") => "cargo",
        Some("git") => "git",
        Some("npm") => "npm",
        Some("rustc") => "rustc",
        Some("tsc") => "tsc",
        Some("vitest") => "vitest",
        _ => "command",
    };
    let action = match (tool, action.as_deref()) {
        ("cargo", Some("build")) => Some("build"),
        ("cargo", Some("check")) => Some("check"),
        ("cargo", Some("clippy")) => Some("clippy"),
        ("cargo", Some("test")) => Some("test"),
        ("git", Some("diff")) => Some("diff"),
        ("git", Some("status")) => Some("status"),
        ("npm", Some("run")) => Some("run"),
        ("npm", Some("test")) => Some("test"),
        _ => None,
    };
    Ok(Some(FailedCheck {
        event_id: *event_id,
        tool: tool.to_owned(),
        action: action.map(str::to_owned),
        exit_code: *exit_code,
    }))
}

pub fn source_path(path: &str) -> bool {
    path.len() <= 384
        && path.split('/').all(|part| {
            !part.is_empty()
                && !part.starts_with('.')
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        })
        && path.rsplit_once('.').is_some_and(|(_, extension)| {
            matches!(
                extension,
                "rs" | "c"
                    | "h"
                    | "cc"
                    | "cpp"
                    | "hpp"
                    | "go"
                    | "java"
                    | "kt"
                    | "swift"
                    | "js"
                    | "mjs"
                    | "cjs"
                    | "jsx"
                    | "ts"
                    | "tsx"
                    | "vue"
                    | "svelte"
                    | "py"
                    | "rb"
                    | "php"
                    | "sh"
                    | "bash"
                    | "sql"
                    | "css"
                    | "scss"
                    | "html"
            )
        })
}

fn sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn retain_result_tx(
    db: &Transaction<'_>,
    task: &str,
    assignment: &str,
    channel: &str,
    result: &Value,
    permitted_path: impl Fn(&str) -> bool,
) -> CoreResult<()> {
    let Some((revision, status, report_id, attempt)) = db
        .query_row(
            "SELECT n.revision,n.status,r.id,c.attempt_id FROM runtime_nodes n
             JOIN reports r ON r.task_id=n.task_id AND r.id=json_extract(n.result_json,'$.report_ref')
             JOIN trusted_channels c ON c.task_id=n.task_id AND c.assignment_id=n.assignment_ref
             WHERE n.task_id=? AND n.assignment_ref=? AND c.channel_id=? AND n.stage='execution'
             AND c.attempt_id IS NOT NULL",
            (task, assignment, channel),
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?)),
        )
        .optional()?
    else {
        return Ok(());
    };
    let failed_check = if status == "failed" {
        failed_check(db, task, result)?
    } else {
        None
    };
    let failure = if status == "failed" {
        let blocker = &result["blocker"]["kind"];
        if review::valid_blocker(blocker) {
            blocker.as_str()
        } else if failed_check.is_some() {
            Some("check_failed")
        } else {
            Some("check_failure_unverified")
        }
    } else {
        None
    };
    let failed_check = failed_check
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;
    let observations = db
        .prepare(
            "SELECT path,sha256,observation FROM (
                 SELECT json_extract(payload_json,'$.path') AS path,
                     json_extract(payload_json,'$.sha256') AS sha256,'write' AS observation
                 FROM runtime_events WHERE task_id=?1 AND type='NATIVE_PROJECT_WRITTEN'
                     AND json_extract(payload_json,'$.channel_id')=?2
                 ORDER BY id DESC LIMIT ?3
             )
             UNION ALL
             SELECT path,sha256,observation FROM (
                 SELECT json_extract(result_json,'$.fact_source_path') AS path,digest AS sha256,'read' AS observation
                 FROM runtime_requests WHERE task_id=?1 AND actor=?2 AND operation='native_file_read'
                     AND json_extract(result_json,'$.complete')=1
                     AND json_extract(result_json,'$.observed')=1
                     AND json_type(result_json,'$.fact_source_path')='text'
                 ORDER BY rowid DESC LIMIT ?3
             )",
        )?
        .query_map((task, channel, OBSERVATION_LIMIT), |row| {
            Ok((row.get::<_, Option<String>>(0)?, row.get::<_, Option<String>>(1)?, row.get::<_, String>(2)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (path, digest, observation) in observations {
        let (Some(path), Some(digest)) = (path, digest) else {
            continue;
        };
        if source_path(&path) && permitted_path(&path) && sha256(&digest) {
            db.execute(
                "INSERT INTO fact_memory(task_id,report_id,revision,assignment_ref,attempt_ref,path,sha256,observation,failure,failed_check)
                 VALUES (?,?,?,?,?,?,?,?,?,?) ON CONFLICT(task_id,assignment_ref,path,observation) DO NOTHING",
                (task, &report_id, revision, assignment, &attempt, path, digest, observation, failure, &failed_check),
            )?;
        }
    }
    Ok(())
}

/// A task identity plus plan revision and runtime-assigned assignment identity identifies the
/// source node even if a later plan reuses its name. No model-provided text travels here.
#[derive(Serialize)]
pub(crate) struct Fact {
    pub sequence: i64,
    pub task_id: String,
    pub report_id: String,
    pub revision: i64,
    pub assignment_ref: String,
    pub attempt_ref: String,
    pub path: String,
    pub sha256: String,
    pub observation: String,
    pub failure: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_check: Option<FailedCheck>,
}

pub(crate) fn recent(
    db: &Connection,
    exact: Option<&str>,
    path_pattern: &str,
    limit: i64,
) -> CoreResult<Vec<Fact>> {
    Ok(db
        .prepare(
            "SELECT sequence,task_id,report_id,revision,assignment_ref,attempt_ref,path,sha256,observation,failure,failed_check
             FROM fact_memory WHERE path=?1 OR path GLOB ?2 ORDER BY sequence DESC LIMIT ?3",
        )?
        .query_map((exact, path_pattern, limit), |r| {
            let failed_check = r.get::<_, Option<String>>(10)?;
            Ok(Fact {
                sequence: r.get(0)?,
                task_id: r.get(1)?,
                report_id: r.get(2)?,
                revision: r.get(3)?,
                assignment_ref: r.get(4)?,
                attempt_ref: r.get(5)?,
                path: r.get(6)?,
                sha256: r.get(7)?,
                observation: r.get(8)?,
                failure: r.get(9)?,
                failed_check: failed_check
                    .map(|value| serde_json::from_str(&value))
                    .transpose()
                    .map_err(|error| rusqlite::Error::FromSqlConversionFailure(
                        10, rusqlite::types::Type::Text, Box::new(error)))?,
            })
        })?
        .collect::<Result<_, _>>()?)
}
