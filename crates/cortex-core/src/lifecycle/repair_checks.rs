//! Keep a failed worker's verified checks attached to the task after a replan.
use super::worker_checks::{self, Run};
use crate::{error::refuse, CoreError, CoreResult};
use rusqlite::{Connection, Transaction};
use serde_json::Value;

fn receipt(db: &Connection, task: &str, assignment: &str, reference: &str) -> CoreResult<Run> {
    let mut query = db.prepare(
        "SELECT r.operation, CASE WHEN r.operation='outside_command' THEN
         json_extract(r.result_json,'$.reply.structuredContent') ELSE r.result_json END
         FROM runtime_requests r JOIN trusted_channels c ON c.channel_id=r.actor AND c.task_id=r.task_id
         JOIN delegation_provisioning p ON p.task_id=c.task_id AND p.attempt_id=c.attempt_id AND p.idempotency_key=c.assignment_id
         WHERE r.task_id=? AND c.assignment_id=? AND r.operation IN ('command_run','outside_command')
         AND coalesce(json_extract(r.result_json,'$.tool_call_ref'),
             json_extract(r.result_json,'$.reply.structuredContent.tool_call_ref'))=? LIMIT 2",
    )?;
    let mut rows = query.query((task, assignment, reference))?;
    let err = || CoreError::Refused("completion_unresolved_prior_worker_check");
    let row = rows.next()?.ok_or_else(err)?;
    let run = Run {
        operation: row.get(0)?,
        receipt: serde_json::from_str(&row.get::<_, String>(1)?)?,
        order: 0,
        output_verified: false,
        work_set: None,
    };
    if rows.next()?.is_some() {
        return Err(err());
    }
    Ok(run)
}

