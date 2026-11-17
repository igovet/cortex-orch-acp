//! Inline native worker publication. Runs only on the owned StoreExecutor lane;
//! no SQLite transaction spans filesystem publication. A durable payload binding
//! precedes publication; the final event, node result and receipt commit together.
use cortex_core::{
    channels::{self, Credential, Identity},
    lifecycle::{
        self,
        completion::{self, EvidenceSnapshot},
        waits,
    },
    store::documents::{Publication, Published, Publisher},
    CoreError, CoreResult, Store,
};
use serde_json::{json, Value};

fn text<'a>(v: &'a Value, field: &str) -> CoreResult<&'a str> {
    v[field]
        .as_str()
        .ok_or(CoreError::Refused("invalid_native_result"))
}
fn validate_result(
    store: &Store,
    c: &Credential,
    i: &Identity,
    args: &Value,
    evidence: &EvidenceSnapshot,
) -> CoreResult<Vec<Value>> {
    if i.role != "worker" || !matches!(i.stage.as_deref(), Some("execution" | "review")) {
        return Err(CoreError::Refused("worker_required"));
    }
    let report = text(args, "report")?;
    if report.trim().is_empty() || report.contains("Cortex draft ID:") || report.contains("<!--") {
        return Err(CoreError::Refused("native_inline_report_required"));
    }
    if text(args, "summary")?.trim().is_empty() {
        return Err(CoreError::Refused("native_summary_required"));
    }
    let title = title(args)?;
    if title.trim().is_empty() || title.chars().count() > 200 {
        return Err(CoreError::Refused("invalid_publication"));
    }
    use cortex_core::lifecycle::worker_checks;
    let submitted = args["checks"]
        .as_array()
        .ok_or(CoreError::Refused("invalid_native_result"))?;
    let mut runs = store.read(|db| worker_checks::load_runs(db, &i.task, &c.channel_id))?;
    let relevant = worker_checks::relevant_references(submitted, &runs);
    for run in &mut runs {
        if !run.receipt["tool_call_ref"]
            .as_str()
            .is_some_and(|r| relevant.contains(r))
        {
            continue;
        }
        if let Some(id) = run.receipt["large_output_id"].as_str() {
            let output = crate::native_commands::read_output(
                store,
                i,
                c,
                &json!({"large_output_id":id,"limit":1}),
            );
            if let Ok(output) = output {
                let v = &output["structuredContent"];
                run.output_verified = v["integrity_verified"] == true
                    && v["truncated"] == false
                    && v["byte_count"] == run.receipt["byte_count"]
                    && v["exit_status"]["exit_code"] == run.receipt["exit_code"]
                    && v["failure"] == run.receipt["failure"];
            }
        }
    }
    // Retain only this attempt's observed failures/pending receipts, including
    // citations whose explicit correction will subsequently be refused.
    let presented_now: Vec<_> = submitted
        .iter()
        .filter(|c| matches!(c["outcome"].as_str(), Some("passed" | "failed")))
        .filter_map(|c| c["tool_call_ref"].as_str())
        .filter(|reference| {
            let matched: Vec<_> = runs
                .iter()
                .filter(|r| r.receipt["tool_call_ref"] == *reference)
                .collect();
            matched.len() == 1 && matched[0].presented_failure()
        })
        .collect();
    if !presented_now.is_empty() {
        store.write(|db| {
        channels::authenticate(db, &c.channel_id, c.secret(), true)?;
        for reference in &presented_now {
                    let exists:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='CHECK_FAILURE_PRESENTED' AND json_extract(payload_json,'$.attempt')=? AND json_extract(payload_json,'$.ref')=?)",
                        (&i.task,&i.attempt,reference),|r|r.get(0))?;
                    if !exists { lifecycle::event_tx(db,&i.task,"CHECK_FAILURE_PRESENTED",json!({"attempt":i.attempt,"ref":reference}))?; }
        }
        Ok(())
    })?;
    }
    let checks = worker_checks::evaluate(submitted, &runs, evidence.project_digest.as_deref())?;
    let presented:Vec<String>=store.read(|db|Ok(db.prepare("SELECT json_extract(payload_json,'$.ref') FROM runtime_events WHERE task_id=? AND type='CHECK_FAILURE_PRESENTED' AND json_extract(payload_json,'$.attempt')=?")?
        .query_map((&i.task,&i.attempt),|r|r.get(0))?.collect::<Result<Vec<_>,_>>()?))?;
    if presented.iter().any(|r| {
        runs.iter()
            .any(|run| run.receipt["tool_call_ref"] == *r && run.verified_failure())
            && !submitted.iter().any(|c| {
                matches!(c["outcome"].as_str(), Some("passed" | "failed"))
                    && c["tool_call_ref"] == *r
            })
    }) {
        return Err(CoreError::Refused("omitted_failed_check"));
    }
    // A reviewer requesting changes may cite an open failed check as its finding.
    let review_requests_changes = i.stage.as_deref() == Some("review")
        && args["verdict"]
            .as_str()
            .is_some_and(|v| v.eq_ignore_ascii_case("REQUEST_CHANGES"));
    for check in &checks {
        if check["outcome"] == "failed"
            && !worker_checks::resolved(check)
            && args["assignment_outcome"] != "failed"
            && !review_requests_changes
        {
            return Err(CoreError::Refused("open_failed_check"));
        }
        if matches!(check["outcome"].as_str(), Some("not_run" | "unverified"))
            && check["blocker"]
                .as_str()
                .is_none_or(|s| s.trim().is_empty())
        {
            return Err(CoreError::Refused("check_blocker_required"));
        }
    }
    Ok(checks)
}
#[cfg(test)]
mod evidence_tests {
    use super::*;
    use cortex_core::{
        lifecycle::{FoundationGuards, TaskMachine},
        OpenMode, ProjectKey, StoreChannel, TaskState,
    };
    use std::os::unix::fs::PermissionsExt;
    /// Owner store 2026-11-17: two `result_submit` calls failed with a bare Forbidden
    /// and nothing retained why. A content refusal now goes back to the worker with its
    /// code, and every refusal is retained by code alone, bound to the attempt.
    #[test]
    fn a_refused_result_is_retained_by_code_and_explained_to_the_worker() {
        let root = std::env::temp_dir().join(format!(
            "native-refusal-{}",
            crate::native_effects::identity().unwrap()
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let store = Store::open(
            ProjectKey::new(&root, StoreChannel::Dev).unwrap(),
            OpenMode::Initialize,
        )
        .unwrap();
        let owner = TaskMachine::new(&store).create("t", "checks", "owner").unwrap();
        TaskMachine::new(&store)
            .transition("t", &owner, "planning", TaskState::Planning, None, &FoundationGuards)
            .unwrap();
        let c = store
            .write(|db| channels::issue_tx(db, "t", &owner, channels::Principal::Coordinator))
            .unwrap();
        let i = Identity {
            task: "t".into(),
            attempt: "xa_fixture".into(),
            principal: "fixture".into(),
            epoch: 1,
            role: "worker".into(),
            profile: Some("worker-debugger".into()),
            stage: Some("execution".into()),
            assignment: Some("a".into()),
            session: None,
        };
        for code in ["open_failed_check", "omitted_failed_check"] {
            record_refusal(&store, &c, &i, code).unwrap();
        }
        let rows: Vec<String> = store
            .read(|db| {
                Ok(db.prepare("SELECT payload_json FROM runtime_events WHERE type='WORKER_RESULT_REFUSED' ORDER BY id")?
                    .query_map([], |r| r.get(0))?
                    .collect::<Result<Vec<_>, _>>()?)
            })
            .unwrap();
        assert_eq!(rows.len(), 2);
        let first: Value = serde_json::from_str(&rows[0]).unwrap();
        assert_eq!(first, json!({"channel_id":c.channel_id,"attempt":"xa_fixture","assignment_ref":"a","code":"open_failed_check"}));
        assert!(correctable("open_failed_check") && correctable("invalid_arguments"));
        assert!(!correctable("channel_identity_changed") && !correctable("worker_required"));
        let reply = refusal_reply("open_failed_check");
        assert_eq!(reply["isError"], true);
        assert_eq!(reply["structuredContent"]["code"], "open_failed_check");
        assert_eq!(reply["structuredContent"]["recorded"], false);
        assert!(reply["structuredContent"]["message"].as_str().unwrap().contains("rerun the exact same command"));
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn passed_receipts_require_digest_nontrivial_command_unique_ref_and_intact_output() {
        let root = std::env::temp_dir().join(format!(
            "native-checks-{}",
            crate::native_effects::identity().unwrap()
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let store = Store::open(
            ProjectKey::new(&root, StoreChannel::Dev).unwrap(),
            OpenMode::Initialize,
        )
        .unwrap();
        let machine = TaskMachine::new(&store);
        let owner = machine.create("t", "checks", "owner").unwrap();
        machine
            .transition(
                "t",
                &owner,
                "planning",
                TaskState::Planning,
                None,
                &FoundationGuards,
            )
            .unwrap();
        let c = store
            .write(|db| channels::issue_tx(db, "t", &owner, channels::Principal::Coordinator))
            .unwrap();
        store
            .write(|db| channels::bind_session_tx(db, &c, "fixture-session"))
            .unwrap();
        let i = Identity {
            task: "t".into(),
            attempt: "fixture".into(),
            principal: "fixture".into(),
            epoch: 1,
            role: "worker".into(),
            profile: Some("worker-backend-dev".into()),
            stage: Some("execution".into()),
            assignment: None,
            session: None,
        };
        let dir = store.key().directory().join("t").join(".native-output");
        cortex_core::store::locks::private_directory(dir.parent().unwrap(), true).unwrap();
        cortex_core::store::locks::private_directory(&dir, true).unwrap();
        let path = dir.join("lo_fixture.bin");
        let mut file = cortex_core::store::locks::open_private(&path, true, false).unwrap();
        use std::io::Write;
        file.write_all(b"complete output").unwrap();
        file.sync_all().unwrap();
        let hash = crate::native_project::digest(b"complete output");
        let digest = "a".repeat(64);
        let receipt = json!({"tool_call_ref":"native-command:fixture","argv":["cargo","test"],
            "cwd":root,"profile":"development","complete":true,"settled":true,"exit_code":0,"failure":null,
            "revision_digest":digest,"large_output_id":"lo_fixture","byte_count":15});
        store.write(|db|{
            db.execute("INSERT INTO large_outputs VALUES ('lo_fixture','t',?,'terminal',?,'lo_fixture.bin',15,0,0,NULL,NULL,'fixture')",
                (&c.channel_id,&hash))?;
            db.execute("INSERT INTO runtime_requests VALUES ('t','command',?,'command_run','bound',?)",(&c.channel_id,receipt.to_string()))?;Ok(())
        }).unwrap();
        let args = json!({"report":"# Evidence","summary":"checked","checks":[{"name":"tests","outcome":"passed","tool_call_ref":"native-command:fixture"}]});
        let evidence = EvidenceSnapshot {
            project_digest: Some(digest.clone()),
            ..Default::default()
        };
        let checks = validate_result(&store, &c, &i, &args, &evidence).unwrap();
        assert_eq!(checks[0]["evidence_strength"], "verified");
        assert_eq!(checks[0]["revision_digest"], digest);
        let stale = EvidenceSnapshot {
            project_digest: Some("b".repeat(64)),
            ..Default::default()
        };
        assert_eq!(
            validate_result(&store, &c, &i, &args, &stale).unwrap()[0]["evidence_gap"],
            "revision_mismatch"
        );
        let mut duplicate = args.clone();
        duplicate["checks"]
            .as_array_mut()
            .unwrap()
            .push(args["checks"][0].clone());
        assert_eq!(
            validate_result(&store, &c, &i, &duplicate, &evidence).unwrap()[1]["evidence_gap"],
            "ref_already_cited"
        );
        let mut trivial = receipt.clone();
        trivial["argv"] = json!(["env", "A=1", "ls"]);
        store
            .write(|db| {
                db.execute(
                    "UPDATE runtime_requests SET result_json=? WHERE request_key='command'",
                    [trivial.to_string()],
                )?;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            validate_result(&store, &c, &i, &args, &evidence).unwrap()[0]["evidence_gap"],
            "trivial_command"
        );
        // Confirmed outside receipts have the same evidence rule and full-output binding.
        store.write(|db|{
            db.execute("UPDATE runtime_requests SET operation='outside_command',result_json=? WHERE request_key='command'",
                [json!({"complete":true,"reply":{"structuredContent":receipt}}).to_string()])?;Ok(())
        }).unwrap();
        assert_eq!(
            validate_result(&store, &c, &i, &args, &evidence).unwrap()[0]["evidence_strength"],
            "verified"
        );
        // Persist a nonzero receipt, refuse it, reopen, then require its history.
        let mut failed_receipt = receipt.clone();
        failed_receipt["tool_call_ref"] = json!("native-command:failed");
        failed_receipt["exit_code"] = json!(1);
        failed_receipt["large_output_id"] = json!("lo_failed");
        let mut rerun = receipt.clone();
        rerun["tool_call_ref"] = json!("native-command:rerun");
        store.write(|db|{
            db.execute("INSERT INTO large_outputs VALUES ('lo_failed','t',?,'terminal',?,'lo_fixture.bin',15,0,1,NULL,NULL,'fixture')",(&c.channel_id,&hash))?;
            db.execute("INSERT INTO runtime_requests VALUES ('t','failed',?,'command_run','bound',?)",(&c.channel_id,failed_receipt.to_string()))?;
            Ok(())
        }).unwrap();
        let mut failed_args = args.clone();
        failed_args["checks"] =
            json!([{"name":"tests","outcome":"failed","tool_call_ref":"native-command:failed"}]);
        let mut invalid = failed_args.clone();
        invalid["checks"][0]["invocation_error"] =
            json!({"corrected_by":"unknown","reason":"not proven"});
        assert_eq!(
            validate_result(&store, &c, &i, &invalid, &evidence)
                .unwrap_err()
                .code(),
            "invalid_check_retraction"
        );
        assert_eq!(
            validate_result(&store, &c, &i, &failed_args, &evidence)
                .unwrap_err()
                .code(),
            "open_failed_check"
        );
        drop(store);
        let store = Store::open(
            ProjectKey::new(&root, StoreChannel::Dev).unwrap(),
            OpenMode::RuntimeValidate,
        )
        .unwrap();
        store
            .write(|db| {
                db.execute(
                    "INSERT INTO runtime_requests VALUES ('t','rerun',?,'command_run','bound',?)",
                    (&c.channel_id, rerun.to_string()),
                )?;
                Ok(())
            })
            .unwrap();
        let mut success_args = args.clone();
        success_args["checks"][0]["tool_call_ref"] = json!("native-command:rerun");
        assert_eq!(
            validate_result(&store, &c, &i, &success_args, &evidence)
                .unwrap_err()
                .code(),
            "omitted_failed_check"
        );
        failed_args["checks"]
            .as_array_mut()
            .unwrap()
            .push(success_args["checks"][0].clone());
        let history = validate_result(&store, &c, &i, &failed_args, &evidence).unwrap();
        assert_eq!(history[0]["outcome"], "failed");
        assert_eq!(history[0]["closed_by"], "native-command:rerun");
        assert_eq!(history[0]["evidence_strength"], "verified");
        assert_eq!(history[1]["outcome"], "passed");
        assert_eq!(
            store
                .read(|db| Ok(db.query_row(
                    "SELECT count(*) FROM runtime_events WHERE type='CHECK_FAILURE_PRESENTED'",
                    [],
                    |r| r.get::<_, i64>(0)
                )?))
                .unwrap(),
            1
        );
        // The retained failed citation must still be present after tampering.
        std::fs::write(path, "tampered").unwrap();
        assert_eq!(
            validate_result(&store, &c, &i, &failed_args, &evidence)
                .unwrap_err()
                .code(),
            "open_failed_check"
        );
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}
/// A value is a report id only when it is exactly `r_` plus twelve lowercase
/// hexadecimal digits. Anything else, including prose that merely starts with
/// `r_` or has surrounding whitespace, is inline report text.
/// Content refusals of `result_submit` that the worker can correct and submit again.
/// Authority and identity refusals are not among them; they stay opaque to the engine.
pub(super) fn correctable(code: &str) -> bool {
    matches!(
        code,
        "open_failed_check"
            | "omitted_failed_check"
            | "check_blocker_required"
            | "invalid_native_result"
            | "native_inline_report_required"
            | "native_summary_required"
            | "invalid_publication"
            | "invalid_check_retraction"
            | "invalid_check_supersession"
            | "result_report_not_owned"
            | "request_key_reused"
            | "invalid_arguments"
    )
}
/// The worker-facing reply to a correctable refusal: its code and what to change.
/// Nothing was recorded or published; the channel stays active.
pub(super) fn refusal_reply(code: &str) -> Value {
    let hint = match code {
        "open_failed_check" => "A cited check failed and is still open. Fix the cause and rerun the exact same command so a later passing run closes it, or submit with assignment_outcome \"failed\".",
        "omitted_failed_check" => "A failed check run that an earlier submission cited is missing. Cite it again as passed (after an exact passing rerun) or failed, or submit with assignment_outcome \"failed\".",
        "check_blocker_required" => "Every not_run or unverified check needs a concrete blocker.",
        "native_inline_report_required" => "The report must be the complete Markdown report, or the exact report_id of a report published by this attempt.",
        "native_summary_required" => "The summary must not be empty.",
        "invalid_publication" => "The report title must be 1 to 200 characters.",
        "invalid_check_retraction" | "invalid_check_supersession" => "The cited correction is not eligible; cite the failed check without it, or submit with assignment_outcome \"failed\".",
        "result_report_not_owned" => "The report_id must name a report this attempt published.",
        "request_key_reused" => "Use a new request_key.",
        "invalid_arguments" => "The arguments do not match the advertised result_submit schema.",
        _ => "",
    };
    let value = json!({"code":code,"recorded":false,
        "message":format!("No result was recorded ({code}). {hint} Correct the result and submit it again.")});
    json!({"content":[{"type":"text","text":value.to_string()}],"structuredContent":value,"isError":true})
}
/// Retains why an authenticated worker's `result_submit` was refused: the refusal
/// code with the attempt binding, never arguments, report text or check output.
pub(super) fn record_refusal(
    store: &Store,
    c: &Credential,
    i: &Identity,
    code: &str,
) -> CoreResult<()> {
    store.write(|db| {
        lifecycle::event_tx(
            db,
            &i.task,
            "WORKER_RESULT_REFUSED",
            json!({"channel_id":c.channel_id,"attempt":i.attempt,"assignment_ref":i.assignment,"code":code}),
        )?;
        Ok(())
    })
}
fn is_report_id(value: &str) -> bool {
    value.strip_prefix("r_").is_some_and(|tail| {
        tail.len() == 12 && tail.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
    })
}
#[cfg(test)]
mod report_id_tests {
    use super::is_report_id;
    #[test]
    fn only_exact_ids_are_report_references() {
        assert!(is_report_id("r_0123456789ab"));
        for inline in [
            "r_check passed after the fix",
            "r_0123456789ab and notes",
            "r_0123456789ab\n",
            " r_0123456789ab",
            "r_0123456789AB",
            "r_0123456789a",
            "r_0123456789abc",
            "r_",
            "r_regression analysis\n\nDetails follow.",
            "# Report",
        ] {
            assert!(!is_report_id(inline), "{inline:?}");
        }
    }
}
fn title(args: &Value) -> CoreResult<&str> {
    if let Some(title) = args["report_title"].as_str() {
        return Ok(title);
    }
    Ok(text(args, "report")?
        .lines()
        .find_map(|line| line.strip_prefix("# "))
        .unwrap_or("Native worker report"))
}
/// Only called after schema validation, on the serial writer lane. Re-authenticate
/// before every phase; external epoch changes may fence a publication in progress.
/// Such a publication remains retained, but is never promoted to a node result.
pub(super) fn submit(
    store: &Store,
    credential: &Credential,
    expected: &Identity,
    args: &Value,
    mut evidence: EvidenceSnapshot,
) -> CoreResult<Value> {
    let i = store
        .read(|db| channels::authenticate(db, &credential.channel_id, credential.secret(), true))?;
    if &i != expected {
        return Err(CoreError::Refused("channel_identity_changed"));
    }
    let checks = validate_result(store, credential, &i, args, &evidence)?;
    let assignment = i
        .assignment
        .as_deref()
        .ok_or(CoreError::Refused("worker_required"))?;
    let key = text(args, "request_key")?;
    let intent_actor = format!("native-publication:{}", credential.channel_id);
    // Conflict-check the complete payload before any file effect. This committed
    // intent makes a crash/retry deterministic, not a new report or changed claim.
    if let Some(refusal) = store.write(|db| {
        let current =
            channels::authenticate(db, &credential.channel_id, credential.secret(), true)?;
        if current != i {
            return Err(CoreError::Refused("channel_identity_changed"));
        }
        lifecycle::review::preflight_result_tx(db, &i.task, assignment, key, &evidence)
    })? {
        return Ok(refusal);
    }
    let source_revision = store.write(|db| {
        channels::authenticate(db, &credential.channel_id, credential.secret(), true)?;
        waits::check_mutation_tx(db, &i.task, assignment)?;
        // An earlier tool on this channel may already own this key. Refuse
        // before publication, not after creating an otherwise unused artifact.
        let used: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=?)",
            (&i.task, &credential.channel_id, key), |r| r.get(0),
        )?;
        if used {
            return Err(CoreError::Refused("request_key_reused"));
        }
        lifecycle::request_tx(
            db,
            &i.task,
            &intent_actor,
            key,
            "result_payload",
            args,
            || {
                let revision: i64 = db.query_row(
                    "SELECT coalesce(max(revision),0) FROM source_revisions WHERE task_id=?",
                    [&i.task],
                    |r| r.get(0),
                )?;
                Ok(json!({"source_revision":revision}))
            },
        )
    })?["source_revision"]
        .as_i64()
        .ok_or(CoreError::Refused("invalid_result_intent"))?;
    let publisher = Publisher::new(store);
    let report = if is_report_id(text(args, "report")?) {
        let id = text(args, "report")?;
        // A report reference is not authority: it must come from this exact
        // channel principal's publication in this task, not a sibling's result.
        store.read(|db| {
            Ok(db
                .query_row(
                    "SELECT r.id,e.digest,e.size_bytes,coalesce(p.source_revision,0),
                coalesce(p.artifacts,'[]') FROM reports r JOIN editions e ON e.report_id=r.id
                LEFT JOIN report_provenance p ON p.edition_sequence=e.sequence
                JOIN drafts d ON d.published_report_id=r.id
                WHERE r.id=? AND r.task_id=? AND d.owner_thread_id=? AND r.kind='report'
                ORDER BY e.sequence DESC LIMIT 1",
                    (id, &i.task, &i.principal),
                    |r| {
                        Ok(Published {
                            report_id: r.get(0)?,
                            sha256: r.get(1)?,
                            size_bytes: r.get::<_, i64>(2)? as u64,
                            source_revision: r.get(3)?,
                            artifacts: serde_json::from_str(&r.get::<_, String>(4)?)
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        })
                    },
                )
                .map_err(|_| CoreError::Refused("result_report_not_owned"))?)
        })?
    } else {
        let draft = publisher.create_draft(&i.task, &i.principal, "implementation", key)?;
        let published: bool = store.read(|db| Ok(db.query_row(
        "SELECT published_report_id IS NOT NULL FROM drafts WHERE id=? AND task_id=? AND owner_thread_id=?",
        (&draft.draft_id, &i.task, &i.principal), |r| r.get(0),
    )?))?;
        if !published {
            publisher.write_draft(
                &i.task,
                &i.principal,
                &draft.draft_id,
                &format!(
                    "Cortex draft ID: `{}`\n\n{}",
                    draft.draft_id,
                    text(args, "report")?
                ),
            )?;
        }
        store.read(|db| {
            channels::authenticate(db, &credential.channel_id, credential.secret(), true)
        })?;
        let report = publisher.publish(
            &i.task,
            &i.principal,
            &draft.draft_id,
            key,
            Publication {
                title: title(args)?,
                // The full untruncated result summary is retained on the result, not
                // squeezed into the catalogue's independent 320-character field.
                summary: "Native worker result; see the immutable report and structured checks.",
                author: i
                    .profile
                    .as_deref()
                    .ok_or(CoreError::Refused("worker_required"))?,
                source_revision,
                artifacts: json!([]),
            },
        )?;
        report
    };
    // Publisher verifies the complete file against the edition digest, even
    // though this read exposes only its bounded first page.
    let observed = publisher.read_report(&i.task, &report.report_id, None, 4000)?;
    evidence
        .artifacts
        .insert(report.report_id.clone(), observed.sha256);
    let mut result = args.clone();
    result["checks"] = json!(checks);
    let object = result
        .as_object_mut()
        .ok_or(CoreError::Refused("invalid_native_result"))?;
    object.remove("report");
    object.remove("report_title");
    object.remove("request_key");
    result["report_ref"] = json!(report.report_id);
    if let Some(verdict) = result["verdict"].as_str() {
        result["verdict"] = json!(verdict.to_ascii_uppercase());
    }
    store.write(|db| {
        let current = channels::authenticate(db, &credential.channel_id, credential.secret(), true)?;
        if current != i {
            return Err(CoreError::Refused("channel_identity_changed"));
        }
        lifecycle::request_tx(db, &i.task, &credential.channel_id, key, "result_submit", args, || {
            // Binding is created by the authenticated host, not by report text.
            db.execute(
                "INSERT INTO task_events(task_id,event_type,assignment_ref,dedup_key,payload_json,origin,created_at)
                 VALUES (?,'worker_result',?,?,?,'native-runtime',?)",
                (&i.task, assignment, format!("native-result:{}:{key}", credential.channel_id),
                    json!({"report_ref":report.report_id,"attempt_id":i.attempt,"checks":checks}).to_string(), cortex_core::now()),
            )?;
            completion::record_result_tx(db, &i.task, assignment, &result, &evidence)?;
            Ok(json!({"outcome":"recorded","report_ref":report.report_id,"assignment_ref":assignment,
                "certified":false}))
        })
    })
}
