//! Local owner view of an existing broker receipt. Never accepts an artifact ID.
use cortex_core::{CoreError, CoreResult, Store};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::{ErrorKind, Read};

const PREVIEW_BYTES: u64 = 4096;
const PAGE_BYTES: u64 = 16384;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ReadOutput {
    task_id: String,
    attempt_id: String,
    tool_call_ref: String,
    offset: Option<u64>,
    limit: Option<u64>,
}

impl ReadOutput {
    pub(crate) fn parse(params: &Value) -> CoreResult<Self> {
        let p: Self = serde_json::from_value(params.clone())
            .map_err(|_| CoreError::Refused("invalid_command_output_request"))?;
        let valid = |s: &str| {
            !s.is_empty()
                && s.len() <= 256
                && s.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_-:.".contains(&c))
        };
        if !valid(&p.task_id)
            || !valid(&p.attempt_id)
            || !valid(&p.tool_call_ref)
            || !p.tool_call_ref.starts_with("native-command:")
            || p.offset.is_none() && p.limit.is_some()
            || p.limit.is_some_and(|n| !(1..=PAGE_BYTES).contains(&n))
        {
            return Err(CoreError::Refused("invalid_command_output_request"));
        }
        Ok(p)
    }
}

/// Only a small, Workspace-only display reference, never a command payload.
/// Runtime authors this marker; engine updates go through their own allowlist.
pub(crate) fn coordinator_marker(
    db: &rusqlite::Connection,
    task: &str,
    actor: &str,
    reference: &str,
    status: &str,
) -> CoreResult<()> {
    let binding: Option<(String, String)> = db
        .query_row(
            "SELECT c.attempt_id,r.operation FROM trusted_channels c
             JOIN runtime_requests r ON r.task_id=c.task_id AND r.actor=c.channel_id
             WHERE c.task_id=?1 AND c.channel_id=?2 AND c.role='coordinator'
                AND c.attempt_id IS NOT NULL
                AND r.operation IN ('command_run','outside_command')
                AND json_extract(r.result_json,'$.tool_call_ref')=?3
                AND c.channel_id=(
                    SELECT CASE WHEN count(*)=1 THEN max(json_extract(s.payload_json,'$.channel_id')) END
                    FROM runtime_events s WHERE s.task_id=c.task_id
                        AND s.type='NATIVE_COMMAND_STARTED' AND json_extract(s.payload_json,'$.ref')=?3)
             GROUP BY c.attempt_id HAVING count(*)=1",
            (task, actor, reference),
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((attempt, operation)) = binding {
        let update = json!({
            "sessionUpdate":"tool_call", "toolCallId":format!("cortex-broker:{attempt}:{reference}"),
            "title":format!("mcp__cortex__{operation}"), "kind":"execute", "status":status,
            "commandOutputRef":{"attemptId":attempt,"toolCallRef":reference}
        });
        db.execute(
            "INSERT INTO host_updates(session_id,seq,update_json,created_at)
             SELECT s.session_id, max(
                 coalesce((SELECT max(seq) FROM host_transcript WHERE session_id=s.session_id),-1),
                 coalesce((SELECT max(seq) FROM host_updates WHERE session_id=s.session_id),-1)
             )+1, ?, ? FROM acp_host_sessions s
             WHERE s.task_id=? AND s.client_info=?",
            (
                update.to_string(),
                cortex_core::now(),
                task,
                r#"{"name":"cortex-native"}"#,
            ),
        )?;
    }
    Ok(())
}

struct OutputFile {
    filename: String,
    digest: Option<String>,
    size: u64,
    truncated: bool,
}

pub(crate) fn read(store: &Store, project: &str, p: ReadOutput) -> CoreResult<Value> {
    let binding = store.read(|db| {
        // An exact, durable broker start event plus the same actor's request binds
        // the call to the selected task and attempt. Neither an ACP call ID nor a
        // guessed output ID, nor a request on a replacement channel, grants access.
        let mut q = db.prepare(
            "SELECT e.runtime_task_id,c.channel_id,r.operation,r.result_json
             FROM workspace_tasks t JOIN workspace_executions e
                ON e.task_id=t.task_id AND e.execution_id=t.execution_id
             JOIN trusted_channels c ON c.task_id=e.runtime_task_id AND c.attempt_id=?3
             JOIN runtime_requests r ON r.task_id=c.task_id AND r.actor=c.channel_id
                AND r.operation IN ('command_run','outside_command')
                AND json_extract(r.result_json,'$.tool_call_ref')=?4
             WHERE t.project_id=?1 AND t.task_id=?2 AND c.channel_id=(
                SELECT CASE WHEN count(*)=1 THEN max(json_extract(s.payload_json,'$.channel_id')) END
                FROM runtime_events s WHERE s.task_id=c.task_id
                    AND s.type='NATIVE_COMMAND_STARTED' AND json_extract(s.payload_json,'$.ref')=?4)
             LIMIT 2",
        )?;
        let rows = q
            .query_map(
                params![project, p.task_id, p.attempt_id, p.tool_call_ref],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        let [(task, channel, operation, request)] = rows.as_slice() else {
            return Err(CoreError::Refused("command_receipt_not_found"));
        };
        // Outside capture may have committed the terminal event just before its
        // reply settles. The event is bound through the exact start above.
        let mut q = db.prepare(
            "SELECT json_extract(payload_json,'$.receipt.observation') FROM runtime_events
             WHERE task_id=? AND type='NATIVE_COMMAND_FINISHED'
                AND json_extract(payload_json,'$.receipt.tool_call_ref')=?
             ORDER BY id DESC LIMIT 2",
        )?;
        let finishes = q
            .query_map((task, &p.tool_call_ref), |r| r.get::<_, Option<String>>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        let receipt: Value = match finishes.as_slice() {
            [] | [None] => serde_json::from_str(request)?,
            [Some(raw)] => serde_json::from_str(raw)?,
            _ => return Err(CoreError::Refused("command_receipt_not_found")),
        };
        let output = match receipt["large_output_id"].as_str() {
            Some(id) => db
                .query_row(
                    "SELECT filename,source_ref,byte_count,truncated FROM large_outputs
                 WHERE task_id=? AND id=? AND owner_scope=? AND source_kind='terminal'",
                    (task, id, channel),
                    |r| {
                        Ok(OutputFile {
                            filename: r.get(0)?,
                            digest: r.get(1)?,
                            size: {
                                let size: i64 = r.get(2)?;
                                u64::try_from(size).map_err(|_| {
                                    rusqlite::Error::IntegralValueOutOfRange(2, size)
                                })?
                            },
                            truncated: r.get(3)?,
                        })
                    },
                )
                .optional()?,
            None => None,
        };
        Ok((task.clone(), operation.clone(), receipt, output))
    })?;
    let (task, operation, receipt, output) = binding;
    let status = if receipt["settled"] != true && receipt["complete"] != true {
        "running"
    } else if receipt["failure"] == "owner_stopped" {
        "stopped"
    } else if receipt["complete"] == true && receipt["exit_code"] == 0 {
        "passed"
    } else {
        "failed"
    };
    let mut result = json!({
        "operation":operation, "status":status, "argv":receipt["argv"], "cwd":receipt["cwd"],
        "exitCode":receipt["exit_code"], "signal":receipt["signal"], "failure":receipt["failure"],
        "output":{"state":if status=="running" {"pending"} else {"unavailable"}}
    });
    let Some(output) = output.filter(|_| status != "running") else {
        return Ok(result);
    };
    let name = format!(
        "{}.bin",
        receipt["large_output_id"].as_str().unwrap_or_default()
    );
    // Use only the broker's private terminal artifact, not the fallback reader
    // for other, model-scoped large results. Never substitute a lossy inline tail.
    if output.filename != name
        || name.contains(['/', '\\'])
        || name.contains("..")
        || !task
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
        || receipt["byte_count"].as_u64() != Some(output.size)
    {
        return Err(CoreError::Refused("command_output_integrity_mismatch"));
    }
    let path = store
        .key()
        .directory()
        .join(task)
        .join(".native-output")
        .join(name);
    let file = match cortex_core::store::locks::open_private(&path, false, true) {
        Ok(f) => f,
        Err(CoreError::Io(e)) if e.kind() == ErrorKind::NotFound => return Ok(result),
        Err(e) => return Err(e),
    };
    let offset = p.offset.unwrap_or(0);
    if offset > output.size {
        return Err(CoreError::Refused("invalid_command_output_offset"));
    }
    let limit = if p.offset.is_some() {
        p.limit.unwrap_or(PAGE_BYTES)
    } else {
        PREVIEW_BYTES
    };
    let chunk = verified_page(file, &output, offset, limit)?;
    let next = offset + chunk.len() as u64;
    let length = chunk.len();
    use base64::Engine;
    let (chunk, encoding) = match String::from_utf8(chunk) {
        Ok(text) => (text, "utf-8"),
        Err(e) => (
            base64::engine::general_purpose::STANDARD.encode(e.as_bytes()),
            "base64",
        ),
    };
    let complete = receipt["complete"] == true && !output.truncated;
    result["output"] = json!({
        "state":"available", "chunk":chunk, "encoding":encoding,
        "offset":offset, "byteLength":length, "totalBytes":output.size,
        "nextOffset":(next < output.size).then_some(next),
        "truncated":output.truncated, "captureComplete":complete,
        "complete":complete && offset == 0 && next == output.size,
    });
    Ok(result)
}

/// Hash the very bytes being paged, not a separate open or read after verification.
/// Memory is bounded by a page and a fixed buffer; only the retained file is read.
fn verified_page(
    mut file: impl Read,
    output: &OutputFile,
    offset: u64,
    limit: u64,
) -> CoreResult<Vec<u8>> {
    let end = offset.saturating_add(limit).min(output.size);
    let mut page = Vec::with_capacity((end - offset) as usize);
    let mut seen = 0;
    let mut hash = Sha256::new();
    let mut buf = [0; 65536];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        let next = seen + n as u64;
        if next > output.size {
            return Err(CoreError::Refused("command_output_integrity_mismatch"));
        }
        hash.update(&buf[..n]);
        let from = seen.max(offset);
        let to = next.min(end);
        if from < to {
            page.extend_from_slice(&buf[(from - seen) as usize..(to - seen) as usize]);
        }
        seen = next;
    }
    if seen != output.size || output.digest.as_deref() != Some(&format!("{:x}", hash.finalize())) {
        return Err(CoreError::Refused("command_output_integrity_mismatch"));
    }
    Ok(page)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortex_core::{
        lifecycle::{self, TaskMachine},
        OpenMode, ProjectKey, StoreChannel,
    };
    use std::{
        fs::{self, OpenOptions},
        io::Write,
        os::unix::fs::OpenOptionsExt,
        path::PathBuf,
    };

    const ATTEMPT: &str = "xa_worker";
    const CALL: &str = "native-command:fixture";

    struct Fixture {
        store: Store,
        root: PathBuf,
        path: PathBuf,
    }
    impl Fixture {
        fn new(operation: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "cortex-owner-output-{}",
                crate::native_effects::identity().unwrap()
            ));
            cortex_core::store::locks::private_directory(&root, true).unwrap();
            let store = Store::open(
                ProjectKey::new(&root, StoreChannel::Dev).unwrap(),
                OpenMode::Initialize,
            )
            .unwrap();
            for task in ["runtime", "other"] {
                TaskMachine::new(&store)
                    .create(task, "output test", "owner")
                    .unwrap();
            }
            store.write(|db| {
                db.execute("INSERT INTO workspace_projects(project_id,project_root,activated_by,activated_at) VALUES ('project',?,'owner','now')", [root.to_str()])?;
                for (task, draft, display, exec) in [("runtime","draft","display","exec"),("other","other-draft","other-display","other-exec")] {
                    db.execute("INSERT INTO workspace_tasks(task_id,project_id,description,execution_id,created_at) VALUES (?,'project','fixture',?,'now')",(draft,exec))?;
                    db.execute("INSERT INTO acp_host_sessions(session_id,project_root,task_id,client_info,client_capabilities,mcp_servers,created_at) VALUES (?,?,? ,?,'{}','[]','now')",
                        (display, root.to_str(), task, r#"{"name":"cortex-native"}"#))?;
                    db.execute("INSERT INTO workspace_executions VALUES (?,?,?,?,1,1,'now')",(exec,draft,task,display))?;
                }
                for (channel, role, assignment, attempt) in [
                    ("worker","worker",Some("assignment"),ATTEMPT),
                    ("replacement","worker",Some("assignment"),"xa_replacement"),
                    ("coordinator","coordinator",None,"xa_coordinator"),
                ] {
                    db.execute("INSERT INTO trusted_channels(channel_id,secret_hash,task_id,principal_thread_id,role,assignment_id,attempt_id,owner_epoch,issued_at) VALUES (?,'unused','runtime',?,?,?, ?,1,'now')",
                        (channel,channel,role,assignment,attempt))?;
                }
                db.execute("INSERT INTO runtime_requests VALUES ('runtime','test','worker',?,'fixture',?)",
                    (operation,json!({"tool_call_ref":CALL,"complete":false,"argv":["printf","%s"]}).to_string()))?;
                lifecycle::event_tx(db, "runtime", "NATIVE_COMMAND_STARTED", json!({"ref":CALL,"channel_id":"worker"}))?;
                Ok(())
            }).unwrap();
            let task = store.key().directory().join("runtime");
            let dir = task.join(".native-output");
            cortex_core::store::locks::private_directory(&task, true).unwrap();
            cortex_core::store::locks::private_directory(&dir, true).unwrap();
            Self {
                store,
                root,
                path: dir.join("lo_fixture.bin"),
            }
        }
        fn settle(&self, bytes: Option<&[u8]>, truncated: bool) {
            let mut observation = json!({"tool_call_ref":CALL,"argv":["printf","%s"],"cwd":"/fixture",
                "settled":true,"exit_code":7,"complete":true});
            self.store.write(|db| {
                if let Some(bytes) = bytes {
                    let mut f = OpenOptions::new().create_new(true).write(true).mode(0o600).open(&self.path)?;
                    f.write_all(bytes)?;
                    observation["byte_count"] = json!(bytes.len());
                    observation["large_output_id"] = json!("lo_fixture");
                    db.execute("INSERT INTO large_outputs VALUES ('lo_fixture','runtime','worker','terminal',?,
                        'lo_fixture.bin',?,?,7,NULL,NULL,'now')",(format!("{:x}", Sha256::digest(bytes)),bytes.len() as i64,truncated))?;
                }
                lifecycle::event_tx(db, "runtime", "NATIVE_COMMAND_FINISHED", json!({"receipt":{
                    "tool_call_ref":CALL,"observation":observation}}))?;
                Ok(())
            }).unwrap();
        }
        fn get(
            &self,
            project: &str,
            task: &str,
            attempt: &str,
            extras: Value,
        ) -> CoreResult<Value> {
            let mut req = json!({"taskId":task,"attemptId":attempt,"toolCallRef":CALL});
            req.as_object_mut()
                .unwrap()
                .extend(extras.as_object().unwrap().clone());
            read(&self.store, project, ReadOutput::parse(&req)?)
        }
        fn output(&self, extras: Value) -> CoreResult<Value> {
            self.get("project", "draft", ATTEMPT, extras)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn command_output_pages_exact_bytes_and_reports_cleanup_and_integrity() {
        let f = Fixture::new("outside_command");
        assert_eq!(
            f.output(json!({})).unwrap()["output"],
            json!({"state":"pending"})
        );
        let mut bytes = vec![b'a'; (PREVIEW_BYTES - 1) as usize];
        bytes.extend("€".as_bytes());
        bytes.extend(vec![b'b'; PAGE_BYTES as usize]);
        f.settle(Some(&bytes), false);
        let preview = f.output(json!({})).unwrap();
        assert_eq!(preview["status"], "failed");
        assert_eq!(preview["operation"], "outside_command");
        assert_eq!(preview["exitCode"], 7);
        assert_eq!(preview["cwd"], "/fixture");
        let output = &preview["output"];
        assert_eq!(output["encoding"], "base64");
        assert_eq!(output["byteLength"], PREVIEW_BYTES);
        assert_eq!(output["totalBytes"], bytes.len());
        assert_eq!(output["nextOffset"], PREVIEW_BYTES);
        assert_eq!(output["captureComplete"], true);
        assert_eq!(output["complete"], false);
        let page = f.output(json!({"offset":PREVIEW_BYTES,"limit":2})).unwrap();
        assert_eq!(page["output"]["chunk"], "gqw=");
        let end = f.output(json!({"offset":PREVIEW_BYTES+2})).unwrap();
        assert_eq!(end["output"]["chunk"], "b".repeat(PAGE_BYTES as usize));
        assert!(end["output"]["nextOffset"].is_null());
        assert_eq!(
            f.output(json!({"offset":bytes.len()+1}))
                .unwrap_err()
                .code(),
            "invalid_command_output_offset"
        );
        assert_eq!(
            f.output(json!({"offset":0,"limit":PAGE_BYTES+1}))
                .unwrap_err()
                .code(),
            "invalid_command_output_request"
        );
        let wrong = vec![b'x'; bytes.len()];
        fs::write(&f.path, wrong).unwrap();
        assert_eq!(
            f.output(json!({})).unwrap_err().code(),
            "command_output_integrity_mismatch"
        );
        fs::remove_file(&f.path).unwrap();
        assert_eq!(
            f.output(json!({})).unwrap()["output"],
            json!({"state":"unavailable"})
        );
    }

    #[test]
    fn command_output_refuses_wrong_project_task_attempt_actor_and_bare_id() {
        let f = Fixture::new("command_run");
        f.settle(Some(b"ok"), true);
        let output = f.output(json!({})).unwrap();
        assert_eq!(output["output"]["truncated"], true);
        assert_eq!(output["output"]["captureComplete"], false);
        for (project, task, attempt) in [
            ("wrong", "draft", ATTEMPT),
            ("project", "other-draft", ATTEMPT),
            ("project", "draft", "xa_replacement"),
            ("project", "draft", "xa_coordinator"),
        ] {
            assert_eq!(
                f.get(project, task, attempt, json!({})).unwrap_err().code(),
                "command_receipt_not_found"
            );
        }
        assert_eq!(
            f.output(json!({"large_output_id":"lo_fixture"}))
                .unwrap_err()
                .code(),
            "invalid_command_output_request"
        );
        f.store
            .write(|db| {
                db.execute("UPDATE large_outputs SET owner_scope='replacement'", [])?;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            f.output(json!({})).unwrap()["output"],
            json!({"state":"unavailable"})
        );
    }

    #[test]
    fn command_output_refuses_ambiguous_starts() {
        let f = Fixture::new("command_run");
        f.settle(Some(b"guarded"), false);
        f.store
            .write(|db| {
                lifecycle::event_tx(
                    db,
                    "runtime",
                    "NATIVE_COMMAND_STARTED",
                    json!({"ref":CALL,"channel_id":"replacement"}),
                )?;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            f.output(json!({})).unwrap_err().code(),
            "command_receipt_not_found"
        );
        f.store
            .read(|db| {
                let items = crate::native_owner_controls::worker_commands(
                    db,
                    "runtime",
                    "assignment",
                    None,
                )?;
                assert!(items
                    .iter()
                    .all(|row| row.get("commandOutputRef").is_none()));
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn command_output_distinguishes_empty_capture_and_unavailable() {
        let empty = Fixture::new("command_run");
        empty.settle(Some(b""), false);
        let output = empty.output(json!({})).unwrap();
        assert_eq!(output["output"]["state"], "available");
        assert_eq!(output["output"]["complete"], true);
        assert_eq!(output["output"]["totalBytes"], 0);
        let absent = Fixture::new("command_run");
        absent.settle(None, false);
        assert_eq!(
            absent.output(json!({})).unwrap()["output"],
            json!({"state":"unavailable"})
        );
        let not_command = Fixture::new("project_git");
        assert_eq!(
            not_command.output(json!({})).unwrap_err().code(),
            "command_receipt_not_found"
        );
    }

    #[test]
    fn command_output_worker_feed_links_only_its_brokered_receipt() {
        for operation in ["command_run", "outside_command", "project_git"] {
            let f = Fixture::new(operation);
            f.settle(Some(b"private fixture output"), false);
            f.store
                .read(|db| {
                    let items = crate::native_owner_controls::worker_commands(
                        db,
                        "runtime",
                        "assignment",
                        None,
                    )?;
                    assert_eq!(items.len(), 1);
                    let row = &items[0];
                    assert_eq!(row["toolCallRef"], CALL);
                    assert_eq!(row["command"]["tail"], "");
                    if operation == "project_git" {
                        assert!(row.get("commandOutputRef").is_none());
                    } else {
                        assert_eq!(
                            row["commandOutputRef"],
                            json!({"attemptId":ATTEMPT,"toolCallRef":CALL})
                        );
                    }
                    assert!(crate::native_owner_controls::worker_commands(
                        db,
                        "runtime",
                        "another-assignment",
                        Some("another-assignment")
                    )?
                    .is_empty());
                    assert!(crate::native_owner_controls::worker_commands(
                        db,
                        "other",
                        "assignment",
                        None
                    )?
                    .is_empty());
                    Ok(())
                })
                .unwrap();
        }
    }

    #[test]
    fn command_output_marker_is_workspace_coordinator_only() {
        let f = Fixture::new("command_run");
        f.store
            .write(|db| {
                db.execute(
                    "INSERT INTO acp_host_sessions(session_id,project_root,task_id,client_info,client_capabilities,mcp_servers,created_at)
                     VALUES ('incoming',?,'runtime',?,'{}','[]','now')",
                    (f.root.to_str(), r#"{"name":"cortex-rust-acp"}"#),
                )?;
                let coordinator_ref = "native-command:coordinator";
                for (key, operation, reference) in [
                    ("git", "project_git", "native-command:git"),
                    ("command", "command_run", coordinator_ref),
                ] {
                    db.execute(
                        "INSERT INTO runtime_requests VALUES ('runtime',?,'coordinator',?,'fixture',?)",
                        (key, operation, json!({"tool_call_ref":reference}).to_string()),
                    )?;
                }
                coordinator_marker(db, "runtime", "worker", CALL, "completed")?;
                coordinator_marker(db, "runtime", "coordinator", "native-command:git", "completed")?;
                lifecycle::event_tx(db, "runtime", "NATIVE_COMMAND_STARTED",
                    json!({"ref":coordinator_ref,"channel_id":"coordinator"}))?;
                coordinator_marker(db, "runtime", "coordinator", coordinator_ref, "completed")?;
                let (session, raw): (String, String) =
                    db.query_row("SELECT session_id,update_json FROM host_updates", [], |r| {
                        Ok((r.get(0)?, r.get(1)?))
                    })?;
                assert_eq!(session, "display");
                let v: Value = serde_json::from_str(&raw)?;
                assert_eq!(
                    v["commandOutputRef"],
                    json!({"attemptId":"xa_coordinator","toolCallRef":coordinator_ref})
                );
                assert!(v.get("rawOutput").is_none());
                assert_eq!(
                    db.query_row("SELECT count(*) FROM host_updates", [], |r| r
                        .get::<_, i64>(0))?,
                    1
                );
                Ok(())
            })
            .unwrap();
    }
}