/// Call only after checking the current workers' results and the final verification
/// against the reviewed digest. A final verifier can cover a project command only
/// when its native output receipt also establishes the exact working directory.
pub(super) fn require_prior_worker_repairs(
    db: &Transaction<'_>,
    task: &str,
    revision: i64,
    verification: &Value,
) -> CoreResult<()> {
    let mut query = db.prepare(
        "SELECT n.assignment_ref,c.value FROM runtime_nodes n, json_each(n.result_json,'$.checks') c
         WHERE n.task_id=?1 AND n.revision<?2 AND n.status='failed' AND n.stage='execution'
         AND n.revision>coalesce((SELECT max(json_extract(payload_json,'$.revision')) FROM runtime_events
           WHERE task_id=?1 AND type='TASK_REOPENED' AND json_extract(payload_json,'$.previous')='COMPLETED'),0)
         AND json_extract(c.value,'$.outcome')='failed' AND json_extract(c.value,'$.evidence_strength')='verified'",
    )?;
    let rows = query.query_map((task, revision), |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    let mut failures = Vec::new();
    for row in rows {
        let (assignment, raw) = row?;
        let check: Value = serde_json::from_str(&raw)?;
        if !worker_checks::resolved(&check) {
            let reference = check["tool_call_ref"].as_str().ok_or(CoreError::Refused(
                "completion_unresolved_prior_worker_check",
            ))?;
            failures.push(receipt(db, task, &assignment, reference)?);
        }
    }
    if failures.is_empty() {
        return Ok(());
    }
    let mut current = db.prepare(
        "SELECT n.assignment_ref,json_extract(c.value,'$.tool_call_ref') FROM runtime_nodes n, json_each(n.result_json,'$.checks') c
         WHERE n.task_id=? AND n.revision=? AND n.status='completed'
         AND json_extract(c.value,'$.outcome')='passed' AND json_extract(c.value,'$.evidence_strength')='verified'
         AND json_extract(c.value,'$.revision_digest')=?",
    )?;
    // A worker's check receipt names the whole checkout it ran against; a
    // verification bound to the task's files records that checkout separately.
    let checkout = verification["checkout_digest"]
        .as_str()
        .or(verification["revision_digest"].as_str());
    let rows = current.query_map((task, revision, checkout), |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (assignment, reference) = row?;
        let run = receipt(db, task, &assignment, &reference)?;
        failures.retain(|failed| !failed.same(&run));
    }
    for check in verification["checks"].as_array().into_iter().flatten() {
        if failures.is_empty() {
            break;
        }
        // The authoritative verification and native output name the same unique run.
        let mut q = db.prepare("SELECT json_extract(result_json,'$.cwd') FROM runtime_requests
            WHERE task_id=? AND actor='native-verifier-output' AND operation='native_command_output'
            AND json_extract(result_json,'$.tool_call_ref')=? AND json_extract(result_json,'$.argv')=?
            AND json_extract(result_json,'$.revision_digest')=? AND json_extract(result_json,'$.observation.complete')=1
            AND json_extract(result_json,'$.observation.exit_code')=0 AND json_extract(result_json,'$.observation.output_truncated')=0
            AND json_extract(result_json,'$.observation.failure') IS NULL LIMIT 2")?;
        let dirs = q
            .query_map(
                (
                    task,
                    check["receipt_ref"].as_str(),
                    check["argv"].to_string(),
                    verification["revision_digest"].as_str(),
                ),
                |r| r.get::<_, String>(0),
            )?
            .collect::<Result<Vec<_>, _>>()?;
        if let [cwd] = dirs.as_slice() {
            failures.retain(|f| {
                !(f.operation == "command_run"
                    && f.receipt["argv"] == check["argv"]
                    && f.receipt["cwd"] == cwd.as_str())
            });
        }
    }
    if !failures.is_empty() {
        return refuse("completion_unresolved_prior_worker_check");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{lifecycle::TaskMachine, OpenMode, ProjectKey, StoreChannel};
    use serde_json::json;

    fn fixture() -> (std::path::PathBuf, crate::Store) {
        let root = std::env::temp_dir().join(format!("repair-checks-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let store = crate::Store::open(
            ProjectKey::new(&root, StoreChannel::Dev).unwrap(),
            OpenMode::Initialize,
        )
        .unwrap();
        TaskMachine::new(&store)
            .create("t", "repair", "owner")
            .unwrap();
        (root, store)
    }

    fn bind(
        db: &Transaction<'_>,
        assignment: &str,
        attempt: &str,
        channel: &str,
        revision: i64,
        result: Value,
        status: &str,
    ) -> crate::CoreResult<()> {
        db.execute("INSERT INTO runtime_nodes(task_id,revision,node_id,role,objective,dependencies_json,scope_json,stage,status,assignment_ref,result_json)
            VALUES ('t',?,'node-'||?, 'worker-backend-dev','work','[]','[\"src\"]','execution',?,?,?)",
            (revision, assignment, status, assignment, result.to_string()))?;
        db.execute("INSERT INTO delegation_provisioning(idempotency_key,task_id,attempt_id,role_id,step,status,channel_id,engine_session_ref,owner_epoch,created_at,updated_at)
            VALUES (?,'t',?,'worker-backend-dev','ready','ready',?,?,'1','now','now')",
            (assignment, attempt, channel, format!("engine-{attempt}")))?;
        db.execute("INSERT INTO trusted_channels(channel_id,secret_hash,task_id,principal_thread_id,role,assignment_id,attempt_id,owner_epoch,issued_at)
            VALUES (?,'fixture','t','thread','worker',?,?,1,'now')", (channel, assignment, attempt))?;
        Ok(())
    }

    fn run(
        db: &Transaction<'_>,
        channel: &str,
        reference: &str,
        argv: Value,
        cwd: &str,
        profile: &str,
        code: i64,
    ) -> crate::CoreResult<()> {
        db.execute(
            "INSERT INTO runtime_requests VALUES ('t',?,?, 'command_run','bound',?)",
            (
                reference,
                channel,
                json!({"tool_call_ref":reference,"complete":true,"exit_code":code,
                "failure":Value::Null,"argv":argv,"cwd":cwd,"profile":profile,
                "revision_digest":"a".repeat(64)})
                .to_string(),
            ),
        )?;
        Ok(())
    }

    #[test]
    fn exact_bound_passing_worker_run_repairs_old_failure() {
        let (root, store) = fixture();
        store.write(|db| {
            db.execute("UPDATE runtime_tasks SET revision=2", [])?;
            bind(db, "old", "attempt-old", "channel-old", 1, json!({"checks":[{"outcome":"failed","evidence_strength":"verified","tool_call_ref":"old-fail"}]}), "failed")?;
            bind(db, "new", "attempt-new", "channel-new", 2, json!({"checks":[{"outcome":"passed","evidence_strength":"verified","tool_call_ref":"new-pass","revision_digest":"a".repeat(64)}]}), "completed")?;
            run(db, "channel-old", "old-fail", json!(["cargo","test"]), "/project", "read_only", 101)?;
            run(db, "channel-new", "new-pass", json!(["cargo","test"]), "/project", "read_only", 0)?;
            require_prior_worker_repairs(db, "t", 2, &json!({"revision_digest":"a".repeat(64),"checks":[]}))
        }).unwrap();
        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn changed_argv_or_cwd_does_not_repair_old_failure() {
        let (root, store) = fixture();
        let error = store.write(|db| {
            db.execute("UPDATE runtime_tasks SET revision=2", [])?;
            bind(db, "old", "attempt-old", "channel-old", 1, json!({"checks":[{"outcome":"failed","evidence_strength":"verified","tool_call_ref":"old-fail"}]}), "failed")?;
            bind(db, "new", "attempt-new", "channel-new", 2, json!({"checks":[{"outcome":"passed","evidence_strength":"verified","tool_call_ref":"new-pass","revision_digest":"a".repeat(64)}]}), "completed")?;
            run(db, "channel-old", "old-fail", json!(["cargo","test"]), "/project", "read_only", 101)?;
            run(db, "channel-new", "new-pass", json!(["cargo","test","--lib"]), "/other", "read_only", 0)?;
            require_prior_worker_repairs(db, "t", 2, &json!({"revision_digest":"a".repeat(64),"checks":[]}))
        }).unwrap_err();
        assert_eq!(error.code(), "completion_unresolved_prior_worker_check");
        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }
    #[test]
    fn final_verification_covers_only_the_matching_project_command() {
        let (root, store) = fixture();
        store.write(|db| {
            let argv = json!(["cargo","test"]);
            let digest = "a".repeat(64);
            let verified = json!({"revision_digest":digest, "checks":[{"argv":argv,"receipt_ref":"final"}]});
            bind(db, "old", "attempt-old", "channel-old", 1, json!({"checks":[{"outcome":"failed","evidence_strength":"verified","tool_call_ref":"old-fail"}]}), "failed")?;
            run(db,"channel-old","old-fail",argv.clone(),"/project","development",101)?;
            let mut raw = json!({"tool_call_ref":"final","argv":argv,"cwd":"/other","revision_digest":digest,
                "observation":{"complete":true,"exit_code":0,"output_truncated":false,"failure":null}});
            db.execute("INSERT INTO runtime_requests VALUES ('t','output','native-verifier-output','native_command_output','bound',?)",[raw.to_string()])?;
            assert_eq!(require_prior_worker_repairs(db,"t",2,&verified).unwrap_err().code(),"completion_unresolved_prior_worker_check");
            raw["cwd"]=json!("/project");
            db.execute("UPDATE runtime_requests SET result_json=? WHERE request_key='output'",[raw.to_string()])?;
            require_prior_worker_repairs(db,"t",2,&verified)
        }).unwrap();
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}
